"""Unit tests for scripts/contract_pins.py (removal, alias, abi, expiry rules).

Everything runs against tmp files; the committed data/contracts/ is not touched.
"""

import importlib.util
import json
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "scripts" / "contract_pins.py"
_spec = importlib.util.spec_from_file_location("contract_pins", SCRIPT)
cp = importlib.util.module_from_spec(_spec)
sys.modules["contract_pins"] = cp
_spec.loader.exec_module(cp)


def alias(pin, old, new="x", until=10):
    return {"pin": pin, "old": old, "new": new, "until": until}


def test_addition_is_free():
    assert cp.uncovered_removals("r_surface", ["R.a"], ["R.a", "R.b"], []) == []


def test_removal_fails():
    assert cp.uncovered_removals("r_surface", ["R.a", "R.b"], ["R.a"], []) == ["R.b"]


def test_rename_fails_without_alias_and_passes_with_it():
    old, new = ["R.old"], ["R.new"]
    assert cp.uncovered_removals("r_surface", old, new, []) == ["R.old"]
    covered = [alias("r_surface", "R.old", "R.new")]
    assert cp.uncovered_removals("r_surface", old, new, covered) == []


def test_alias_is_per_pin():
    assert cp.uncovered_removals("manifest_ops", ["ability:x"], [],
                                 [alias("r_surface", "ability:x")]) == ["ability:x"]


def test_parse_deprecated():
    text = ("# c\n\nr_surface R.old -> R.new until-loader 65\n"
            "json_bridge config get -> cfg get until-loader 70\n")
    assert cp.parse_deprecated(text) == [
        {"pin": "r_surface", "old": "R.old", "new": "R.new", "until": 65},
        {"pin": "json_bridge", "old": "config get", "new": "cfg get", "until": 70},
    ]


@pytest.mark.parametrize("line", ["r_surface R.old R.new", "bogus R.a -> R.b until-loader 1",
                                  "r_surface R.a -> R.b until-loader x"])
def test_parse_deprecated_rejects_malformed(line):
    with pytest.raises(ValueError):
        cp.parse_deprecated(line)


def test_expired_alias():
    aliases = [alias("r_surface", "R.a", until=63), alias("r_surface", "R.b", until=64)]
    assert cp.expired_aliases(aliases, 64) == [aliases[0]]


def test_bridge_abi_change_is_reported():
    out = cp.bridge_breaking("abi 1\nlist\n", "abi 2\nlist\n", [])
    assert out == ["json_bridge: abi 1 -> abi 2"]


def test_bridge_removed_line_is_reported_and_alias_covers_it():
    old, new = "abi 1\nlist\nlist --x\n", "abi 1\nlist\n"
    assert cp.bridge_breaking(old, new, []) == ["json_bridge: list --x removed"]
    assert cp.bridge_breaking(old, new, [alias("json_bridge", "list --x")]) == []


def test_write_refuses_bridge_removal_unless_abi_bumped():
    committed = {"json_bridge": "abi 1\nlist\nlist --x\n"}
    assert cp.write_refusals(committed, {"json_bridge": "abi 1\nlist\n"}, [])
    assert cp.write_refusals(committed, {"json_bridge": "abi 2\nlist\n"}, []) == []
    grown = {"json_bridge": "abi 1\nlist\nlist --x\nlist --y\n"}
    assert cp.write_refusals(committed, grown, []) == []


def test_write_refuses_removed_name():
    assert cp.write_refusals({"manifest_ops": "a:x\na:y\n"}, {"manifest_ops": "a:x\n"}, []) == [
        "manifest_ops: a:y removed"]


@pytest.fixture
def sandbox(tmp_path, monkeypatch):
    contracts = tmp_path / "contracts"
    contracts.mkdir()
    version = tmp_path / "loader_version.json"
    version.write_text(json.dumps({"loader_version": 64}), encoding="utf-8")
    monkeypatch.setattr(cp, "CONTRACTS", contracts)
    monkeypatch.setattr(cp, "LOADER_VERSION", version)
    snaps = {"manifest_ops": "a:x\na:y\n", "json_bridge": "abi 1\nlist\n"}
    monkeypatch.setattr(cp, "generate", lambda surface: dict(snaps))
    for name, text in snaps.items():
        (contracts / f"{name}.txt").write_text(text, encoding="utf-8")
    return contracts, snaps


def test_check_passes_when_in_sync(sandbox):
    assert cp.cmd_check(None) == 0


def test_check_fails_when_a_line_is_deleted_from_the_snapshot(sandbox, capsys):
    contracts, _ = sandbox
    (contracts / "manifest_ops.txt").write_text("a:x\n", encoding="utf-8")
    assert cp.cmd_check(None) == 1
    assert "write" in capsys.readouterr().out


def test_check_fails_when_code_loses_a_name(sandbox, monkeypatch):
    monkeypatch.setattr(cp, "generate", lambda surface: {"manifest_ops": "a:x\n",
                                                         "json_bridge": "abi 1\nlist\n"})
    assert cp.cmd_check(None) == 1


def test_check_fails_on_expired_alias(sandbox):
    contracts, _ = sandbox
    (contracts / "deprecated.txt").write_text(
        "manifest_ops a:old -> a:y until-loader 63\n", encoding="utf-8")
    assert cp.cmd_check(None) == 1


def test_check_accepts_alias_in_its_release(sandbox):
    contracts, _ = sandbox
    (contracts / "deprecated.txt").write_text(
        "manifest_ops a:old -> a:y until-loader 64\n", encoding="utf-8")
    assert cp.cmd_check(None) == 0


def test_write_refuses_then_succeeds_with_alias(sandbox, monkeypatch):
    contracts, _ = sandbox
    monkeypatch.setattr(cp, "generate", lambda surface: {"manifest_ops": "a:x\n"})
    assert cp.cmd_write(None) == 1
    assert (contracts / "manifest_ops.txt").read_text(encoding="utf-8") == "a:x\na:y\n"
    (contracts / "deprecated.txt").write_text(
        "manifest_ops a:y -> a:x until-loader 64\n", encoding="utf-8")
    assert cp.cmd_write(None) == 0
    assert (contracts / "manifest_ops.txt").read_text(encoding="utf-8") == "a:x\n"


def test_breaking_reads_the_base_ref(sandbox, monkeypatch, capsys):
    contracts, _ = sandbox
    base = {"manifest_ops": "a:x\na:y\na:z\n", "json_bridge": "abi 1\nlist\n"}
    monkeypatch.setattr(cp, "_git_show",
                        lambda ref, rel: base.get(Path(rel).stem, ""))
    (contracts / "deprecated.txt").write_text(
        "manifest_ops a:z -> a:x until-loader 64\n", encoding="utf-8")
    (contracts / "manifest_ops.txt").write_text("a:x\n", encoding="utf-8")
    assert cp.cmd_breaking("origin/main") == 0
    assert capsys.readouterr().out.strip() == "manifest_ops: a:y removed"


def test_parser_walk_lists_nested_commands_and_args():
    import argparse
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd")
    p = sub.add_parser("config")
    cs = p.add_subparsers(dest="c")
    g = cs.add_parser("get")
    g.add_argument("mod_id")
    g.add_argument("--live", action="store_true")
    out = []
    cp._walk_parser(ap, "", out)
    assert sorted(out) == ["config", "config get", "config get --live", "config get mod_id"]
