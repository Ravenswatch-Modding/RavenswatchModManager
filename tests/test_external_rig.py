"""A skeleton of the model's own (hero from scratch, ``frame.skeleton``): the
glb's armature becomes the game skeleton (skeleton_codec.from_glb), the mesh
binds to it by name (geometry_cook ``palette``), clips are cooked for its bones
with no shipped track list (anim_cook.cook_own), and the hero's bone layers
list its bones (hero_frame.own_layers)."""

from __future__ import annotations

import json
import struct

import pytest

from rsmm.engine import anim_cook as AC
from rsmm.engine import cooked, corpus, gltf
from rsmm.engine import skeleton_codec as SK
from rsmm.engine.cooked_schemas import animation as A

#: (name, parent index, local translation)
_ARMATURE = [("Hips", -1, (0.0, 1.0, 0.0)), ("Spine", 0, (0.0, 0.5, 0.0)),
             ("Head", 1, (0.0, 0.4, 0.0))]
_SHIPPED = "Characters\\Heroes\\Kintaro\\Animations\\Kintaro_FightingIdle_Loop.fbx"
_TAIL = "3D/Characters/Heroes/Kintaro/Animations/Kintaro_FightingIdle_Loop.fbx.Animation.gen"
_RIG_GEO = "3D/Characters/Heroes/Kintaro/Kintaro_GEO.fbx.Geometry.gen"
needs_clip = pytest.mark.skipif(corpus.read(_TAIL) is None, reason="no animation corpus")
needs_geo = pytest.mark.skipif(corpus.read(_RIG_GEO) is None, reason="no hero corpus")


def _world_y(i: int) -> float:
    y = 0.0
    while i >= 0:
        y += _ARMATURE[i][2][1]
        i = _ARMATURE[i][1]
    return y


def _with_animation(glb: bytes, name: str, node: int, times: list[float],
                    quats: list[tuple], path: str = "rotation") -> bytes:
    """``glb`` with one rotation (or ``path``) clip on ``node`` appended."""
    doc, binc = gltf.read_glb(glb)
    binc = bytearray(binc)
    vec = "VEC4" if path == "rotation" else "VEC3"
    for vals, typ in ((times, "SCALAR"), ([c for q in quats for c in q], vec)):
        data = struct.pack(f"<{len(vals)}f", *vals)
        doc.setdefault("bufferViews", []).append({"buffer": 0, "byteOffset": len(binc),
                                   "byteLength": len(data)})
        binc += data
        doc.setdefault("accessors", []).append({"bufferView": len(doc["bufferViews"]) - 1,
                                 "componentType": 5126, "count": len(times), "type": typ})
    n = len(doc["accessors"])
    doc.setdefault("animations", []).append({"name": name, "samplers": [
        {"input": n - 2, "output": n - 1, "interpolation": "LINEAR"}],
        "channels": [{"sampler": 0, "target": {"node": node, "path": path}}]})
    doc["buffers"] = [{"byteLength": len(binc)}]
    js = json.dumps(doc).encode()
    js += b" " * (-len(js) % 4)
    binc += b"\0" * (-len(binc) % 4)
    total = 12 + 8 + len(js) + 8 + len(binc)
    return (struct.pack("<III", 0x46546C67, 2, total) + struct.pack("<II", len(js), 0x4E4F534A)
            + js + struct.pack("<II", len(binc), 0x004E4942) + bytes(binc))


def _body() -> bytes:
    """Three joints (Hips > Spine > Head) on none of the game's bone names,
    a tetrahedron skinned one vertex per joint, and a clip turning the spine."""
    b = gltf.GlbBuilder()
    for k, (name, _p, t) in enumerate(_ARMATURE):
        b.add_node(gltf.Node(name=name, translation=t,
                             children=[k + 1] if k + 1 < len(_ARMATURE) else []),
                   is_root=k == 0)
    ibm = b.add_mat4_array([[1.0 if i % 5 == 0 else 0.0 for i in range(12)]
                            + [0.0, -_world_y(k), 0.0, 1.0] for k in range(len(_ARMATURE))])
    skin = b.add_skin(gltf.Skin(name="s", joints=[0, 1, 2], inverse_bind_matrices=ibm))
    mesh = b.add_mesh(gltf.Mesh(name="body", primitives=[gltf.Primitive(attributes={
        "POSITION": b.add_positions([(0, 1, 0), (0.3, 1.5, 0), (0, 1.9, 0.3), (0, 2, 0)]),
        "NORMAL": b.add_vec3([(0, 1, 0)] * 4), "TEXCOORD_0": b.add_vec2([(0, 0)] * 4),
        "JOINTS_0": b.add_joints([(0, 0, 0, 0), (1, 0, 0, 0), (2, 0, 0, 0), (2, 0, 0, 0)]),
        "WEIGHTS_0": b.add_weights([(1.0, 0.0, 0.0, 0.0)] * 4)},
        indices=b.add_indices([0, 1, 2, 0, 1, 3, 0, 2, 3, 1, 2, 3]))]))
    b.add_node(gltf.Node(name="body", mesh=mesh, skin=skin), is_root=True)
    turn = (0.0, 0.3826834, 0.0, 0.9238795)          # 45 degrees about Y
    return _with_animation(b.build_glb(), "Turn", 1, [0.0, 0.5, 1.0],
                           [(0.0, 0.0, 0.0, 1.0), turn, (0.0, 0.0, 0.0, 1.0)])


def _empty() -> SK.Skeleton:
    return SK.Skeleton(0, 0, 0, [], (0.0,) * 6)


def test_the_glbs_armature_becomes_the_skeleton_with_a_root_and_sockets():
    s = SK.from_glb(_body(), _empty(), root="DEF.RootMotion",
                    sockets={"DEF.Head": ("Head", (0.0, 0.1, 0.0))})
    assert [(b.name, b.parent) for b in s.bones] == [
        ("DEF.RootMotion", -1), ("Hips", 0), ("Spine", 1), ("Head", 2), ("DEF.Head", 3)]
    assert all(b.name2 == b.name and b.flag == 0 for b in s.bones)
    world = {b.name: b.world[13] for b in s.bones}
    assert world["Head"] == pytest.approx(1.9) and world["DEF.Head"] == pytest.approx(2.0)
    # the inverse rest pose is the glb's bind pose
    for b, k in ((s.bones[i + 1], i) for i in range(3)):
        assert b.inverse[13] == pytest.approx(-_world_y(k))


def test_sockets_and_roots_that_clash_or_dangle_are_refused():
    with pytest.raises(SK.SkeletonError, match="already a bone"):
        SK.from_glb(_body(), _empty(), sockets={"Head": ("Hips", (0, 0, 0))})
    with pytest.raises(SK.SkeletonError, match="no bone 'Tail'"):
        SK.from_glb(_body(), _empty(), sockets={"DEF.Head": ("Tail", (0, 0, 0))})
    with pytest.raises(SK.SkeletonError, match="already has a bone"):
        SK.from_glb(_body(), _empty(), root="Hips")


@needs_clip
def test_a_clip_cooks_onto_the_skeletons_own_bones():
    tail = b"".join(s.payload for s in cooked.parse(corpus.read(_TAIL)).sections)
    bones = ["DEF.RootMotion", "Hips", "Spine", "Head"]
    payload = AC.cook_own(_body(), bones, tail, name="Turn")
    anim = A.parse_payload(payload)
    assert A.emit_payload(anim) == payload
    assert [t.name for t in anim.tracks] == bones
    assert anim.duration == pytest.approx(1.0)
    spine = anim.tracks[2]
    assert len(spine.r_times) == 3
    assert A._decode_quat(spine.r_values[1])[1] == pytest.approx(0.3826834, abs=1e-4)
    # bones the clip leaves alone hold the glb's rest; the added root is identity
    head = anim.tracks[3]
    assert A._decode_trans_scale(head.t_values[0]) == pytest.approx((0.0, 0.4, 0.0), abs=1e-3)
    assert A._decode_trans_scale(anim.tracks[0].t_values[0]) == (0.0, 0.0, 0.0)
    with pytest.raises(AC.AnimCookError, match="lacks"):
        AC.cook_own(_body(), ["Hips", "Head"], tail, name="Turn")


def _library(armature: list[tuple]) -> bytes:
    """An animation library's glb: a bare armature (``(name, translation)``,
    a chain) on the body's bone names at lengths of its own, and a clip
    stretching its spine 10%."""
    b = gltf.GlbBuilder()
    for k, (name, t) in enumerate(armature):
        b.add_node(gltf.Node(name=name, translation=t,
                             children=[k + 1] if k + 1 < len(armature) else []),
                   is_root=k == 0)
    spine = next(i for i, (n, _) in enumerate(armature) if n == "Spine")
    t = armature[spine][1]
    return _with_animation(b.build_glb(), "Stretch", spine, [0.0, 1.0],
                           [t, tuple(c * 1.1 for c in t)], path="translation")


def _cook_for_body(glb: bytes, tail: bytes) -> A.Animation:
    """``glb``'s clip cooked for the body's bones as heros.emit does for a
    clip whose source is not the model."""
    model = _body()
    payload = AC.cook_own(glb, ["DEF.RootMotion", "Hips", "Spine", "Head"], tail,
                          name="Stretch", rest=AC.glb_rest(model))
    payload = AC.retarget(payload, AC.glb_rest_translations(glb),
                          AC.glb_rest_translations(model))
    return A.parse_payload(payload)


def _ty(track) -> list[float]:
    return [A._decode_trans_scale(v)[1] for v in track.t_values]


@needs_clip
def test_a_library_clip_takes_the_models_bone_lengths():
    tail = b"".join(s.payload for s in cooked.parse(corpus.read(_TAIL)).sections)
    lib = _library([("Hips", (0.0, 1.2, 0.0)), ("Spine", (0.0, 0.6, 0.0)),
                    ("Head", (0.0, 0.48, 0.0))])
    root, hips, spine, head = _cook_for_body(lib, tail).tracks
    # the stretch keeps its 10%, from the model's spine (0.5), not the library's
    assert _ty(spine) == pytest.approx([0.5, 0.55], abs=2e-3)
    # bones the clip holds at the library's rest sit at the model's
    assert _ty(hips) == pytest.approx([1.0], abs=2e-3)
    assert _ty(head) == pytest.approx([0.4], abs=2e-3)
    assert _ty(root) == [0.0]                   # no length: left alone


@needs_clip
def test_a_bone_only_one_glb_has_holds_the_models_rest():
    tail = b"".join(s.payload for s in cooked.parse(corpus.read(_TAIL)).sections)
    # no Head in the library; a Tail the body lacks, not animated
    lib = _library([("Hips", (0.0, 1.2, 0.0)), ("Spine", (0.0, 0.6, 0.0)),
                    ("Tail", (0.0, 0.3, 0.0))])
    anim = _cook_for_body(lib, tail)
    assert [t.name for t in anim.tracks] == ["DEF.RootMotion", "Hips", "Spine", "Head"]
    assert _ty(anim.tracks[3]) == pytest.approx([0.4], abs=2e-3)
    assert _ty(anim.tracks[2]) == pytest.approx([0.5, 0.55], abs=2e-3)


@needs_clip
def test_a_clip_of_the_models_own_glb_is_unchanged_by_retargeting():
    tail = b"".join(s.payload for s in cooked.parse(corpus.read(_TAIL)).sections)
    bones = ["DEF.RootMotion", "Hips", "Spine", "Head"]
    plain = AC.cook_own(_body(), bones, tail, name="Turn")
    rest = AC.glb_rest_translations(_body())
    assert AC.retarget(plain, rest, rest) is plain


@needs_geo
def test_the_mesh_binds_to_the_skeletons_own_bones():
    from rsmm.engine import geometry_cook as G
    from rsmm.engine import prop_cook as PC
    tpl = corpus.read(_RIG_GEO)
    names = [b.name for b in SK.from_glb(_body(), _empty(), root="DEF.RootMotion").bones]
    geo = PC.cook_model(_body(), tpl, transform={"skin": "gltf", "submeshes": "map",
                                                 "palette": names})
    cf = cooked.parse(geo)
    t = next(i for i, s in enumerate(cf.sections) if G._find_records(s.payload))
    first, *rest = G._record_palettes(cf.sections[t].payload)
    assert first == ["Hips", "Spine", "Head"]
    # records with no geometry name only the root, never a bone of the rig's
    assert rest and all(set(p) == {"DEF.RootMotion"} for p in rest)


def test_a_bone_table_lists_the_bones_with_their_split():
    from rsmm.engine import hero_frame as HF
    raw = HF.bone_table(["R", "A", "B"], [0.0, 1.0, 1.0], "A", tail=(1, 0.05))
    assert raw[0] == 1 and raw[1:5] == struct.pack("<I", 1) and raw[5:6] == b"A"
    assert struct.unpack_from("<I3f", raw, 6) == (3, 0.0, 1.0, 1.0)
    assert raw.endswith(struct.pack("<I", 1) + b"B" + struct.pack("<Bf", 1, 0.05))


def _install():
    from rsmm.cli.apply_mods import find_game_dir
    try:
        return find_game_dir()
    except Exception:  # noqa: BLE001 — no install reachable
        return None


@needs_geo
@needs_clip
@pytest.mark.skipif(_install() is None, reason="no game install")
def test_a_scratch_hero_wears_a_skeleton_of_its_own(tmp_path):
    from rsmm.engine import hero_frame as HF
    from rsmm.engine.entity_graph_edit import EntityFile
    from rsmm.sdk.content import ContentDef, ContentError
    from rsmm.sdk.kinds import heros
    (tmp_path / "body.glb").write_bytes(_body())
    frame = {"rig": "Hero_Kintaro", "mesh": "Characters\\Heroes\\Kintaro\\Kintaro_GEO.fbx",
             "material": "Characters\\Heroes\\Kintaro\\Textures\\M_Kintaro.mat.ot",
             "model": "body.glb",
             "skeleton": {"fallback": "anim:Turn", "split": "Spine",
                          "sockets": {"DEF.Head": "Head", "DEF.SpineMiddle": "Spine"}},
             "animations": {"Turn": {"source": "body.glb", "clip": "Turn",
                                     "template": "Kintaro_FightingIdle_Loop"}}}
    files = heros.emit("m", ContentDef(kind="hero", id="Own", fields={"frame": frame}),
                       tmp_path / "assets")
    geo = next(p for p in files if p.name == "Hero_Own_GEO.fbx.Geometry.gen")
    bones = SK.decode(geo.read_bytes()).bones
    assert [b.name for b in bones] == ["DEF.RootMotion", "Hips", "Spine", "Head", "DEF.Head",
                                       "DEF.SpineMiddle", "DEF.HealthBarAttach"]
    clip = next(p for p in files if p.name == "Hero_Own_Turn.fbx.Animation.gen")
    anim = A.parse_payload(b"".join(s.payload for s in cooked.parse(clip.read_bytes()).sections))
    assert [t.name for t in anim.tracks] == [b.name for b in bones]
    # no clip of the rig's is copied or named: every slot plays its own
    assert [p.name for p in files if p.name.endswith(".Animation.gen")] == [clip.name]
    main = next(p for p in files if p.name == "Hero_Own.entity.ot.EntitySettingsResource.gen")
    raw = main.read_bytes()
    assert b"Kintaro_FightingIdle_Loop" not in raw and raw.count(b"Hero_Own_Turn.fbx") > 1
    ef = EntityFile(raw, "Hero_Own")
    p, a, b = HF._field_span(ef, "Move Split Layer", "bytes_7")
    assert bytes(p[a:b]) == HF.bone_table(
        [x.name for x in bones], [0, 0, 1, 1, 1, 1, 0], "Spine", tail=(1, 0.05))
    assert HF.layer_bones(ef, "Physic Layer") == len(bones)
    # a clip from another glb (same bone names, other lengths) is cooked at the
    # model's lengths; the model's own clip is exactly what cook_own makes
    (tmp_path / "lib.glb").write_bytes(_library([("Hips", (0.0, 1.2, 0.0)),
                                                 ("Spine", (0.0, 0.6, 0.0))]))
    files = heros.emit("m", ContentDef(kind="hero", id="Own", fields={"frame": {
        **frame, "animations": {**frame["animations"], "Stretch": {
            "source": "lib.glb", "clip": "Stretch",
            "template": "Kintaro_FightingIdle_Loop"}},
        "slots": {"Default Idle Anim": {"clip": "anim:Stretch"}}}}), tmp_path / "a1")

    def payload(name):
        p = next(p for p in files if p.name == f"Hero_Own_{name}.fbx.Animation.gen")
        return b"".join(s.payload for s in cooked.parse(p.read_bytes()).sections)
    tracks = {t.name: t for t in A.parse_payload(payload("Stretch")).tracks}
    assert _ty(tracks["Spine"]) == pytest.approx([0.5, 0.55], abs=2e-3)
    assert _ty(tracks["Hips"]) == pytest.approx([1.0], abs=2e-3)
    assert _ty(tracks["Head"]) == pytest.approx([0.4], abs=2e-3)
    tail = b"".join(s.payload for s in cooked.parse(corpus.read(_TAIL)).sections)
    assert payload("Turn") == AC.cook_own(_body(), [b.name for b in bones], tail, name="Turn")
    # a shipped clip cannot play on these bones
    with pytest.raises(ContentError, match="shipped clip"):
        heros.emit("m", ContentDef(kind="hero", id="Own", fields={"frame": {
            **frame, "slots": {"Default Idle Anim": {"clip": _SHIPPED}}}}),
            tmp_path / "a2")
    with pytest.raises(ContentError, match="fallback"):
        heros.emit("m", ContentDef(kind="hero", id="Own", fields={"frame": {
            **frame, "skeleton": {"split": "Spine"}}}), tmp_path / "a3")
