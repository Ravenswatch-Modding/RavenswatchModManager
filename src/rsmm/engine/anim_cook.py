"""Cook a glTF animation into the game's ``oCAnimation`` format.

The inverse of the preview exporter in :mod:`cooked_schemas.animation`. Every
quantizer here is proven against the shipped corpus (all 2240 animations,
2026-09-24): translation and scale re-encode byte-identically, and rotation
matches on every keyframe except ~0.16% near-ties (two components equal to
five decimals), where the other axis is dropped instead. Those decode to the
SAME rotation, and :func:`cook` keeps the template's original blob whenever
the rotation is unchanged, so an unedited round trip is byte-identical.

On-disk encoding (``oCAnimationTrack``):

* time — u16 in a u32 wire, ``round(seconds / duration * 65535)``;
* translation / scale — 3 × i16, ``round(clamp(v, -32, 31) * 1024)``;
* rotation — 48-bit "smallest three": the largest-magnitude component is
  dropped (index in bits 45-46, sign flipped so it is positive) and the other
  three are signed 15-bit fields scaled by ``sqrt(2) * 16383``.

A cooked clip is built ON A TEMPLATE — the shipped clip being replaced — which
supplies the bone order, the per-clip tail fields and any bone the glTF does
not animate. Bones are matched by NAME: a glTF node animated by a channel must
be named exactly like the game bone (``DEF-LEG-TOP.R``), which is what the
exporter writes and what a Blender armature keeps on export.
"""

from __future__ import annotations

import json
import struct
from dataclasses import replace

from .cooked_schemas import animation as A

_SC = A._QUAT_SCALE
_TIME_MAX = 65535


class AnimCookError(ValueError):
    pass


# --------------------------------------------------------------------------
# quantizers (proven against the corpus; see module docstring)
# --------------------------------------------------------------------------

def quant_time(t: float, duration: float) -> int:
    if duration <= 0:
        return 0
    return max(0, min(_TIME_MAX, int(round(t / duration * _TIME_MAX))))


def quant_ts(v) -> bytes:
    return struct.pack("<hhh", *[
        max(-32768, min(32767, int(round(max(-32.0, min(31.0, float(c))) * 1024))))
        for c in v])


def quant_quat(q) -> bytes:
    n = sum(c * c for c in q) ** 0.5 or 1.0
    q = [c / n for c in q]
    idx = max(range(4), key=lambda i: abs(q[i]))
    s = -1.0 if q[idx] < 0 else 1.0
    f = [int(round(q[i] * s * _SC)) & 0x7FFF for i in range(4) if i != idx]
    raw = (idx << 45) | (f[0] << 30) | (f[1] << 15) | f[2]
    return struct.pack("<IH", raw & 0xFFFFFFFF, raw >> 32)


def _same_rotation(a: bytes, b: bytes) -> bool:
    qa, qb = A._decode_quat(a), A._decode_quat(b)
    return abs(sum(x * y for x, y in zip(qa, qb, strict=True))) > 1 - 1e-6


# --------------------------------------------------------------------------
# glTF animation reader
# --------------------------------------------------------------------------

_COMP = {5126: ("f", 4), 5120: ("b", 1), 5121: ("B", 1), 5122: ("h", 2), 5123: ("H", 2)}
_NORM = {5120: 127.0, 5121: 255.0, 5122: 32767.0, 5123: 65535.0}
_NCOMP = {"SCALAR": 1, "VEC3": 3, "VEC4": 4}


def _read_glb(glb: bytes) -> tuple[dict, bytes]:
    from .gltf import read_glb
    try:
        return read_glb(glb)
    except ValueError as e:              # the cooker reports its own error type
        raise AnimCookError(str(e)) from e


def _accessor(doc: dict, binc: bytes, i: int) -> list:
    acc = doc["accessors"][i]
    if "sparse" in acc:
        raise AnimCookError("sparse accessors are not supported")
    fmt, size = _COMP[acc["componentType"]]
    n = _NCOMP[acc["type"]]
    view = doc["bufferViews"][acc["bufferView"]]
    base = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    stride = view.get("byteStride") or size * n
    norm = acc.get("normalized") and acc["componentType"] in _NORM
    out = []
    for k in range(acc["count"]):
        vals = struct.unpack_from(f"<{n}{fmt}", binc, base + k * stride)
        if norm:
            vals = tuple(max(-1.0, v / _NORM[acc["componentType"]]) for v in vals)
        out.append(float(vals[0]) if n == 1 else tuple(float(v) for v in vals))
    return out


def read_gltf_animation(glb: bytes, name: str | None = None) -> dict[str, dict[str, list]]:
    """``{bone: {"translation"|"rotation"|"scale": [(seconds, value), ...]}}``.

    Takes the animation called ``name``, or the first one. CUBICSPLINE
    samplers keep their value (the middle of each in-tangent/value/out-tangent
    triple); the engine interpolates linearly between keys.
    """
    doc, binc = _read_glb(glb)
    anims = doc.get("animations") or []
    if not anims:
        raise AnimCookError("glb contains no animation")
    anim = next((a for a in anims if a.get("name") == name), None) if name else anims[0]
    if anim is None:
        raise AnimCookError(f"no animation named {name!r}; have "
                            f"{[a.get('name') for a in anims]}")
    nodes = doc.get("nodes") or []
    out: dict[str, dict[str, list]] = {}
    for ch in anim["channels"]:
        tgt = ch.get("target") or {}
        path = tgt.get("path")
        if path not in ("translation", "rotation", "scale") or "node" not in tgt:
            continue
        bone = nodes[tgt["node"]].get("name")
        if not bone:
            raise AnimCookError(f"animated node {tgt['node']} has no name to match a bone")
        smp = anim["samplers"][ch["sampler"]]
        times = _accessor(doc, binc, smp["input"])
        vals = _accessor(doc, binc, smp["output"])
        if smp.get("interpolation") == "CUBICSPLINE":
            vals = vals[1::3]
        if len(vals) != len(times):
            raise AnimCookError(f"{bone}.{path}: {len(times)} times but {len(vals)} values")
        out.setdefault(bone, {})[path] = list(zip(times, vals, strict=True))
    return out


# --------------------------------------------------------------------------
# cooker
# --------------------------------------------------------------------------

def _stream(keys, template_times, template_vals, duration, quant, same):
    times = [quant_time(t, duration) for t, _ in keys]
    vals = [quant(v) for _, v in keys]
    # Unedited round trip: keep the template's own blob where the value is the
    # same (a near-tie rotation re-encodes to different, equivalent bytes).
    if times == template_times and len(vals) == len(template_vals):
        vals = [o if (n != o and same(n, o)) else n
                for n, o in zip(vals, template_vals, strict=True)]
    return times, vals


def cook(glb: bytes, template_payload: bytes, *, name: str | None = None,
         strict: bool = False) -> tuple[bytes, list[str]]:
    """Cook the glTF animation onto ``template_payload`` (a shipped clip).

    Returns ``(payload, notes)``. A template bone the glTF does not animate
    keeps its first template key as a constant pose (listed in ``notes``);
    with ``strict`` that is an error. A glTF bone the template does not have
    is an error: the skeleton would have no joint to drive.
    """
    tpl = A.parse_payload(template_payload)
    src = read_gltf_animation(glb, name)
    names = {t.name for t in tpl.tracks}
    unknown = sorted(set(src) - names)
    if unknown:
        raise AnimCookError(f"glTF animates bone(s) the target skeleton lacks: {unknown[:8]}")
    duration = max((t for ch in src.values() for keys in ch.values() for t, _ in keys),
                   default=0.0)
    if duration <= 0:
        raise AnimCookError("animation has no duration (all keys at t=0)")
    # A clip's stored length is not always its last key time (60% of shipped
    # clips stop keying before the end), so a glb from our own exporter carries
    # the real length in extras; keys added past it extend the clip.
    declared = declared_duration(glb, name)
    if declared and declared > duration - 1e-6:
        duration = declared

    notes: list[str] = []
    tracks = []
    for tr in tpl.tracks:
        ch = src.get(tr.name, {})
        new = replace(tr, duration=duration)
        for path, tattr, vattr, quant, same, dec in (
                ("translation", "t_times", "t_values", quant_ts, lambda a, b: a == b,
                 A._decode_trans_scale),
                ("rotation", "r_times", "r_values", quant_quat, _same_rotation, A._decode_quat),
                ("scale", "s_times", "s_values", quant_ts, lambda a, b: a == b,
                 A._decode_trans_scale)):
            keys = ch.get(path)
            if keys is None:
                old = getattr(tr, vattr)
                if not old:
                    continue
                if strict:
                    raise AnimCookError(f"{tr.name}.{path} is not animated in the glTF")
                notes.append(f"{tr.name}.{path}: not in glTF, held at the template's first key")
                keys = [(0.0, dec(old[0]))]
            keys = sorted(keys, key=lambda k: k[0])
            times, vals = _stream(keys, getattr(tr, tattr), getattr(tr, vattr),
                                  duration, quant, same)
            new = replace(new, **{tattr: times, vattr: vals})
        tracks.append(new)

    return A.emit_payload(_with_tail(tpl, tracks, duration)), notes


def _with_tail(tpl: A.Animation, tracks: list, duration: float) -> A.Animation:
    """``tpl`` with new tracks, its length fields moved to ``duration``."""
    five = list(tpl.five_f32)
    if abs(five[1] - tpl.duration) < 1e-4:
        five[1] = duration
    frame_step = duration if abs(tpl.frame_step - tpl.duration) < 1e-4 else tpl.frame_step
    return replace(tpl, tracks=tracks, five_f32=tuple(five), frame_step=frame_step)


#: The bone whose translation the game turns into the hero's movement during
#: an ability (root motion): every shipped hero rig has it, keyed along +Z
#: (forward). Kintaro's three basic attacks carry him 2.4, 1.8 and 3.7 m, most
#: of it in the first half of each clip; a clip that leaves it still plays in
#: place (Jack's chop, playtest 2026-11-23).
ROOT_MOTION_BONE = "DEF.RootMotion"
_TRAVEL_FPS = 30


def with_travel(payload: bytes, metres: float, until: float | None = None) -> bytes:
    """``payload`` (a cooked clip) whose root motion carries the hero
    ``metres`` forward, easing out (fast start, slow finish, as a lunge),
    done at ``until`` seconds (default: 55% of the clip) and held after. The
    root's sideways and vertical keys keep their first value."""
    if not -31 <= metres <= 31:
        raise AnimCookError(f"travel {metres} m is out of range (-31..31)")
    anim = A.parse_payload(payload)
    duration = anim.duration
    track = next((t for t in anim.tracks if t.name == ROOT_MOTION_BONE), None)
    if track is None:
        raise AnimCookError(f"the clip has no {ROOT_MOTION_BONE} bone to carry the travel")
    end = 0.55 * duration if until is None else float(until)
    if not 0 < end <= duration:
        raise AnimCookError(f"travel ends at {end} s, outside the clip (0..{duration:.2f} s)")
    x, y, z = A._decode_trans_scale(track.t_values[0]) if track.t_values else (0.0, 0.0, 0.0)
    n = max(2, int(end * _TRAVEL_FPS))
    keys = [(end * i / n, (x, y, z + metres * (1 - (1 - i / n) ** 2))) for i in range(n + 1)]
    if end < duration:
        keys.append((duration, keys[-1][1]))
    times, vals = _stream(keys, track.t_times, track.t_values, duration, quant_ts,
                          lambda a, b: a == b)
    tracks = [replace(t, t_times=times, t_values=vals) if t is track else t
              for t in anim.tracks]
    return A.emit_payload(replace(anim, tracks=tracks))


def glb_rest(glb: bytes) -> dict[str, tuple]:
    """Each named node's rest ``(translation, rotation xyzw, scale)``."""
    doc, _ = _read_glb(glb)
    return {nd["name"]: (tuple(nd.get("translation", (0.0, 0.0, 0.0))),
                         tuple(nd.get("rotation", (0.0, 0.0, 0.0, 1.0))),
                         tuple(nd.get("scale", (1.0, 1.0, 1.0))))
            for nd in doc.get("nodes") or [] if nd.get("name")}


def glb_rest_translations(glb: bytes) -> dict[str, tuple]:
    """Each named node's local rest translation in a glb: the space its clips
    key translations in, as :func:`retarget` takes it."""
    return {n: t for n, (t, _, _) in glb_rest(glb).items()}


def cook_own(glb: bytes, bones: list[str], tail_payload: bytes, *,
             name: str | None = None, rest: dict[str, tuple] | None = None) -> bytes:
    """Cook a glTF animation for a skeleton of its own (one built by
    :func:`rsmm.engine.skeleton_codec.from_glb`), with no shipped clip
    supplying the tracks.

    ``bones``: the skeleton's bone names in skeleton order; one track each, in
    that order. A bone the clip does not animate holds the glb's rest pose
    (identity for a bone the glb has no node for, such as an added root).
    ``rest``: rest poses (as :func:`glb_rest` gives them) for bones the glb
    has no node for -- the model's own, when the clip comes from a separate
    animation library that lacks some of its bones.
    ``tail_payload``: any shipped clip; only its per-clip tail fields
    (``five_f32``, ``frame_step``, the version struct) and the track prelude
    are borrowed. Unproven in game."""
    tpl = A.parse_payload(tail_payload)
    src = read_gltf_animation(glb, name)
    unknown = sorted(set(src) - set(bones))
    if unknown:
        raise AnimCookError(f"glTF animates bone(s) the skeleton lacks: {unknown[:8]}")
    duration = max((t for ch in src.values() for keys in ch.values() for t, _ in keys),
                   default=0.0)
    declared = declared_duration(glb, name)
    if declared and declared > duration - 1e-6:
        duration = declared
    if duration <= 0:
        raise AnimCookError("animation has no duration (all keys at t=0)")
    rest = {**(rest or {}), **glb_rest(glb)}
    ident = ((0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0))
    first = tpl.tracks[0]
    tracks = []
    for bone in bones:
        ch, r = src.get(bone, {}), rest.get(bone, ident)
        streams = {}
        for k, (path, quant) in enumerate((("translation", quant_ts),
                                           ("rotation", quant_quat),
                                           ("scale", quant_ts))):
            keys = sorted(ch.get(path) or [(0.0, r[k])], key=lambda kv: kv[0])
            streams[path] = ([quant_time(t, duration) for t, _ in keys],
                             [quant(v) for _, v in keys])
        tracks.append(A.AnimationTrack(
            res_prelude=first.res_prelude, leading_u32=first.leading_u32, name=bone,
            t_times=streams["translation"][0], t_values=streams["translation"][1],
            r_times=streams["rotation"][0], r_values=streams["rotation"][1],
            s_times=streams["scale"][0], s_values=streams["scale"][1],
            duration=duration))
    return A.emit_payload(_with_tail(tpl, tracks, duration))


#: A bone whose rest translation is shorter than this (metres) has no length to
#: scale: the root motion bone (rest 0) and helpers sitting on their parent.
#: Their keys are left alone, so root motion and lunge distances never change.
RETARGET_MIN_LENGTH = 0.01


def retarget(payload: bytes, rig_rest: dict[str, tuple], own_rest: dict[str, tuple]) -> bytes:
    """A rig's clip re-proportioned for a body with its own bone lengths.

    Measured on the corpus (Kintaro's 57 on-foot clips, 2026-11-15): a clip
    keys every bone's TRANSLATION in parent-local space (145 bones move during
    a clip: the rig stretches), at the rig's own lengths -- so played on a
    skeleton of other proportions it would pull every bone back to the rig's
    length and stretch the mesh. Rotation keys are absolute parent-local too,
    and are kept: a body built on the rig's armature points its bones the same
    way (Jack against Kintaro: rest swing median 4.6 deg, twist under 5 deg),
    so it takes the rig's poses as they are.

    Each translation key is scaled by ``|own rest| / |rig rest|`` for its
    bone, keeping the key's direction: rest maps to rest and a stretch stays
    proportional. Bones missing from either rest, or shorter than
    :data:`RETARGET_MIN_LENGTH`, and bones whose length did not change keep
    their bytes, so the same skeleton in gives the same clip out. Unproven in
    game."""
    anim = A.parse_payload(payload)
    changed = False
    for t in anim.tracks:
        a, b = rig_rest.get(t.name), own_rest.get(t.name)
        if a is None or b is None or not t.t_values:
            continue
        la = sum(c * c for c in a) ** 0.5
        if la < RETARGET_MIN_LENGTH:
            continue
        s = sum(c * c for c in b) ** 0.5 / la
        if abs(s - 1.0) < 1e-4:
            continue
        t.t_values = [quant_ts([c * s for c in A._decode_trans_scale(v)]) for v in t.t_values]
        changed = True
    return A.emit_payload(anim) if changed else payload


def declared_duration(glb: bytes, name: str | None = None) -> float | None:
    """The clip length our exporters recorded, if any: per-animation
    ``extras.rsmm.duration`` (character export) or the file-level
    ``extras.rsmm.decoded.duration`` (single-clip export)."""
    doc, _ = _read_glb(glb)
    anims = doc.get("animations") or []
    anim = next((a for a in anims if a.get("name") == name), None) if name else \
        (anims[0] if anims else None)
    d = (((anim or {}).get("extras") or {}).get("rsmm") or {}).get("duration")
    if d is None:
        d = (((doc.get("extras") or {}).get("rsmm") or {}).get("decoded") or {}).get("duration")
    return float(d) if isinstance(d, (int, float)) and d > 0 else None


# --------------------------------------------------------------------------
# comparison
# --------------------------------------------------------------------------

def _curve(times, vals, duration, dec):
    """A stream decoded once: ``(seconds, values)``, or None when it has no keys."""
    return (A._decode_times(times, duration), [dec(v) for v in vals]) if times else None


def _sample(curve, t, quat):
    if curve is None:
        return None
    import bisect
    secs, decoded = curve
    i = bisect.bisect_left(secs, t)
    if i == 0:
        return decoded[0]
    if i == len(secs):
        return decoded[-1]
    a = (t - secs[i - 1]) / ((secs[i] - secs[i - 1]) or 1.0)
    x, y = decoded[i - 1], decoded[i]
    if quat and sum(p * q for p, q in zip(x, y, strict=True)) < 0:
        y = tuple(-c for c in y)
    v = tuple(p + (q - p) * a for p, q in zip(x, y, strict=True))
    if quat:
        n = sum(c * c for c in v) ** 0.5 or 1.0
        v = tuple(c / n for c in v)
    return v


def pose_difference(payload_a: bytes, payload_b: bytes, fps: int = 60) -> tuple[float, float]:
    """``(max rotation difference in degrees, max translation difference in metres)``
    between two clips, sampled at ``fps`` over the longer one; a bone missing from
    either side, or a length change, counts as a large difference."""
    import math
    a, b = A.parse_payload(payload_a), A.parse_payload(payload_b)
    if abs(a.duration - b.duration) > 1e-3:
        return 180.0, float("inf")
    tb = {t.name: t for t in b.tracks}
    if set(tb) != {t.name for t in a.tracks}:
        return 180.0, float("inf")
    n = max(2, int(max(a.duration, b.duration) * fps) + 1)
    worst_rot = worst_t = 0.0
    for tr in a.tracks:
        o = tb[tr.name]
        curves = [(_curve(x.r_times, x.r_values, x.duration, A._decode_quat),
                   _curve(x.t_times, x.t_values, x.duration, A._decode_trans_scale),
                   _curve(x.s_times, x.s_values, x.duration, A._decode_trans_scale))
                  for x in (tr, o)]
        for k in range(n):
            t = a.duration * k / (n - 1)
            qa, qb = _sample(curves[0][0], t, True), _sample(curves[1][0], t, True)
            if qa and qb:
                dot = min(1.0, abs(sum(x * y for x, y in zip(qa, qb, strict=True))))
                worst_rot = max(worst_rot, math.degrees(2 * math.acos(dot)))
            for c in (1, 2):
                va, vb = _sample(curves[0][c], t, False), _sample(curves[1][c], t, False)
                if va and vb:
                    worst_t = max(worst_t, max(abs(x - y) for x, y in zip(va, vb, strict=True)))
    return worst_rot, worst_t
