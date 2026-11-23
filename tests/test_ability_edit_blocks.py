"""Building-block steps (engine/ability_edit.py `_expand`): `targets`,
`notify_owner`, `no_damage` write the same parts a mod used to wire by hand,
with nothing left pointing into the donor entities."""

from __future__ import annotations

import pytest

from rsmm.engine import ability_edit as AE
from rsmm.engine import entity_fields as EF
from rsmm.engine import entity_graph as EG

ORB = "Heroes\\Hero_Carmilla\\Hero_Carmilla_Power_Orb.entity.ot"


def _raw():
    try:
        return AE.CK._corpus_reader(ORB)
    except Exception:  # noqa: BLE001 — no corpus
        return None


needs_corpus = pytest.mark.skipif(_raw() is None, reason="no corpus")


def _fields(raw: bytes, part: str) -> dict[str, str]:
    c = next(c for c in EG.parse(raw).components if c.name == part)
    return {f.name: f.text for f in EF.fields(c)}


def _apply(steps):
    return AE.apply({"O": _raw()}, steps, main="O", seed="t").files["O"]


@needs_corpus
def test_targets_is_self_contained():
    out = _apply([{"targets": "Spread", "count": 3, "radius": 4.5,
                   "to": "Event Initialize", "hit": "Attacking Damage"}])
    assert b"Hero_Kintaro" not in out and b"Secondary_Gain_Charge" not in out
    comp = _fields(out, "Spread Target Computer")
    assert "Spread Traverser" in comp["ref_3"] and "Spread Max Targets" in comp["value_5"]
    assert comp["ref_12"] == "(none)"
    assert _fields(out, "Spread Radius")["value"] == "f32 4.5"
    assert _fields(out, "Spread Max Targets")["value"] == "int 3"
    attack = _fields(out, "Spread Target Attack")
    assert "Spread Target Computer" in attack["ref_2"] and "Attacking Damage" in attack["ref_4"]
    on = _fields(out, "Event Initialize")["activates"]
    assert all(f"Spread {p}" in on for p in ("Radius", "Max Targets", "Traverser",
                                             "Target Computer"))
    assert out.count(b"Spread Radius") >= 2       # the traverser reads it


@needs_corpus
def test_notify_owner_and_no_damage():
    out = _apply([{"event": "Event Boom"},
                  {"notify_owner": "MY_EVENT", "to": "Event Boom"},
                  {"no_damage": "Attacking Damage"}])
    assert _fields(out, "Owner MY_EVENT Sender")["event"] == "MY_EVENT"
    assert "Owner MY_EVENT Sender" in _fields(out, "Event Boom")["activates"]
    dmg = _fields(out, "Attacking Damage")
    assert dmg["value_2"] == "f32 0" and dmg["value_35"] == "int -1"


@needs_corpus
@pytest.mark.parametrize("bad", [
    {"targets": "X"}, {"targets": "X", "radius": 2, "count": -1},
    {"targets": "X", "radius": 2, "bogus": 1}, {"notify_owner": "E"},
    {"no_damage": 3}, {"targets": "X", "radius": 1, "no_damage": "Y"}])
def test_bad_blocks_raise(bad):
    with pytest.raises(AE.AbilityEditError, match="step 1"):
        _apply([bad])
