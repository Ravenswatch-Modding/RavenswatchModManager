"""`map` kind: clone a shipped mapdef and point a chapter at it by resref."""

from __future__ import annotations

from pathlib import Path

import pytest

from rsmm.engine import cooked
from rsmm.engine import map_cook as MC
from rsmm.engine import rsc_cache as RC
from rsmm.engine.paths import DATA_DIR
from rsmm.sdk.content import ContentDef, ContentError
from rsmm.sdk.kinds import maps

U = DATA_DIR / "uncooked" / "Definitions"
MODE = U / "GameModes" / f"All_Chapters{MC.MODE_GEN_SUFFIX}"
DH = U / "Maps" / f"Dark_Hills_LiveOps_Update5{MC.MAP_GEN_SUFFIX}"

needs_corpus = pytest.mark.skipif(not (MODE.is_file() and DH.is_file()),
                                  reason="uncooked corpus absent")


@needs_corpus
def test_shipped_chapters_decode_as_the_enum_branch():
    chapters = MC.read_chapters(MODE.read_bytes())
    assert [c["biome"] for c in chapters] == [0, 1, 2, 3]
    assert [c["more"] for c in chapters] == [1, 1, 1, 0]


@needs_corpus
def test_resref_chapter_round_trips_and_leaves_the_rest_alone():
    mode = MODE.read_bytes()
    new = MC.set_chapter_map(mode, 0, "Twilight_Hills")
    chapters = MC.read_chapters(new)
    assert chapters[0] == {"more": 1, "map": ("Definitions", "Maps\\Twilight_Hills.mapdef.ot")}
    assert chapters[1:] == MC.read_chapters(mode)[1:]
    assert cooked.emit(cooked.parse(new)) == new
    # The last chapter keeps "no more chapters follow".
    assert MC.read_chapters(MC.set_chapter_map(mode, 3, "X"))[3]["more"] == 0


@needs_corpus
def test_unknown_chapter_is_refused():
    with pytest.raises(MC.MapCookError):
        MC.set_chapter_map(MODE.read_bytes(), 4, "X")


@needs_corpus
def test_clone_is_the_base_unless_the_tribe_moves():
    base = DH.read_bytes()
    assert MC.clone_mapdef(base) == base
    body = MC.read_mapdef(MC.clone_mapdef(base, tribe="Gnolls"))
    assert body["tribe_ref"] == ["Definitions", "EnemyTribes\\Gnolls.enemytribedef.ot"]
    assert body["level_ref"] == MC.read_mapdef(base)["level_ref"]


@needs_corpus
def test_emit_writes_clone_cache_and_chapter(tmp_path):
    files = maps.emit("m", ContentDef(kind="map", id="Twilight_Hills", fields={
        "base": "Dark_Hills", "tribe": "Gnolls", "chapter": 0}), tmp_path)
    rel = sorted(f.relative_to(tmp_path).as_posix() for f in files)
    assert rel == [
        "Definitions/GameModes/All_Chapters.gamemodedefaultdef.UsedRscCache.ot",
        f"Definitions/GameModes/All_Chapters{MC.MODE_GEN_SUFFIX}",
        "Definitions/Maps/Twilight_Hills.mapdef.UsedRscCache.ot",
        f"Definitions/Maps/Twilight_Hills{MC.MAP_GEN_SUFFIX}",
        "Ot/DarkHills/Map_Twilight_Hills.level.ot.GameStream.gen",
        "Ot/DarkHills/Map_Twilight_Hills_TileGeneration.level.ot.GameStream.gen",
    ]
    cache = RC.parse((tmp_path / rel[2]).read_bytes())
    assert "Ot|DarkHills\\Map_Twilight_Hills_TileGeneration.level.ot|oCGameStream" in cache
    assert not any("Map_Dark_Hills_LiveOps_Update5" in ln for ln in cache)
    assert cache == sorted(cache), "the engine looks lines up; the cache must stay sorted"
    assert "Definitions|Maps\\Twilight_Hills.mapdef.ot|oCDtMapDefinition" in cache
    assert "Definitions|Maps\\Dark_Hills_LiveOps_Update5.mapdef.ot|oCDtMapDefinition" not in cache
    assert any("EnemyTribes\\Gnolls.enemytribedef.ot" in ln for ln in cache)
    mode_cache = RC.parse((tmp_path / rel[0]).read_bytes())
    assert "Definitions|Maps\\Twilight_Hills.mapdef.ot|oCDtMapDefinition" in mode_cache


@needs_corpus
def test_emit_without_chapter_leaves_the_game_mode_alone(tmp_path):
    files = maps.emit("m", ContentDef(kind="map", id="Plain_Hills",
                                      fields={"base": "Dark_Hills"}), tmp_path)
    assert all("GameModes" not in f.as_posix() for f in files)


@pytest.mark.parametrize("fields", [
    {"base": "Nowhere"},
    {"base": "Dark_Hills", "tribe": "Ogres"},        # ships no resource cache
    {"base": "Dark_Hills", "chapter": "0"},
])
@needs_corpus
def test_bad_fields_are_refused(tmp_path, fields):
    with pytest.raises(ContentError):
        maps.emit("m", ContentDef(kind="map", id="Bad_Map", fields=fields), tmp_path)


def test_shipped_names_cannot_be_reused(tmp_path):
    with pytest.raises(ContentError):
        maps.emit("m", ContentDef(kind="map", id="Dark_Hills_LiveOps_Update5",
                                  fields={"base": "Dark_Hills"}), tmp_path)


@needs_corpus
def test_clone_generates_from_its_own_pool(tmp_path):
    """mapdef -> own root level -> own tilegen level -> backref to the CLONE.
    Reusing the base's tilegen level made the clone place the base's pool."""
    from rsmm.engine import prop_cook as PC

    maps.emit("m", ContentDef(kind="map", id="Twilight_Hills",
                              fields={"base": "Dark_Hills"}), tmp_path)

    def refs(rel):
        return PC._refs_doc("oCGameStream", (tmp_path / rel).read_bytes())[1]["asset_refs"]

    mapdef = MC.read_mapdef((tmp_path / "Definitions/Maps" /
                             f"Twilight_Hills{MC.MAP_GEN_SUFFIX}").read_bytes())
    root_ref = "DarkHills\\Map_Twilight_Hills.level.ot"
    tg_ref = "DarkHills\\Map_Twilight_Hills_TileGeneration.level.ot"
    assert mapdef["level_ref"] == ["Ot", root_ref]
    root = refs(PC.level_cooked_path(root_ref))
    assert root[0] == root_ref and tg_ref in root
    assert "DarkHills\\Map_Dark_Hills_Terrain.level.ot" not in root  # via tilegen
    tg = refs(PC.level_cooked_path(tg_ref))
    assert "Maps\\Twilight_Hills.mapdef.ot" in tg
    assert "Maps\\Dark_Hills_LiveOps_Update5.mapdef.ot" not in tg
    assert "DarkHills\\Map_Dark_Hills_Terrain.level.ot" in tg        # terrain shared
    # Own identity: a level keeping its donor's GUID never registers.
    for ref in (root_ref, tg_ref):
        assert PC.level_guid((tmp_path / PC.level_cooked_path(ref)).read_bytes()) \
            not in {PC.level_guid(b) for b in (
                DATA_DIR.joinpath("uncooked", PC.level_cooked_path(
                    "DarkHills\\Map_Dark_Hills_LiveOps_Update5.level.ot")).read_bytes(),
                DATA_DIR.joinpath("uncooked", PC.level_cooked_path(
                    "DarkHills\\Map_Dark_Hills_LiveOps_Update5_TileGeneration.level.ot"
                )).read_bytes())}


@needs_corpus
def test_a_map_without_tile_generation_reuses_its_levels(tmp_path):
    files = maps.emit("m", ContentDef(kind="map", id="Other_Hut",
                                      fields={"base": "Baba_Yaga"}), tmp_path)
    assert not any(f.name.endswith(".level.ot.GameStream.gen") for f in files)


# ---- pool + tilegen on the clone ------------------------------------------

def _emit(tmp_path, **fields):
    maps.emit("m", ContentDef(kind="map", id="Twilight_Hills",
                              fields={"base": "Dark_Hills", **fields}), tmp_path)
    mapdef = (tmp_path / "Definitions/Maps" / f"Twilight_Hills{MC.MAP_GEN_SUFFIX}").read_bytes()
    cache = RC.parse((tmp_path / "Definitions/Maps/Twilight_Hills.mapdef.UsedRscCache.ot")
                     .read_bytes())
    return mapdef, cache


def _tiledef_lines(cache):
    return sorted(ln.split("|")[1] for ln in cache if ln.endswith("|oCDtTileDefinition"))


@needs_corpus
def test_pool_edit_keeps_pool_and_cache_one_for_one(tmp_path):
    from rsmm.engine import map_pool as MP

    dh = MP.read_pool(DH.read_bytes())
    av = MP.read_pool((U / "Maps" / f"Avalon_LiveOps_Update5{MC.MAP_GEN_SUFFIX}").read_bytes())
    extra = next(t for t in av if t not in dh)
    gone = "Tiles\\Dark_Hills\\3x3_Blocker_Props_01.tiledef.ot"
    assert gone in dh
    mapdef, cache = _emit(tmp_path, pool={
        "add": ["Avalon/" + extra.rsplit("\\", 1)[-1].removesuffix(".tiledef.ot")],
        "remove": ["3x3_Blocker_Props_01"]})
    pool = MP.read_pool(mapdef)
    assert extra in pool and gone not in pool and len(pool) == len(dh)
    # A pooled tile with no cache line is never loaded, so never placed.
    assert _tiledef_lines(cache) == sorted(pool)
    assert cache == sorted(cache)
    # The base map is not touched.
    assert MP.read_pool(DH.read_bytes()) == dh


@pytest.mark.parametrize("pool", [
    {"remove": ["40x40_Dark_Hills_Start_Update3"]},   # Start still has count 1
    {"remove": ["6x6_Teleporter_01"]},                 # the only Teleporter tile
    {"add": ["3x3_Crystal_01_Nope"]},                  # not a tile
    {"add": ["Nowhere/3x3_Crystal_01"]},
    {"remove": ["Avalon/6x6_Teleporter_01"]},          # not in this pool
    {"add": []},                                        # changes nothing
    {"swap": []},
])
@needs_corpus
def test_bad_pool_edits_are_refused(tmp_path, pool):
    with pytest.raises(ContentError):
        _emit(tmp_path, pool=pool)


@needs_corpus
def test_tilegen_edit_lands_on_the_clone_not_the_base(tmp_path):
    from rsmm.engine import map_editor as ME
    from rsmm.engine import prop_cook as PC
    from rsmm.engine import tilegen as TG

    _emit(tmp_path, tilegen={"kinds": {"Camp": {"count": 2}},
                             "fill": {"40x40": [], "64x64": []}})
    tg_ref = "DarkHills\\Map_Twilight_Hills_TileGeneration.level.ot"
    tg = TG.read((tmp_path / PC.level_cooked_path(tg_ref)).read_bytes())
    assert {k.name: k.count for k in tg.kinds.values()}["Camp"] == 2
    base = ME.load(ME.find_chapter("DarkHills"))
    assert {k.name: k.count for k in base.kinds.values()}["Camp"] == 5
    # Still the clone's own level: the backref names the clone.
    refs = PC._refs_doc("oCGameStream", (tmp_path / PC.level_cooked_path(tg_ref))
                        .read_bytes())[1]["asset_refs"]
    assert "Maps\\Twilight_Hills.mapdef.ot" in refs


@pytest.mark.parametrize("fields", [
    {"tilegen": {"kinds": {"Nope": {"count": 1}}}},
    {"tilegen": {"kinds": {"Camp": {"count": 5}}}},   # already vanilla
    {"tilegen": "Camp=2"},
])
@needs_corpus
def test_bad_tilegen_edits_are_refused(tmp_path, fields):
    with pytest.raises(ContentError):
        _emit(tmp_path, **fields)


@needs_corpus
def test_a_map_without_tile_generation_refuses_pool_and_tilegen(tmp_path):
    for fields in ({"pool": {"add": ["Dark_Hills/6x6_Teleporter_01"]}},
                   {"tilegen": {"kinds": {"Camp": {"count": 1}}}}):
        with pytest.raises(ContentError):
            maps.emit("m", ContentDef(kind="map", id="Other_Hut",
                                      fields={"base": "Baba_Yaga", **fields}), tmp_path)


# ---- hand-placed tiles ------------------------------------------------------

@needs_corpus
def test_a_placed_tile_is_pinned_to_its_slot_and_nowhere_else(tmp_path):
    from rsmm.engine import map_pool as MP
    from rsmm.engine import prop_cook as PC
    from rsmm.engine import tile_cook as TC
    from rsmm.engine import tilegen as TG

    base_tg = TG.read((U.parent / "Ot/DarkHills/Map_Dark_Hills_LiveOps_Update5_TileGeneration"
                       ".level.ot.GameStream.gen").read_bytes())
    sid = next(i for i in base_tg.sizes[base_tg.spawner.size_ids[
        base_tg.size_names.index("40x40")]].slots)
    shipped_pos = base_tg.slots[sid].pos
    maps.emit("m", ContentDef(kind="map", id="Hills_P", fields={
        "base": "Dark_Hills",
        "tilegen": {"add_slots": [{"group": "64x64", "pos": [30.0, 0.0, 40.0]}]},
        "place": {"new0": "Avalon/64x64_Ponds_And_Fountains_Camp",
                  str(sid): "Dark_Hills/40x40_Dark_Hills_Start_Update3"}}), tmp_path)
    tg = TG.read((tmp_path / PC.level_cooked_path(
        "DarkHills\\Map_Hills_P_TileGeneration.level.ot")).read_bytes())
    flags = ["RSMM_Hills_P_P1", "RSMM_Hills_P_P2"]
    assert tg.kind_names[-2:] == flags
    for flag, pos in zip(flags, [(30.0, 0.0, 40.0), shipped_pos], strict=True):
        ki = tg.kind_names.index(flag)
        assert tg.kinds[tg.spawner.kind_ids[ki]].count == 1
        [slot] = [s for s in tg.slots.values() if s.kinds and s.kinds[ki]]
        assert slot.pos == pos and sum(slot.kinds) == 1
        assert all(row[ki] == 1 for row in slot.compat)
        # Every shipped kind; another placement's kind requires its own flag,
        # which this copy does not carry.
        others = [tg.kinds[k] for k in tg.spawner.kind_ids if tg.kinds[k].name not in flags]
        assert all(flag in k.filter.excluded for k in others)
        assert all(flag in tg.sizes[g].filter.excluded for g in tg.spawner.size_ids)

    pool = MP.read_pool((tmp_path / "Definitions/Maps" /
                         f"Hills_P{MC.MAP_GEN_SUFFIX}").read_bytes())
    copy = "Tiles\\Avalon\\Hills_P_P1_64x64_Ponds_And_Fountains_Camp.tiledef.ot"
    assert copy in pool
    cache = RC.parse((tmp_path / "Definitions/Maps/Hills_P.mapdef.UsedRscCache.ot").read_bytes())
    assert _tiledef_lines(cache) == sorted(pool)
    td = TC.read((tmp_path / "Definitions/Tiles/Avalon" /
                  "Hills_P_P1_64x64_Ponds_And_Fountains_Camp.tiledef.ot.DtTileDefinition.gen")
                 .read_bytes())
    assert td.kinds == ["Camp", "RSMM_Hills_P_P1"]


@pytest.mark.parametrize("place", [
    {"9999": "Dark_Hills/6x6_Teleporter_01"},                  # no such slot
    {"new0": "Dark_Hills/6x6_Teleporter_01"},                  # no added slot
    {"25": "Dark_Hills/6x6_Teleporter_01"},                    # 64x64 slot, 6x6 tile
    {"25": "Nowhere/X"},
    {},
])
@needs_corpus
def test_bad_placements_are_refused(tmp_path, place):
    with pytest.raises(ContentError):
        maps.emit("m", ContentDef(kind="map", id="Hills_P", fields={
            "base": "Dark_Hills", "place": place}), tmp_path)


# ---- painted terrain -------------------------------------------------------

@needs_corpus
def test_terrain_strokes_give_the_clone_its_own_ground(tmp_path):
    from rsmm.engine import prop_cook as PC
    from rsmm.engine import terrain as TR
    from rsmm.engine import tilegen as TG

    maps.emit("m", ContentDef(kind="map", id="Hills_T", fields={
        "base": "Dark_Hills", "terrain": [
            {"op": "raise", "x": -56, "z": 147, "radius": 40, "amount": 6}]}), tmp_path)

    def refs(ref):
        return PC._refs_doc("oCGameStream", (tmp_path / PC.level_cooked_path(ref))
                            .read_bytes())[1]["asset_refs"]

    own = "DarkHills\\Map_Hills_T_Terrain.level.ot"
    assert "DarkHills\\Map_Hills_T_Scenery.level.ot" in refs("DarkHills\\Map_Hills_T.level.ot")
    for lvl in ("DarkHills\\Map_Hills_T_TileGeneration.level.ot",
                "DarkHills\\Map_Hills_T_Scenery.level.ot"):
        assert own in refs(lvl)
        assert "DarkHills\\Map_Dark_Hills_Terrain.level.ot" not in refs(lvl)
    t = TR.read((tmp_path / PC.level_cooked_path(own)).read_bytes(), {TR.HEIGHT})
    tg = TG.read((tmp_path / PC.level_cooked_path(
        "DarkHills\\Map_Hills_T_TileGeneration.level.ot")).read_bytes())
    # every slot sits on the painted ground
    for s in tg.slots.values():
        if s.kinds:
            assert s.pos[1] == pytest.approx(t.height_at(s.pos[0], s.pos[2]), abs=0.02)
    cache = RC.parse((tmp_path / "Definitions/Maps/Hills_T.mapdef.UsedRscCache.ot").read_bytes())
    assert f"Ot|{own}|oCGameStream" in cache


@needs_corpus
def test_a_map_without_terrain_refuses_strokes(tmp_path):
    with pytest.raises(ContentError):
        maps.emit("m", ContentDef(kind="map", id="Other_Hut", fields={
            "base": "Baba_Yaga", "terrain": [{"op": "raise", "x": 0, "z": 0,
                                               "radius": 5, "amount": 1}]}), tmp_path)


# ---- blank map -------------------------------------------------------------

@needs_corpus
@pytest.mark.parametrize("base,folder", [("Dark_Hills", "DarkHills"),
                                         ("Storm_Island", "Storm_Island"),
                                         ("Avalon", "Avalon")])
def test_blank_map_is_flat_empty_and_owns_its_levels(tmp_path, base, folder):
    from rsmm.engine import level_placements as LP
    from rsmm.engine import prop_cook as PC
    from rsmm.engine import terrain as TR
    from rsmm.engine import tilegen as TG

    mid = f"Blank_{folder}"
    maps.emit("m", ContentDef(kind="map", id=mid, fields={
        "base": base, "blank": {"height": 4}}), tmp_path)

    def own(ref):
        return (tmp_path / PC.level_cooked_path(ref)).read_bytes()

    def refs(blob):
        return PC._refs_doc("oCGameStream", blob)[1]["asset_refs"]

    root = own(f"{folder}\\Map_{mid}.level.ot")
    scenery = own(f"{folder}\\Map_{mid}_Scenery.level.ot")
    raw_terrain = own(f"{folder}\\Map_{mid}_Terrain.level.ot")
    terrain = TR.read(raw_terrain)
    # flat at 4 m; every water layer (Storm Island / Avalon have two) and LD Block zero
    h = terrain.layers[TR.HEIGHT].cells
    assert min(h) == max(h) == pytest.approx((4 + 50) / 100)
    for name in ("LD Block", "Base Water Height"):
        layer = terrain.layers.get(name)
        if layer is not None:
            assert max(layer.cells) == 0.0, name
    assert TR.fill(raw_terrain, "Base Water Height", 0.0) == raw_terrain
    # no painted surfaces, white vertex colour
    for name, layer in terrain.layers.items():
        if name not in TR.METRE_LAYERS and layer.kind == "float":
            assert max(layer.cells) == 0.0, name
        elif layer.kind == "rgba":
            assert set(layer.cells) == {255}, name
    # every vanilla scenery / collision sublevel is replaced by an emptied clone
    listed = refs(root) + refs(scenery)
    vanilla = [r for r in listed if "Scenery_" in r or r.endswith("_Collisions.level.ot")]
    assert vanilla and all(f"Map_{mid}_" in r for r in vanilla), vanilla
    for r in vanilla:
        blob = own(r)
        assert all(p.pos[1] < -500 for _, p in LP.objects(blob))
        assert all(min(y for _x, y, _z in pts) < -500 for _, _n, pts in LP.walls(blob))
    assert all(p.pos[1] < -500 for _, p in LP.objects(scenery))
    # no level of the clone still names the vanilla terrain or scenery parent
    tg = own(f"{folder}\\Map_{mid}_TileGeneration.level.ot")
    for blob in (root, scenery, tg):
        assert not any(r.endswith(f"\\Map_{base}_Terrain.level.ot") for r in refs(blob))
    assert not any(r.endswith(f"\\Map_{base}_Scenery.level.ot") for r in refs(root) + refs(tg))
    # slots sit on the flat ground
    assert {round(s.pos[1], 2) for s in TG.read(tg).slots.values() if s.kinds} == {4.0}


@needs_corpus
def test_blank_map_keeps_its_paint_strokes(tmp_path):
    from rsmm.engine import prop_cook as PC
    from rsmm.engine import terrain as TR

    maps.emit("m", ContentDef(kind="map", id="Blank_Paint", fields={
        "base": "Dark_Hills", "blank": {"height": 4}, "terrain": []}), tmp_path)
    ref = "DarkHills\\Map_Blank_Paint_Terrain.level.ot"
    t = TR.read((tmp_path / PC.level_cooked_path(ref)).read_bytes())
    surf = next(n for n, ly in t.layers.items()
                if ly.kind == "float" and n not in TR.METRE_LAYERS and n != "LD Block")
    rgba = next(n for n, ly in t.layers.items() if ly.kind == "rgba")
    maps.emit("m", ContentDef(kind="map", id="Blank_Paint", fields={
        "base": "Dark_Hills", "blank": {"height": 4}, "terrain": [
            {"op": "set", "layer": surf, "x": 0, "z": 0, "radius": 20, "amount": 1},
            {"op": "tint", "layer": rgba, "x": 0, "z": 0, "radius": 20,
             "amount": 1, "color": "#ff0000"}]}), tmp_path)
    t = TR.read((tmp_path / PC.level_cooked_path(ref)).read_bytes())
    assert max(t.layers[surf].cells) > 0, surf
    assert set(t.layers[rgba].cells) != {255}, rgba


@needs_corpus
def test_surfaces_give_the_terrain_the_mods_own_textures(tmp_path):
    from rsmm.engine import image as IMG
    from rsmm.engine import map_scene as MS
    from rsmm.engine import prop_cook as PC
    from rsmm.engine import terrain_look as TL

    mod = tmp_path / "mod"
    (mod / "art" / "ground").mkdir(parents=True)
    (mod / "art" / "ground" / "moss.png").write_bytes(
        IMG.encode_png(8, 8, bytes([40, 160, 60, 255] * 64)))
    out = mod / "assets"
    maps.emit("m", ContentDef(kind="map", id="Mossy", fields={
        "base": "Dark_Hills", "surfaces": {"R": "art/ground/moss.png"}}), out)
    terrain = (out / PC.level_cooked_path("DarkHills\\Map_Mossy_Terrain.level.ot")).read_bytes()
    mat_ref = TL.terrain_material_ref(terrain)
    assert mat_ref.endswith("M_Terrain_Mossy.mat.ot")
    albs = TL.slot_albedos((out / PC.art_cooked_path(mat_ref)).read_bytes())
    assert albs["R"].endswith("T_Mossy_R_ALB.tga")
    assert albs["G"] == TL.slot_albedos(MS._bytes(
        "3D", "Scenery\\DarkHills\\M_Terrain_DarkHills.mat.ot"))["G"]   # the rest stay shipped
    assert (out / PC.art_cooked_path(albs["R"])).is_file()
    cache = RC.parse((out / "Definitions/Maps/Mossy.mapdef.UsedRscCache.ot").read_bytes())
    assert any("M_Terrain_Mossy" in ln for ln in cache)
    assert any("T_Mossy_R_ALB" in ln for ln in cache)
    for bad in ({"Q": "art/ground/moss.png"}, {"R": ""}, "moss"):
        with pytest.raises(ContentError):
            maps.emit("m", ContentDef(kind="map", id="Mossy2", fields={
                "base": "Dark_Hills", "surfaces": bad}), out)


@needs_corpus
def test_minimap_is_the_mods_own_picture(tmp_path):
    from rsmm.engine import image as IMG
    from rsmm.engine import level_placements as LP
    from rsmm.engine import prop_cook as PC

    mod = tmp_path / "mod"
    (mod / "art").mkdir(parents=True)
    (mod / "art" / "minimap.png").write_bytes(IMG.encode_png(8, 8, bytes([90, 120, 60, 255] * 64)))
    out = mod / "assets"
    maps.emit("m", ContentDef(kind="map", id="Drawn", fields={
        "base": "Dark_Hills", "minimap": "art/minimap.png"}), out)
    gameplay = sorted((out / "Ot" / "DarkHills").glob("Map_Drawn_Gameplay*.GameStream.gen"))
    assert len(gameplay) == 1
    blob = gameplay[0].read_bytes()
    refs = PC._refs_doc("oCGameStream", blob)[1]["asset_refs"]
    assert "MiniMap\\Drawn_Mini_Map.png" in refs and "MiniMap\\Drawn_Map_High.png" in refs
    assert "MiniMap\\Dark_Hills_Map_High_Mask.png" in refs           # the masks stay shipped
    assert LP.minimap_rect(blob) == (-225.0, -220.0, 450.0, 450.0)
    for kind in ("Mini_Map", "Map_High"):
        assert (out / "Ui" / "MiniMap" / f"Drawn_{kind}.png.Texture.dxt").is_file()
    root = (out / PC.level_cooked_path("DarkHills\\Map_Drawn.level.ot")).read_bytes()
    assert any(r.endswith(gameplay[0].name.split(".")[0] + ".level.ot")
               for r in PC._refs_doc("oCGameStream", root)[1]["asset_refs"])
    cache = RC.parse((out / "Definitions/Maps/Drawn.mapdef.UsedRscCache.ot").read_bytes())
    assert "Ui|MiniMap\\Drawn_Map_High.png|oCTexture" in cache


@needs_corpus
def test_blank_is_validated(tmp_path):
    for bad in ({"height": 80}, {"height": "x"}, {"level": 1}, "yes"):
        with pytest.raises(ContentError):
            maps.emit("m", ContentDef(kind="map", id="Bad_Blank", fields={
                "base": "Dark_Hills", "blank": bad}), tmp_path)


# ---- objects ---------------------------------------------------------------

def _tetra_glb() -> bytes:
    from rsmm.engine import gltf
    b = gltf.GlbBuilder()
    pi = b.add_positions([(0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1)])
    ni = b.add_vec3([(0, 1, 0)] * 4)
    ui = b.add_vec2([(0, 0)] * 4)
    ii = b.add_indices([0, 1, 2, 0, 1, 3, 0, 2, 3, 1, 2, 3])
    m = b.add_mesh(gltf.Mesh(name="t", primitives=[gltf.Primitive(
        attributes={"POSITION": pi, "NORMAL": ni, "TEXCOORD_0": ui}, indices=ii)]))
    b.add_node(gltf.Node(name="t", mesh=m), is_root=True)
    return b.build_glb()


@needs_corpus
@pytest.mark.parametrize("base,folder", [("Dark_Hills", "DarkHills"),
                                         ("Storm_Island", "Storm_Island")])
def test_objects_stand_in_the_maps_own_scenery(tmp_path, base, folder):
    from rsmm.engine import image as IMG
    from rsmm.engine import level_placements as LP
    from rsmm.engine import prop_cook as PC

    mod = tmp_path / "mod"
    (mod / "art").mkdir(parents=True)
    (mod / "art" / "hut.glb").write_bytes(_tetra_glb())
    (mod / "art" / "hut.png").write_bytes(IMG.encode_png(4, 4, bytes([200, 30, 30, 255] * 16)))
    shipped = "DarkHills\\SceneryObjects_DarkHills\\Wall_Ruins_Block_Small_A.entity.ot"
    hut = {"model": "art/hut.glb", "texture": "art/hut.png",
           "collision": [{"kind": "ground", "size": [2, 1, 2], "at": [0, -1, 0]}]}
    mid = f"Built_{folder}"
    out = mod / "assets"
    maps.emit("m", ContentDef(kind="map", id=mid, fields={
        "base": base, "blank": True, "objects": [
            {"entity": shipped, "pos": [10, 0, 20], "yaw": 90},
            {**hut, "pos": [5, 0, 5], "scale": 2},
            {**hut, "pos": [-5, 0, 5], "quat": [0.5, 0, 0, 0.5], "scale": [1, 2, 3]}]}), out)

    own = f"DarkHills\\SceneryObjects_DarkHills\\{mid}_Obj0.entity.ot"
    assert (out / PC.entity_cooked_path(own)).is_file()
    assert not (out / PC.entity_cooked_path(own.replace("Obj0", "Obj1"))).is_file()  # one per model
    levels = sorted((out / "Ot" / folder).glob(f"Map_{mid}_Scenery_*.level.ot.GameStream.gen"))
    placed = [p for lv in levels for _i, p in LP.objects(lv.read_bytes()) if p.pos[1] > -500]
    assert sorted((p.ref, p.pos) for p in placed) == sorted([
        (shipped, (10.0, 0.0, 20.0)), (own, (5.0, 0.0, 5.0)), (own, (-5.0, 0.0, 5.0))])
    tilted = next(p for p in placed if p.pos == (-5.0, 0.0, 5.0))
    assert tilted.quat == pytest.approx((0.5 ** 0.5, 0, 0, 0.5 ** 0.5), abs=1e-5)
    assert tilted.scale == pytest.approx((1, 2, 3))
    cache = RC.parse((out / f"Definitions/Maps/{mid}.mapdef.UsedRscCache.ot").read_bytes())
    for ref in (shipped, own):
        assert any(f"|{ref}|" in ln for ln in cache), ref


_RUIN = "DarkHills\\SceneryObjects_DarkHills\\Wall_Ruins_Block_Small_A.entity.ot"


@needs_corpus
def test_colliders_and_wandering_camps(tmp_path):
    from rsmm.engine import collision_volumes as CV
    from rsmm.engine import level_placements as LP
    from rsmm.engine import prop_cook as PC

    camp = "DarkHills\\Dark_Hills_Enemy_Spawner\\Dark_Hills_Basic_Enemy_Camp_Wandering.entity.ot"
    wall = {"kind": "wall", "size": [4, 3, 0.5], "at": [0, 1.5, 0]}
    deck = {"kind": "ground", "size": [4, 0.5, 4], "at": [0, -0.25, 0], "rot": [10, 0, 0]}
    out = tmp_path / "assets"
    maps.emit("m", ContentDef(kind="map", id="Walls_And_Camps", fields={
        "base": "Dark_Hills", "blank": True, "objects": [
            {"collider": True, "collision": [wall], "pos": [1, 0, 1]},
            {"collider": True, "collision": [wall, deck], "pos": [5, 0, 5], "yaw": 45},
            {"entity": camp, "pos": [20, 0, 20]},
            {"entity": camp, "pos": [30, 0, 20],
             "camp": {"difficulty": "Very_Hard", "power": [300, 600]}}]}), out)

    own = "DarkHills\\Objects_DarkHills\\Walls_And_Camps_Obj{}.entity.ot"
    vols = [CV.read_volumes((out / PC.entity_cooked_path(own.format(k))).read_bytes())
            for k in (0, 1)]
    live = [[(v["group"], v["half"]) for _n, v in vs if v["obj_4"][8] > -1000] for vs in vols]
    assert live == [[(8, (2.0, 1.5, 0.25))], [(8, (2.0, 1.5, 0.25)), (7, (2.0, 0.25, 2.0))]]
    levels = sorted((out / "Ot" / "DarkHills").glob(
        "Map_Walls_And_Camps_Scenery_*.level.ot.GameStream.gen"))
    placed = {p.ref for lv in levels for _i, p in LP.objects(lv.read_bytes()) if p.pos[1] > -500}
    tuned = "Enemies\\Enemy_Spawner\\Walls_And_Camps_Camp2.entity.ot"
    assert placed == {own.format(0), own.format(1), camp, tuned}
    from rsmm.engine.entity_graph_edit import EntityFile
    ef = EntityFile((out / PC.entity_cooked_path(tuned)).read_bytes(), tuned)
    sel = "Enemy camp entity selector to spawn"
    assert (ef.get_value(sel, "value_4"), ef.get_value(sel, "value_5")) == (300, 600)
    assert b"Very_Hard.enemycampdifficultydef" in ef.to_bytes()
    cache = RC.parse((out / "Definitions/Maps/Walls_And_Camps.mapdef.UsedRscCache.ot").read_bytes())
    for ref in (camp, own.format(0), own.format(1), tuned,
                "EnemyCampDifficulties\\Very_Hard.enemycampdifficultydef.ot"):
        assert any(f"|{ref}|" in ln for ln in cache), ref


@needs_corpus
def test_tiling_sets_the_clones_terrain_material(tmp_path):
    from rsmm.engine import terrain_look as TL

    out = tmp_path / "assets"
    maps.emit("m", ContentDef(kind="map", id="Big_Ground", fields={
        "base": "Dark_Hills", "tiling": {"base": 0.04, "R": 0.3}}), out)
    mats = list(out.rglob("M_Terrain_Big_Ground.mat.ot*"))
    assert len(mats) == 1
    p = TL.material_params(mats[0].read_bytes())
    assert p["u_UV_base_scale"][0] == pytest.approx(0.04)
    assert p["u_UV_R_scale"][0] == pytest.approx(0.3)
    assert p["u_UV_G_scale"][0] == pytest.approx(0.12)          # untouched
    with pytest.raises(ContentError, match="tiling"):
        maps.emit("m", ContentDef(kind="map", id="Bad_Tiling", fields={
            "base": "Dark_Hills", "tiling": {"Q": 0.1}}), tmp_path / "bad")


@needs_corpus
def test_objects_are_validated(tmp_path):
    for bad in ([{"pos": [0, 0, 0]}],
                [{"entity": "Nope\\X.entity.ot", "pos": [0, 0, 0]}],
                [{"model": "art/missing.glb", "pos": [0, 0, 0]}],
                [{"entity": _RUIN, "pos": [0, 0]}],
                [{"entity": _RUIN, "pos": [0, 0, 0], "yaw": 1, "quat": [0, 0, 0, 1]}],
                [{"entity": _RUIN, "pos": [0, 0, 0], "quat": [0, 0, 0, 0]}],
                [{"entity": _RUIN, "pos": [0, 0, 0], "scale": [1, 0, 1]}],
                [{"collider": True, "pos": [0, 0, 0]}],
                [{"entity": _RUIN, "pos": [0, 0, 0], "camp": {"difficulty": "Hard"}}],
                [{"collider": True, "entity": _RUIN, "pos": [0, 0, 0],
                  "collision": [{"kind": "wall", "size": [1, 1, 1], "at": [0, 0, 0]}]}]):
        with pytest.raises(ContentError):
            maps.emit("m", ContentDef(kind="map", id="Bad_Obj", fields={
                "base": "Dark_Hills", "objects": bad}), tmp_path / "assets")


@needs_corpus
def test_scenery_edits_land_on_the_clones_copy(tmp_path):
    from rsmm.engine import corpus
    from rsmm.engine import level_placements as LP
    from rsmm.engine import map_editor as ME
    from rsmm.engine import prop_cook as PC
    from rsmm.engine import scenery_edit as SE

    ch = ME.find_chapter("DarkHills")
    level = next(lv for lv in SE.levels(ch)
                 if lv.endswith("_Scenery_NorthEast.level.ot.GameStream.gen"))
    oid, pl = LP.objects(corpus.read(level))[5]
    edits = {level: {"removes": {str(oid): {"entity": pl.ref, "at": list(pl.pos)}}}}
    maps.emit("m", ContentDef(kind="map", id="Hills_S", fields={
        "base": "Dark_Hills", "scenery": edits}), tmp_path)
    own = tmp_path / PC.level_cooked_path("DarkHills\\Map_Hills_S_Scenery_NorthEast.level.ot")
    got = dict(LP.objects(own.read_bytes()))
    assert got[oid].pos[1] < pl.pos[1] - 500          # sunk on the clone's copy
    assert not (tmp_path / level).exists()            # the shipped level is not emitted
    assert dict(LP.objects(corpus.read(level)))[oid].pos == pl.pos


@needs_corpus
def test_a_blank_map_has_no_enemy_camps_but_keeps_its_day_cycle(tmp_path):
    from rsmm.engine import level_placements as LP
    from rsmm.engine import prop_cook as PC

    maps.emit("m", ContentDef(kind="map", id="Calm_Hills", fields={
        "base": "Dark_Hills", "blank": True}), tmp_path)
    level = PC.level_cooked_path("DarkHills\\Map_Calm_Hills_Gameplay.level.ot")
    own = (tmp_path / level).read_bytes()
    placed = [p for _i, p in LP.objects(own)]
    camps = [p for p in placed if "Enemy_Camp" in p.ref]
    assert len(camps) == 104 and all(p.pos[1] < -500 for p in camps)
    day = [p for p in placed if "Day_Cycle_Manager" in p.ref]
    assert len(day) == 1 and day[0].pos[1] > -500
