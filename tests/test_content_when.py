"""A `[[content]]` block's `when = "<bool field>"` gates the whole block."""

from __future__ import annotations

import types

import pytest

from rsmm.sdk import content as C
from rsmm.sdk.content import ContentError, ContentRegistry
from rsmm.sdk.kinds import entities


def _mod(tmp_path, config=""):
    (tmp_path / "config_schema.toml").write_text(
        '[fields.tentacle]\ntype = "bool"\ndefault = true\n'
        '[fields.faceless]\ntype = "bool"\ndefault = true\n'
        '[fields.health]\ntype = "float"\ndefault = 1.5\n', encoding="utf-8")
    if config:
        (tmp_path / "config.toml").write_text(config, encoding="utf-8")
    out = tmp_path / "assets"
    out.mkdir()
    return out


@pytest.fixture
def seen(monkeypatch):
    """Swap every kind's emit for one that records the def and the steps that
    survive step-level `when`."""
    calls = []

    def emit(mod_id, defn, out_dir):
        steps = defn.fields.get("steps")
        if steps:
            steps = entities._resolve_config(defn, steps, out_dir.parent)
        calls.append((defn.kind, defn.id, dict(defn.fields), steps))
        return []

    monkeypatch.setattr(C, "_load_kind", lambda kind: types.SimpleNamespace(emit=emit))
    return calls


def _register(kind="item", **fields):
    cr = ContentRegistry(mod_id="M", experimental=True)
    cr.register(kind, id="B", **fields)
    return cr


def test_a_block_whose_switch_is_off_emits_nothing(tmp_path, seen):
    out = _mod(tmp_path, "tentacle = false\n")
    _register("entity", entity="X", steps=[{"remove_link": "A.b[0]"}], when="tentacle").emit(out)
    assert seen == []


def test_a_block_whose_switch_is_on_emits_without_the_key(tmp_path, seen):
    out = _mod(tmp_path)
    _register("entity", entity="X", steps=[{"remove_link": "A.b[0]"}], when="tentacle").emit(out)
    assert [(k, i) for k, i, _, _ in seen] == [("entity", "B")]
    assert "when" not in seen[0][2]


def test_a_step_when_still_skips_just_that_step_in_an_on_block(tmp_path, seen):
    out = _mod(tmp_path, "faceless = false\n")
    steps = [{"remove_link": "A.b[0]", "when": "tentacle"},
             {"remove_link": "C.d[0]", "when": "faceless"},
             {"remove_link": "E.f[0]"}]
    _register("entity", entity="X", steps=steps, when="tentacle").emit(out)
    assert seen[0][3] == [{"remove_link": "A.b[0]"}, {"remove_link": "E.f[0]"}]


def test_an_off_block_skips_its_on_steps_too(tmp_path, seen):
    out = _mod(tmp_path, "tentacle = false\n")
    steps = [{"remove_link": "C.d[0]", "when": "faceless"}]
    _register("entity", entity="X", steps=steps, when="tentacle").emit(out)
    assert seen == []


@pytest.mark.parametrize("kind, fields", [
    ("item", {"base": "Z"}),
    ("enemy", {"base": "Crab"}),
])
def test_other_kinds_are_gated_too(tmp_path, seen, kind, fields):
    off = _mod(tmp_path, "tentacle = false\n")
    _register(kind, when="tentacle", **fields).emit(off)
    assert seen == []
    (tmp_path / "config.toml").write_text("tentacle = true\n", encoding="utf-8")
    _register(kind, when="tentacle", **fields).emit(off)
    assert [k for k, *_ in seen] == [kind]


@pytest.mark.parametrize("name, msg", [
    ("health", "must name an on/off"),
    ("nope", "not a field"),
])
def test_a_when_that_is_not_a_bool_field_names_the_block(tmp_path, seen, name, msg):
    out = _mod(tmp_path)
    with pytest.raises(ContentError, match=rf"item B: when.*{msg}|item B: when = .*{msg}"):
        _register("item", base="Z", when=name).emit(out)
    assert seen == []
