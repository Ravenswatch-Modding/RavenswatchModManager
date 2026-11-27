//! What the author changed, as data: the same document the browser editor
//! saves (`sceneryDoc()` in map.html), so a mod made in one opens in the other.
//!
//! Scenery edits are `{level: {moves, removes, adds}}`. Moves and removes are
//! keyed by the object's id in its level and anchored to its shipped entity and
//! position, so `scenery_edit.apply` refuses an edit the game has since moved.

use std::collections::{BTreeMap, BTreeSet};

use glam::{Quat, Vec3};
use serde_json::{json, Map, Value};

use crate::recipe::{KindVals, Spot, SpotNow};
use crate::scene::{Scenery, Xf};

#[derive(Clone, Debug, PartialEq)]
pub struct Add {
    /// Stable id: selection, hiding and undo refer to an added object by it,
    /// so deleting another add does not shift what is selected.
    pub uid: u64,
    pub entity: String,
    pub level: String,
    pub pos: Vec3,
    /// Degrees about +Y, as `scenery_edit._yaw_quat` reads it.
    pub yaw: f32,
    pub scale: f32,
    /// The author's own name for it (Rename); empty = the entity's name.
    pub name: String,
    /// One of the mod's own models: this copy's collision, when it differs
    /// from the model's default (Inspector > Collision).
    pub col: Option<Col>,
}

/// A wall box around a placed model, in the model's own space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Col {
    pub solid: bool,
    pub size: Vec3,
    pub at: Vec3,
}

impl Add {
    pub fn xf(&self) -> Xf {
        Xf { pos: self.pos, rot: Quat::from_rotation_y(self.yaw.to_radians()), scale: Vec3::splat(self.scale) }
    }

    pub fn name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        if let Some(rel) = self.entity.strip_prefix(crate::models::PREFIX) {
            return crate::models::stem(rel).to_owned();
        }
        let e = self.entity.rsplit('\\').next().unwrap_or(&self.entity);
        e.trim_end_matches(".entity.ot").to_owned()
    }
}

/// One selectable thing: a shipped scenery object (by index) or an added one
/// (by its stable uid).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Sel {
    Obj(usize),
    Add(u64),
    /// A tile spot of the generation recipe: a shipped id, or a new one (>= NEW_SPOT).
    Spot(i64),
}

/// New spots get ids from here up, so they never clash with shipped ones.
pub const NEW_SPOT: i64 = 1_000_000;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NewSpot {
    pub id: i64,
    pub group: String,
    pub pos: [f32; 3],
    pub allow: BTreeMap<String, bool>,
}

/// One terrain brush dab, exactly the map kind's stroke
/// (`terrain.apply_strokes`): `{op, x, z, radius, amount, falloff, layer, color}`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stroke {
    pub op: String,
    pub x: f32,
    pub z: f32,
    pub radius: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<f32>,
    pub falloff: String,
    pub layer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edits {
    pub moves: BTreeMap<usize, Xf>,
    pub removed: BTreeSet<usize>,
    pub adds: Vec<Add>,
    pub next_uid: u64,
    // the generation recipe
    pub spot_moves: BTreeMap<i64, Vec3>,
    pub spot_removed: BTreeSet<i64>,
    pub spot_adds: Vec<NewSpot>,
    pub spot_allow: BTreeMap<i64, BTreeMap<String, bool>>,
    pub kinds: BTreeMap<String, KindVals>,
    // the ground
    pub strokes: Vec<Stroke>,
}

impl Edits {
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    pub fn scenery_count(&self) -> usize {
        self.moves.len() + self.removed.len() + self.adds.len()
    }

    pub fn recipe_count(&self) -> usize {
        self.spot_moves.len() + self.spot_removed.len() + self.spot_adds.len() + self.spot_allow.len() + self.kinds.len()
    }

    /// A shipped spot or a new one, as it stands now.
    pub fn spot(&self, s: &Scenery, id: i64) -> Option<SpotNow> {
        if id >= NEW_SPOT {
            let n = self.spot_adds.iter().find(|n| n.id == id)?;
            return Some(SpotNow {
                id,
                pos: Vec3::from(n.pos),
                group: n.group.clone(),
                whole_map: false,
                allow: n.allow.clone(),
                overrides: Vec::new(),
                new: true,
            });
        }
        if self.spot_removed.contains(&id) {
            return None;
        }
        let o: &Spot = s.spots.iter().find(|x| x.id == id)?;
        let mut allow = o.allow.clone();
        if let Some(ch) = self.spot_allow.get(&id) {
            allow.extend(ch.iter().map(|(k, v)| (k.clone(), *v)));
        }
        Some(SpotNow {
            id,
            pos: self.spot_moves.get(&id).copied().unwrap_or(o.pos),
            group: o.group.clone(),
            whole_map: o.whole_map,
            allow,
            overrides: o.overrides.clone(),
            new: false,
        })
    }

    pub fn spots(&self, s: &Scenery) -> Vec<SpotNow> {
        s.spots
            .iter()
            .map(|x| x.id)
            .chain(self.spot_adds.iter().map(|n| n.id))
            .filter_map(|id| self.spot(s, id))
            .collect()
    }

    pub fn add_spot(&mut self, group: String, pos: Vec3, allow: BTreeMap<String, bool>) -> Sel {
        let id = NEW_SPOT + self.spot_adds.iter().map(|n| n.id - NEW_SPOT + 1).max().unwrap_or(0);
        self.spot_adds.push(NewSpot { id, group, pos: pos.into(), allow });
        Sel::Spot(id)
    }

    /// Allow / forbid a kind on a spot.
    pub fn set_allow(&mut self, s: &Scenery, id: i64, kind: &str, on: bool) {
        if id >= NEW_SPOT {
            if let Some(n) = self.spot_adds.iter_mut().find(|n| n.id == id) {
                n.allow.insert(kind.to_owned(), on);
            }
            return;
        }
        let shipped = s.spots.iter().find(|x| x.id == id).and_then(|x| x.allow.get(kind)).copied().unwrap_or(false);
        let ch = self.spot_allow.entry(id).or_default();
        if shipped == on {
            ch.remove(kind);
        } else {
            ch.insert(kind.to_owned(), on);
        }
        if ch.is_empty() {
            self.spot_allow.remove(&id);
        }
    }

    pub fn add(&self, uid: u64) -> Option<&Add> {
        self.adds.iter().find(|a| a.uid == uid)
    }

    /// Append an added object; returns its selection handle.
    pub fn push_add(&mut self, mut a: Add) -> Sel {
        self.next_uid += 1;
        a.uid = self.next_uid;
        self.adds.push(a);
        Sel::Add(self.next_uid)
    }

    /// Does this selection still exist (not removed, not a deleted add)?
    pub fn exists(&self, s: &Scenery, sel: Sel) -> bool {
        match sel {
            Sel::Obj(i) => i < s.obj_xf.len() && !self.removed.contains(&i),
            Sel::Add(u) => self.add(u).is_some(),
            Sel::Spot(id) => self.spot(s, id).is_some(),
        }
    }

    pub fn count(&self) -> usize {
        self.scenery_count() + self.recipe_count() + usize::from(!self.strokes.is_empty())
    }

    /// Where a selection stands now.
    pub fn xf(&self, s: &Scenery, sel: Sel) -> Option<Xf> {
        match sel {
            Sel::Obj(i) => Some(self.moves.get(&i).copied().unwrap_or(*s.obj_xf.get(i)?)),
            Sel::Add(u) => self.add(u).map(Add::xf),
            Sel::Spot(id) => self.spot(s, id).map(|p| Xf { pos: p.pos, rot: Quat::IDENTITY, scale: Vec3::ONE }),
        }
    }

    pub fn set_xf(&mut self, s: &Scenery, sel: Sel, xf: Xf) {
        match sel {
            Sel::Obj(i) => {
                if s.obj_xf.get(i) == Some(&xf) {
                    self.moves.remove(&i);
                } else {
                    self.moves.insert(i, xf);
                }
            }
            Sel::Add(u) => {
                if let Some(a) = self.adds.iter_mut().find(|a| a.uid == u) {
                    a.pos = xf.pos;
                    // Adds carry yaw only: keep the rotation about Y.
                    let (y, _, _) = xf.rot.to_euler(glam::EulerRot::YXZ);
                    a.yaw = y.to_degrees();
                    a.scale = xf.scale.x.clamp(0.05, 20.0);
                }
            }
            // A spot is a position only: rotation and scale do not apply.
            Sel::Spot(id) if id >= NEW_SPOT => {
                if let Some(n) = self.spot_adds.iter_mut().find(|n| n.id == id) {
                    n.pos = xf.pos.into();
                }
            }
            Sel::Spot(id) => {
                let shipped = s.spots.iter().find(|x| x.id == id).map(|x| x.pos);
                if shipped.is_some_and(|p| (p - xf.pos).length() < 1e-4) {
                    self.spot_moves.remove(&id);
                } else if shipped.is_some() {
                    self.spot_moves.insert(id, xf.pos);
                }
            }
        }
    }

    pub fn delete(&mut self, sel: Sel) {
        match sel {
            Sel::Obj(i) => {
                self.moves.remove(&i);
                self.removed.insert(i);
            }
            Sel::Add(u) => self.adds.retain(|a| a.uid != u),
            Sel::Spot(id) if id >= NEW_SPOT => self.spot_adds.retain(|n| n.id != id),
            Sel::Spot(id) => {
                self.spot_moves.remove(&id);
                self.spot_allow.remove(&id);
                self.spot_removed.insert(id);
            }
        }
    }

    /// The recipe part of a save body's `edits`, in `map_editor.apply_edits`
    /// form: kinds, slots (move / allow, anchored by the shipped position),
    /// remove_slots, add_slots. `kinds` are the shipped kind values.
    pub fn recipe_doc(&self, s: &Scenery, kinds: &[(String, KindVals)]) -> Map<String, Value> {
        let mut out = Map::new();
        let v3 = |v: Vec3| json!([v.x, v.y, v.z]);
        let mut kd = Map::new();
        for (name, orig) in kinds {
            let Some(now) = self.kinds.get(name) else { continue };
            let mut d = Map::new();
            if now.count != orig.count {
                d.insert("count".into(), json!(now.count));
            }
            if (now.min_distance - orig.min_distance).abs() > 1e-4 {
                d.insert("min_distance".into(), json!(now.min_distance));
            }
            let fp: Map<String, Value> = now
                .footprints
                .iter()
                .filter(|(g, on)| orig.footprints.get(*g) != Some(*on))
                .map(|(g, on)| (g.clone(), json!(on)))
                .collect();
            if !fp.is_empty() {
                d.insert("footprints".into(), Value::Object(fp));
            }
            if !d.is_empty() {
                kd.insert(name.clone(), Value::Object(d));
            }
        }
        if !kd.is_empty() {
            out.insert("kinds".into(), Value::Object(kd));
        }
        let mut slots = Map::new();
        let ids: BTreeSet<i64> = self.spot_moves.keys().chain(self.spot_allow.keys()).copied().collect();
        for id in ids {
            let Some(o) = s.spots.iter().find(|x| x.id == id) else { continue };
            let mut d = Map::new();
            d.insert("pos".into(), v3(o.pos));
            if let Some(p) = self.spot_moves.get(&id) {
                d.insert("move".into(), v3(*p));
            }
            if let Some(a) = self.spot_allow.get(&id) {
                d.insert("allow".into(), json!(a));
            }
            slots.insert(id.to_string(), Value::Object(d));
        }
        if !slots.is_empty() {
            out.insert("slots".into(), Value::Object(slots));
        }
        let rem: Map<String, Value> = self
            .spot_removed
            .iter()
            .filter_map(|id| s.spots.iter().find(|x| x.id == *id).map(|o| (id.to_string(), v3(o.pos))))
            .collect();
        if !rem.is_empty() {
            out.insert("remove_slots".into(), Value::Object(rem));
        }
        let adds: Vec<Value> = self
            .spot_adds
            .iter()
            .map(|n| {
                let allow: BTreeMap<&String, bool> = n.allow.iter().filter(|(_, on)| **on).map(|(k, v)| (k, *v)).collect();
                let mut d = json!({"group": n.group, "pos": n.pos});
                if !allow.is_empty() {
                    d["allow"] = json!(allow);
                }
                d
            })
            .collect();
        if !adds.is_empty() {
            out.insert("add_slots".into(), Value::Array(adds));
        }
        out
    }

    /// Read a saved `edits` document's recipe part back; returns the keys it
    /// did not handle (quotas, fill, …) so the caller keeps them unchanged.
    pub fn load_recipe(&mut self, doc: &Value, s: &Scenery, kinds: &[(String, KindVals)]) -> Map<String, Value> {
        let mut rest = Map::new();
        let Some(o) = doc.as_object() else { return rest };
        let v3 = |v: &Value| -> Option<Vec3> {
            let a = v.as_array()?;
            Some(Vec3::new(a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32))
        };
        for (k, v) in o {
            match k.as_str() {
                "kinds" => {
                    for (name, ke) in v.as_object().into_iter().flatten() {
                        let Some((_, orig)) = kinds.iter().find(|(n, _)| n == name) else { continue };
                        let mut now = orig.clone();
                        if let Some(c) = ke["count"].as_i64() {
                            now.count = c;
                        }
                        if let Some(d) = ke["min_distance"].as_f64() {
                            now.min_distance = d as f32;
                        }
                        for (g, on) in ke["footprints"].as_object().into_iter().flatten() {
                            now.footprints.insert(g.clone(), on.as_bool().unwrap_or(false));
                        }
                        self.kinds.insert(name.clone(), now);
                    }
                }
                "slots" => {
                    for (id, se) in v.as_object().into_iter().flatten() {
                        let Ok(id) = id.parse::<i64>() else { continue };
                        if let Some(p) = v3(&se["move"]) {
                            self.spot_moves.insert(id, p);
                        }
                        for (kn, on) in se["allow"].as_object().into_iter().flatten() {
                            self.set_allow(s, id, kn, on.as_bool().unwrap_or(false));
                        }
                    }
                }
                "remove_slots" => {
                    for id in v.as_object().into_iter().flatten().filter_map(|(id, _)| id.parse::<i64>().ok()) {
                        self.spot_removed.insert(id);
                    }
                }
                "add_slots" => {
                    for a in v.as_array().into_iter().flatten() {
                        let (Some(g), Some(p)) = (a["group"].as_str(), v3(&a["pos"])) else { continue };
                        let allow = a["allow"].as_object().map(|m| m.iter().map(|(k, b)| (k.clone(), b.as_bool().unwrap_or(false))).collect()).unwrap_or_default();
                        self.add_spot(g.to_owned(), p, allow);
                    }
                }
                _ => {
                    rest.insert(k.clone(), v.clone());
                }
            }
        }
        rest
    }

    /// The `scenery` member of a check/save body.
    /// Added objects as the `map` kind's `objects` (a map clone stands them in
    /// a scenery level of its own, so they carry no shipped level).
    pub fn objects_doc(&self) -> Value {
        Value::Array(
            self.adds
                .iter()
                .map(|a| {
                    let mut o = json!({"entity": a.entity, "pos": [a.pos.x, a.pos.y, a.pos.z], "yaw": a.yaw, "scale": a.scale});
                    if !a.name.is_empty() {
                        o["name"] = json!(a.name);
                    }
                    o
                })
                .collect(),
        )
    }

    /// Read `map.objects` back as adds.
    pub fn load_objects(&mut self, doc: &Value) {
        for o in doc.as_array().into_iter().flatten() {
            let entity = match (o["entity"].as_str(), o["model"].as_str()) {
                (Some(e), _) => e.to_owned(),
                (None, Some(m)) => format!("{}{m}", crate::models::PREFIX),
                _ => continue,
            };
            let Some(p) = o["pos"].as_array() else { continue };
            let f = |i: usize| p.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
            self.push_add(Add {
                uid: 0,
                entity,
                level: String::new(),
                pos: Vec3::new(f(0), f(1), f(2)),
                yaw: o["yaw"].as_f64().unwrap_or(0.0) as f32,
                scale: o["scale"].as_f64().unwrap_or(1.0) as f32,
                name: o["name"].as_str().unwrap_or_default().to_owned(),
                col: o["model"].as_str().map(|_| {
                    let b = &o["collision"][0];
                    let v = |k: &str, d: f32| {
                        let a = b[k].as_array();
                        let f = |i: usize| a.and_then(|a| a.get(i)).and_then(Value::as_f64).map_or(d, |x| x as f32);
                        Vec3::new(f(0), f(1), f(2))
                    };
                    Col { solid: b.is_object(), size: v("size", 1.0), at: v("at", 0.0) }
                }),
            });
        }
    }

    /// `with_adds = false` leaves added objects out (a map clone saves them as
    /// `map.objects` instead, see `objects_doc`).
    pub fn scenery_doc(&self, s: &Scenery, with_adds: bool) -> Value {
        #[derive(Default)]
        struct Level {
            moves: Map<String, Value>,
            removes: Map<String, Value>,
            adds: Vec<Value>,
        }
        let mut out: BTreeMap<String, Level> = BTreeMap::new();
        let r = |v: Vec3| json!([v.x, v.y, v.z]);
        let level = |i: usize| s.levels.get(s.obj_level[i] as usize).cloned().unwrap_or_default();
        let entity = |i: usize| s.entities.get(s.obj_entity[i] as usize).cloned().unwrap_or_default();
        for (&i, xf) in &self.moves {
            out.entry(level(i)).or_default().moves.insert(
                s.obj_id[i].to_string(),
                json!({
                    "entity": entity(i),
                    "at": r(s.obj_xf[i].pos),
                    "pos": r(xf.pos),
                    "quat": [xf.rot.x, xf.rot.y, xf.rot.z, xf.rot.w],
                    "scale": r(xf.scale),
                }),
            );
        }
        for &i in &self.removed {
            out.entry(level(i))
                .or_default()
                .removes
                .insert(s.obj_id[i].to_string(), json!({"entity": entity(i), "at": r(s.obj_xf[i].pos)}));
        }
        for a in self.adds.iter().filter(|_| with_adds) {
            out.entry(a.level.clone())
                .or_default()
                .adds
                .push(json!({"entity": a.entity, "pos": r(a.pos), "yaw": a.yaw, "scale": a.scale}));
        }
        let mut doc = Map::new();
        for (lv, e) in out {
            let mut m = Map::new();
            if !e.moves.is_empty() {
                m.insert("moves".into(), Value::Object(e.moves));
            }
            if !e.removes.is_empty() {
                m.insert("removes".into(), Value::Object(e.removes));
            }
            if !e.adds.is_empty() {
                m.insert("adds".into(), Value::Array(e.adds));
            }
            doc.insert(lv, Value::Object(m));
        }
        Value::Object(doc)
    }

    /// Read a saved `scenery` document back. Entries that no longer match the
    /// chapter are reported, not silently dropped.
    pub fn from_doc(doc: &Value, s: &Scenery) -> (Edits, Vec<String>) {
        let mut e = Edits::default();
        let mut skipped = Vec::new();
        let Some(levels) = doc.as_object() else { return (e, skipped) };
        let v3 = |v: &Value| -> Option<Vec3> {
            let a = v.as_array()?;
            Some(Vec3::new(a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32))
        };
        for (level, ed) in levels {
            let li = s.levels.iter().position(|l| l == level);
            let find = |id: &str| -> Option<usize> {
                let li = li? as u16;
                let id: u32 = id.parse().ok()?;
                (0..s.obj_id.len()).find(|&i| s.obj_level[i] == li && s.obj_id[i] == id)
            };
            for (id, m) in ed.get("moves").and_then(Value::as_object).into_iter().flatten() {
                let Some(i) = find(id) else {
                    skipped.push(format!("move {id} in {level}: no such object"));
                    continue;
                };
                let shipped = s.obj_xf[i];
                let rot = m["quat"]
                    .as_array()
                    .and_then(|q| {
                        let f = |k: usize| q.get(k).and_then(Value::as_f64).map(|x| x as f32);
                        Some(Quat::from_xyzw(f(0)?, f(1)?, f(2)?, f(3)?).normalize())
                    })
                    .or_else(|| m["yaw"].as_f64().map(|y| Quat::from_rotation_y((y as f32).to_radians())))
                    .unwrap_or(shipped.rot);
                let scale = v3(&m["scale"]).or_else(|| m["scale"].as_f64().map(|k| Vec3::splat(k as f32))).unwrap_or(shipped.scale);
                e.moves.insert(i, Xf { pos: v3(&m["pos"]).unwrap_or(shipped.pos), rot, scale });
            }
            for id in ed.get("removes").and_then(Value::as_object).into_iter().flatten().map(|(k, _)| k) {
                match find(id) {
                    Some(i) => {
                        e.removed.insert(i);
                    }
                    None => skipped.push(format!("remove {id} in {level}: no such object")),
                }
            }
            for a in ed.get("adds").and_then(Value::as_array).into_iter().flatten() {
                let (Some(entity), Some(pos)) = (a["entity"].as_str(), v3(&a["pos"])) else { continue };
                e.push_add(Add {
                    uid: 0,
                    entity: entity.to_owned(),
                    level: level.clone(),
                    pos,
                    yaw: a["yaw"].as_f64().unwrap_or(0.0) as f32,
                    scale: a["scale"].as_f64().unwrap_or(1.0) as f32,
                    name: String::new(),
                    col: None,
                });
            }
        }
        (e, skipped)
    }
}

/// Snapshot undo: an edit set is small (a few thousand entries at most).
#[derive(Default)]
pub struct History {
    undo: Vec<Edits>,
    redo: Vec<Edits>,
}

impl History {
    pub fn push(&mut self, before: &Edits) {
        if self.undo.last() != Some(before) {
            self.undo.push(before.clone());
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, cur: &mut Edits) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(cur, prev));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, cur: &mut Edits) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(cur, next));
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Every snapshot, so a rename outside the history (a model moved to
    /// another folder) holds after undo / redo too.
    pub fn for_each_mut(&mut self, mut f: impl FnMut(&mut Edits)) {
        self.undo.iter_mut().chain(self.redo.iter_mut()).for_each(&mut f);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
