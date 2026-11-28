"""engine.render_look: field layouts of the chapter light settings.

Offsets follow the engine's own Serialize order (oCDtAmbientLightingSettings
FUN_140295b40, oCDtFogSettings FUN_140296100); these tests pin the decoder to
that order on synthetic payloads, no game install needed.
"""

from __future__ import annotations

import struct

from rsmm.engine import render_look as RL
from rsmm.engine import talent_values as TV


def _f(*v):
    return struct.pack(f"<{len(v)}f", *v)


def test_ambient_fields_in_serialize_order():
    body = _f(0.1, 0.2, 0.3, 1) + _f(0.4, 0.5, 0.6, 1) + _f(0, 0, 0, 1) + _f(0.75, 2.0)
    a = RL.ambient(b"\x00" * 9 + body, 9)
    assert a["horizon"][:3] == [0.1, 0.2, 0.3]       # +0x20, the original single colour
    assert a["top"][:3] == [0.4, 0.5, 0.6]           # +0x10
    assert a["bottom"][:3] == [0.0, 0.0, 0.0]        # +0x30
    assert (a["mul"], a["horizon_pow"]) == (0.75, 2.0)


def test_fog_fields_in_serialize_order():
    body = (_f(1, 0.5, 1) + _f(0, 0, 0, 0.5) + _f(0.25, 0.25, 0.5, 0.75) + _f(0.5, 0.5, 0.5, 1)
            + _f(5, 1, -15) + _f(-7.5, 25) + _f(0, 20, 40))
    f = RL.fog(b"\x00" * 9 + body, 9)
    assert (f["intensity"], f["dark"], f["brightness"]) == (1.0, 0.5, 1.0)
    assert f["mid"] == [0.25, 0.25, 0.5, 0.75]
    assert f["heights"] == [5.0, 1.0, -15.0]
    assert f["noise"] == [-7.5, 25.0]
    assert f["distances"] == [0.0, 20.0, 40.0]


def test_inline_value_is_read_and_a_sourced_one_is_not():
    def node(picker_flag: bytes, type_code: int, value: bytes) -> bytes:
        union = struct.pack("<I", 2) + struct.pack("<II", type_code, 0) + value
        return TV._BEGIN + struct.pack("<I", 1) + picker_flag + TV._BEGIN + union + TV._END

    inline = b"Sun" + node(b"\x00", 4, _f(1, 0.9, 0.7, 1))
    assert RL._value_after(inline, 3) == [1.0, 0.9, 0.7, 1.0]
    f32 = b"Sun" + node(b"\x00", 0, _f(8.0))
    assert RL._value_after(f32, 3) == [8.0]
    sourced = b"Sun" + node(b"\x01", 0, _f(8.0))   # the picker points elsewhere
    assert RL._value_after(sourced, 3) is None
