//! Triangle BVH for ray picking: one per mesh, built the first time the mesh
//! is picked. Same scheme as Godot's TriangleMesh (core/math/triangle_mesh.cpp:
//! split each node's triangles at the median along its box's longest axis).
//!
//! Godot Engine, MIT License. Copyright (c) 2014-present Godot Engine
//! contributors; Copyright (c) 2007-2014 Juan Linietsky, Ariel Manzur.

use crate::assets::Vertex;
use glam::Vec3;

const LEAF: usize = 4;

struct Node {
    lo: Vec3,
    hi: Vec3,
    /// Leaf: first slot in `order`. Inner: index of the right child (the left
    /// child is the next node).
    at: u32,
    /// Triangles in the leaf; 0 for an inner node.
    count: u32,
}

pub struct Bvh {
    nodes: Vec<Node>,
    /// Triangle numbers (index into `indices / 3`), grouped by leaf.
    order: Vec<u32>,
}

fn tri(v: &[Vertex], idx: &[u32], t: u32) -> Option<[Vec3; 3]> {
    let i = t as usize * 3;
    let p = |k: usize| v.get(idx[i + k] as usize).map(|v| Vec3::from(v.pos));
    Some([p(0)?, p(1)?, p(2)?])
}

impl Bvh {
    pub fn build(v: &[Vertex], idx: &[u32]) -> Bvh {
        // Triangles with a bad index can never be hit; leave them out.
        let mut items: Vec<(u32, Vec3, Vec3)> = (0..(idx.len() / 3) as u32)
            .filter_map(|t| {
                let [a, b, c] = tri(v, idx, t)?;
                Some((t, a.min(b).min(c), a.max(b).max(c)))
            })
            .collect();
        let mut bvh = Bvh { nodes: Vec::with_capacity(items.len() / LEAF * 2 + 1), order: Vec::with_capacity(items.len()) };
        if !items.is_empty() {
            bvh.split(&mut items);
        }
        bvh
    }

    fn split(&mut self, items: &mut [(u32, Vec3, Vec3)]) {
        let (lo, hi) = items.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), it| (l.min(it.1), h.max(it.2)));
        let me = self.nodes.len();
        if items.len() <= LEAF {
            self.nodes.push(Node { lo, hi, at: self.order.len() as u32, count: items.len() as u32 });
            self.order.extend(items.iter().map(|it| it.0));
            return;
        }
        self.nodes.push(Node { lo, hi, at: 0, count: 0 });
        let ext = hi - lo;
        let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
        let mid = items.len() / 2;
        items.select_nth_unstable_by(mid, |a, b| (a.1[axis] + a.2[axis]).total_cmp(&(b.1[axis] + b.2[axis])));
        let (l, r) = items.split_at_mut(mid);
        self.split(l);
        self.nodes[me].at = self.nodes.len() as u32;
        self.split(r);
    }

    /// Nearest hit of a ray with the mesh, in ray units.
    pub fn ray(&self, o: Vec3, d: Vec3, v: &[Vertex], idx: &[u32]) -> Option<f32> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = d.recip();
        let mut best = f32::INFINITY;
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if !box_hit(o, inv, node.lo, node.hi, best) {
                continue;
            }
            if node.count == 0 {
                stack.push(node.at as usize);
                stack.push(n + 1);
                continue;
            }
            for &t in &self.order[node.at as usize..(node.at + node.count) as usize] {
                if let Some(h) = tri(v, idx, t).and_then(|p| ray_tri(o, d, p)) {
                    best = best.min(h);
                }
            }
        }
        best.is_finite().then_some(best)
    }
}

/// Slab test: does the ray enter the box before `max_t`?
fn box_hit(o: Vec3, inv: Vec3, lo: Vec3, hi: Vec3, max_t: f32) -> bool {
    let (a, b) = ((lo - o) * inv, (hi - o) * inv);
    // f32::min/max skip a NaN (0 * inf on a slab edge), which keeps that axis open.
    let near = a.min(b).max_element().max(0.0);
    let far = a.max(b).min_element().min(max_t);
    near <= far
}

/// Möller–Trumbore: ray parameter of the hit, if it is in front of the origin.
pub fn ray_tri(o: Vec3, d: Vec3, [a, b, c]: [Vec3; 3]) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let w = d.dot(q) * inv;
    if w < 0.0 || u + w > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_brute_force() {
        // A bumpy 40x40 grid plus an index past the end, which must be skipped.
        let n = 40u32;
        let mut v = Vec::new();
        for z in 0..=n {
            for x in 0..=n {
                let y = ((x * 7 + z * 13) % 5) as f32 * 0.3;
                v.push(Vertex { pos: [x as f32, y, z as f32], nrm: [0.0; 3], uv: [0.0; 2] });
            }
        }
        let mut idx = Vec::new();
        for z in 0..n {
            for x in 0..n {
                let i = z * (n + 1) + x;
                idx.extend([i, i + 1, i + n + 1, i + 1, i + n + 2, i + n + 1]);
            }
        }
        idx.extend([0, 1, 99_999]);
        let bvh = Bvh::build(&v, &idx);
        let brute = |o: Vec3, d: Vec3| {
            (0..(idx.len() / 3) as u32).filter_map(|t| tri(&v, &idx, t).and_then(|p| ray_tri(o, d, p))).min_by(f32::total_cmp)
        };
        let mut hits = 0;
        for k in 0..500 {
            let f = k as f32;
            let o = Vec3::new((f * 0.37) % 45.0 - 2.0, 10.0, (f * 0.61) % 45.0 - 2.0);
            // Straight down too: zero x/z direction exercises the NaN slab path.
            let d = if k % 5 == 0 { Vec3::NEG_Y } else { Vec3::new((f * 0.13).sin(), -1.0, (f * 0.29).cos()).normalize() };
            let (a, b) = (bvh.ray(o, d, &v, &idx), brute(o, d));
            assert_eq!(a.is_some(), b.is_some(), "ray {k}");
            if let (Some(a), Some(b)) = (a, b) {
                assert!((a - b).abs() < 1e-4, "ray {k}: {a} vs {b}");
                hits += 1;
            }
        }
        assert!(hits > 300);
    }
}
