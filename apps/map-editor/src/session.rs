//! What the editor keeps between runs: autosave (crash recovery), recent mods,
//! and the author's view state (hidden / locked objects, groups).
//!
//! Stored under the OS config dir (`$XDG_CONFIG_HOME/rsmm-map-editor`,
//! `%APPDATA%\rsmm-map-editor`). Nothing here is part of a mod: the mod is
//! only what Save writes through the backend.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Group {
    pub name: String,
    /// `o<index>` for a shipped object, `a<uid>` for an added one.
    pub members: Vec<String>,
}

/// One chapter's unsaved work, written every autosave tick while dirty.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Autosave {
    pub chapter: String,
    pub mod_id: String,
    pub mod_name: String,
    pub saved_at: u64,
    /// The `scenery` save document.
    pub scenery: Value,
    pub keep_edits: Value,
    pub keep_map: Option<Value>,
    pub hidden: Vec<String>,
    pub locked: Vec<String>,
    pub groups: Vec<Group>,
    #[serde(default)]
    pub recipe: Value,
    #[serde(default)]
    pub strokes: Value,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Prefs {
    pub recent: Vec<String>,
    pub light_theme: bool,
    pub grid_size: Option<f32>,
}

pub fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let d = base.join("rsmm-map-editor");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

fn autosave_path(chapter: &str) -> Option<PathBuf> {
    let safe: String = chapter.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    Some(dir()?.join(format!("autosave-{safe}.json")))
}

/// Write atomically: a crash mid-write must not destroy the previous autosave.
fn write_atomic(path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

pub fn save_autosave(a: &Autosave) -> std::io::Result<()> {
    let p = autosave_path(&a.chapter).ok_or_else(|| std::io::Error::other("no config dir"))?;
    write_atomic(&p, &serde_json::to_vec_pretty(a)?)
}

pub fn load_autosave(chapter: &str) -> Option<Autosave> {
    let p = autosave_path(chapter)?;
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

pub fn clear_autosave(chapter: &str) {
    if let Some(p) = autosave_path(chapter) {
        let _ = std::fs::remove_file(p);
    }
}

pub fn load_prefs() -> Prefs {
    dir()
        .and_then(|d| std::fs::read(d.join("prefs.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_prefs(p: &Prefs) {
    if let (Some(d), Ok(b)) = (dir(), serde_json::to_vec_pretty(p)) {
        let _ = write_atomic(&d.join("prefs.json"), &b);
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
