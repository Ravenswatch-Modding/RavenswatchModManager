//! Meshes and textures, fetched through the backend and decoded off the UI
//! thread. What comes back is CPU-side data ready for one GPU upload.
//!
//! The decode mirrors `src/rsmm/cli/editor/pages/models.js` so both editors
//! draw the same thing: every node that has a mesh is merged *without* its node
//! transform, a node named `AABB` is skipped, and a texture flagged `swap_rb`
//! has its red and blue channels swapped back.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{bail, Context as _, Result};
use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};

use crate::backend::{q, Backend};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub nrm: [f32; 3],
    pub uv: [f32; 2],
}

pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub center: Vec3,
    pub radius: f32,
}

impl MeshData {
    pub fn bounds(vertices: &[Vertex]) -> (Vec3, f32) {
        if vertices.is_empty() {
            return (Vec3::ZERO, 0.0);
        }
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for v in vertices {
            let p = Vec3::from(v.pos);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let c = (lo + hi) * 0.5;
        let r = vertices.iter().map(|v| (Vec3::from(v.pos) - c).length()).fold(0.0, f32::max);
        (c, r)
    }

    /// The unit decal quad: a 1x1 square in XZ facing +Y.
    pub fn decal_quad() -> MeshData {
        let v = |x: f32, z: f32, u: f32, w: f32| Vertex { pos: [x, 0.0, z], nrm: [0.0, 1.0, 0.0], uv: [u, w] };
        let vertices = vec![v(-0.5, -0.5, 0.0, 1.0), v(0.5, -0.5, 1.0, 1.0), v(0.5, 0.5, 1.0, 0.0), v(-0.5, 0.5, 0.0, 0.0)];
        let (center, radius) = MeshData::bounds(&vertices);
        MeshData { vertices, indices: vec![0, 1, 2, 0, 2, 3], center, radius }
    }
}

pub struct TextureData {
    /// RGBA8 sRGB levels, largest first.
    pub levels: Vec<(u32, u32, Vec<u8>)>,
    pub has_alpha: bool,
}

pub enum Loaded {
    Mesh(String, Result<MeshData>),
    Texture(String, bool, Result<TextureData>),
}

pub enum Job {
    Mesh(String),
    Texture(String, bool),
}

pub struct Loader {
    jobs: Sender<Job>,
    pub done: Receiver<Loaded>,
}

impl Loader {
    pub fn new(backend: Backend, workers: usize, wake: impl Fn() + Send + Sync + 'static) -> Loader {
        let (jtx, jrx) = channel::<Job>();
        let (dtx, drx) = channel::<Loaded>();
        let jrx = Arc::new(Mutex::new(jrx));
        let wake = Arc::new(wake);
        for _ in 0..workers {
            let (jrx, dtx, be, wake) = (jrx.clone(), dtx.clone(), backend.clone(), wake.clone());
            thread::spawn(move || loop {
                let job = match jrx.lock().unwrap().recv() {
                    Ok(j) => j,
                    Err(_) => return,
                };
                let out = match job {
                    Job::Mesh(path) => {
                        // `file.glb#i` = only sub-mesh i (one material slot).
                        let (file, sub) = split_submesh(&path);
                        let r = fetch(&be, file).and_then(|b| decode_glb_part(&b, sub));
                        Loaded::Mesh(path, r)
                    }
                    Job::Texture(path, swap) => {
                        let r = fetch(&be, &path).and_then(|b| decode_png(&b, swap));
                        Loaded::Texture(path, swap, r)
                    }
                };
                if dtx.send(out).is_err() {
                    return;
                }
                wake();
            });
        }
        Loader { jobs: jtx, done: drx }
    }

    pub fn request(&self, job: Job) {
        let _ = self.jobs.send(job);
    }
}

fn fetch(be: &Backend, rel: &str) -> Result<Vec<u8>> {
    // The mod's own models and textures are read from disk (`models.rs`).
    if let Some(path) = rel.strip_prefix("file:") {
        return std::fs::read(path).with_context(|| path.to_owned());
    }
    let r = be.call("GET", &format!("/map/api/file?path={}", q(rel)), None)?;
    if !r.ok() {
        bail!("{rel}: {}", r.error_text());
    }
    Ok(r.body)
}

/// `"3D/x.fbx.glb#2"` -> `("3D/x.fbx.glb", Some(2))`.
pub fn split_submesh(path: &str) -> (&str, Option<usize>) {
    match path.rsplit_once('#') {
        Some((f, k)) => match k.parse() {
            Ok(k) => (f, Some(k)),
            Err(_) => (path, None),
        },
        None => (path, None),
    }
}

/// The glb's meshes merged, or only node `submesh_<k>` when `only` is set.
pub fn decode_glb_part(bytes: &[u8], only: Option<usize>) -> Result<MeshData> {
    let want = only.map(|k| format!("submesh_{k}"));
    let g = gltf::Gltf::from_slice(bytes).context("not a glb")?;
    let blob = g.blob.as_deref();
    // Each node's world matrix, walked down from the scene roots (as `geometry_cook._glb_parse`).
    let mut world = vec![None; g.nodes().len()];
    let roots: Vec<gltf::Node> = match g.default_scene().or_else(|| g.scenes().next()) {
        Some(sc) => sc.nodes().collect(),
        None => {
            let kids: std::collections::HashSet<usize> = g.nodes().flat_map(|n| n.children().map(|c| c.index())).collect();
            g.nodes().filter(|n| !kids.contains(&n.index())).collect()
        }
    };
    let mut stack: Vec<(gltf::Node, Mat4)> = roots.into_iter().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((n, parent)) = stack.pop() {
        if world[n.index()].is_some() {
            continue;
        }
        let m = parent * Mat4::from_cols_array_2d(&n.transform().matrix());
        world[n.index()] = Some(m);
        stack.extend(n.children().map(|c| (c, m)));
    }
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for node in g.nodes() {
        // glTF ignores a skinned mesh's node transform.
        let m = if node.skin().is_some() { Mat4::IDENTITY } else { world[node.index()].unwrap_or(Mat4::IDENTITY) };
        let nm = Mat3::from_mat4(m).inverse().transpose();
        let Some(mesh) = node.mesh() else { continue };
        if node.name() == Some("AABB") || want.as_deref().is_some_and(|w| node.name() != Some(w)) {
            continue;
        }
        for prim in mesh.primitives() {
            let reader = prim.reader(|b| match b.source() {
                gltf::buffer::Source::Bin => blob,
                gltf::buffer::Source::Uri(_) => None,
            });
            let Some(pos) = reader.read_positions() else { continue };
            let pos: Vec<[f32; 3]> = pos.collect();
            let base = vertices.len() as u32;
            let nrm: Vec<[f32; 3]> = reader.read_normals().map(|n| n.collect()).unwrap_or_default();
            let uv: Vec<[f32; 2]> = reader.read_tex_coords(0).map(|t| t.into_f32().collect()).unwrap_or_default();
            for (i, p) in pos.iter().enumerate() {
                let n = nrm.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
                vertices.push(Vertex {
                    pos: m.transform_point3(Vec3::from(*p)).into(),
                    nrm: (nm * Vec3::from(n)).normalize_or(Vec3::Y).into(),
                    uv: uv.get(i).copied().unwrap_or([0.0, 0.0]),
                });
            }
            match reader.read_indices() {
                Some(ix) => indices.extend(ix.into_u32().map(|i| i + base)),
                None => indices.extend(base..base + pos.len() as u32),
            }
        }
    }
    if vertices.is_empty() {
        bail!("glb has no drawable mesh");
    }
    let (center, radius) = MeshData::bounds(&vertices);
    Ok(MeshData { vertices, indices, center, radius })
}

pub fn decode_png(bytes: &[u8], swap_rb: bool) -> Result<TextureData> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).context("not a png")?;
    let has_alpha = img.color().has_alpha();
    let mut rgba = img.to_rgba8();
    if swap_rb {
        for px in rgba.pixels_mut() {
            px.0.swap(0, 2);
        }
    }
    let mut levels = Vec::new();
    let (mut w, mut h) = rgba.dimensions();
    levels.push((w, h, rgba.as_raw().clone()));
    let mut cur = rgba;
    while w > 1 || h > 1 {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        cur = image::imageops::resize(&cur, w, h, image::imageops::FilterType::Triangle);
        levels.push((w, h, cur.as_raw().clone()));
    }
    Ok(TextureData { levels, has_alpha })
}

#[cfg(test)]
mod tests {
    use super::split_submesh;

    #[test]
    fn submesh_suffix() {
        assert_eq!(split_submesh("3D/a.fbx.glb#2"), ("3D/a.fbx.glb", Some(2)));
        assert_eq!(split_submesh("3D/a.fbx.glb"), ("3D/a.fbx.glb", None));
        assert_eq!(split_submesh("3D/a#b.glb"), ("3D/a#b.glb", None));
    }
}
