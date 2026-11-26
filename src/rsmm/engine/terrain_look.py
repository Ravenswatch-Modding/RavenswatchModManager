"""What a chapter's ground LOOKS like: its terrain material and blend weights.

Read-only, for the native map editor's renderer (`apps/map-editor`). The game
draws terrain with one `Dt_TerrainBlendFlavor5` material per chapter
(``M_Terrain_<Chapter>.mat.ot``, referenced from the chapter's terrain level):
a ``base`` texture set plus four layers ``R``, ``G``, ``B``, ``A``, each with an
albedo / MRA / normal texture, a UV scale, an albedo tint, a height-blend
multiply/offset and a blend smoothness.

**Material parameters** (static read of the shipped materials, 2026-11-26):
each is ``u32 len, name, u32 name-hash, value``, the value being a float, a
vec2, a vec4 or a texture reference (``u32 n, root, u32 m, path``). Hashes are
per NAME (the same in every material), so a parameter a material leaves out
is simply absent and the shader's default applies.

**Blend weights**: the terrain level binds the material's ``u_mixmap`` to four
generated layers, one per channel. Dark Hills: ``Grass LD Path Difference``
(grass with the paths cut out), ``Tiles Puddles``, ``Tiles Mud``,
``Tiles Puddles``. A ``Tiles <X>`` layer is the painted ``Base <X>`` with the
run's placed tiles blended on top, so for the editor (no tiles placed) the base
layer IS the weight. ``u_colormap`` is the terrain vertex colour the same way.
"""

from __future__ import annotations

import base64
import functools
import re
import struct

from . import map_scene as MS
from . import terrain as TR

#: The terrain shader's channels, in the order the renderer blends them.
SLOTS = ("base", "R", "G", "B", "A")
_PARAM_RE = re.compile(rb"u_[A-Za-z0-9_]+")


class LookError(ValueError):
    pass


def _texture_rel(path: str) -> str:
    """The png the editor's ``/api/file`` serves for a cooked texture path."""
    return "3D/" + path.replace("\\", "/").rsplit(".", 1)[0] + ".png"


def material_params(data: bytes) -> dict[str, object]:
    """``{name: [floats] | {"texture": rel}}`` for every shader parameter."""
    found: list[tuple[int, int, str]] = []
    for m in _PARAM_RE.finditer(data):
        s = m.start()
        if s < 4:
            continue
        n = struct.unpack_from("<I", data, s - 4)[0]
        if 3 < n < 64 and s + n <= len(data) and data[s:s + n].isascii():
            name = data[s:s + n].decode("ascii")
            if _PARAM_RE.fullmatch(data[s:s + n]):
                found.append((s, s + n, name))
    out: dict[str, object] = {}
    for i, (_s, e, name) in enumerate(found):
        end = found[i + 1][0] - 4 if i + 1 < len(found) else len(data)
        seg = data[e + 4:end]                       # skip the name hash
        refs = MS.resource_refs(seg)
        tex = next((p for _r, p in refs if p.lower().endswith((".tga", ".png"))), None)
        if tex:
            out[name] = {"texture": _texture_rel(tex)}
            continue
        vals = []
        for k in range(0, min(len(seg) - 3, 16), 4):
            v = struct.unpack_from("<f", seg, k)[0]
            if v != v or abs(v) > 1e6:              # NaN / garbage: the value ended
                break
            vals.append(round(v, 6))
        out[name] = vals
    return out


def _slot(params: dict, slot: str) -> dict:
    def tex(kind):
        v = params.get(f"u_{kind}_{slot}")
        return v.get("texture") if isinstance(v, dict) else None

    def num(key, n, default):
        v = params.get(key)
        return list(v[:n]) if isinstance(v, list) and len(v) >= n else default

    return {
        "albedo": tex("ALB"), "mra": tex("MRA"), "normal": tex("NRM"),
        "uv_scale": num(f"u_UV_{slot}_scale", 1, [0.1])[0],
        "color": num(f"u_ALB_{slot}_color", 4, [0.5, 0.5, 0.5, 1.0]),
        "shadow_color": num(f"u_ShadowColor_{slot}", 4, None),
        "height_mul_offset": num(f"u_HeightMulOffset_{slot}", 2, [1.0, 0.0]),
        "shadow_step": num(f"u_HeightShadowStep_{slot}", 1, [0.0])[0],
        "smooth": num(f"u_Layer_Smooth_{slot}", 1, [0.1])[0],
    }


def _bound_layers(level: bytes, sampler: bytes, want: int) -> list[str] | None:
    """The layer names bound to ``sampler`` in the terrain-mesh binding: the
    occurrence followed by a ``u_heightmap`` (the decoration and debug
    bindings that also use ``u_mixmap`` are not)."""
    for m in re.finditer(re.escape(sampler), level):
        window = level[m.end():m.end() + 400]
        names = []
        k = 0
        while len(names) < want and k + 4 <= len(window):
            n = struct.unpack_from("<I", window, k)[0]
            if 2 <= n <= 64 and k + 4 + n <= len(window):
                s = window[k + 4:k + 4 + n]
                if s.isascii() and all(32 <= c < 127 for c in s) and not s.startswith(b"u_"):
                    names.append(s.decode("ascii").strip())
                    k += 4 + n
                    continue
                if s.startswith(b"u_"):
                    break
            k += 1
        if len(names) >= (want if want == 1 else 3) and b"u_heightmap" in window:
            return names
    return None


#: Painted ``Base <X>`` float layers that are not surface weights.
_NOT_SURFACE = ("Height", "Flying", "Water", "Meshes", "Fog", "BlendMask", "Splashes")


def surface_layers(t: TR.Terrain) -> list[str]:
    """The painted surface-weight layers in file order. ``Tiles R/G/B/A`` are
    these by POSITION: Avalon paints Soil, Road, Creep, Grass, and its material
    takes Creep (Tiles B) as R, Grass (Tiles A) as G, Soil and Road (Tiles R, G)
    as B and A, which is exactly what its textures are named."""
    return [n for n, lay in t.layers.items()
            if n.startswith("Base ") and lay.kind == "float"
            and not any(w in n for w in _NOT_SURFACE)]


def _resolve(t: TR.Terrain, name: str, n: int) -> list[float] | None:
    """A generated layer's value for the editor, from the painted layers."""
    def get(nm):
        lay = t.layers.get(nm)
        return TR.resample(lay, n) if lay is not None and lay.kind == "float" else None

    # A generated "... Difference" layer is recomputed even when a layer of that
    # name is stored: Dark Hills stores a near-empty one (0.6% coverage) while
    # the generator's output is grass minus paths (~53%).
    if (v := get(name)) is not None and not name.endswith(" Difference") \
            and not name.startswith("LD Path Difference "):
        return v
    if name.startswith("Tiles "):
        x = name[6:]
        if (v := get("Base " + x)) is not None:
            return v
        if x in ("R", "G", "B", "A"):
            surf = surface_layers(t)
            i = "RGBA".index(x)
            return get(surf[i]) if i < len(surf) else None
    if name.startswith("LD Path Difference "):
        # Tiles <X> with the level-design path cut out.
        va = _resolve(t, "Tiles " + name.rsplit(" ", 1)[1], n)
        vb = get("LD Path")
        if va is not None and vb is not None:
            return [max(0.0, x - y) for x, y in zip(va, vb, strict=True)]
        return va
    if name.endswith(" Difference"):
        # "<A> <B> Difference" = A with B cut out: try every split point.
        words = name[: -len(" Difference")].split(" ")
        for i in range(1, len(words)):
            a, b = " ".join(words[:i]), " ".join(words[i:])
            va = get(a) or get("Base " + a)
            vb = get(b) or get("Base " + b)
            if va is not None and vb is not None:
                return [max(0.0, x - y) for x, y in zip(va, vb, strict=True)]
    return None


def source_layer(t: TR.Terrain, name: str) -> str | None:
    """The PAINTED layer a generated channel layer is made from: the one a
    brush has to paint for that channel to change (Dark Hills' "Grass LD Path
    Difference" is painted as "Base Grass"; Avalon's "Tiles B" as its third
    surface layer)."""
    def painted(nm):
        lay = t.layers.get(nm)
        return nm if lay is not None and lay.kind == "float" else None

    if name.startswith("LD Path Difference "):
        return source_layer(t, "Tiles " + name.rsplit(" ", 1)[1])
    if name.endswith(" Difference"):
        words = name[: -len(" Difference")].split(" ")
        for i in range(1, len(words)):
            a = " ".join(words[:i])
            if hit := painted(a) or painted("Base " + a):
                return hit
        return None
    if name.startswith("Tiles "):
        x = name[6:]
        if hit := painted("Base " + x):
            return hit
        if x in ("R", "G", "B", "A"):
            surf = surface_layers(t)
            i = "RGBA".index(x)
            return surf[i] if i < len(surf) else None
    return painted(name)


def _rgba(t: TR.Terrain, name: str, n: int) -> bytes | None:
    for nm in (name, "Base " + name[6:] if name.startswith("Tiles ") else name):
        lay = t.layers.get(nm)
        if lay is not None and lay.kind == "rgba":
            s, src, out = lay.size, lay.cells, bytearray(n * n * 4)
            for r in range(n):
                row = min(s - 1, int((r + 0.5) * s / n)) * s
                for c in range(n):
                    i = (row + min(s - 1, int((c + 0.5) * s / n))) * 4
                    out[(r * n + c) * 4:(r * n + c) * 4 + 4] = src[i:i + 4]
            return bytes(out)
    return None


@functools.lru_cache(maxsize=4)
def look_json(chapter, n: int = 512) -> dict:
    """The terrain material and per-cell blend weights of ``chapter``."""
    from . import map_editor as ME

    dec = ME.terrain_decoded(chapter)
    level = ME._shipped(dec) if dec else None
    if level is None:
        raise LookError(f"{chapter.key}: no terrain level in the game install")
    mats = [p for r, p in MS.resource_refs(level)
            if r == "3D" and "\\M_Terrain_" in p and "_Fog" not in p]
    if not mats:
        raise LookError(f"{chapter.key}: the terrain level names no M_Terrain_ material")
    mat = MS._bytes("3D", mats[0])
    if mat is None:
        raise LookError(f"{chapter.key}: {mats[0]} is not in the game install")
    params = material_params(mat)
    t = ME._terrain_all(chapter)

    channels = _bound_layers(level, b"u_mixmap", 4) or []
    weights = bytearray(n * n * 4)
    used = []
    for ci, name in enumerate(channels[:4]):
        v = _resolve(t, name, n)
        used.append(name if v is not None else None)
        if v is None:
            continue
        for i, x in enumerate(v):
            weights[i * 4 + ci] = min(255, max(0, round(x * 255)))
    color_names = _bound_layers(level, b"u_colormap", 1) or ["Base Terrain Vertex Color"]
    color = _rgba(t, color_names[0], n)
    water = None
    wmat = next((p for r, p in MS.resource_refs(level) if r == "3D" and "\\M_Water" in p), None)
    wdata = MS._bytes("3D", wmat) if wmat else None
    if wdata:
        wp = material_params(wdata)

        def col(key):
            v = wp.get(key)
            return v[:4] if isinstance(v, list) and len(v) >= 4 else None

        water = {"material": wmat, "deep": col("u_color_A"), "shallow": col("u_color_B"),
                 "banks": col("u_Banks_colorMul"), "outline": col("u_Banks_outlineCol")}
    return {
        "grid": n,
        "material": mats[0],
        "water": water,
        "slots": {s: _slot(params, s) for s in SLOTS},
        "slope_min": (params.get("u_SlopeLimitation_Min") or [None])[0],
        "channels": channels[:4],
        "channels_resolved": used,
        "channel_sources": [source_layer(t, c) for c in channels[:4]],
        "color_layer": next((nm for nm in (color_names[0], "Base Terrain Vertex Color")
                             if (lay := t.layers.get(nm if not nm.startswith("Tiles ")
                                                     else "Base " + nm[6:])) is not None
                             and lay.kind == "rgba"), None),
        "weights": base64.b64encode(bytes(weights)).decode("ascii"),
        "color": base64.b64encode(color).decode("ascii") if color else None,
    }
