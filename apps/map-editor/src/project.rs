//! The Project panel's folders, Unity style: `Assets` holds the mod's own
//! models (`art/` in the mod), `Game` mirrors where each placeable prop lives
//! in the game's files (`Game/DarkHills/SceneryObjects_DarkHills/...`).

use std::collections::BTreeSet;

pub const ASSETS: &str = "Assets";
pub const GAME: &str = "Game";

/// The folder a game prop is shown in: its entity path without the file name.
/// `DarkHills\SceneryObjects_DarkHills\Rock_A.entity.ot` ->
/// `Game/DarkHills/SceneryObjects_DarkHills`.
pub fn folder_of(entity: &str) -> String {
    let mut parts: Vec<&str> = entity.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    parts.pop();
    std::iter::once(GAME).chain(parts).collect::<Vec<_>>().join("/")
}

/// Every folder the items live in, with all of their parents.
pub fn folders<'a>(item_folders: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = [ASSETS.to_owned(), GAME.to_owned()].into();
    for f in item_folders {
        let mut acc = String::new();
        for seg in f.split('/') {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            out.insert(acc.clone());
        }
    }
    out
}

/// The folders directly inside `dir`.
pub fn children<'a>(all: &'a BTreeSet<String>, dir: &str) -> Vec<&'a str> {
    let prefix = format!("{dir}/");
    all.iter().filter(|f| f.strip_prefix(&prefix).is_some_and(|rest| !rest.contains('/'))).map(String::as_str).collect()
}

/// `Game/DarkHills` -> `DarkHills`; a folder's own name.
pub fn leaf(dir: &str) -> &str {
    dir.rsplit('/').next().unwrap_or(dir)
}

/// The folder above `dir`, if any.
pub fn parent(dir: &str) -> Option<&str> {
    dir.rsplit_once('/').map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn props_are_filed_by_their_entity_path() {
        assert_eq!(folder_of("DarkHills\\SceneryObjects_DarkHills\\Rock_A.entity.ot"), "Game/DarkHills/SceneryObjects_DarkHills");
        let all = folders(["Game/DarkHills/SceneryObjects_DarkHills", "Game/Avalon"]);
        assert_eq!(children(&all, "Game"), vec!["Game/Avalon", "Game/DarkHills"]);
        assert_eq!(children(&all, "Game/DarkHills"), vec!["Game/DarkHills/SceneryObjects_DarkHills"]);
        assert!(all.contains(ASSETS));
        assert_eq!(parent("Game/DarkHills"), Some("Game"));
        assert_eq!(leaf("Game/DarkHills"), "DarkHills");
    }
}
