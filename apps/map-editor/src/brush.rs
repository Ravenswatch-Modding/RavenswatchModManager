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

/// One float op on a value in its layer's own units; `src` is the pre-stroke
/// grid for `smooth`.
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

/// Apply a height stroke to the preview heights (metres). Returns the rows
/// touched (to re-upload), or None if it missed.
pub fn apply_height(t: &mut Terrain, st: &Stroke) -> Option<(usize, usize)> {
    let n = t.n;
    let hits = cells(t, n, st);
    if hits.is_empty() {
        return None;
    }
    let target = match (st.op.as_str(), st.amount) {
        ("flatten", None) => t.height_at(st.x, st.z),
        (_, a) => a,
    };
    let src = if st.op == "smooth" { Some(t.h.clone()) } else { None };
    let (lo, hi) = (t.min.y, t.max.y);
    let (mut r0, mut r1) = (usize::MAX, 0);
    for (r, c, w) in hits {
        let i = r * n + c;
        let mean = || {
            let s = src.as_ref().unwrap();
            let (mut sum, mut k) = (0.0, 0.0);
            for rr in r.saturating_sub(1)..(r + 2).min(n) {
                for cc in c.saturating_sub(1)..(c + 2).min(n) {
                    sum += s[rr * n + cc];
                    k += 1.0;
                }
            }
            sum / k
        };
        t.h[i] = op_value(&st.op, t.h[i], target, w, mean).clamp(lo, hi);
        r0 = r0.min(r);
        r1 = r1.max(r);
    }
    Some((r0.saturating_sub(1), (r1 + 1).min(n - 1)))
}

/// Apply a 0..1 surface stroke to channel `ch` of an RGBA8 grid.
pub fn apply_channel(t: &Terrain, n: usize, grid: &mut [u8], ch: usize, st: &Stroke) -> bool {
    let hits = cells(t, n, st);
    if hits.is_empty() {
        return false;
    }
    let get = |g: &[u8], i: usize| g[i * 4 + ch] as f32 / 255.0;
    let target = match (st.op.as_str(), st.amount) {
        ("flatten", None) => {
            let u = ((st.x - t.min.x) / (t.max.x - t.min.x) * n as f32) as usize;
            let v = ((st.z - t.min.z) / (t.max.z - t.min.z) * n as f32) as usize;
            Some(get(grid, v.min(n - 1) * n + u.min(n - 1)))
        }
        (_, a) => a,
    };
    let src = if st.op == "smooth" { Some(grid.to_vec()) } else { None };
    for (r, c, w) in hits {
        let i = r * n + c;
        let mean = || {
            let s = src.as_ref().unwrap();
            let (mut sum, mut k) = (0.0, 0.0);
            for rr in r.saturating_sub(1)..(r + 2).min(n) {
                for cc in c.saturating_sub(1)..(c + 2).min(n) {
                    sum += get(s, rr * n + cc);
                    k += 1.0;
                }
            }
            sum / k
        };
        let v = op_value(&st.op, get(grid, i), target, w, mean).clamp(0.0, 1.0);
        grid[i * 4 + ch] = (v * 255.0).round() as u8;
    }
    true
}

/// Apply a tint stroke to an RGBA8 colour grid.
pub fn apply_tint(t: &Terrain, n: usize, grid: &mut [u8], st: &Stroke) -> bool {
    let Some(rgb) = st.color.as_deref().and_then(parse_hex) else { return false };
    let amount = st.amount.unwrap_or(0.5);
    let hits = cells(t, n, st);
    for &(r, c, w) in &hits {
        let i = (r * n + c) * 4;
        for k in 0..3 {
            let old = grid[i + k] as f32;
            grid[i + k] = (old + (rgb[k] as f32 - old) * w * amount).round().clamp(0.0, 255.0) as u8;
        }
    }
    !hits.is_empty()
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
    /// Paint material slot (1..=4 = R..A) up / down.
    Paint(usize),
    Erase(usize),
    Tint,
}

#[derive(Clone, Debug)]
pub struct Brush {
    pub mode: Mode,
    pub radius: f32,
    /// Metres per dab for height, 0..1 per dab for paint / tint.
    pub strength: f32,
    pub level: f32,
    pub falloff: &'static str,
    pub color: [f32; 3],
}

impl Default for Brush {
    fn default() -> Self {
        Brush { mode: Mode::Raise, radius: 8.0, strength: 0.25, level: 0.0, falloff: "smooth", color: [0.55, 0.45, 0.35] }
    }
}

impl Brush {
    /// The stroke for one dab at `at`; `invert` swaps raise/lower, paint/erase.
    /// `sources` maps a material slot to its painted layer.
    pub fn stroke(&self, at: Vec3, invert: bool, sources: &[Option<String>], color_layer: Option<&str>) -> Option<Stroke> {
        let (op, layer, amount, color) = match (self.mode, invert) {
            (Mode::Raise, false) | (Mode::Lower, true) => ("raise", HEIGHT.to_owned(), Some(self.strength), None),
            (Mode::Lower, false) | (Mode::Raise, true) => ("lower", HEIGHT.to_owned(), Some(self.strength), None),
            (Mode::Flatten, _) => ("flatten", HEIGHT.to_owned(), None, None),
            (Mode::Smooth, _) => ("smooth", HEIGHT.to_owned(), None, None),
            (Mode::Level, _) => ("flatten", HEIGHT.to_owned(), Some(self.level), None),
            (Mode::Paint(k), inv) | (Mode::Erase(k), inv) => {
                let up = matches!(self.mode, Mode::Paint(_)) != inv;
                let layer = sources.get(k.checked_sub(1)?)?.clone()?;
                (if up { "raise" } else { "lower" }, layer, Some(self.strength.clamp(0.01, 1.0)), None)
            }
            (Mode::Tint, _) => ("tint", color_layer?.to_owned(), Some(self.strength.clamp(0.01, 1.0)), Some(hex(self.color))),
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
