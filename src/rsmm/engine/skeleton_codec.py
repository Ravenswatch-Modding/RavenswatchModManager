"""A mesh's skeleton (``oCSkeleton`` inside an ``oCGeometry``), decoded and
written: the roadmap's "skeleton writer" (Hero from scratch D), so a hero can
have proportions of its own.

Layout (every shipped hero mesh re-encodes byte for byte, tests): inside the
geometry's main section, ``BEGIN <oCSkeleton> u32 count`` then ``count`` bones,
each ``BEGIN <oCBone> lstr name lstr name2`` + three 4x4 f32 matrices + ``i32
parent`` (-1 = root) + ``u8 flag`` + ``END``:

* ``M0`` the INVERSE world rest pose (bind), ``M1`` the world rest pose,
  ``M2`` the local rest pose (relative to the parent): ``M1 = M2 @ M1(parent)``
  in the row-vector convention the file uses (translation in the last row).

A hero's main body header lists no bones of its own (Kintaro: 0), so the rest
pose lives here only. Changing it moves every bone's rest; the mesh's
vertices and the clips' translation keys must agree: a clip keys every bone's
translation at the rig's lengths, so the hero kind re-proportions each rig clip
it plays (:func:`rsmm.engine.anim_cook.retarget`, unproven in game).
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

from . import cooked

_B, _E = cooked.MARK_BEGIN, cooked.MARK_END


class SkeletonError(ValueError):
    pass


Mat = tuple[float, ...]       # 16 floats, row-major, row-vector convention


@dataclass
class Bone:
    name: str
    name2: str
    inverse: Mat              # M0
    world: Mat                # M1
    local: Mat                # M2
    parent: int
    flag: int


@dataclass
class Skeleton:
    section: int              # which section of the geometry holds it
    start: int                # payload offset of BEGIN <oCSkeleton>
    end: int                  # payload offset just past its END
    bones: list[Bone]
    bounds: tuple[float, ...] = (0.0,) * 6   # 6 f32 after the bones (min xyz, max xyz)


def _lstr(p: bytes, o: int) -> tuple[str, int]:
    n = struct.unpack_from("<I", p, o)[0]
    return p[o + 4:o + 4 + n].decode("utf-8"), o + 4 + n


def decode(geo: bytes) -> Skeleton:
    cf = cooked.parse(geo)
    names = [c.name for c in cf.classes]
    if "oCSkeleton" not in names:
        raise SkeletonError("this geometry has no skeleton")
    skel, bone = names.index("oCSkeleton"), names.index("oCBone")
    head = _B + struct.pack("<I", skel)
    for si, sec in enumerate(cf.sections):
        p = sec.payload
        o = p.find(head)
        if o < 0:
            continue
        start = o
        o += 8
        count = struct.unpack_from("<I", p, o)[0]
        o += 4
        bones = []
        for _ in range(count):
            if p[o:o + 8] != _B + struct.pack("<I", bone):
                raise SkeletonError(f"no bone at {o}")
            name, o = _lstr(p, o + 8)
            name2, o = _lstr(p, o)
            m = struct.unpack_from("<48f", p, o)
            o += 192
            parent, flag = struct.unpack_from("<iB", p, o)
            o += 5
            if p[o:o + 4] != _E:
                raise SkeletonError(f"bone {name!r} does not end where expected")
            o += 4
            bones.append(Bone(name, name2, m[:16], m[16:32], m[32:48], parent, flag))
        bounds = struct.unpack_from("<6f", p, o)
        o += 24
        if p[o:o + 4] != _E:
            raise SkeletonError("skeleton does not end after its bones and bounds")
        return Skeleton(si, start, o + 4, bones, bounds)
    raise SkeletonError("no skeleton block found")


def _encode_block(skel: Skeleton, names: list[str]) -> bytes:
    out = bytearray(_B + struct.pack("<II", names.index("oCSkeleton"), len(skel.bones)))
    for b in skel.bones:
        out += _B + struct.pack("<I", names.index("oCBone"))
        for s in (b.name, b.name2):
            e = s.encode("utf-8")
            out += struct.pack("<I", len(e)) + e
        out += struct.pack("<48f", *b.inverse, *b.world, *b.local)
        out += struct.pack("<iB", b.parent, b.flag) + _E
    out += struct.pack("<6f", *skel.bounds)
    return bytes(out + _E)


def encode(geo: bytes, skel: Skeleton) -> bytes:
    """``geo`` with its skeleton block replaced by ``skel``."""
    cf = cooked.parse(geo)
    names = [c.name for c in cf.classes]
    p = cf.sections[skel.section].payload
    cf.sections[skel.section].payload = p[:skel.start] + _encode_block(skel, names) + \
        p[skel.end:]
    return cooked.emit(cf)


# ---- rest-pose math (row-vector 4x4, translation in the last row) -----------

def _mul(a: Mat, b: Mat) -> Mat:
    return tuple(sum(a[r * 4 + k] * b[k * 4 + c] for k in range(4))
                 for r in range(4) for c in range(4))


def _inverse(m: Mat) -> Mat:
    """General 4x4 inverse (Gauss-Jordan): shipped bones carry non-uniform
    scale (Carmilla's), which a rigid inverse gets wrong."""
    a = [list(m[r * 4:r * 4 + 4]) + [1.0 if r == c else 0.0 for c in range(4)]
         for r in range(4)]
    for c in range(4):
        piv = max(range(c, 4), key=lambda r: abs(a[r][c]))
        if abs(a[piv][c]) < 1e-12:
            raise SkeletonError("a bone's rest pose is not invertible")
        a[c], a[piv] = a[piv], a[c]
        d = a[c][c]
        a[c] = [x / d for x in a[c]]
        for r in range(4):
            if r != c and a[r][c]:
                f = a[r][c]
                a[r] = [x - f * y for x, y in zip(a[r], a[c], strict=True)]
    return tuple(a[r][4 + c] for r in range(4) for c in range(4))


def with_rest(skel: Skeleton, locals_: dict[str, Mat]) -> Skeleton:
    """``skel`` with the named bones' LOCAL rest poses replaced; every bone's
    world and inverse rest poses recomputed down the hierarchy."""
    bones = [Bone(b.name, b.name2, b.inverse, b.world, locals_.get(b.name, b.local),
                  b.parent, b.flag) for b in skel.bones]
    done: dict[int, Mat] = {}

    def world(i: int) -> Mat:
        if i in done:
            return done[i]
        b = bones[i]
        w = b.local if b.parent < 0 else _mul(b.local, world(b.parent))
        done[i] = w
        return w
    for i, b in enumerate(bones):
        w = world(i)
        bones[i] = Bone(b.name, b.name2, _inverse(w), w, b.local, b.parent, b.flag)
    return Skeleton(skel.section, skel.start, skel.end, bones, skel.bounds)


def scale_lengths(skel: Skeleton, factors: dict[str, float]) -> dict[str, Mat]:
    """Local rest poses with each named bone's OFFSET from its parent scaled
    (``{"DEF.TopLeg.L": 1.15}`` = that bone sits 15% further from its parent:
    the parent segment is longer). Rotations are kept."""
    out = {}
    for b in skel.bones:
        f = factors.get(b.name)
        if f is None:
            continue
        m = list(b.local)
        m[12], m[13], m[14] = m[12] * f, m[13] * f, m[14] * f
        out[b.name] = tuple(m)
    return out


def rest_translations(skel: Skeleton) -> dict[str, tuple[float, float, float]]:
    """Each bone's local rest translation (relative to its parent), by name:
    the space a clip keys translations in (:func:`anim_cook.retarget`)."""
    return {b.name: tuple(b.local[12:15]) for b in skel.bones}


def locals_from_glb(glb: bytes) -> dict[str, Mat]:
    """Each joint node's local rest pose in a glb, as the skeleton stores it
    (the exporter writes the game's locals unchanged: translation, rotation
    rows and scale match, measured on Kintaro's 157 bones)."""
    import json
    jlen = struct.unpack_from("<I", glb, 12)[0]
    doc = json.loads(glb[20:20 + jlen])
    joints = {i for sk in doc.get("skins", []) for i in sk.get("joints", [])}
    out: dict[str, Mat] = {}
    for i in joints:
        nd = doc["nodes"][i]
        x, y, z, w = nd.get("rotation", [0.0, 0.0, 0.0, 1.0])
        sx, sy, sz = nd.get("scale", [1.0, 1.0, 1.0])
        tx, ty, tz = nd.get("translation", [0.0, 0.0, 0.0])
        rows = [[1 - 2 * (y * y + z * z), 2 * (x * y + z * w), 2 * (x * z - y * w)],
                [2 * (x * y - z * w), 1 - 2 * (x * x + z * z), 2 * (y * z + x * w)],
                [2 * (x * z + y * w), 2 * (y * z - x * w), 1 - 2 * (x * x + y * y)]]
        rows = [[v * sc for v in row] for row, sc in zip(rows, (sx, sy, sz), strict=True)]
        out[nd["name"]] = (*rows[0], 0.0, *rows[1], 0.0, *rows[2], 0.0, tx, ty, tz, 1.0)
    return out


def from_glb(glb: bytes, like: Skeleton, *, root: str = "",
             sockets: dict[str, tuple[str, tuple[float, float, float]]] | None = None
             ) -> Skeleton:
    """A skeleton made of the glb's own armature, to replace ``like`` (the
    geometry's current one, which supplies where the block sits and its
    bounds) with bones that need not match any shipped rig. Bones are the
    skin's joints, parents before children; ``name2`` repeats ``name`` and
    ``flag`` is 0, as every shipped bone has them (Kintaro, 157 of 157).

    ``root``: the name of a bone to add in front of the armature's own roots
    (identity rest pose; the game's skeletons start at ``DEF.RootMotion``,
    which the engine moves a character by). Leave it empty when the armature
    carries its own.

    ``sockets``: bones the game's shared hero parts name (``DEF.Head``,
    ``DEF.SpineMiddle``, ``DEF.HealthBarAttach``), added after the armature
    as ``{name: (parent bone, local offset)}``; the parent is a joint of the
    glb or ``root``, and the socket carries no rotation."""
    import json
    jlen = struct.unpack_from("<I", glb, 12)[0]
    doc = json.loads(glb[20:20 + jlen])
    nodes = doc["nodes"]
    joints = [i for sk in doc.get("skins", []) for i in sk.get("joints", [])]
    if not joints:
        raise SkeletonError("the glb has no skin")
    parent_of = {c: i for i, nd in enumerate(nodes) for c in nd.get("children", [])}
    locals_ = locals_from_glb(glb)
    ident: Mat = (1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0,
                  0.0, 0.0, 0.0, 1.0)
    order: list[int] = []
    seen: set[int] = set()

    def visit(i: int) -> None:
        if i in seen:
            return
        p = parent_of.get(i)
        if p in joints:
            visit(p)
        seen.add(i)
        order.append(i)
    for i in joints:
        visit(i)
    names = [nodes[i].get("name") or f"joint{i}" for i in order]
    if len(set(names)) != len(names):
        raise SkeletonError("the glb's joints share a name; bones are matched by name")
    index = {i: k + (1 if root else 0) for k, i in enumerate(order)}
    bones: list[Bone] = []
    if root:
        if root in names:
            raise SkeletonError(f"the glb already has a bone named {root!r}")
        bones.append(Bone(root, root, ident, ident, ident, -1, 0))
    for i, name in zip(order, names, strict=True):
        p = parent_of.get(i)
        parent = index[p] if p in index else (0 if root else -1)
        bones.append(Bone(name, name, ident, ident, locals_[name], parent, 0))
    for name, (parent_name, offset) in (sockets or {}).items():
        where = {b.name: k for k, b in enumerate(bones)}
        if name in where:
            raise SkeletonError(f"socket {name!r} is already a bone of the glb")
        if parent_name not in where:
            raise SkeletonError(f"socket {name!r}: no bone {parent_name!r} to hang it on")
        local = (*ident[:12], *map(float, offset), 1.0)
        bones.append(Bone(name, name, ident, ident, local, where[parent_name], 0))
    out = Skeleton(like.section, like.start, like.end, bones, like.bounds)
    return with_rest(out, {b.name: b.local for b in bones})


__all__ = ["Bone", "Skeleton", "SkeletonError", "decode", "encode", "from_glb",
           "locals_from_glb", "scale_lengths", "with_rest"]
