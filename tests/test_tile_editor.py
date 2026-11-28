"""The Map tab's 3D tile mode (`engine/tile_editor` + `cli/editor/tiles`).

Pure checks run anywhere; the end-to-end ones need game data (the uncooked
mirror or an install) and skip without it.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from rsmm.engine import corpus
from rsmm.engine import tile_editor as TE

TILE = "Tiles\\Dark_Hills\\6x6_Crystal_01.tiledef.ot"
PROP = "DarkHills\\SceneryObjects_DarkHills\\Wall_Ruins_Block_Small_A.entity.ot"

_TILEDEF = "Definitions/Tiles/Dark_Hills/6x6_Crystal_01.tiledef.ot.DtTileDefinition.gen"
needs_game = pytest.mark.skipif(
    corpus.read(_TILEDEF) is None,
    reason="no game data (uncooked mirror or install)")


def test_base_of_and_poi_id():
    assert TE.base_of(TILE) == "Dark_Hills/6x6_Crystal_01"
    assert TE.poi_id(TILE) == "6x6_crystal_01"
    with pytest.raises(TE.TileEditError):
        TE.base_of("Tiles\\..\\x.tiledef.ot")


@pytest.mark.parametrize("bad, msg", [
    ([], "at least one"),
    ([{"entity": "x", "pos": [0, 0, 0]}], "not an entity"),
    ([{"entity": PROP, "pos": [0, 0]}], "three numbers"),
    ([{"entity": PROP, "pos": [0, 9999, 0]}], "outside"),
    ([{"entity": PROP, "pos": [0, 0, 0], "scale": 0}], "outside"),
    ([{"entity": PROP, "pos": [0, 0, True]}], "expected a number"),
])
def test_clean_places_refuses_bad_input(bad, msg):
    with pytest.raises(TE.TileEditError, match=msg):
        TE.clean_places(bad)


@pytest.mark.parametrize("bad, msg", [
    ({"remove": [{"entity": PROP, "at": [0, 0, 0]}]}, "needs an own copy"),
    ({"own": True}, "nothing to save"),
    ({"own": True, "remove": [{"entity": PROP, "at": [0, 0]}]}, "three numbers"),
    ({"own": True, "move": [{"entity": PROP, "at": [0, 0, 0]}]}, "pos: needs three"),
    ({"own": True, "clear": True, "copies": 99}, "outside"),
])
def test_clean_edit_refuses_bad_input(bad, msg):
    with pytest.raises(TE.TileEditError, match=msg):
        TE.clean_edit(bad)


def test_clean_edit_clear_all_drops_single_removes():
    e = TE.clean_edit({"own": True, "clear": True, "remove": [{"entity": PROP, "at": [0, 0, 0]}]})
    assert e["clear"] is True and e["remove"] == []


@needs_game
def test_own_copy_removes_and_moves_one_game_object():
    tile = "Tiles\\Dark_Hills\\6x6_3Pigs_Material_Crates.tiledef.ot"
    objs = TE.objects_json(tile)["objects"]
    # two placements of ONE entity: removing the first must leave the second
    # standing for the move to find, so this proves `at` picks one placement
    a, b = objs[0], objs[3]
    assert a["entity"] == b["entity"]
    edit = {"own": True, "copies": 2,
            "remove": [{"entity": a["entity"], "at": a["pos"]}],
            "move": [{"entity": b["entity"], "at": b["pos"],
                      "pos": [b["pos"][0] + 1, b["pos"][1], b["pos"][2]]}]}
    files = TE.check(tile, edit)
    assert any(f.endswith(".level.ot.GameStream.gen") for f in files)
    text = TE.poi_toml(tile, edit)
    assert "own_level = true" in text and "replace_base" not in text
    assert TE.read_edit(text) == (tile, TE.clean_edit(edit))
    # an anchor that names no placement fails the emit instead of doing nothing
    with pytest.raises(TE.TileEditError, match="stands at"):
        TE.check(tile, {"own": True, "remove": [{"entity": a["entity"], "at": [99, 99, 99]}]})
    # one object is either moved or removed, never both
    both = {"own": True, "remove": [{"entity": a["entity"], "at": a["pos"]}],
            "move": [{"entity": a["entity"], "at": a["pos"], "pos": [0, 0, 0]}]}
    with pytest.raises(TE.TileEditError, match="also cleared"):
        TE.check(tile, both)
    assert TE.objects_json(tile)["at_tolerance"] > 0


@needs_game
def test_poi_toml_round_trips():
    places = [{"entity": PROP, "pos": [1.5, 0, -2], "yaw": 45, "scale": 1.2}]
    assert TE.read_edit(TE.poi_toml(TILE, {"places": places})) == (
        TILE, TE.clean_edit({"places": places}))


@needs_game
def test_objects_and_check_use_the_real_poi_emit():
    data = TE.objects_json(TILE)
    assert data["chapters"] == ["Dark_Hills"] and data["objects"]
    assert all(len(o["pos"]) == 3 and len(o["quat"]) == 4 for o in data["objects"])
    files = TE.check(TILE, {"places": [{"entity": PROP, "pos": [2, 0, 1], "yaw": 30, "scale": 1}]})
    assert any(f.endswith(".level.ot.GameStream.gen") for f in files)
    assert any(f.endswith(".tiledef.UsedRscCache.ot") for f in files)


@needs_game
def test_save_list_reopen_and_lint(tmp_path):
    from rsmm.cli.editor.bridge import Bridge
    from rsmm.cli.lint import lint_one

    b = Bridge(mods=tmp_path, tab="map")
    hdr = {"X-RSMM-Token": b.token, "Content-Type": "application/json"}

    def post(path, body):
        return b.request("POST", path, json.dumps(body).encode(), hdr)
    places = [{"entity": PROP, "pos": [2, 0, 1], "yaw": 30, "scale": 1}]
    st, _h, out = post("/map/api/t3/save", {"tile": TILE, "places": places, "mod_id": "my-tile"})
    assert st == 200, out
    assert json.loads(out)["path"] == "mods/my-tile/manifest.toml"
    mod = tmp_path / "my-tile"
    assert (mod / "manifest.toml").read_text().startswith(TE.TILE_MARK)

    st, _h, out = b.request("GET", "/map/api/t3/mods")
    assert json.loads(out)["mods"] == [{"id": "my-tile", "tiles": [TILE]}]
    st, _h, out = b.request("GET", "/map/api/t3/mod?id=my-tile")
    assert json.loads(out)["tiles"] == {TILE: TE.clean_edit({"places": places})}

    errors, _warnings = lint_one(mod)
    assert errors == 0

    # Saving the tile with nothing added takes its poi back out.
    st, _h, _ = post("/map/api/t3/save", {"tile": TILE, "places": [], "mod_id": "my-tile"})
    assert st == 200 and not (mod / "pois" / TE.poi_id(TILE)).exists()


def test_save_refuses_a_mod_the_editor_did_not_make(tmp_path):
    from rsmm.cli.editor.bridge import Bridge

    (tmp_path / "other").mkdir()
    (tmp_path / "other" / "manifest.toml").write_text('[mod]\nid = "other"\n')
    b = Bridge(mods=tmp_path)
    st, _h, out = b.request("POST", "/map/api/t3/save", json.dumps(
        {"tile": TILE, "places": [], "mod_id": "other"}).encode(),
        {"X-RSMM-Token": b.token, "Content-Type": "application/json"})
    assert st == 409, out
    assert Path(tmp_path / "other" / "manifest.toml").read_text() == '[mod]\nid = "other"\n'


@needs_game
def test_places_and_moves_take_any_rotation_and_scale(tmp_path):
    """A tilted, stretched place and a turned move survive the toml round
    trip and land in the emitted level with that rotation and scale."""
    import math

    from rsmm.engine import level_placements as LP
    from rsmm.sdk.content import ContentDef
    from rsmm.sdk.kinds import poi

    def emit(tile, edit, out):
        poi.emit("editor", ContentDef(kind="poi", id=TE.poi_id(tile),
                                      fields=TE._fields(tile, edit)), out)
        return [p for lv in out.rglob("*.level.ot.GameStream.gen")
                for _i, p in LP.objects(lv.read_bytes())]

    h = math.sqrt(0.5)
    places = [{"entity": PROP, "pos": [1, 0, 1], "quat": [h, 0, 0, h], "scale": [1, 2, 3]}]
    text = TE.poi_toml(TILE, {"places": places})
    assert "quat" in text and "scale  = [1, 2, 3]" in text
    tile, edit = TE.read_edit(text)
    assert edit == TE.clean_edit({"places": places})
    placed = [p for p in emit(tile, edit, tmp_path / "a") if p.ref == PROP]
    assert any(p.quat == pytest.approx((h, 0, 0, h), abs=1e-4)
               and p.scale == pytest.approx((1, 2, 3)) for p in placed)
    # a move may carry a turn and a size too
    tile2 = "Tiles\\Dark_Hills\\6x6_3Pigs_Material_Crates.tiledef.ot"
    o = TE.objects_json(tile2)["objects"][0]
    mv = {"own": True, "move": [{"entity": o["entity"], "at": o["pos"], "pos": o["pos"],
                                 "quat": [0, h, 0, h], "scale": 2}]}
    assert TE.read_edit(TE.poi_toml(tile2, mv)) == (tile2, TE.clean_edit(mv))
    moved = [p for p in emit(tile2, TE.clean_edit(mv), tmp_path / "b")
             if p.ref == o["entity"] and p.scale == pytest.approx((2, 2, 2))]
    assert moved and moved[0].quat == pytest.approx((0, h, 0, h), abs=1e-4)


@needs_game
def test_blank_tile_keeps_nothing_of_its_base(tmp_path):
    """A blank tile: every placement sunk (enemies too), its own kinds, flat
    ground; it reads back from its toml."""
    from rsmm.engine import level_placements as LP
    from rsmm.sdk.content import ContentDef
    from rsmm.sdk.kinds import poi

    tile = "Tiles\\Dark_Hills\\6x6_3Pigs_Material_Crates.tiledef.ot"
    # a 6x6 tile stands on the map's ground: no height of its own to set
    edit = {"own": True, "blank": {"kinds": ["Crystal"]},
            "places": [{"entity": PROP, "pos": [0, 0, 0], "yaw": 0, "scale": 1}]}
    assert TE.read_edit(TE.poi_toml(tile, edit)) == (tile, TE.clean_edit(edit))
    fields = TE._fields(tile, TE.clean_edit(edit))
    assert fields["kinds"] == ["Crystal"] and fields["clear"] == {"keep": [], "walls": True}
    poi.emit("editor", ContentDef(kind="poi", id=TE.poi_id(tile), fields=fields), tmp_path)
    standing = [p.ref for lv in tmp_path.rglob("*.level.ot.GameStream.gen")
                for _i, p in LP.objects(lv.read_bytes()) if p.pos[1] > -500]
    assert standing == [PROP]
