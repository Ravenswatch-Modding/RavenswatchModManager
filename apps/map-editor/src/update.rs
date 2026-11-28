//! Self-update: `rsmm update-editor` (src/rsmm/cli/cmd_update_editor.py)
//! checks the signed `map-editor` release channel and swaps this folder's
//! files; the editor only asks and restarts.

use std::path::PathBuf;

/// This build's number on the channel (CI sets `RSMM_EDITOR_BUILD`); 0 is a
/// developer build, which never updates itself.
pub fn build() -> u64 {
    option_env!("RSMM_EDITOR_BUILD").and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// The folder to update: the editor's own, and only when the rsmm it runs is
/// the one shipped beside it (not a source checkout's).
pub fn editor_dir(rsmm: &[String]) -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let cli = PathBuf::from(rsmm.first()?);
    (rsmm.len() == 1 && cli.parent() == Some(dir.as_path())).then_some(dir)
}

/// Run `rsmm update-editor` (`check`: report only) and read its JSON line.
pub fn run(rsmm: &[String], dir: &std::path::Path, check: bool) -> Result<serde_json::Value, String> {
    let mut c = std::process::Command::new(&rsmm[0]);
    c.args(["update-editor", "--json", "--current", &build().to_string(), "--dir"]).arg(dir);
    if check {
        c.arg("--check");
    }
    let out = c.output().map_err(|e| format!("rsmm update-editor: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('{')).ok_or_else(|| {
        let err = String::from_utf8_lossy(&out.stderr);
        format!("rsmm update-editor: {}", err.lines().last().unwrap_or("no answer"))
    })?;
    let v: serde_json::Value = serde_json::from_str(line).map_err(|e| format!("rsmm update-editor: {e}"))?;
    match v["status"].as_str() {
        Some("error") => Err(v["error"].as_str().unwrap_or("failed").to_owned()),
        _ => Ok(v),
    }
}

/// Start the (new) editor with the same arguments; the caller then closes.
/// Linux follows the running file to its new name (`….old`): start the
/// file that now holds the old name.
pub fn relaunch() -> Result<(), String> {
    let mut exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if let Some(name) = exe.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".old")).map(str::to_owned) {
        exe.set_file_name(name);
    }
    std::process::Command::new(exe).args(std::env::args_os().skip(1)).spawn().map(|_| ()).map_err(|e| e.to_string())
}
