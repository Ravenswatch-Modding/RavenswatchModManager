"""`[[patch]]` fields may read the mod's config: { config = "<field>", times = n }."""

import pytest

from rsmm.cli.merge import _resolve_config
from rsmm.sdk.config import ConfigError


def _mod(tmp_path, saved=""):
    (tmp_path / "config_schema.toml").write_text(
        '[fields.pack]\ntype = "float"\ndefault = 1.5\n', encoding="utf-8")
    if saved:
        (tmp_path / "config.toml").write_text(saved, encoding="utf-8")
    return tmp_path


def test_config_refs_resolve_and_literals_pass_through(tmp_path):
    data = {"name": "Easy", "min": {"config": "pack", "times": 10}, "max": 20}
    assert _resolve_config(_mod(tmp_path), data) == {"name": "Easy", "min": 15.0, "max": 20}


def test_the_players_saved_value_wins_over_the_default(tmp_path):
    out = _resolve_config(_mod(tmp_path, "pack = 2.0\n"), {"min": {"config": "pack", "times": 10}})
    assert out["min"] == 20.0


def test_an_unknown_field_is_an_error(tmp_path):
    with pytest.raises(ConfigError, match="not a field"):
        _resolve_config(_mod(tmp_path), {"min": {"config": "nope"}})


def test_a_scale_ref_resolves_to_a_multiplier_for_the_merge(tmp_path):
    from rsmm.cli.merge import _Scale
    out = _resolve_config(_mod(tmp_path), {"name": "Easy", "min": {"scale": "pack"}, "max": 20})
    assert out == {"name": "Easy", "min": _Scale(1.5), "max": 20}


@pytest.mark.parametrize("ref, msg", [
    ({"scale": "pack", "times": 2}, "stands alone"),
    ({"scale": "pack", "config": "pack"}, "stands alone"),
])
def test_a_scale_ref_stands_alone(tmp_path, ref, msg):
    with pytest.raises(ValueError, match=msg):
        _resolve_config(_mod(tmp_path), {"min": ref})


def test_an_unknown_or_non_number_scale_field_is_an_error(tmp_path):
    root = _mod(tmp_path)
    with pytest.raises(ConfigError, match="not a field"):
        _resolve_config(root, {"min": {"scale": "nope"}})
    (root / "config_schema.toml").write_text(
        '[fields.on]\ntype = "bool"\ndefault = true\n', encoding="utf-8")
    with pytest.raises(ValueError, match="needs a number"):
        _resolve_config(root, {"min": {"scale": "on"}})


def test_a_stat_patch_scales_the_shipped_value(tmp_path, monkeypatch):
    from rsmm.cli import merge as M
    from rsmm.engine import corpus
    from rsmm.engine.stat_schemas import SCHEMAS, Entry, read_field
    shipped = corpus.read("GlobalValues/GlobalValues_Common/"
                          "Card_Attack_Damage.globalvalue.ot.GlobalEntityValueSettings.gen")
    if shipped is None:
        pytest.skip("no global-value corpus")
    schema = next(s for s in SCHEMAS if s.cls == "oCGlobalEntityValueSettings")
    live = tmp_path / "Card.gen"
    live.write_bytes(shipped)
    entry = Entry(schema=schema, cooked_path=live, decoded_relpath="Card.gen",
                  short_name="Card_Attack_Damage", values={})
    monkeypatch.setattr(M, "index_entries", lambda cooking, amap: [entry])
    out = tmp_path / "out"
    patches = [M._Patch("m", 100, "stat", {"name": "Card_Attack_Damage", "value": M._Scale(1.5)})]
    assert M._stat_patches(patches, tmp_path, out, []) == 1
    got = (out / "Card.gen").read_bytes()
    assert read_field(got, schema, "value") == pytest.approx(0.3)
    # A second run over the already-patched live file still scales the shipped value.
    live.write_bytes(got)
    (tmp_path / "Card.gen.rsmm.bak").write_bytes(shipped)
    assert M._stat_patches(patches, tmp_path, out, []) == 1
    assert read_field((out / "Card.gen").read_bytes(), schema, "value") == pytest.approx(0.3)
