//! The mod's own models and textures (Project > Your models).
//!
//! An imported `.glb` / `.png` is read from disk, checked, given a place in
//! the mod (`art/<name>`), and uploaded once to the backend's staging area
//! (`/map/api/upload`); a save copies it into `mods/<id>/art/`. The editor
//! draws it straight from the file on disk (mesh key `file:<path>`), and a
//! placed copy is an added object whose entity is `model:art/<name>.glb`,
//! which the save turns into the `map` kind's `{model, texture, collision}`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use base64::Engine as _;
use glam::Vec3;
use serde_json::{json, Value};

use crate::backend::Backend;
use crate::scene::PartKey;

/// The prefix that marks an added object as one of the mod's own models.
pub const PREFIX: &str = "model:";

#[derive(Clone, Debug)]
pub struct Art {
    /// Where it lives in the mod: `art/<name>.glb|png`.
    pub rel: String,
    /// The file the editor reads it from.
    pub file: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Model {
    pub mesh: Art,
    pub texture: Option<Art>,
    /// A wall box around the model's bounds, so heroes cannot walk through it.
    pub solid: bool,
    /// Bounds in the model's own space (metres, Y up).
    pub lo: Vec3,
    pub hi: Vec3,
}

impl Model {
    pub fn entity(&self) -> String {
        format!("{PREFIX}{}", self.mesh.rel)
    }

    pub fn name(&self) -> &str {
        stem(&self.mesh.rel)
    }

    /// How the editor draws it: the mesh and texture straight from disk.
    pub fn part(&self) -> PartKey {
        PartKey {
            mesh: format!("file:{}", self.mesh.file.display()),
            texture: self.texture.as_ref().map(|t| format!("file:{}", t.file.display())),
            swap_rb: false,
            color: self.texture.is_none().then(|| "#b8b2a8".to_owned()),
            decal: false,
            vfx: false,
            normal: None,
            normal_swap: false,
        }
    }

    /// The `map` kind's fields for one placed copy (beside pos / yaw / scale).
    pub fn fields(&self) -> Value {
        let mut o = json!({"model": self.mesh.rel});
        if let Some(t) = &self.texture {
            o["texture"] = json!(t.rel);
        }
        if self.solid {
            o["collision"] = crate::edit::ColBox::list_json(&self.default_cols());
        }
        o
    }

    /// A placed copy's boxes unless it has its own: one wall box around the
    /// model's bounds when it is solid.
    pub fn default_cols(&self) -> Vec<crate::edit::ColBox> {
        if !self.solid {
            return Vec::new();
        }
        vec![crate::edit::ColBox::wall((self.hi - self.lo).max(Vec3::splat(0.1)), (self.hi + self.lo) * 0.5)]
    }
}

/// `art/hut.glb` -> `hut`.
pub fn stem(rel: &str) -> &str {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// A file name the backend accepts (`[A-Za-z0-9][A-Za-z0-9_.-]*`), from any.
pub fn clean_stem(path: &Path) -> String {
    let raw = path.file_stem().and_then(|s| s.to_str()).unwrap_or("model");
    let mut s: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    s.truncate(60);
    if !s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
        s.insert(0, 'm');
    }
    s
}

/// Read a `.glb`, check it draws, and return it with its bounds.
pub fn read_glb(file: &Path) -> Result<(Vec<u8>, Vec3, Vec3)> {
    let bytes = std::fs::read(file).with_context(|| format!("{}", file.display()))?;
    let mesh = crate::assets::decode_glb_part(&bytes, None).with_context(|| format!("{}", file.display()))?;
    if mesh.vertices.len() > 65535 {
        bail!("{}: {} vertices; the game takes at most 65535 per model, decimate it first", file.display(), mesh.vertices.len());
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in &mesh.vertices {
        lo = lo.min(Vec3::from(v.pos));
        hi = hi.max(Vec3::from(v.pos));
    }
    Ok((bytes, lo, hi))
}

/// Stage a file with the backend; returns its staged id.
pub fn upload(be: &Backend, rel: &str, bytes: &[u8]) -> Result<String> {
    // The backend's `/api/upload` cap (`app.MAX_UPLOAD`).
    if bytes.len() > 96 << 20 {
        bail!("{rel}: {} MiB is over the 96 MiB import limit", bytes.len() >> 20);
    }
    let body = json!({"name": rel.rsplit('/').next().unwrap_or(rel), "data": base64::engine::general_purpose::STANDARD.encode(bytes)});
    let r = be.call("POST", "/map/api/upload", Some(&body))?;
    if !r.ok() {
        bail!("{rel}: {}", r.error_text());
    }
    Ok(r.json()?["id"].as_str().unwrap_or_default().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned_for_the_backend() {
        assert_eq!(clean_stem(Path::new("/x/My Hut (v2).glb")), "My_Hut__v2_");
        assert_eq!(clean_stem(Path::new("_hidden.png")), "m_hidden");
        assert_eq!(stem("art/hut.glb"), "hut");
    }

    #[test]
    fn a_solid_model_gets_a_wall_box_around_its_bounds() {
        let m = Model {
            mesh: Art { rel: "art/hut.glb".into(), file: "/x/hut.glb".into() },
            texture: None,
            solid: true,
            lo: Vec3::new(-1.0, 0.0, -2.0),
            hi: Vec3::new(1.0, 3.0, 2.0),
        };
        let f = m.fields();
        assert_eq!(f["collision"][0]["size"], json!([2.0, 3.0, 4.0]));
        assert_eq!(f["collision"][0]["at"], json!([0.0, 1.5, 0.0]));
        assert_eq!(m.entity(), "model:art/hut.glb");
    }
}
