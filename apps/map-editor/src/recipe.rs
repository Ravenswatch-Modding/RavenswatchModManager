//! The chapter's map-generation recipe: tile spots, the kinds of tile each
//! spot may take, and the tile (POI) pool. From `/map/api/recipe` and
//! `/map/api/tiles`; the same data and rules the browser editor uses
//! (`eligible()` / `sampleLayout()` in map.html).

use std::collections::{BTreeMap, BTreeSet};

use glam::Vec3;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct KindVals {
    pub count: i64,
    pub min_distance: f32,
    pub footprints: BTreeMap<String, bool>,
}

#[derive(Clone, Debug)]
pub struct Kind {
    pub name: String,
    pub vals: KindVals,
    pub required: Vec<String>,
    pub excluded: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Spot {
    pub id: i64,
    pub pos: Vec3,
    pub group: String,
    pub whole_map: bool,
    pub allow: BTreeMap<String, bool>,
    /// Per scenario: kind -> 0 (never) / 1 (always), else the allow table.
    pub overrides: Vec<BTreeMap<String, i64>>,
}

#[derive(Clone, Debug)]
pub struct Quota {
    pub flags: Vec<String>,
    pub limit: i64,
}

#[derive(Clone, Debug)]
pub struct PoolTile {
    pub path: String,
    pub name: String,
    pub flags: Vec<String>,
    pub width: i64,
    pub height: i64,
    pub pick_weight: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Recipe {
    pub kinds: Vec<Kind>,
    pub groups: Vec<String>,
    pub quotas: Vec<Quota>,
    pub spots: Vec<Spot>,
    pub scenarios: Vec<String>,
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

fn bools(v: &Value) -> BTreeMap<String, bool> {
    v.as_object().map(|o| o.iter().map(|(k, b)| (k.clone(), b.as_bool().unwrap_or(false))).collect()).unwrap_or_default()
}

impl Recipe {
    pub fn from_json(v: &Value) -> Recipe {
        let r = &v["recipe"];
        let kinds = r["kinds"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|k| Kind {
                name: k["name"].as_str().unwrap_or_default().to_owned(),
                vals: KindVals {
                    count: k["count"].as_i64().unwrap_or(0),
                    min_distance: k["min_distance"].as_f64().unwrap_or(0.0) as f32,
                    footprints: bools(&k["footprints"]),
                },
                required: strs(&k["required"]),
                excluded: strs(&k["excluded"]),
            })
            .collect();
        let groups = r["groups"].as_array().into_iter().flatten().filter_map(|g| g["name"].as_str().map(str::to_owned)).collect();
        let quotas = r["quotas"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|q| Quota { flags: strs(&q["flags"]), limit: q["limit"].as_i64().unwrap_or(0) })
            .collect();
        let spots = r["slots"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                let p = s["pos"].as_array();
                let f = |i: usize| p.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                Spot {
                    id: s["id"].as_i64().unwrap_or(-1),
                    pos: Vec3::new(f(0), f(1), f(2)),
                    group: s["group"].as_str().unwrap_or_default().to_owned(),
                    whole_map: s["whole_map"].as_bool().unwrap_or(false),
                    allow: bools(&s["allow"]),
                    overrides: s["overrides"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|o| o.as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(-1))).collect()).unwrap_or_default())
                        .collect(),
                }
            })
            .collect();
        let scenarios = r["scenarios"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| s["label"].as_str().or(s["id"].as_str()).unwrap_or_default().to_owned())
            .collect();
        Recipe { kinds, groups, quotas, spots, scenarios }
    }
}

pub fn pool_from_json(v: &Value) -> Vec<PoolTile> {
    v["tiles"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| PoolTile {
            path: t["path"].as_str().unwrap_or_default().to_owned(),
            name: t["name"].as_str().unwrap_or_default().to_owned(),
            flags: strs(&t["flags"]),
            width: t["width"].as_i64().unwrap_or(0),
            height: t["height"].as_i64().unwrap_or(0),
            pick_weight: t["pick_weight"].as_f64().unwrap_or(1.0) as f32,
        })
        .collect()
}

/// A footprint group's edge in metres: "40x40" -> 40.
pub fn group_size(group: &str) -> f32 {
    group.split('x').next().and_then(|n| n.parse().ok()).unwrap_or(6.0)
}

/// Spot colour per footprint size.
pub fn group_color(group: &str) -> [f32; 4] {
    match group_size(group) as i64 {
        0..=4 => [0.62, 0.62, 0.66, 0.9],
        5..=12 => [0.35, 0.78, 1.0, 0.95],
        13..=48 => [1.0, 0.82, 0.3, 0.95],
        _ => [0.95, 0.4, 0.95, 0.95],
    }
}

/// A spot as it stands now (after the author's edits).
#[derive(Clone, Debug)]
pub struct SpotNow {
    pub id: i64,
    pub pos: Vec3,
    pub group: String,
    pub whole_map: bool,
    pub allow: BTreeMap<String, bool>,
    pub overrides: Vec<BTreeMap<String, i64>>,
    pub new: bool,
}

/// Can `kind` go on `spot` in scenario `scen`? "yes", "blocked" (allowed but
/// the kind does not fit this footprint), or "no".
pub fn eligible(spot: &SpotNow, kind: &Kind, vals: &KindVals, scen: usize) -> &'static str {
    if spot.whole_map {
        return "no";
    }
    let allowed = match spot.overrides.get(scen).and_then(|o| o.get(&kind.name)) {
        Some(1) => true,
        Some(0) => false,
        _ => spot.allow.get(&kind.name).copied().unwrap_or(false),
    };
    if !allowed {
        return "no";
    }
    if vals.footprints.get(&spot.group).copied().unwrap_or(false) { "yes" } else { "blocked" }
}

/// A rough stand-in for the generator, for looking at, not balancing: the
/// browser editor's `sampleLayout`, same rules, same order.
pub fn sample(spots: &[SpotNow], kinds: &[(Kind, KindVals)], quotas: &[Quota], pool: &[PoolTile], scen: usize, seed: u32) -> BTreeMap<i64, String> {
    let mut a = seed;
    let mut rnd = || {
        a = a.wrapping_add(0x6D2B_79F5);
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = (t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t))) ^ t;
        ((t ^ (t >> 14)) as f64) / 4_294_967_296.0
    };
    let mut used: BTreeSet<i64> = BTreeSet::new();
    let mut placed: Vec<(String, Vec3)> = Vec::new();
    let mut quota = vec![0i64; quotas.len()];
    let mut out = BTreeMap::new();
    for (k, v) in kinds {
        let mut cand: Vec<&SpotNow> = spots.iter().filter(|s| eligible(s, k, v, scen) == "yes" && !used.contains(&s.id)).collect();
        for i in (1..cand.len()).rev() {
            let j = (rnd() * (i + 1) as f64) as usize;
            cand.swap(i, j.min(i));
        }
        let mut n = 0;
        for s in cand {
            if n >= v.count {
                break;
            }
            if placed.iter().any(|(kn, p)| kn == &k.name && Vec3::new(p.x - s.pos.x, 0.0, p.z - s.pos.z).length() < v.min_distance) {
                continue;
            }
            let fits: Vec<&PoolTile> = pool
                .iter()
                .filter(|t| format!("{}x{}", t.width, t.height) == s.group)
                .filter(|t| k.required.iter().all(|f| t.flags.contains(f)) && !k.excluded.iter().any(|f| t.flags.contains(f)))
                .filter(|t| quotas.iter().enumerate().all(|(qi, q)| !q.flags.iter().all(|f| t.flags.contains(f)) || quota[qi] < q.limit))
                .collect();
            if fits.is_empty() {
                continue;
            }
            let w = |t: &PoolTile| if t.pick_weight > 0.0 { t.pick_weight as f64 } else { 1.0 };
            let total: f64 = fits.iter().map(|t| w(t)).sum();
            let mut r = rnd() * total;
            let mut tile = fits[fits.len() - 1];
            for t in &fits {
                r -= w(t);
                if r <= 0.0 {
                    tile = t;
                    break;
                }
            }
            for (qi, q) in quotas.iter().enumerate() {
                if q.flags.iter().all(|f| tile.flags.contains(f)) {
                    quota[qi] += 1;
                }
            }
            used.insert(s.id);
            placed.push((k.name.clone(), s.pos));
            out.insert(s.id, tile.path.clone());
            n += 1;
        }
    }
    out
}
