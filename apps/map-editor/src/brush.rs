//! Terrain brushes: the preview side of the map kind's strokes.
//!
//! A port of `src/rsmm/engine/terrain.py::apply_strokes`, run on the editor's
//! preview grids so what you paint is what `rsmm apply` will write. The saved
//! mod carries the STROKES (`map.terrain`), never a grid; the backend replays
//! them onto the full-resolution vanilla layers.
//!
//! * height (`Base Height`, metres): raise / lower / set / flatten / smooth;
//! * a surface layer (0..1, e.g. `Base Grass`): the same ops, shown on the
//!   weight-map channels made from that layer;
//! * the vertex-colour layer: `tint` toward a colour.

use glam::Vec3;

use crate::edit::Stroke;
use crate::scene::Terrain;

pub const HEIGHT: &str = "Base Height";
/// The painted water surface (metres), as `terrain.METRE_LAYERS` names it.
pub const WATER: &str = "Base Water Height";

/// Weight at normalised distance `t` (0 centre .. 1 edge).
fn falloff(t: f32, kind: &str) -> f32 {
    match kind {
        "constant" => 1.0,
        "linear" => 1.0 - t,
        _ => 1.0 - t * t * (3.0 - 2.0 * t),
    }
}

/// The cells a stroke covers on an `n` x `n` grid over the terrain box:
/// `(index, weight)`, the same cell maths as the Python.
fn cells(t: &Terrain, n: usize, st: &Stroke) -> Vec<(usize, usize, f32)> {
    let cell_m = (t.max.x - t.min.x) / n as f32;
    let cu = (st.x - t.min.x) / (t.max.x - t.min.x);
    let cv = (st.z - t.min.z) / (t.max.z - t.min.z);
    let (cc, cr) = (cu * n as f32 - 0.5, cv * n as f32 - 0.5);
    let rc = st.radius / cell_m;
    let mut out = Vec::new();
    let r0 = (cr - rc).max(0.0) as usize;
    let r1 = ((cr + rc) as isize + 2).clamp(0, n as isize) as usize;
    let c0 = (cc - rc).max(0.0) as usize;
    let c1 = ((cc + rc) as isize + 2).clamp(0, n as isize) as usize;
    for r in r0..r1 {
        for c in c0..c1 {
            let d = ((c as f32 - cc).powi(2) + (r as f32 - cr).powi(2)).sqrt() / rc;
            if d < 1.0 {
                out.push((r, c, falloff(d, &st.falloff)));
            }
        }
    }
    out
}

/// One float op on a value in its layer's own units; `mean` is the 3x3 mean
/// of the pre-stroke grid for `smooth`.
fn op_value(op: &str, v: f32, target: Option<f32>, w: f32, mean: impl Fn() -> f32) -> f32 {
    match (op, target) {
        ("raise", Some(a)) => v + a * w,
        ("lower", Some(a)) => v - a * w,
        ("set", Some(a)) => {
            if w > 0.0 {
                a
            } else {
                v
            }
        }
        ("flatten", Some(a)) => v + (a - v) * w,
        ("smooth", _) => v + (mean() - v) * w,
        _ => v,
    }
}

/// 3x3 mean around (r, c) of an `n` x `n` grid read through `get`.
fn mean3(n: usize, r: usize, c: usize, get: impl Fn(usize) -> f32) -> f32 {
    let (mut sum, mut k) = (0.0, 0.0);
    for rr in r.saturating_sub(1)..(r + 2).min(n) {
        for cc in c.saturating_sub(1)..(c + 2).min(n) {
            sum += get(rr * n + cc);
            k += 1.0;
        }
    }
    sum / k
}

/// Smallest height / water change (metres) a live dab must make to be kept:
/// scrubbing a converged flatten / smooth / set adds no dabs.
pub const MIN_METRES: f32 = 0.005;

/// Apply a height stroke to the preview heights (metres). Returns the rows
/// touched (to re-upload), or None if it missed or no cell moved by `min`
/// metres (then nothing is written). Replays pass `min = 0`.
pub fn apply_height(t: &mut Terrain, st: &Stroke, min: f32) -> Option<(usize, usize)> {
    let n = t.n;
    let target = match (st.op.as_str(), st.amount) {
        // off the map: the edge's height, as the map kind's nearest cell
        ("flatten", None) => Some(t.height_clamped(st.x, st.z)),
        (_, a) => a,
    };
    let (lo, hi) = (t.min.y, t.max.y);
    // Two passes: compute against the untouched grid (which is also what
    // `smooth` reads), write only if the dab does something.
    let h = &t.h;
    let new: Vec<(usize, f32)> = cells(t, n, st)
        .into_iter()
        .map(|(r, c, w)| {
            let i = r * n + c;
            (i, op_value(&st.op, h[i], target, w, || mean3(n, r, c, |j| h[j])).clamp(lo, hi))
        })
        .collect();
    let moved = new.iter().fold(0.0f32, |m, &(i, v)| m.max((v - h[i]).abs()));
    if new.is_empty() || moved < min {
        return None;
    }
    let (mut r0, mut r1) = (usize::MAX, 0);
    for (i, v) in new {
        t.h[i] = v;
        r0 = r0.min(i / n);
        r1 = r1.max(i / n);
    }
    Some((r0.saturating_sub(1), (r1 + 1).min(n - 1)))
}

/// Apply a water stroke to the terrain's water surface (`t.water`, made dry
/// first if the map had none). Returns the rows touched, or None if no cell
/// moved by `min` metres (then nothing is written).
pub fn apply_water(t: &mut Terrain, st: &Stroke, min: f32) -> Option<(usize, usize)> {
    let n = t.n;
    let lo = t.min.y - 50.0;
    let target = st.amount.map(|a| a.max(lo));
    let cur = |i: usize| t.water.as_ref().map_or(lo, |w| w[i]);
    let new: Vec<(usize, f32)> = cells(t, n, st)
        .into_iter()
        .map(|(r, c, k)| {
            let i = r * n + c;
            (i, op_value(&st.op, cur(i), target, k, || cur(i)))
        })
        .collect();
    let moved = new.iter().fold(0.0f32, |m, &(i, v)| m.max((v - cur(i)).abs()));
    if new.is_empty() || moved < min {
        return None;
    }
    let w = t.water.get_or_insert_with(|| vec![lo; n * n]);
    let (mut r0, mut r1) = (usize::MAX, 0);
    for (i, v) in new {
        w[i] = v;
        r0 = r0.min(i / n);
        r1 = r1.max(i / n);
    }
    Some((r0, r1))
}

/// Apply a 0..1 surface stroke to channel `ch` of an RGBA8 grid. Returns
/// whether any cell changed.
pub fn apply_channel(t: &Terrain, n: usize, grid: &mut [u8], ch: usize, st: &Stroke) -> bool {
    let get = |g: &[u8], i: usize| g[i * 4 + ch] as f32 / 255.0;
    let target = match (st.op.as_str(), st.amount) {
        ("flatten", None) => {
            let u = ((st.x - t.min.x) / (t.max.x - t.min.x) * n as f32) as usize;
            let v = ((st.z - t.min.z) / (t.max.z - t.min.z) * n as f32) as usize;
            Some(get(grid, v.min(n - 1) * n + u.min(n - 1)))
        }
        (_, a) => a,
    };
    // Two passes so `smooth` reads the pre-stroke grid without copying it.
    let g = &*grid;
    let new: Vec<(usize, u8)> = cells(t, n, st)
        .into_iter()
        .map(|(r, c, w)| {
            let i = r * n + c;
            let v = op_value(&st.op, get(g, i), target, w, || mean3(n, r, c, |j| get(g, j))).clamp(0.0, 1.0);
            (i * 4 + ch, (v * 255.0).round() as u8)
        })
        .collect();
    let mut changed = false;
    for (i, v) in new {
        changed |= grid[i] != v;
        grid[i] = v;
    }
    changed
}

/// Apply a tint stroke to an RGBA8 colour grid.
pub fn apply_tint(t: &Terrain, n: usize, grid: &mut [u8], st: &Stroke) -> bool {
    let Some(rgb) = st.color.as_deref().and_then(parse_hex) else { return false };
    let amount = st.amount.unwrap_or(0.5);
    let mut changed = false;
    for (r, c, w) in cells(t, n, st) {
        let i = (r * n + c) * 4;
        for k in 0..3 {
            let old = grid[i + k];
            let v = (old as f32 + (rgb[k] as f32 - old as f32) * w * amount).round().clamp(0.0, 255.0) as u8;
            changed |= v != old;
            grid[i + k] = v;
        }
    }
    changed
}

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.strip_prefix('#')?;
    (s.len() == 6).then_some(())?;
    let h = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([h(0)?, h(2)?, h(4)?])
}

pub fn hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// What the brush does, as the UI offers it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Raise,
    Lower,
    Flatten,
    Smooth,
    /// Set an exact height (metres).
    Level,
    /// Lift (Ctrl: drop) the ground one terrace step, the way the shipped
    /// chapters are built: flat plateaus on multiples of `step` metres
    /// (Dark Hills -10/-8/-4/0/4/8/12, 86-94% of cells flat). The target is
    /// picked once per drag (`level`), so one drag paints one flat layer.
    Layer,
    /// Paint material slot (1..=4 = R..A) up / down.
    Paint(usize),
    Erase(usize),
    Tint,
    /// Paint blocked ground (Ctrl: clear it): invisible walls on save.
    Block,
    /// Water at `level` metres (Ctrl: dry the ground under the brush).
    Water,
}

impl Mode {
    /// Back from `format!("{mode:?}")` (the saved prefs).
    pub fn parse(s: &str) -> Option<Mode> {
        let slot = |p: &str| s.strip_prefix(p)?.strip_suffix(')')?.parse().ok();
        Some(match s {
            "Raise" => Mode::Raise,
            "Lower" => Mode::Lower,
            "Flatten" => Mode::Flatten,
            "Smooth" => Mode::Smooth,
            "Level" => Mode::Level,
            "Layer" => Mode::Layer,
            "Tint" => Mode::Tint,
            "Block" => Mode::Block,
            "Water" => Mode::Water,
            _ => slot("Paint(").map(Mode::Paint).or_else(|| slot("Erase(").map(Mode::Erase))?,
        })
    }
}

/// The falloff names a brush offers, as `&'static str`.
pub fn falloff_name(s: &str) -> &'static str {
    match s {
        "linear" => "linear",
        "constant" => "constant",
        _ => "smooth",
    }
}

#[derive(Clone, Debug)]
pub struct Brush {
    pub mode: Mode,
    pub radius: f32,
    /// Metres per dab for height, 0..1 per dab for paint / tint.
    pub strength: f32,
    pub level: f32,
    /// Terrace height for `Mode::Layer`, metres.
    pub step: f32,
    pub falloff: &'static str,
    pub color: [f32; 3],
    /// A brush shape (built in, or a picture); `None` is the round brush.
    pub stamp: Option<Stamp>,
}

/// Cells across a stamp: each dab becomes up to STAMP_N² small dabs.
/// ponytail: a stamp is rasterised into ordinary strokes so the map kind and
/// the game need nothing new; a finer grid costs dabs (20000 per save).
pub const STAMP_N: usize = 5;

/// A brush shape: STAMP_N x STAMP_N weights 0..1, row-major, +z down the rows.
#[derive(Clone, Debug, PartialEq)]
pub struct Stamp {
    pub name: String,
    pub mask: Vec<f32>,
}

impl Stamp {
    /// The shapes offered without a picture.
    pub fn builtin() -> Vec<Stamp> {
        let n = STAMP_N as f32;
        let at = |f: &dyn Fn(f32, f32) -> f32| -> Vec<f32> {
            (0..STAMP_N * STAMP_N)
                .map(|i| {
                    // -1..1 at cell centres
                    let x = ((i % STAMP_N) as f32 + 0.5) / n * 2.0 - 1.0;
                    let z = ((i / STAMP_N) as f32 + 0.5) / n * 2.0 - 1.0;
                    f(x, z).clamp(0.0, 1.0)
                })
                .collect()
        };
        vec![
            Stamp { name: "Square".into(), mask: at(&|_, _| 1.0) },
            Stamp { name: "Ring".into(), mask: at(&|x, z| if ((x * x + z * z).sqrt() - 0.7).abs() < 0.3 { 1.0 } else { 0.0 }) },
            Stamp { name: "Cross".into(), mask: at(&|x, z| if x.abs() < 0.25 || z.abs() < 0.25 { 1.0 } else { 0.0 }) },
            Stamp {
                name: "Speckle".into(),
                mask: at(&|x, z| {
                    let h = ((x * 12.9898 + z * 78.233).sin() * 43758.547).fract().abs();
                    if h > 0.55 { 1.0 } else { 0.0 }
                }),
            },
        ]
    }

    /// A picture as a brush: brightness times opacity, sampled per cell.
    pub fn from_png(name: &str, bytes: &[u8]) -> anyhow::Result<Stamp> {
        let img = image::load_from_memory(bytes)?.to_rgba8();
        let (w, h) = img.dimensions();
        let small = image::imageops::resize(&img, STAMP_N as u32, STAMP_N as u32, image::imageops::FilterType::Triangle);
        anyhow::ensure!(w > 0 && h > 0, "empty picture");
        let mask = small.pixels().map(|p| (p.0[0] as f32 + p.0[1] as f32 + p.0[2] as f32) / (3.0 * 255.0) * p.0[3] as f32 / 255.0).collect();
        Ok(Stamp { name: name.to_owned(), mask })
    }
}

impl Default for Brush {
    fn default() -> Self {
        Brush { mode: Mode::Raise, radius: 8.0, strength: 0.25, level: 0.0, step: 4.0, falloff: "smooth", color: [0.55, 0.45, 0.35], stamp: None }
    }
}

/// Record a live dab. A raise / lower folds into an earlier identical one at
/// the same spot when only raises / lowers touched that layer since (they
/// commute), so scrubbing one place never grows the list.
/// ponytail: exact except where the folded run hit the layer's clamp.
pub fn record(strokes: &mut Vec<Stroke>, st: Stroke) {
    if matches!(st.op.as_str(), "raise" | "lower") {
        for s in strokes.iter_mut().rev().filter(|s| s.layer == st.layer) {
            if !matches!(s.op.as_str(), "raise" | "lower") {
                break;
            }
            if (&s.op, s.x, s.z, s.radius, &s.falloff) == (&st.op, st.x, st.z, st.radius, &st.falloff) {
                if let (Some(a), Some(b)) = (s.amount.as_mut(), st.amount) {
                    *a += b;
                    return;
                }
            }
        }
    }
    strokes.push(st);
}

impl Brush {
    /// Distance between dabs along a drag; dab centres snap to this grid so
    /// repeat dabs land on the same spot and fold (see [`record`]).
    pub fn spacing(&self) -> f32 {
        (self.radius * 0.3).max(0.25)
    }

    pub fn snap(&self, at: Vec3) -> Vec3 {
        let s = self.spacing();
        Vec3::new((at.x / s).round() * s, at.y, (at.z / s).round() * s)
    }

    /// The terrace a `Layer` drag starting on ground at height `h` paints:
    /// the next step above (`down`: below). Ground already on a step moves a
    /// whole step, not onto itself.
    pub fn layer_target(&self, h: f32, down: bool) -> f32 {
        let step = self.step.max(0.5);
        let k = h / step;
        let eps = 0.05;
        if down { ((k - eps).ceil() - 1.0) * step } else { ((k + eps).floor() + 1.0) * step }
    }

    /// The stroke for one dab at `at`; `invert` swaps raise/lower, paint/erase.
    /// `sources` maps a material slot to its painted layer.
    pub fn stroke(&self, at: Vec3, invert: bool, sources: &[Option<String>], color_layer: Option<&str>) -> Option<Stroke> {
        let (op, layer, amount, color) = match (self.mode, invert) {
            (Mode::Raise, false) | (Mode::Lower, true) => ("raise", HEIGHT.to_owned(), Some(self.strength), None),
            (Mode::Lower, false) | (Mode::Raise, true) => ("lower", HEIGHT.to_owned(), Some(self.strength), None),
            (Mode::Flatten, _) => ("flatten", HEIGHT.to_owned(), None, None),
            (Mode::Smooth, _) => ("smooth", HEIGHT.to_owned(), None, None),
            (Mode::Level, _) => ("flatten", HEIGHT.to_owned(), Some(self.level), None),
            (Mode::Layer, _) => ("set", HEIGHT.to_owned(), Some(self.level), None),
            (Mode::Paint(k), inv) | (Mode::Erase(k), inv) => {
                let up = matches!(self.mode, Mode::Paint(_)) != inv;
                let layer = sources.get(k.checked_sub(1)?)?.clone()?;
                (if up { "raise" } else { "lower" }, layer, Some(self.strength.clamp(0.01, 1.0)), None)
            }
            (Mode::Tint, _) => ("tint", color_layer?.to_owned(), Some(self.strength.clamp(0.01, 1.0)), Some(hex(self.color))),
            (Mode::Block, _) => return None,
            (Mode::Water, false) => ("set", WATER.to_owned(), Some(self.level), None),
            // Dry: the water surface under the lowest ground there can be.
            (Mode::Water, true) => ("set", WATER.to_owned(), Some(-1000.0), None),
        };
        Some(Stroke {
            op: op.to_owned(),
            x: (at.x * 100.0).round() / 100.0,
            z: (at.z * 100.0).round() / 100.0,
            radius: self.radius,
            amount,
            falloff: self.falloff.to_owned(),
            layer,
            color,
        })
    }
}

impl Brush {
    /// One dab as strokes: the round brush is one; a stamp is one small dab
    /// per cell it covers, its weight scaling the amount (a target height or
    /// a smooth is all or nothing per cell).
    pub fn strokes(&self, at: Vec3, invert: bool, sources: &[Option<String>], color_layer: Option<&str>) -> Vec<Stroke> {
        let Some(one) = self.stroke(at, invert, sources, color_layer) else { return Vec::new() };
        let Some(st) = &self.stamp else { return vec![one] };
        let cell = 2.0 * self.radius / STAMP_N as f32;
        let scaled = matches!(one.op.as_str(), "raise" | "lower" | "tint");
        let mut out = Vec::new();
        for (i, w) in st.mask.iter().enumerate() {
            if *w < if scaled { 0.05 } else { 0.5 } {
                continue;
            }
            let dx = ((i % STAMP_N) as f32 + 0.5) * cell - self.radius;
            let dz = ((i / STAMP_N) as f32 + 0.5) * cell - self.radius;
            let mut s = one.clone();
            s.x = ((at.x + dx) * 100.0).round() / 100.0;
            s.z = ((at.z + dz) * 100.0).round() / 100.0;
            s.radius = (cell * 0.75).max(0.5);
            if scaled {
                s.amount = s.amount.map(|a| (a * w * 1000.0).round() / 1000.0);
            }
            out.push(s);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_becomes_one_dab_per_covered_cell() {
        let mut b = Brush::default();
        assert_eq!(b.strokes(Vec3::ZERO, false, &[], None).len(), 1);
        b.stamp = Stamp::builtin().into_iter().find(|s| s.name == "Square");
        let dabs = b.strokes(Vec3::ZERO, false, &[], None);
        assert_eq!(dabs.len(), STAMP_N * STAMP_N);
        assert!(dabs.iter().all(|d| d.x.abs() < b.radius && d.z.abs() < b.radius));
        let mut half = b.stamp.clone().unwrap();
        half.mask.iter_mut().for_each(|w| *w = 0.5);
        b.stamp = Some(half);
        assert!(b.strokes(Vec3::ZERO, false, &[], None).iter().all(|d| (d.amount.unwrap() - b.strength * 0.5).abs() < 1e-3));
    }

    #[test]
    fn scrubbing_one_spot_does_not_grow_the_dabs() {
        let b = Brush::default();
        let mut strokes = Vec::new();
        let at = b.snap(Vec3::new(1.03, 0.0, 2.01));
        for _ in 0..50 {
            for st in b.strokes(b.snap(at + Vec3::new(0.1, 0.0, 0.0)), false, &[], None) {
                record(&mut strokes, st);
            }
        }
        assert_eq!(strokes.len(), 1);
        assert!((strokes[0].amount.unwrap() - 50.0 * b.strength).abs() < 1e-3);
        // a lower is a different dab; a set on the layer ends the fold
        record(&mut strokes, b.stroke(at, true, &[], None).unwrap());
        let mut set = strokes[0].clone();
        set.op = "set".into();
        record(&mut strokes, set);
        record(&mut strokes, b.stroke(at, false, &[], None).unwrap());
        assert_eq!(strokes.len(), 4);
    }

    #[test]
    fn modes_round_trip_through_prefs() {
        for m in [Mode::Raise, Mode::Layer, Mode::Paint(3), Mode::Erase(1), Mode::Water] {
            assert_eq!(Mode::parse(&format!("{m:?}")), Some(m));
        }
        assert_eq!(Mode::parse("nonsense"), None);
    }

    #[test]
    fn layer_target_steps_one_terrace() {
        let b = Brush::default(); // 4 m steps
        assert_eq!(b.layer_target(0.0, false), 4.0);
        assert_eq!(b.layer_target(0.0, true), -4.0);
        assert_eq!(b.layer_target(2.5, false), 4.0);
        assert_eq!(b.layer_target(2.5, true), 0.0);
        assert_eq!(b.layer_target(7.99, false), 12.0);
        assert_eq!(b.layer_target(-8.0, true), -12.0);
    }
}
