"""A chapter's light and fog, for the native map editor's renderer.

Read-only. Static RE, 2026-11-26 (build b9c3):

**Sun** — the chapter's ``<Chapter>_Day_Cycle_Manager`` entity holds named
``oCEntityValueUnion`` values: ``Sun DAY Color`` (type 4, RGBA),
``Sunlight Intensity Value`` (8 Dark Hills, 12 Avalon, 10 Storm Island),
``Sun Zenith Value`` (0..1) and ``Light Node Y Rotation`` (degrees).

**Ambient** — ``oCDtAmbientLightingSettings::Serialize`` (vftable 0x140f1db98
slot 3, FUN_140295b40) reads a colour at +0x20, then (version > 0) +0x10 and
+0x30, then floats +0x40 and (version > 1) +0x44; an old version copies +0x20
into the other two. So +0x20 is the original single ambient colour — the
HORIZON (``g_AmbientHorColor`` in the lighting shaders) — and +0x10 / +0x30 the
TOP and BOTTOM added around it; the floats are ``g_AmbientMul`` and
``g_AmbientHorPow``.

**Fog** — ``oCDtFogSettings::Serialize`` (vftable 0x140f1e748 slot 3,
FUN_140296100): floats +0x10/+0x14/+0x18 (version-gated), colours
+0x1c/+0x2c/+0x3c, a vec3 +0x4c, a vec2 +0x58, a vec3 +0x60. Shader names seen
in the DXIL: ``g_fFogIntensity``, ``g_fDarkFog``, ``g_cFogMidColor``,
``g_v3FogHeightRange``. Read as: intensity / dark-fog / brightness, BOTTOM /
MID / TOP colours (alpha = strength), HEIGHT RANGE (top, mid, bottom: Dark
Hills 5, 1, -15), noise scroll, DISTANCE RANGE (start, mid, end). The field
values are exact; which one feeds which shader term is inferred, so the editor
shows them in an editable panel rather than hiding a guess.

In a cooked stream each settings object is ``class index, u8 base, fields``;
inside an entity the same payload follows a ``1111bbaa`` BEGIN marker and its
u32 class index. A day-cycle entity carries a night pair then a day pair
(fog, then ambient); the editor shows the DAY one, i.e. the last fog and the
ambient after it.
"""

from __future__ import annotations

import functools
import re
import struct

from . import corpus
from . import talent_values as TV

_SUN_LABELS = {
    "Sun DAY Color": "color",
    "Sunlight Intensity Value": "intensity",
    "Sun Zenith Value": "zenith",
    "Light Node Y Rotation": "yaw",
}


class RenderLookError(ValueError):
    pass


def _floats(d: bytes, o: int, n: int) -> list[float]:
    return [round(struct.unpack_from("<f", d, o + 4 * i)[0], 5) for i in range(n)]


def _value_after(d: bytes, label_end: int) -> list[float] | None:
    """The inline value of the picker/union node after a label (f32 or colour)."""
    p1 = d.find(TV._BEGIN, label_end, label_end + 64)
    if p1 < 0:
        return None
    p2 = d.find(TV._BEGIN, p1 + 8, p1 + 80)
    end = d.find(TV._END, p2 + 8) if p2 >= 0 else -1
    if p2 < 0 or end < 0 or any(d[p1 + 8:p2]):   # sourced elsewhere: not inline
        return None
    tc = struct.unpack_from("<I", d, p2 + 8)[0]
    body = d[p2 + 16:end]
    if tc == 0 and len(body) == 4:
        return _floats(body, 0, 1)
    if tc == 4 and len(body) == 16:
        return _floats(body, 0, 4)
    return None


def ambient(d: bytes, o: int) -> dict:
    """An ``oCDtAmbientLightingSettings`` payload's fields, at field offset ``o``."""
    return {"horizon": _floats(d, o, 4), "top": _floats(d, o + 16, 4),
            "bottom": _floats(d, o + 32, 4), "mul": _floats(d, o + 48, 1)[0],
            "horizon_pow": _floats(d, o + 52, 1)[0]}


def fog(d: bytes, o: int) -> dict:
    """An ``oCDtFogSettings`` payload's fields, at field offset ``o``."""
    f = _floats(d, o, 3)
    return {"intensity": f[0], "dark": f[1], "brightness": f[2],
            "bottom": _floats(d, o + 12, 4), "mid": _floats(d, o + 28, 4),
            "top": _floats(d, o + 44, 4), "heights": _floats(d, o + 60, 3),
            "noise": _floats(d, o + 72, 2), "distances": _floats(d, o + 80, 3)}


def _day_cycle_path(chapter) -> str | None:
    from .asset_map import decoded_to_encoded

    want = chapter.key.replace("_", "").lower()
    hits = sorted(k for k in decoded_to_encoded()
                  if k.startswith("EntitySettings/")
                  and k.endswith("Day_Cycle_Manager.entity.ot.EntitySettingsResource.gen")
                  and want in k.replace("_", "").lower())
    return hits[0] if hits else None


@functools.lru_cache(maxsize=8)
def environment_json(chapter) -> dict:
    """Sun, ambient and fog of ``chapter`` by day."""
    path = _day_cycle_path(chapter)
    d = corpus.read(path) if path else None
    if d is None:
        raise RenderLookError(f"{chapter.key}: no day-cycle manager entity in the game install")
    names = TV.class_names(d) or []
    sun: dict = {}
    for label, key in _SUN_LABELS.items():
        for m in re.finditer(re.escape(label.encode()), d):
            s = m.start()
            if s < 4 or struct.unpack_from("<I", d, s - 4)[0] != len(label):
                continue
            v = _value_after(d, m.end())
            if v is not None:
                sun[key] = v if key == "color" else v[0]
                break
    fogs, ambs = [], []
    for m in re.finditer(re.escape(TV._BEGIN), d):
        cn = TV._class_at(d, names, m.start())
        if cn == "oCDtFogSettings":
            fogs.append((m.start(), fog(d, m.start() + 9)))
        elif cn == "oCDtAmbientLightingSettings":
            ambs.append((m.start(), ambient(d, m.start() + 9)))
    day_fog = fogs[-1] if fogs else None
    day_amb = None
    if day_fog:
        day_amb = next((a for a in ambs if a[0] > day_fog[0]), None)
    if day_amb is None and ambs:
        day_amb = ambs[-1]
    return {"source": path, "sun": sun,
            "ambient": day_amb[1] if day_amb else None,
            "fog": day_fog[1] if day_fog else None}
