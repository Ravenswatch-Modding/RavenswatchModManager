"""`kind = "entity"`: graph steps written over ONE shipped entity file, in place."""

from __future__ import annotations

from pathlib import Path

import pytest

from rsmm.engine import corpus
from rsmm.engine import entity_fields as EF
from rsmm.engine import entity_graph as EG
from rsmm.sdk.content import ContentDef, ContentError, ContentRegistry
from rsmm.sdk.kinds import entities

_ENTITY = "Objects/Map_Boss_Spawner/Map_Boss_Spawner_Graph_Model"
_REL = f"EntitySettings/{_ENTITY}.entity.ot.EntitySettingsResource.gen"

needs_corpus = pytest.mark.skipif(corpus.read(_REL) is None, reason="no entity corpus")


def _emit(tmp_path, *steps, id="Tint", entity=_ENTITY):
    return entities.emit("m", ContentDef(
        kind="entity", id=id, fields={"entity": entity, "steps": list(steps)}), tmp_path)


def _links(blob, part, field):
    g = EG.parse(blob, "x")
    c = next(x for x in g.components if x.name == part)
    return next(f for f in EF.fields(c) if f.name == field).text


@needs_corpus
def test_the_edit_lands_at_the_vanilla_path_and_the_original_is_untouched(tmp_path):
    written = _emit(tmp_path, {"remove_link": "State FX PreOvertime.while_active[1]"})
    assert [p.relative_to(tmp_path).as_posix() for p in written] == [_REL]
    new = _links(written[0].read_bytes(), "State FX PreOvertime", "while_active")
    old = _links(corpus.read(_REL), "State FX PreOvertime", "while_active")
    assert "Pre Overtime WARNING Render Settings" in old
    assert "Pre Overtime WARNING Render Settings" not in new
    assert "Pre Overtime Warning Curve" in new          # its neighbour stays


@needs_corpus
@pytest.mark.parametrize("entity", [
    _ENTITY, _ENTITY + ".entity.ot", _ENTITY.replace("/", "\\"),
    "EntitySettings/" + _ENTITY,
])
def test_the_path_is_accepted_in_every_spelling(tmp_path, entity):
    written = _emit(tmp_path, {"remove_link": "Event FX Overtime Warning.activates[0]"},
                    entity=entity)
    assert [p.relative_to(tmp_path).as_posix() for p in written] == [_REL]


@needs_corpus
def test_two_blocks_editing_one_file_both_land(tmp_path):
    _emit(tmp_path, {"remove_link": "State FX PreOvertime.while_active[1]"}, id="A")
    written = _emit(tmp_path, {"remove_link": "Event FX Overtime Warning.activates[0]"}, id="B")
    blob = written[0].read_bytes()
    assert "WARNING Render Settings" not in _links(blob, "State FX PreOvertime", "while_active")
    assert "Cinematic" not in _links(blob, "Event FX Overtime Warning", "activates")


@needs_corpus
def test_a_packed_mods_shipped_output_is_rebuilt_not_edited_again(tmp_path):
    """A packed mod ships its emitted file without .rsmm_emitted.json, so the
    player's apply finds last build's output on disk. Re-running the steps over
    it cut while_active[2] from a two-element list (IndexError, 2026-10-09)."""
    steps = [{"remove_link": "State FX PreOvertime.while_active[1]"},
             {"remove_link": "Event FX Overtime Warning.activates[0]"},
             {"remove_link": "State FX Overtime.while_active[2]"}]

    def registry_emit():
        cr = ContentRegistry(mod_id="m")
        cr.register("entity", id="Tint", entity=_ENTITY, steps=steps)
        return cr.emit(tmp_path)

    first = registry_emit()[0].read_bytes()
    again = registry_emit()                     # the shipped file is still there
    assert again[0].read_bytes() == first


def test_an_out_of_range_link_names_the_list(tmp_path):
    from rsmm.engine.ability_edit import AbilityEditError, apply
    if corpus.read(_REL) is None:
        pytest.skip("no entity corpus")
    with pytest.raises(AbilityEditError, match=r"while_active\[9\].*element"):
        apply({"x": corpus.read(_REL)},
              [{"remove_link": "State FX Overtime.while_active[9]"}], main="x", seed="s")


@pytest.mark.parametrize("fields,msg", [
    ({"steps": [{"remove_link": "A.b[0]"}]}, "needs 'entity'"),
    ({"entity": _ENTITY, "steps": "oops"}, "needs 'steps'"),
    ({"entity": _ENTITY, "steps": [{"clone": "X", "as": "Y", "from": "Juliet"}]},
     "cross-file clone steps"),
    ({"entity": "Objects/Nope/Nothing_Here", "steps": [{"remove_link": "A.b[0]"}]},
     "no shipped entity"),
])
def test_bad_blocks_are_refused_with_the_reason(tmp_path, fields, msg):
    with pytest.raises(ContentError, match=msg):
        entities.emit("m", ContentDef(kind="entity", id="Bad", fields=fields), tmp_path)


def _cfg_root(tmp_path):
    (tmp_path / "config_schema.toml").write_text(
        '[fields.health]\ntype = "float"\ndefault = 1.5\n'
        '[fields.on]\ntype = "bool"\ndefault = true\n', encoding="utf-8")
    return tmp_path


def test_a_set_step_can_take_its_value_from_config(tmp_path):
    d = ContentDef(kind="entity", id="e", fields={})
    steps = [{"set": "A.obj_4[1]", "config": "health", "times": 2.4},
             {"set": "B.value", "config": "on"},
             {"remove_link": "C.list[0]"}]
    assert entities._resolve_config(d, steps, _cfg_root(tmp_path)) == [
        {"set": "A.obj_4[1]", "value": 3.6},
        {"set": "B.value", "value": True},
        {"remove_link": "C.list[0]"},
    ]


@pytest.mark.parametrize("step, msg", [
    ({"set": "A.v", "config": "nope"}, "not a field"),
    ({"set": "A.v", "config": "on", "times": 2}, "needs a number"),
    ({"set": "A.v", "config": "health", "value": 1}, "in place of 'value'"),
])
def test_config_steps_fail_loudly(tmp_path, step, msg):
    d = ContentDef(kind="entity", id="e", fields={})
    with pytest.raises(ContentError, match=msg):
        entities._resolve_config(d, [step], _cfg_root(tmp_path))


def test_when_gates_any_step_on_a_bool_field(tmp_path):
    root = _cfg_root(tmp_path)
    d = ContentDef(kind="entity", id="e", fields={})
    steps = [{"link": "T.on_false", "to": "S", "when": "on"}, {"remove_link": "C.l[0]"}]
    assert entities._resolve_config(d, steps, root) == [{"link": "T.on_false", "to": "S"},
                                                        {"remove_link": "C.l[0]"}]
    (root / "config.toml").write_text("on = false\n", encoding="utf-8")
    assert entities._resolve_config(d, steps, root) == [{"remove_link": "C.l[0]"}]
    with pytest.raises(ContentError, match="on/off"):
        entities._resolve_config(d, [{"remove_link": "C.l[0]", "when": "health"}], root)


@needs_corpus
def test_an_add_selector_step_keeps_its_condition_beside_the_config_guard(tmp_path, monkeypatch):
    """`when` is the config guard on every step; add_selector's in-game test is
    `condition`, so one step can carry both."""
    from rsmm.engine.entity_graph_edit import EntityFile
    seen = []
    monkeypatch.setattr(EntityFile, "prepend_selector_entry",
                        lambda self, comp, target, condition:
                        seen.append((comp, target, condition)))
    out = _cfg_root(tmp_path) / "assets"
    out.mkdir()
    entities.emit("m", ContentDef(kind="entity", id="Tint", fields={
        "entity": _ENTITY,
        "steps": [{"add_selector": "Some Selector", "to": "Some Branch",
                   "when": "on", "condition": "State Empowered"},
                  {"remove_link": "State FX PreOvertime.while_active[1]"}]}), out)
    assert seen == [("Some Selector", "Some Branch", "State Empowered")]


@needs_corpus
def test_ui_layouter_fields_edit_cleanly(tmp_path):
    written = _emit(
        tmp_path,
        {"set": "GameModifier List Ui Layouter.layout_mode", "value": 5},
        {"set": "GameModifier List Ui Layouter.columns", "value": 2},
        {"set": "GameModifier List Ui Layouter.vertical_sizing", "value": 3},
        {"set": "GameModifier List Ui Layouter.horizontal_sizing", "value": 3},
        entity="GameUis/All_Book_Pages/Play_Book_Page",
    )
    blob = written[0].read_bytes()
    assert _links(blob, "GameModifier List Ui Layouter", "layout_mode") == "5"
    assert _links(blob, "GameModifier List Ui Layouter", "columns") == "int 2"
    assert _links(blob, "GameModifier List Ui Layouter", "vertical_sizing") == "3"
    assert _links(blob, "GameModifier List Ui Layouter", "horizontal_sizing") == "3"


_BAR = "GameUis/Common_Ui/Life_Bar_NPC_Common"
_BAR_REL = f"EntitySettings/{_BAR}.entity.ot.EntitySettingsResource.gen"


@pytest.mark.skipif(corpus.read(_BAR_REL) is None, reason="no life bar in corpus")
def test_read_points_a_number_slot_at_another_parts_accessor(tmp_path):
    import struct
    hp, stagger = struct.pack("<I", 0x1acadef8), struct.pack("<I", 0x1ad2c982)
    old = corpus.read(_BAR_REL)
    (out,) = _emit(tmp_path, {"read": "Stagger Label.value_22", "to": "Dt Entity Hittable Listener",
                              "accessor": 0x1acadef8}, id="Hp", entity=_BAR)
    new = out.read_bytes()
    assert new.count(hp) == old.count(hp) + 1
    assert new.count(stagger) == old.count(stagger) - 1
    with pytest.raises(ContentError):
        _emit(tmp_path, {"read": "Stagger Label.value_99", "to": "Dt Entity Hittable Listener",
                         "accessor": 1}, id="Bad", entity=_BAR)


_BOSS = "GameUis/Common_Ui/Life_Bar_Boss_Common"
_BOSS_REL = f"EntitySettings/{_BOSS}.entity.ot.EntitySettingsResource.gen"
_BOSS_FORMAT = {
    "label_format": "Boss Name", "text": "{0}   {1}",
    "inputs": [
        {"node": "Name Get Value", "accessor": 0x16abf2f0, "type": "text"},
        {"node": "GameUis\\Common_Ui\\Life_Bar_Common.entity.ot|Dt Entity Hittable Listener",
         "accessor": 0x1acadef8, "type": "number"},
    ],
}


@pytest.mark.skipif(corpus.read(_BOSS_REL) is None, reason="no boss bar in corpus")
def test_label_format_keeps_the_text_and_adds_a_number(tmp_path):
    import struct
    (out,) = _emit(tmp_path, _BOSS_FORMAT, id="Hp", entity=_BOSS)
    new = out.read_bytes()
    g = EG.parse(new, "x")
    fmt = next(c for c in g.components if c.name == "Boss Name Format")
    fields = {f.name: f.text for f in EF.fields(fmt)}
    assert fields["value_2"] == "text '{0}   {1}'"
    assert "Name Get Value" in fields["obj_4"]
    assert "Life_Bar_Common\\Getter\\Dt Entity Hittable Listener" in fields["obj_5"]
    label = _links(new, "Boss Name", "value_21")
    assert "[String Format] Life_Bar_Boss_Common\\Boss Name Format" in label
    assert struct.pack("<I", 0x13800d8d) in new          # label reads a String Format
    with pytest.raises(ContentError):
        _emit(tmp_path, {**_BOSS_FORMAT, "inputs": [{"node": "Name Get Value", "type": "text"}]},
              id="Bad", entity=_BOSS)


_PIPER = "Heroes/Hero_Piper/Hero_Piper"
_PIPER_REL = f"EntitySettings/{_PIPER}.entity.ot.EntitySettingsResource.gen"
_DELAY = "Primary Ability Shots Delay.value"
needs_piper = pytest.mark.skipif(corpus.read(_PIPER_REL) is None, reason="no hero corpus")


def _scale_emit(root, *steps, id="Scale", config=""):
    from rsmm.engine.entity_graph_edit import EntityFile
    _cfg_root(root)
    if config:
        (root / "config.toml").write_text(config, encoding="utf-8")
    out = root / "assets"
    out.mkdir(exist_ok=True)
    entities.emit("m", ContentDef(kind="entity", id=id, fields={
        "entity": _PIPER, "steps": list(steps)}), out)
    return EntityFile((out / Path(*_PIPER_REL.split("/"))).read_bytes())


def _delay(ef):
    return ef.get_value("Primary Ability Shots Delay", "value")


@needs_piper
def test_scale_writes_the_shipped_number_times_the_config_field(tmp_path):
    from rsmm.engine.entity_graph_edit import EntityFile
    shipped = EntityFile(corpus.read(_PIPER_REL)).get_value("Primary Ability Shots Delay", "value")
    ef = _scale_emit(tmp_path, {"set": _DELAY, "scale": "health"})    # health = 1.5
    assert _delay(ef) == pytest.approx(round(float(f"{shipped:.7g}") * 1.5, 6), rel=1e-6)


@needs_piper
def test_scale_matches_a_literal_times_of_the_shipped_value(tmp_path):
    from rsmm.engine.entity_graph_edit import EntityFile
    shipped = EntityFile(corpus.read(_PIPER_REL)).get_value("Primary Ability Shots Delay", "value")
    (tmp_path / "a").mkdir()
    (tmp_path / "b").mkdir()
    a = _scale_emit(tmp_path / "a", {"set": _DELAY, "scale": "health"})
    b = _scale_emit(tmp_path / "b", {"set": _DELAY, "config": "health",
                                     "times": float(f"{shipped:.7g}")})
    assert a.to_bytes() == b.to_bytes()


@needs_piper
def test_two_blocks_scaling_one_value_do_not_compound(tmp_path):
    from rsmm.engine.entity_graph_edit import EntityFile
    shipped = EntityFile(corpus.read(_PIPER_REL)).get_value("Primary Ability Shots Delay", "value")
    _scale_emit(tmp_path, {"set": _DELAY, "scale": "health"}, id="A")
    ef = _scale_emit(tmp_path, {"set": _DELAY, "scale": "health"}, id="B")
    assert _delay(ef) == pytest.approx(shipped * 1.5, rel=1e-6)       # not shipped * 2.25


@needs_piper
def test_when_gates_a_scale_step(tmp_path):
    from rsmm.engine.entity_graph_edit import EntityFile
    shipped = EntityFile(corpus.read(_PIPER_REL)).get_value("Primary Ability Shots Delay", "value")
    on = _scale_emit(tmp_path, {"set": _DELAY, "scale": "health", "when": "on"})
    assert _delay(on) == pytest.approx(shipped * 1.5, rel=1e-6)
    d = ContentDef(kind="entity", id="e", fields={})
    steps = [{"set": _DELAY, "scale": "health", "when": "on"}]
    (tmp_path / "config.toml").write_text("on = false\n", encoding="utf-8")
    assert entities._resolve_config(d, steps, tmp_path, corpus.read(_PIPER_REL)) == []


@needs_piper
@pytest.mark.parametrize("step, msg", [
    ({"set": _DELAY, "scale": "health", "value": 1}, "in place of"),
    ({"set": _DELAY, "scale": "health", "config": "health"}, "in place of"),
    ({"set": _DELAY, "scale": "health", "times": 2}, "in place of"),
    ({"scale": "health"}, "goes on a set step"),
    ({"set": _DELAY, "scale": "nope"}, "not a field"),
    ({"set": _DELAY, "scale": "on"}, "needs a number field"),
    ({"set": "Ability Trait Seed 3d Node.value_5", "scale": "health"}, "needs a number field"),
    ({"set": "Ability Trait Seed 3d Node.flag_81", "scale": "health"}, "needs a number field"),
    ({"set": "No Such Part.value", "scale": "health"}, "No Such Part"),
])
def test_scale_errors_name_the_step(tmp_path, step, msg):
    d = ContentDef(kind="entity", id="e", fields={})
    with pytest.raises(ContentError, match=msg):
        entities._resolve_config(d, [step], _cfg_root(tmp_path), corpus.read(_PIPER_REL))


_PIPER_COPY = "Heroes/Hero_Piper/Hero_Piper_Copy"
_PIPER_COPY_REL = f"EntitySettings/{_PIPER_COPY}.entity.ot.EntitySettingsResource.gen"


def _list_emit(tmp_path, monkeypatch, entity, *, id="L"):
    """Emit a block over ``entity``; the Piper copy ships a different delay."""
    from rsmm.engine.entity_graph_edit import EntityFile
    real = corpus.read

    def read(rel):
        if rel != _PIPER_COPY_REL:
            return real(rel)
        ef = EntityFile(real(_PIPER_REL))
        ef.set_value("Primary Ability Shots Delay", "value", 4.0)
        return ef.to_bytes()

    monkeypatch.setattr(corpus, "read", read)
    _cfg_root(tmp_path)
    out = tmp_path / "assets"
    out.mkdir(exist_ok=True)
    entities.emit("m", ContentDef(kind="entity", id=id, fields={
        "entity": entity, "steps": [{"set": _DELAY, "scale": "health"}]}), out)
    return out


@needs_piper
def test_a_list_scales_each_entity_from_its_own_shipped_value(tmp_path, monkeypatch):
    from rsmm.engine.entity_graph_edit import EntityFile
    shipped = EntityFile(corpus.read(_PIPER_REL)).get_value("Primary Ability Shots Delay", "value")
    out = _list_emit(tmp_path, monkeypatch, [_PIPER, _PIPER_COPY])
    one = EntityFile((out / Path(*_PIPER_REL.split("/"))).read_bytes())
    two = EntityFile((out / Path(*_PIPER_COPY_REL.split("/"))).read_bytes())
    assert _delay(one) == pytest.approx(shipped * 1.5, rel=1e-6)
    assert _delay(two) == pytest.approx(6.0, rel=1e-6)                # 4.0 * 1.5


@needs_piper
def test_a_string_still_means_one_entity(tmp_path, monkeypatch):
    out = _list_emit(tmp_path, monkeypatch, _PIPER)
    assert [p.name for p in out.rglob("*.gen")] == [Path(_PIPER_REL).name]


@pytest.mark.parametrize("entity, msg", [
    ([], "needs 'entity'"),
    ([_ENTITY, 3], "needs 'entity'"),
    ([_ENTITY, ""], "needs 'entity'"),
    ([_ENTITY, _ENTITY + ".entity.ot"], f"entity Dup: {_ENTITY!r}.* listed twice"),
])
def test_a_bad_entity_list_is_refused_naming_the_block(tmp_path, entity, msg):
    with pytest.raises(ContentError, match=msg):
        entities.emit("m", ContentDef(kind="entity", id="Dup", fields={
            "entity": entity, "steps": [{"remove_link": "A.b[0]"}]}), tmp_path)
    assert not list(tmp_path.rglob("*.gen"))


def test_an_error_in_one_listed_entity_names_the_block_and_the_entity(tmp_path):
    with pytest.raises(ContentError, match=r"entity Bad \(Objects/Nope/Nothing_Here\): no shipped"):
        entities.emit("m", ContentDef(kind="entity", id="Bad", fields={
            "entity": ["Objects/Nope/Nothing_Here"], "steps": [{"remove_link": "A.b[0]"}]}),
            tmp_path)
