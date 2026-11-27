"""Custom statuses, CLI half: the row codec, the merge into Common_Settings, the
`status` kind's staged files, and the id checks in lint and compat.

The codec is tested on a stream built here (CI has no game) and, when the game
is installed, on the shipped stream itself.
"""

from __future__ import annotations

import json
import struct
import zlib
from pathlib import Path
from types import SimpleNamespace

import pytest

from rsmm.cli import compat, lint
from rsmm.engine import cooked, corpus, named_id
from rsmm.engine import status_table as ST
from rsmm.sdk.content import ContentDef, ContentError, ContentRegistry
from rsmm.sdk.kinds import statuses as SK

# --- a synthetic Common_Settings -------------------------------------------

def _lstr(b: bytes) -> bytes:
    return struct.pack("<I", len(b)) + b


def _frame(p: bytes) -> bytes:
    return cooked.MARK_BEGIN + p + cooked.MARK_END


def _stream(rows: list[ST.Row], *, status_version: int = 1) -> bytes:
    classes = [("oCDtStatusSettingsGo", 0x16EDCFBB, 0), ("oCDtStatus", 0x16EDDFD0,
                                                         status_version),
               ("oCDtStatusIconStep", 0x16EE0867, 0)]
    ct = struct.pack("<I", len(classes)) + b"".join(
        _lstr(n.encode()) + struct.pack("<IIII", cid, 1, ver, 0x1DA16C)
        for n, cid, ver in classes)
    body = b"".join(ST._encode_row(r, 1, 2, status_version != 0) for r in rows)
    settings = (struct.pack("<I", 0) + _lstr(b"Status Go") + b"\0\0\0\0\0\1"
                + struct.pack("<I", len(rows)) + body)
    inner = (_frame(ct) + _frame(struct.pack("<II", 1, 0)) + _frame(settings)
             + _frame(struct.pack("<II", 0, 0)))
    wrapper = struct.pack("<IIII", 0, 0, len(inner), len(inner)) + inner
    return cooked.emit(cooked.CookedFile(
        "B", 0x10, 0, sections=[cooked.Section(struct.pack("<I", 0)),
                                cooked.Section(wrapper)],
        classes=[cooked.ClassDef("oCGameStream", 1, 1, 1, 0)]))


def _vanillaish() -> list[ST.Row]:
    return [
        ST.Row(0x16EDE056, look=1, hidden=0, kind=0,
               steps=[ST.Step(0.0, b"Ui", b"HUD\\Buff_Strength_Icon.png")]),
        ST.Row(0x1A40367D, look=0, hidden=1, kind=3,
               steps=[ST.Step(0.0, b"", b"")]),
    ]


def _frost(mod: str = "frost", name: str = "Frostbite") -> ST.Row:
    return ST.make_row(id=named_id.of(f"{mod}:{name}"), look="debuff", hidden=False,
                       combine="max", icon="HUD\\Debuff_Chilled.png")


def test_synthetic_stream_round_trips():
    data = _stream(_vanillaish())
    t = ST.decode(data)
    assert [r.id for r in t.rows] == [0x16EDE056, 0x1A40367D]
    assert (t.rows[1].look, t.rows[1].hidden, t.rows[1].kind) == (0, 1, 3)
    assert t.rows[0].steps[0].path == b"HUD\\Buff_Strength_Icon.png"
    assert ST.encode(data, t.rows) == data


def test_appended_rows_decode_and_the_original_comes_back():
    data = _stream(_vanillaish())
    out = ST.append_rows(data, [_frost()])
    rows = ST.decode(out).rows
    assert len(rows) == 3
    assert rows[2].id == named_id.of("frost:Frostbite")
    assert (rows[2].look, rows[2].hidden, rows[2].kind) == (0, 0, 3)
    assert rows[2].steps[0].path == b"HUD\\Debuff_Chilled.png"
    assert ST.encode(out, rows[:2]) == data
    # The stream's own length fields follow the growth.
    sec = cooked.parse(out).sections[-1].payload
    assert struct.unpack_from("<II", sec, 8) == (len(sec) - 16,) * 2


def test_a_stream_without_the_hidden_byte_reads_and_writes_without_it():
    rows = [ST.Row(0x16EDE056, look=1, steps=[ST.Step(0.0, b"", b"")])]
    data = _stream(rows, status_version=0)
    t = ST.decode(data)
    assert not t.has_hidden and t.rows[0].look == 1
    assert ST.encode(data, t.rows) == data
    with pytest.raises(ST.StatusTableError):
        ST.append_rows(data, [ST.Row(5, hidden=1)])


def test_duplicate_id_is_refused_by_the_codec():
    with pytest.raises(ST.StatusTableError):
        ST.append_rows(_stream(_vanillaish()), [ST.Row(0x16EDE056)])


def test_unknown_row_bytes_are_an_error_not_a_guess():
    good = ST._encode_row(_frost(), 1, 2, True)
    extra = good[:-8] + struct.pack("<I", 7) + good[-8:]      # a field after the id
    data = _stream([ST.Row(1, raw=extra)])
    with pytest.raises(ST.StatusTableError):
        ST.decode(data)


# --- the shipped stream ------------------------------------------------------

def test_vanilla_stream_round_trips_byte_identical():
    data = corpus.read(ST.STREAM_DECODED)
    if data is None:
        pytest.skip("Ravenswatch is not installed")
    t = ST.decode(data)
    assert sorted(r.id for r in t.rows) == sorted(ST.VANILLA_IDS)
    assert all(r.raw is not None for r in t.rows)
    assert ST.encode(data, t.rows) == data
    # A row we build for the same fields is the bytes the game shipped.
    r = t.rows[0]
    rebuilt = ST.Row(r.id, r.look, r.hidden, r.kind, r.steps)
    assert ST._encode_row(rebuilt, t.status_cls, t.step_cls, t.has_hidden) == r.raw
    grown = ST.append_rows(data, [_frost()])
    assert ST.encode(grown, ST.decode(grown).rows[:20]) == data


# --- the merge ---------------------------------------------------------------

def _mod(tmp_path: Path, mod_id: str, names: list[str], enabled: bool = True):
    root = tmp_path / mod_id
    rows = []
    for n in names:
        _loader, row = SK.records(mod_id, {"name": n, "look": "buff", "icon": ""})
        rows.append(row)
    stage = root / "assets" / ST.STAGING
    stage.mkdir(parents=True)
    (stage / ST.ROWS_FILE).write_text(json.dumps(rows), encoding="utf-8")
    return SimpleNamespace(id=mod_id, enabled=enabled, root=root,
                           assets_dir=root / "assets")


def test_two_mods_compose_in_load_order_and_disabling_restores_vanilla(tmp_path):
    original = _stream(_vanillaish())
    stream = tmp_path / "game" / "Common_Settings.gen"
    stream.parent.mkdir()
    stream.write_bytes(original)
    a = _mod(tmp_path, "aaa", ["One", "Two"])
    b = _mod(tmp_path, "bbb", ["Three"])

    assert ST.sync_status_table(stream, True, [a, b]) == 1       # dry run
    assert stream.read_bytes() == original

    assert ST.sync_status_table(stream, False, [a, b]) == 1
    ids = [r.id for r in ST.decode(stream.read_bytes()).rows]
    assert ids[2:] == [named_id.of("aaa:One"), named_id.of("aaa:Two"),
                       named_id.of("bbb:Three")]
    assert ST.sync_status_table(stream, False, [a, b]) == 0      # idempotent

    assert ST.sync_status_table(stream, False, [b, a]) == 1      # order follows the list
    ids = [r.id for r in ST.decode(stream.read_bytes()).rows]
    assert ids[2] == named_id.of("bbb:Three")

    a.enabled = b.enabled = False
    assert ST.sync_status_table(stream, False, [a, b]) == 1
    assert stream.read_bytes() == original
    assert not stream.with_name(stream.name + ".rsmm.bak").exists()
    assert ST.sync_status_table(stream, False, [a, b]) == 0


def test_a_mod_that_replaces_the_stream_is_left_alone(tmp_path, capsys):
    original = _stream(_vanillaish())
    stream = tmp_path / "Common_Settings.gen"
    stream.write_bytes(original)
    a = _mod(tmp_path, "aaa", ["One"])
    assert ST.sync_status_table(stream, False, [a], owner="other") == 0
    assert stream.read_bytes() == original
    assert "other replaces Common_Settings" in capsys.readouterr().err


# --- the kind and its ids -----------------------------------------------------

def _crc(s: str) -> int:
    return zlib.crc32(s.encode())


def test_ids_match_values_computed_by_hand():
    # Pinned literals: the hash of "<mod>:<name>" as the contract defines it, and
    # a shipped ingredient id (named_id's own anchor, verified against the game).
    assert named_id.of("blood-wolf:Frostbite") == 0xAF14EC8D
    assert SK.status_id("blood-wolf", "Frostbite") == 0xAF14EC8D
    assert named_id.of("Straw") == 0xE9F9A52E


def test_status_id_is_the_engine_hash_of_the_key():
    key = "blood-wolf:Frostbite"
    c = _crc(key)
    inner = bytes([0, c & 255, 0, (c >> 8) & 255, 0, (c >> 16) & 255, 0, c >> 24])
    assert SK.status_key("blood-wolf", "Frostbite") == key
    assert SK.status_id("blood-wolf", "Frostbite") == zlib.crc32(inner)
    assert named_id.of(key) == zlib.crc32(inner)
    # The engine's own "Status" category: the same combine, the shipped value.
    c = _crc("Status")
    assert named_id.from_crc(0, c) == zlib.crc32(
        bytes([0, c & 255, 0, (c >> 8) & 255, 0, (c >> 16) & 255, 0, c >> 24]))


def test_emit_writes_the_contract_file_and_the_rows(tmp_path):
    reg = ContentRegistry(mod_id="frost", experimental=True)
    reg.register("status", id="Frostbite", name="Frostbite", look="debuff",
                 icon="HUD\\Debuff_Chilled.png", combine="max", reapply="refresh")
    reg.register("status", id="Warmth", name="Warmth", look="buff", hidden=True,
                 value="int", combine="add", reapply="keep", max_stacks=3)
    written = reg.emit(tmp_path / "assets")
    assert sorted(p.name for p in written) == ["rows.json", "statuses.json"]
    doc = json.loads((tmp_path / "assets" / SK.STAGING / "statuses.json")
                     .read_text(encoding="utf-8"))
    assert doc == [
        {"key": "frost:Frostbite", "name": "Frostbite",
         "id": named_id.of("frost:Frostbite"), "value": "float", "combine": 3,
         "reapply": 2, "max_stacks": 0},
        {"key": "frost:Warmth", "name": "Warmth", "id": named_id.of("frost:Warmth"),
         "value": "int", "combine": 0, "reapply": 0, "max_stacks": 3},
    ]
    rows = json.loads((tmp_path / "assets" / SK.STAGING / "rows.json")
                      .read_text(encoding="utf-8"))
    assert [(r["look"], r["hidden"], r["combine"]) for r in rows] == \
        [("debuff", False, 3), ("buff", True, 0)]


def test_hud_combine_feeds_only_the_row(tmp_path):
    reg = ContentRegistry(mod_id="m", experimental=True)
    reg.register("status", id="A", name="A", look="buff", combine="max",
                 hud_combine="add")
    reg.register("status", id="B", name="B", look="buff", combine="min")
    reg.emit(tmp_path / "assets")
    stage = tmp_path / "assets" / SK.STAGING
    defs = json.loads((stage / "statuses.json").read_text(encoding="utf-8"))
    rows = json.loads((stage / "rows.json").read_text(encoding="utf-8"))
    assert [d["combine"] for d in defs] == [3, 4]
    assert all("hud_combine" not in d for d in defs)
    assert [r["combine"] for r in rows] == [0, 4]


def test_lint_warns_on_an_effective_sub_or_min_hud_kind(capsys):
    def run(**f):
        blocks = [{"kind": "status", "name": "S", "look": "buff", **f}]
        capsys.readouterr()
        errs, warns = lint._lint_content("m", blocks, experimental=True, mod_id="m")
        return errs, warns, capsys.readouterr().out

    base = run()[1]
    for f in ({"combine": "sub"}, {"combine": "max", "hud_combine": "min"}):
        errs, warns, out = run(**f)
        assert errs == 0 and warns == base + 1
        assert "no icon step" in out
    assert run(combine="sub", hud_combine="max")[1] == base
    assert run(combine="max")[1] == base


@pytest.mark.parametrize("fields, needle", [
    ({"look": "evil"}, "look"),
    ({"look": "buff", "combine": "avg"}, "combine"),
    ({"look": "buff", "hud_combine": "avg"}, "hud_combine"),
    ({"look": "buff", "reapply": "never"}, "reapply"),
    ({"look": "buff", "value": "string"}, "value"),
    ({"look": "buff", "max_stacks": -1}, "max_stacks"),
    ({"look": "buff", "hidden": "no"}, "hidden"),
    ({"look": "buff", "name": " padded "}, "name"),
])
def test_bad_status_fields_are_refused(tmp_path, fields, needle):
    fields = {"name": "X", **fields}
    with pytest.raises(ContentError, match=needle):
        SK.emit("m", ContentDef("status", "X", fields), tmp_path)


def test_status_is_experimental_and_needs_the_opt_in():
    reg = ContentRegistry(mod_id="m")
    with pytest.raises(ContentError, match="experimental"):
        reg.register("status", id="X", name="X", look="buff")


# --- lint and compat ----------------------------------------------------------

def test_lint_rejects_a_duplicate_status_name(capsys):
    blocks = [{"kind": "status", "name": "Frost", "look": "buff"},
              {"kind": "status", "name": "Frost", "look": "debuff"}]
    errs, _w = lint._lint_content("m", blocks, experimental=True, mod_id="m")
    assert errs >= 1
    assert "declared twice" in capsys.readouterr().out


def test_lint_accepts_distinct_status_names_declared_by_name(capsys):
    blocks = [{"kind": "status", "name": "Frost", "look": "buff"},
              {"kind": "status", "name": "Burn", "look": "debuff"}]
    errs, _w = lint._lint_content("m", blocks, experimental=True, mod_id="m")
    assert errs == 0, capsys.readouterr().out


def test_lint_rejects_a_vanilla_id(monkeypatch, capsys):
    monkeypatch.setattr(ST, "VANILLA_IDS", (named_id.of("m:Frost"),))
    blocks = [{"kind": "status", "name": "Frost", "look": "buff"}]
    errs, _w = lint._lint_content("m", blocks, experimental=True, mod_id="m")
    assert errs == 1
    assert "ships" in capsys.readouterr().out


def test_lint_reports_a_bad_field(capsys):
    blocks = [{"kind": "status", "name": "Frost", "look": "sideways"}]
    errs, _w = lint._lint_content("m", blocks, experimental=True, mod_id="m")
    assert errs == 1
    assert "look" in capsys.readouterr().out


def _manifest(root: Path, mod_id: str, names: list[str]) -> None:
    root.mkdir(parents=True)
    blocks = "".join(f'\n[[content]]\nkind = "status"\nname = "{n}"\nlook = "buff"\n'
                     for n in names)
    (root / "manifest.toml").write_text(
        f'[mod]\nid = "{mod_id}"\nname = "{mod_id}"\nversion = "1.0.0"\n'
        f'experimental = true\n{blocks}', encoding="utf-8")


def test_compat_flags_one_id_claimed_by_two_mods(tmp_path, monkeypatch):
    _manifest(tmp_path / "a", "a", ["Frost"])
    _manifest(tmp_path / "b", "b", ["Frost"])
    monkeypatch.setattr(compat, "MODS_DIR", tmp_path)
    assert not compat.analyze().status_collisions
    # Two mods that hash to one id: force the clash through the hash.
    real = SK.status_id
    monkeypatch.setattr(SK, "status_id", lambda mod, name: real("a", name))
    rep = compat.analyze()
    assert rep.has_errors and len(rep.status_collisions) == 1
    assert "a:Frost" in rep.status_collisions[0] and "b:Frost" in rep.status_collisions[0]


def test_compat_flags_a_vanilla_id(tmp_path, monkeypatch):
    _manifest(tmp_path / "a", "a", ["Frost"])
    monkeypatch.setattr(compat, "MODS_DIR", tmp_path)
    monkeypatch.setattr(ST, "VANILLA_IDS", (named_id.of("a:Frost"),))
    rep = compat.analyze()
    assert rep.has_errors and "ships" in rep.status_collisions[0]
