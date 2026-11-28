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

/// `Assets/rocks` -> `art/rocks`: where a Project folder lives in the mod.
pub fn art_dir(dir: &str) -> Option<String> {
    let rest = dir.strip_prefix(ASSETS)?;
    (rest.is_empty() || rest.starts_with('/')).then(|| format!("art{rest}"))
}

/// `art/rocks/hut.glb` -> `Assets/rocks`: the Project folder a mod file shows in.
pub fn folder_of_art(rel: &str) -> String {
    let dir = rel.rsplit_once('/').map_or("", |(d, _)| d);
    format!("{ASSETS}{}", dir.strip_prefix("art").unwrap_or(""))
}

/// `file` (a bare name) inside the Project folder `dir` (`Assets` if `dir` is not one).
pub fn rel_in(dir: &str, file: &str) -> String {
    format!("{}/{file}", art_dir(dir).unwrap_or_else(|| "art".to_owned()))
}

/// A folder name the backend accepts (`[A-Za-z0-9][A-Za-z0-9_.-]*`, at most 80).
pub fn clean_folder(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).take(80).collect();
    match s.chars().next() {
        Some(c) if c.is_ascii_alphanumeric() => s,
        Some(_) => format!("F{s}").chars().take(80).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_folders_map_to_the_mods_art() {
        assert_eq!(art_dir("Assets"), Some("art".into()));
        assert_eq!(art_dir("Assets/rocks/big"), Some("art/rocks/big".into()));
        assert_eq!(art_dir("Assetsy"), None);
        assert_eq!(art_dir("Game/DarkHills"), None);
        assert_eq!(folder_of_art("art/hut.glb"), "Assets");
        assert_eq!(folder_of_art("art/rocks/hut.glb"), "Assets/rocks");
        assert_eq!(rel_in("Assets/rocks", "hut.glb"), "art/rocks/hut.glb");
        assert_eq!(rel_in("Game/x", "hut.glb"), "art/hut.glb");
        assert_eq!(clean_folder(" My rocks! "), "My_rocks_");
        assert_eq!(clean_folder(".."), "F..");
        assert_eq!(clean_folder("   "), "");
    }

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
