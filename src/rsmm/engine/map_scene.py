"""What a chapter LOOKS like: placed entities resolved down to meshes and textures.

Read-only, for the map editor's renderer. Nothing here is shipped: every byte is
read from the user's own install (or the dev `data/uncooked` mirror) on request.
The meshes and textures the page draws (`*.fbx.glb`, `*.png`) are the mirror's
copies when a dev checkout has them, else converted from the cooked install
files on request by :func:`asset_bytes` — a player's machine and the web editor
have only the install. Textures from the install are cut to about
:data:`TEXTURE_EDGE` pixels, which is all a map preview needs.

**Resource references.** Cooked entities, materials and levels name other
resources as two length-prefixed strings, ``u32 n, root, u32 m, path`` — the same
``<Root>|<Path>`` pair a `*.UsedRscCache.ot` line spells out. The roots that
matter here:

* ``3D``: a mesh (``Scenery\\DarkHills\\Rock_Medium_B.fbx``), then the materials
  it is drawn with (``...\\M_Rocks_Moss_Medium_Small_Atlas.mat.ot``), in order;
* ``EntitySettings``: another entity — a parent it inherits from, a child node
  (grass patches), or one alternative of an "Entity selector to spawn";
* ``Ot``: a composition level (``Prefab``), itself a list of placements.

**What an entity draws**, in the order tried: its own meshes; else the entities
of its prefab level, each at its placement; else its referenced entities — only
the first that draws anything when it is a random selector, all of them
otherwise. Depth-bounded and memoised.

**Known approximations** (visual only; nothing is written back):

* child-node LOCAL transforms inside an entity are not read, so a patch of three
  grass meshes draws them at the patch origin;
* a random selector always shows its first drawable alternative;
* LOD meshes are ignored (``_LOD1``) and one albedo texture per mesh is used;
* a decal (``DecalCube_1x1`` + a decal material) is drawn as its texture on a
  flat quad rather than projected onto what is under it;
* level-design helpers the game never shows are skipped: ``LD_Basic``
  primitives, ``LD_Terrain_Shapes`` (they stamp terrain height) and ``_anim_``
  copies of a mesh;
* ``Dt_Color_<Name>`` materials (the black silhouettes that frame a map) draw
  as their flat colour.

Coordinates stay in engine space (Y up; the page mirrors Z once for WebGL).
"""

from __future__ import annotations

import functools
import math
import re
import struct
from dataclasses import dataclass
from pathlib import Path

from . import level_placements as LP
from .paths import DATA_DIR

MAX_DEPTH = 5
#: Parts one entity may expand to. A composition of compositions can multiply
#: quickly; past this the rest is dropped rather than stalling the page.
MAX_PARTS = 4000
_MAX_ROOT = 32
_MAX_PATH = 300
_SELECTOR = b"Entity selector to spawn"
#: Albedo naming, best first. Anything else is a mask, normal or lookup texture.
_ALBEDO_HINTS = ("_alb", "_albedo", "_basecolor", "_base_color", "_diffuse", "_col", "_color", "_d")
#: Suffixes of the other maps a material samples.
_NOT_ALBEDO = ("_nrm", "_normal", "_mra", "_msk", "_mask", "_emi", "_rough", "_ao")
#: Engine placeholder textures (``Textures\\Black.png``), never an albedo.
_PLACEHOLDERS = ("black", "white", "grey", "gray")
_TPI_SENTINEL = bytes.fromhex("2222bbaa1111bbaa000000002222bbaa1111bbaa00000000")
_FMT_RGBA = 0

Matrix = tuple[float, ...]   # 16 floats, column-major, like glTF / WebGL
IDENTITY: Matrix = (1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0)


#: Mesh paths that are editor helpers, invisible in game.
_HELPER_MESHES = ("3D/LD_Basic/",)
_HELPER_ENTITIES = ("ld_terrain_shapes\\",)
_DECAL_MESH = "3D/DecalCube_1x1.fbx.glb"
#: Flat colours of the engine's ``Dt_Color_<Name>`` materials.
_FLAT_COLORS = {"black": "#0b0b0c", "white": "#eeeeee", "grey": "#808080", "gray": "#808080",
                "red": "#b03030", "green": "#3f7f3f", "blue": "#3a5fa0"}


@dataclass(frozen=True)
class Part:
    mesh: str                 # data/uncooked-relative glb path, "/"-separated
    texture: str | None       # data/uncooked-relative png path
    swap_rb: bool             # the png came from an uncompressed (R/B-swapped) texture
    color: str | None = None  # flat colour when the material has no texture
    decal: bool = False       # draw the texture on a flat quad, not the mesh
    vfx: bool = False         # an effect's sprites: camera-facing glowing quads
    normal: str | None = None  # the material's normal map png, when it has one
    normal_swap: bool = False


# --------------------------------------------------------------------------
# asset lookup
# --------------------------------------------------------------------------

@functools.lru_cache(maxsize=1)
def _index() -> tuple[dict[str, str], frozenset[str]]:
    """``"root/path".lower()`` -> decoded key of the cooked file, plus the roots."""
    from .asset_map import decoded_to_encoded

    out: dict[str, str] = {}
    roots: set[str] = set()
    for dec in decoded_to_encoded():
        if not dec.endswith(".gen") or "/" not in dec:
            continue
        stem = dec[:-4]
        cut = stem.rfind(".")
        if cut <= 0:
            continue
        out.setdefault(stem[:cut].lower(), dec)
        roots.add(dec.split("/", 1)[0])
    return out, frozenset(roots)


def _decoded(root: str, path: str) -> str | None:
    return _index()[0].get(f"{root}/{path.replace(chr(92), '/')}".lower())


def _bytes(root: str, path: str) -> bytes | None:
    from .map_editor import _shipped

    dec = _decoded(root, path)
    return _shipped(dec) if dec else None


def resource_refs(data: bytes) -> list[tuple[str, str]]:
    """Every ``(root, path)`` pair in a cooked blob, in stream order."""
    out: list[tuple[str, str]] = []
    pos, end = 0, len(data) - 8
    # Candidates are exactly the offsets holding a known root as an lstr; the
    # lookahead keeps overlapping ones, which the old per-byte loop also tried.
    for mt in _root_lstrs(_index()[1]).finditer(data):
        i = mt.start()
        if i < pos or i >= end:
            continue
        n = struct.unpack_from("<I", data, i)[0]
        if i + 8 + n > len(data):
            continue
        (m,) = struct.unpack_from("<I", data, i + 4 + n)
        j = i + 8 + n
        path = data[j:j + m]
        if 0 < m <= _MAX_PATH and len(path) == m and not path.translate(None, _PRINTABLE):
            out.append((data[i + 4:i + 4 + n].decode("ascii"), path.decode("ascii")))
            pos = j + m
    return out


_PRINTABLE = bytes(range(32, 127))


@functools.lru_cache(maxsize=4)
def _root_lstrs(roots: frozenset[str]) -> re.Pattern[bytes]:
    """Every root in ``roots`` as ``u32 length + text``, the shape
    `resource_refs` looks for. Keyed on the set, so a different index (a test's
    stand-in) never reuses another's pattern."""
    alts = sorted((struct.pack("<I", len(r)) + r.encode("ascii")
                   for r in roots if r.isascii() and 0 < len(r) <= _MAX_ROOT),
                  key=len, reverse=True)
    return re.compile(b"(?=" + b"|".join(re.escape(a) for a in alts) + b")") if alts \
        else re.compile(b"(?!)")


def _uncooked(rel: str) -> Path | None:
    """The dev mirror's copy of ``rel``, kept inside the mirror: the editor's
    file route passes page-supplied paths here, and on Windows a segment like
    ``C:`` re-anchors a join."""
    base = (DATA_DIR / "uncooked").resolve()
    p = base.joinpath(*rel.split("/")).resolve()
    return p if p.is_relative_to(base) and p.is_file() else None


#: Longest edge of a texture decoded from the install. The page only tints
#: instanced scenery with it, and a 2048 albedo costs seconds of pure-Python
#: block decoding for nothing visible, so the smallest mip at least this big wins.
TEXTURE_EDGE = 256


def _cooked_key(rel: str) -> str | None:
    """Decoded key of the cooked file a page path (``3D/...fbx.glb`` or
    ``3D/....png``) is converted from, when the game ships one."""
    if rel.startswith("FX/") and rel.endswith(".png"):
        pass                             # an effect's sprite (see `vfx_parts`)
    elif not rel.startswith("3D/"):
        return None
    path = rel[3:].replace("/", "\\")
    if path.endswith(".fbx.glb"):
        return _decoded("3D", path[:-4])
    if path.endswith(".png"):
        # The page path drops the source extension; albedos are .tga or .png.
        # Textures cook to `.Texture.dxt`, which `_index` (the `.gen` files) skips.
        from .asset_map import decoded_to_encoded

        shipped = decoded_to_encoded()
        # Normal maps cook to `.Texture.nrm` (the same header as `.dxt`).
        for ext in (".tga", ".png"):
            for kind in (".Texture.dxt", ".Texture.nrm"):
                key = rel[:-4] + ext + kind
                if key in shipped:
                    return key
    return None


def available(rel: str) -> bool:
    """Can :func:`asset_bytes` produce ``rel``? Cheap: no file is decoded."""
    return _uncooked(rel) is not None or _cooked_key(rel) is not None


def _texture_png(cooked_bytes: bytes) -> bytes:
    from . import cooked
    from . import icon_decode as ID
    from .cooked_schemas.texture import TextureHandler

    t = TextureHandler.parse_payload(cooked.parse(cooked_bytes).sections[-1].payload)
    level, w, h = t.pixels, t.width, t.height
    for i, mip in enumerate(t.mips, 1):
        mw, mh = max(1, t.width >> i), max(1, t.height >> i)
        if max(mw, mh) < TEXTURE_EDGE or min(mw, mh) < 4:
            break
        level, w, h = mip, mw, mh
    level, w, h = _skip_blocks(level, w, h, t.format_name)
    rgba, w, h = ID.resize_to_max(ID.decode_to_rgba(level, w, h, t.format_name), w, h,
                                  TEXTURE_EDGE)
    return ID.rgba_to_png(rgba, w, h)


#: Bytes per 4x4 block of the block-compressed formats `icon_decode` reads.
_BLOCK_BYTES = {"BC1": 8, "BC3": 16, "BC5": 16, "BC5U": 16}


def _skip_blocks(px: bytes, w: int, h: int, fmt: str | None) -> tuple[bytes, int, int]:
    """Keep every n-th 4x4 block of a block-compressed level so it decodes at
    about :data:`TEXTURE_EDGE`.

    Most scenery textures ship WITHOUT mips (six 2048x2048 BC1 albedos in Dark
    Hills alone), and decoding one whole in pure Python took ~2 s. Copying blocks
    is a byte slice per block, and the decoder then touches only what is kept.
    """
    size = _BLOCK_BYTES.get(fmt or "")
    step = max(w, h) // TEXTURE_EDGE
    if not size or step < 2:
        return px, w, h
    bw, bh = (w + 3) // 4, (h + 3) // 4
    if len(px) < bw * bh * size:
        return px, w, h                   # let the decoder report the short payload
    keep_x, keep_y = range(0, bw, step), range(0, bh, step)
    out = bytearray()
    for by in keep_y:
        row = by * bw
        for bx in keep_x:
            o = (row + bx) * size
            out += px[o:o + size]
    return bytes(out), len(keep_x) * 4, len(keep_y) * 4


@functools.lru_cache(maxsize=128)
def asset_bytes(rel: str) -> bytes | None:
    """The glb or png the page draws: the dev mirror's copy when there is one,
    else converted from the cooked file in the user's install — which is all a
    player's machine and the web editor have. None when neither can make it.

    A texture from the install decodes with its real colours, so it never needs
    the page's R/B swap; :func:`material_albedo` reports ``swap_rb`` to match.
    """
    if p := _uncooked(rel):
        return p.read_bytes()
    key = _cooked_key(rel)
    data = _shipped_key(key) if key else None
    if not data:
        return None
    try:
        if rel.endswith(".glb"):
            from . import cooked
            from .cooked_schemas import geometry as GEO

            cf = cooked.parse(data)
            g = GEO.parse_payload(b"".join(s.payload for s in cf.sections),
                                  [len(s.payload) for s in cf.sections])
            return GEO._build_glb_preview(g)
        return _texture_png(data)
    except Exception:  # noqa: BLE001 — an undecodable asset only means less is drawn
        return None


def _shipped_key(key: str) -> bytes | None:
    from .map_editor import _shipped

    return _shipped(key)


# --------------------------------------------------------------------------
# materials and textures
# --------------------------------------------------------------------------

def _texture_uncompressed(tex_path: str) -> bool:
    """Is the cooked texture stored uncompressed? Its png copy is then R/B-swapped."""
    data = _bytes("3D", tex_path)
    if not data:
        return False
    s = data.find(_TPI_SENTINEL, 0, 4096)
    if s < 0 or s + len(_TPI_SENTINEL) + 16 > len(data):
        return False
    fmt = struct.unpack_from("<I", data, s + len(_TPI_SENTINEL) + 16)[0]
    return fmt == _FMT_RGBA


def material_color(mat_path: str) -> str | None:
    """The flat colour of a ``Dt_Color_<Name>`` material, else None."""
    stem = mat_path.rsplit("\\", 1)[-1].lower().removesuffix(".mat.ot").removesuffix("_skinned")
    if not stem.startswith("dt_color_"):
        return None
    return _FLAT_COLORS.get(stem.removeprefix("dt_color_"))


@functools.lru_cache(maxsize=4096)
def submesh_count(rel: str) -> int:
    """How many ``submesh_<i>`` nodes the glb at `rel` has (the uncooker
    writes one per material slot; 0 when it cannot be read)."""
    import json
    data = asset_bytes(rel)
    if not data or data[:4] != b"glTF":
        return 0
    try:
        (n,) = struct.unpack_from("<I", data, 12)
        doc = json.loads(data[20:20 + n])
    except (struct.error, ValueError):
        return 0
    return sum(1 for nd in doc.get("nodes", []) if str(nd.get("name", "")).startswith("submesh_"))


@functools.lru_cache(maxsize=4096)
def material_albedo(mat_path: str, root: str = "3D") -> tuple[str | None, bool]:
    """``(png path, swap_rb)`` of a material's albedo texture, when one can be drawn."""
    data = _bytes(root, mat_path)
    if not data:
        return None, False
    texs = [p for r, p in resource_refs(data)
            if r == root and p.lower().endswith((".tga", ".png"))]

    def score(p: str) -> int:
        stem = p.rsplit("\\", 1)[-1].rsplit(".", 1)[0].lower()
        if stem in _PLACEHOLDERS or stem.endswith(_NOT_ALBEDO):
            return 99
        for i, h in enumerate(_ALBEDO_HINTS):
            if stem.endswith(h):
                return i
        return 50

    for p in sorted(texs, key=score):
        if score(p) == 99:
            break
        found = _texture_rel(root, p)
        if found[0]:
            return found
    return None, False


def _texture_rel(root: str, p: str) -> tuple[str | None, bool]:
    rel = f"{root}/" + p.replace("\\", "/").rsplit(".", 1)[0] + ".png"
    if _uncooked(rel):
        return rel, _texture_uncompressed(p)
    if _cooked_key(rel):
        return rel, False                # decoded from the install, true colours
    return None, False


@functools.lru_cache(maxsize=4096)
def material_normal(mat_path: str, root: str = "3D") -> tuple[str | None, bool]:
    """``(png path, swap_rb)`` of a material's normal map (``*_nrm`` / ``*_normal``), if any."""
    data = _bytes(root, mat_path)
    if not data:
        return None, False
    for r, p in resource_refs(data):
        stem = p.rsplit("\\", 1)[-1].rsplit(".", 1)[0].lower()
        is_map = p.lower().endswith((".tga", ".png"))
        if r == root and is_map and stem.endswith(("_nrm", "_normal")):
            found = _texture_rel(root, p)
            if found[0]:
                return found
    return None, False


# --------------------------------------------------------------------------
# transforms
# --------------------------------------------------------------------------

def part_json(p: Part) -> dict:
    """A part as the editors read it (``normal`` only when the material has one)."""
    out = {"mesh": p.mesh, "texture": p.texture, "swap_rb": p.swap_rb,
           "color": p.color, "decal": p.decal, "vfx": p.vfx}
    if p.normal:
        out["normal"], out["normal_swap"] = p.normal, p.normal_swap
    return out


def trs(pos, quat, scale) -> Matrix:
    """Column-major matrix from translation, quaternion (x, y, z, w) and scale."""
    x, y, z, w = quat
    n = math.sqrt(x * x + y * y + z * z + w * w) or 1.0
    x, y, z, w = x / n, y / n, z / n, w / n
    sx, sy, sz = scale
    return (
        (1 - 2 * (y * y + z * z)) * sx, (2 * (x * y + z * w)) * sx, (2 * (x * z - y * w)) * sx, 0.0,
        (2 * (x * y - z * w)) * sy, (1 - 2 * (x * x + z * z)) * sy, (2 * (y * z + x * w)) * sy, 0.0,
        (2 * (x * z + y * w)) * sz, (2 * (y * z - x * w)) * sz, (1 - 2 * (x * x + y * y)) * sz, 0.0,
        float(pos[0]), float(pos[1]), float(pos[2]), 1.0,
    )


def mul(a: Matrix, b: Matrix) -> Matrix:
    """``a * b`` for column-major 4x4 matrices."""
    return tuple(
        sum(a[k * 4 + r] * b[c * 4 + k] for k in range(4))
        for c in range(4) for r in range(4)
    )


# --------------------------------------------------------------------------
# entities
# --------------------------------------------------------------------------

#: Sprites one effect previews with, at most (each emitter is one sprite cloud).
MAX_VFX_SPRITES = 6
#: Particles the editor draws per sprite (`apps/map-editor` shader.wgsl `vs_billboard`).
VFX_PARTICLES = 8


#: FX material inputs that are shader helpers, not the sprite's shape.
_FX_HELPERS = ("square_", "noise", "\\patterns\\", "flowmap", "gradient", "ramp")


@functools.lru_cache(maxsize=1024)
def _fx_sprite(mat_path: str, root: str) -> tuple[str | None, bool]:
    """The texture an effect material draws its particles with: the first
    that is not a helper (a grey square, a noise or flow pattern)."""
    data = _bytes(root, mat_path)
    if not data:
        return None, False
    texs = [p for r, p in resource_refs(data) if r == root and p.lower().endswith((".tga", ".png"))]
    texs.sort(key=lambda p: any(h in p.lower() for h in _FX_HELPERS))
    for p in texs:
        rel = f"{root}/" + p.replace("\\", "/").rsplit(".", 1)[0] + ".png"
        if _uncooked(rel):
            return rel, _texture_uncompressed(p)
        if _cooked_key(rel):
            return rel, False
    return None, False


def _curve_ends(curve) -> tuple[float, float] | None:
    """First and last value of an effect curve (``keys`` of ``[t, v, ...]``)."""
    keys = [k for k in (curve or {}).get("keys") or [] if isinstance(k, list) and len(k) >= 2]
    return (float(keys[0][1]), float(keys[-1][1])) if keys else None


def _particle_params(emitter: dict, effectors: list, rend: dict):
    """``(size m, colour, params)`` of one particle system, for the editor's
    animated preview (`apps/map-editor` shader.wgsl ``vs_billboard``):

    ``params`` = life s, speed m/s, share alive, end size x, start size x,
    flipbook cells across, first frame, last frame, emission box x / y / z
    (half extents, m), pivot v, turn range (rad, ``range_88``). Read from the
    emitter (``range_80`` size, ``range_8`` lifetime, ``value_14`` rate,
    ``value_d0`` speed, ``vec_d8`` box), the renderer's ``uv_4c`` pivot and
    the effectors before it. A preview: forces, damping, and the game's own
    curves are not simulated."""
    def mid(key, lo, hi, default):
        v = emitter.get(key)
        x = sum(v) / len(v) if isinstance(v, list) and v else default
        return min(hi, max(lo, float(x)))

    size = mid("range_80", 0.02, 6.0, 0.5)
    life = mid("range_8", 0.1, 8.0, 1.0)
    rate = float(emitter.get("value_14") or 1.0)
    alive = min(1.0, max(0.12, rate * life / VFX_PARTICLES))
    speed = min(10.0, max(0.0, float(emitter.get("value_d0") or 0.0)))
    box = [min(4.0, abs(float(b))) for b in (emitter.get("vec_d8") or [0.0, 0.0, 0.0])[:3]]
    by = {o.cls.replace("oC3dParticleEffector", "").replace("Settings", ""): o.fields or {}
          for o in effectors}
    s0, s1 = 1.0, 1.0
    if "SizeRamp" in by:
        ends = _curve_ends(by["SizeRamp"].get("size"))
        scale = float(by["SizeRamp"].get("scale") or 1.0)
        if ends:
            s0, s1 = max(0.05, ends[0] * scale), max(0.05, ends[1] * scale)
    cells, f0, f1 = 1.0, 0.0, 0.0
    grid = by.get("SubImageIndex", {}).get("grid_60")
    if isinstance(grid, list) and len(grid) == 2 and grid[1] > 0:
        f0, f1 = float(grid[0]), float(grid[1])
        cells = float(math.ceil(math.sqrt(f1 + 1)))
    color = _ramp_color(by.get("ColorRamp", {}).get("ramp"))
    if color is None and emitter.get("color_kind") == 1:
        c = emitter.get("color_start") or []
        color = _ramp_color({"keys": [[0.0, *c[:3], 1.0]]}) if len(c) >= 3 else None
    pivot = (rend.get("uv_4c") or [0.5, 0.5])[1]
    turn = max(abs(float(x)) for x in (emitter.get("range_88") or [0.0]))
    return size, color, [life, speed, alive, s1, s0, cells, f0, f1, *box, float(pivot),
                         min(6.3, turn)]


def _ramp_color(ramp) -> str | None:
    """An emitter's colour over life (keys ``[t, r, g, b, a]``, HDR) as one
    ``#rrggbb``: its first half averaged, brightest channel scaled to 1."""
    keys = [k for k in (ramp or {}).get("keys") or [] if isinstance(k, list) and len(k) >= 4]
    early = [k for k in keys if k[0] <= 0.5] or keys[:1]
    if not early:
        return None
    rgb = [sum(k[i] for k in early) / len(early) for i in (1, 2, 3)]
    top = max(rgb)
    if top <= 0:
        return None
    return "#" + "".join(f"{round(min(1.0, c / top) * 255):02x}" for c in rgb)


#: Effects a prop plays on an event, not while it stands there: the editor
#: shows only the idle ones (fire, glow, smoke loops).
_EVENT_FX = ("destruct", "explos", "explod", "snuff", "trail", "hit", "impact", "death",
             "_die", "spawn", "appear", "disappear", "_open", "break", "pickup", "activat",
             "_end", "_start", "launch", "shot", "ping", "reveal", "collect", "heal")


def idle_effect(path: str) -> bool:
    low = path.lower().rsplit("\\", 1)[-1]
    return not any(w in low for w in _EVENT_FX)


@functools.lru_cache(maxsize=2048)
def vfx_parts(path: str) -> tuple[tuple[Part, Matrix], ...]:
    """An effect (``FX`` root, ``*.vfx.ot``) as a few sprites: each billboard
    emitter's texture, placed over its emission box and sized by its spawn
    radius. A preview, not the game's simulation (no motion, ramps or timing)."""
    from . import vfx_codec as V

    data = _bytes("FX", path)
    if not data:
        return ()
    try:
        objs = V.decode(data).objects
    except Exception:  # noqa: BLE001 — an effect the codec cannot read only draws nothing
        return ()
    out: list[tuple[Part, Matrix]] = []
    # An effect lists each particle system as its effectors (colour, size,
    # alpha, flipbook ...), then its emitter, then its renderer.
    pending: list = []
    emitter = None
    for o in objs:
        if o.cls == "oC3dParticleEmitterSettings":
            emitter, effectors, pending = o.fields or {}, pending, []
            continue
        if o.cls.startswith("oC3dParticleEffector"):
            pending.append(o)
            continue
        if o.cls != "oI3dBillboardParticleRendererSettings" or emitter is None:
            continue
        rend = o.fields or {}
        tex, swap = _fx_sprite(rend.get("material", ""), rend.get("material_root") or "FX")
        fx = _particle_params(emitter, effectors, rend) if tex else None
        emitter = None
        if fx is None:
            continue
        size, color, params = fx
        m = (size, 0.0, 0.0, 0.0, 0.0, size, 0.0, 0.0, 0.0, 0.0, size, 0.0, 0.0, 0.0, 0.0, 1.0)
        mesh = f"vfx:{tex}?" + ",".join(f"{v:.4g}" for v in params)
        out.append((Part(mesh, tex, swap, color, False, True), m))
        if len(out) >= MAX_VFX_SPRITES:
            break
    return tuple(out)


@functools.lru_cache(maxsize=8192)
def entity_parts(ref: str, depth: int = 0) -> tuple[tuple[Part, Matrix], ...]:
    """What entity `ref` (an ``EntitySettings`` path) draws, in its own space:
    its meshes (or its children's) plus its effects' sprites."""
    if depth > MAX_DEPTH or ref.lower().startswith(_HELPER_ENTITIES):
        return ()
    data = _bytes("EntitySettings", ref)
    if not data:
        return ()
    fx = tuple(p for path in _idle_effects(ref) for p in vfx_parts(path))
    return _entity_meshes(ref, data, depth) + fx[:MAX_VFX_SPRITES]


@functools.lru_cache(maxsize=8192)
def _idle_effects(ref: str, depth: int = 0) -> tuple[str, ...]:
    """The idle effects an entity plays, its child entities' included (a
    torch's flame lives in a child): each path once, in order."""
    data = _bytes("EntitySettings", ref) if depth <= MAX_DEPTH else None
    if not data:
        return ()
    out: list[str] = []
    for r, path in resource_refs(data):
        if r == "FX" and path.lower().endswith(".vfx.ot") and idle_effect(path):
            out.append(path)
        elif r == "EntitySettings" and path.lower() != ref.lower():
            out.extend(_idle_effects(path, depth + 1))
    return tuple(dict.fromkeys(out))


def _entity_meshes(ref: str, data: bytes, depth: int) -> tuple[tuple[Part, Matrix], ...]:
    refs = resource_refs(data)

    own: list[tuple[Part, Matrix]] = []
    meshes = 0
    mesh: str | None = None
    mats: list[str] = []
    seen: set[str] = set()

    def flush():
        if not mesh or mesh in seen:
            return                       # a mesh named again (skeleton / anim refs) draws once
        seen.add(mesh)
        if mesh == _DECAL_MESH:
            tex = next((t for t in map(material_albedo, mats) if t[0]), (None, False))
            if tex[0]:
                own.append((Part(mesh, tex[0], tex[1], None, True), IDENTITY))
            return                       # a decal with nothing to show draws nothing
        if len(mats) > 1 and submesh_count(mesh) == len(mats):
            # One material per sub-mesh, in order (166/166 sampled scenery
            # meshes): each slot drawn with its own texture.
            for i, m in enumerate(mats):
                tex = material_albedo(m)
                nrm = material_normal(m)
                own.append((Part(f"{mesh}#{i}", tex[0], tex[1],
                                 None if tex[0] else material_color(m),
                                 normal=nrm[0], normal_swap=nrm[1]), IDENTITY))
            return
        tex = next((t for t in map(material_albedo, mats) if t[0]), (None, False))
        color = next((c for c in map(material_color, mats) if c), None)
        nrm = next((n for n in map(material_normal, mats) if n[0]), (None, False))
        own.append((Part(mesh, tex[0], tex[1], None if tex[0] else color,
                         normal=nrm[0], normal_swap=nrm[1]), IDENTITY))

    for root, path in refs:
        low = path.lower()
        if root == "3D" and low.endswith(".fbx"):
            flush()
            meshes += 1
            rel = "3D/" + path.replace("\\", "/") + ".glb"
            name = low.rsplit("\\", 1)[-1]
            skip = ("_lod" in name or "_anim_" in name or "\\animations\\" in low
                    or rel.startswith(_HELPER_MESHES))
            mesh = rel if not skip and available(rel) else None
            mats = []
        elif root == "3D" and low.endswith(".mat.ot") and mesh:
            mats.append(path)
    flush()
    if own or meshes:
        return tuple(own)                # an entity with meshes of its own stops here

    for root, path in refs:
        if root == "Ot" and path.lower().endswith(".level.ot"):
            got = level_parts("Ot", path, depth + 1)
            if got:
                return got

    children = [p for r, p in refs if r == "EntitySettings" and p.lower() != ref.lower()]
    out: list[tuple[Part, Matrix]] = []
    for child in children:
        got = entity_parts(child, depth + 1)
        if not got:
            continue
        if _SELECTOR in data:
            return got
        out.extend(got)
        if len(out) >= MAX_PARTS:
            break
    return tuple(out[:MAX_PARTS])


@functools.lru_cache(maxsize=1024)
def level_parts(root: str, path: str, depth: int = 0) -> tuple[tuple[Part, Matrix], ...]:
    """Every part a level's placements draw, in the level's space."""
    data = _bytes(root, path)
    if not data:
        return ()
    return tuple(placed_parts(data, depth))


def placed_parts(level_bytes: bytes, depth: int = 0) -> list[tuple[Part, Matrix]]:
    try:
        placements = LP.decode(level_bytes)
    except LP.LevelPlacementError:
        return []
    out: list[tuple[Part, Matrix]] = []
    for pl in placements:
        m = trs(pl.pos, pl.quat, pl.scale)
        for part, local in entity_parts(pl.ref, depth):
            out.append((part, m if local is IDENTITY else mul(m, local)))
        if len(out) >= MAX_PARTS * 8:
            break
    return out


# --------------------------------------------------------------------------
# page payload
# --------------------------------------------------------------------------

def to_json(parts: list[tuple[Part, Matrix]]) -> dict:
    """Instances grouped per part: ``{"parts": [...], "matrices": [base64 f32]}``."""
    import array
    import base64
    import sys

    order: dict[Part, int] = {}
    mats: list[array.array] = []
    for part, m in parts:
        i = order.get(part)
        if i is None:
            i = order[part] = len(mats)
            mats.append(array.array("f"))
        mats[i].extend(m)
    if sys.byteorder != "little":
        for a in mats:
            a.byteswap()
    return {
        "parts": [part_json(p) for p in order],
        "matrices": [base64.b64encode(a.tobytes()).decode("ascii") for a in mats],
        "instances": len(parts),
    }
