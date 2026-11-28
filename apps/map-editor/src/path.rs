//! The Path tool's curve: a Catmull-Rom spline through the clicked points,
//! and copies spaced evenly along it (Godot's Path3D + "place along").
//!
//! The spline is Bevy's CubicCardinalSpline at tension 1/2 (bevy_curve,
//! src/cubic_splines/mod.rs: its characteristic matrix and the mirrored end
//! points). Bevy Engine, MIT License, Copyright (c) 2020 Carter Anderson.

use glam::Vec3;

const S: f32 = 0.5;
/// Samples per segment for walking the arc length.
const STEPS: usize = 32;

/// Point at `t` (0..1) of the segment p1..p2, with p0 / p3 its neighbours.
fn segment(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let c1 = -S * p0 + S * p2;
    let c2 = 2.0 * S * p0 + (S - 3.0) * p1 + (3.0 - 2.0 * S) * p2 - S * p3;
    let c3 = -S * p0 + (2.0 - S) * p1 + (S - 2.0) * p2 + S * p3;
    p1 + t * (c1 + t * (c2 + t * c3))
}

/// The curve through `pts` as a polyline (STEPS points per segment).
pub fn polyline(pts: &[Vec3]) -> Vec<Vec3> {
    match pts.len() {
        0 => return Vec::new(),
        1 => return pts.to_vec(),
        _ => {}
    }
    // Mirror the ends (P_-1 = 2 P_0 - P_1), as Bevy does, so the curve starts
    // and ends on the first and last points.
    let n = pts.len();
    let mut ext = Vec::with_capacity(n + 2);
    ext.push(pts[0] * 2.0 - pts[1]);
    ext.extend_from_slice(pts);
    ext.push(pts[n - 1] * 2.0 - pts[n - 2]);
    let mut out = Vec::with_capacity((n - 1) * STEPS + 1);
    for w in ext.windows(4) {
        for k in 0..STEPS {
            out.push(segment(w[0], w[1], w[2], w[3], k as f32 / STEPS as f32));
        }
    }
    out.push(pts[n - 1]);
    out
}

/// Positions every `spacing` metres along the curve (measured on the ground,
/// first one at the start) and the flat direction of travel there.
pub fn along(pts: &[Vec3], spacing: f32) -> Vec<(Vec3, Vec3)> {
    let line = polyline(pts);
    let spacing = spacing.max(0.1);
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
    let mut out = Vec::new();
    let mut next = 0.0;
    let mut walked = 0.0;
    for w in line.windows(2) {
        let len = flat(w[1] - w[0]).length();
        if len < 1e-6 {
            continue;
        }
        let dir = flat(w[1] - w[0]) / len;
        while next <= walked + len {
            let t = (next - walked) / len;
            out.push((w[0].lerp(w[1], t), dir));
            next += spacing;
        }
        walked += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_through_its_points_and_spaces_evenly() {
        let pts = [Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 10.0)];
        let line = polyline(&pts);
        for p in pts {
            assert!(line.iter().any(|q| (*q - p).length() < 1e-4), "{p} on the curve");
        }
        // A straight path: copies exactly `spacing` apart, facing along it.
        let got = along(&[Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)], 2.5);
        assert_eq!(got.len(), 5);
        for (k, (p, d)) in got.iter().enumerate() {
            assert!((p.x - 2.5 * k as f32).abs() < 1e-3 && p.z.abs() < 1e-4, "{p}");
            assert!((*d - Vec3::X).length() < 1e-4);
        }
    }
}
