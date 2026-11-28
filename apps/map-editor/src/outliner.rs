//! The outliner: the chapter's objects as a tree, not one flat list.
//!
//! Level → folder → object type → instances. A branch's eye / lock / select
//! acts on every object under it. Added objects and the author's groups sit
//! beside the shipped levels.

use std::collections::BTreeMap;

use crate::edit::Sel;
use crate::scene::Scenery;

pub struct Node {
    pub label: String,
    pub children: Vec<Node>,
    /// Every object under this node (leaves included), for branch actions.
    pub members: Vec<Sel>,
    /// The single object a leaf stands for.
    pub leaf: Option<Sel>,
}

impl Node {
    fn branch(label: String, children: Vec<Node>) -> Node {
        let members = children.iter().flat_map(|c| c.members.iter().copied()).collect();
        Node { label, children, members, leaf: None }
    }
}

/// `Ot/DarkHills/Map_Dark_Hills_Scenery_NorthEast.level.ot.GameStream.gen`
/// → `Scenery NorthEast`.
pub fn level_label(path: &str) -> String {
    if path.ends_with(".tiledef.ot") {
        // A tile edited alone: its own name.
        let f = path.rsplit('\\').next().unwrap_or(path).trim_end_matches(".tiledef.ot");
        return f.replace('_', " ");
    }
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.split(".level").next().unwrap_or(file);
    let parts: Vec<&str> = stem.split('_').collect();
    // Drop the "Map_<Chapter words>" prefix: keep from the first part after
    // the chapter name that names the level's role.
    let roles = ["Gameplay", "Scenery", "Terrain", "Render", "Audio", "Props", "Decor", "Fx", "Lights"];
    match parts.iter().position(|p| roles.iter().any(|r| p.starts_with(r))) {
        Some(i) => parts[i..].join(" "),
        None => stem.replace('_', " "),
    }
}

/// `DarkHills\SceneryObjects_DarkHills\Tree_Big_Roots.entity.ot` → folder
/// `SceneryObjects DarkHills`, type `Tree_Big_Roots`.
fn split_entity(e: &str) -> (String, String) {
    let parts: Vec<&str> = e.split('\\').collect();
    let name = parts.last().copied().unwrap_or(e).trim_end_matches(".entity.ot").to_owned();
    let folder = if parts.len() >= 3 {
        parts[parts.len() - 2].replace('_', " ")
    } else if parts.len() == 2 {
        parts[0].replace('_', " ")
    } else {
        "Other".to_owned()
    };
    (folder, name)
}

pub fn build(s: &Scenery) -> Vec<Node> {
    // level -> folder -> type -> objects
    let mut tree: BTreeMap<u16, BTreeMap<String, BTreeMap<String, Vec<usize>>>> = BTreeMap::new();
    for i in 0..s.obj_xf.len() {
        let e = s.entities.get(s.obj_entity[i] as usize).map(String::as_str).unwrap_or("?");
        let (folder, name) = split_entity(e);
        tree.entry(s.obj_level[i]).or_default().entry(folder).or_default().entry(name).or_default().push(i);
    }
    tree.into_iter()
        .map(|(li, folders)| {
            let label = s.levels.get(li as usize).map(|p| level_label(p)).unwrap_or_else(|| format!("Level {li}"));
            let children = folders
                .into_iter()
                .map(|(folder, types)| {
                    let kids = types
                        .into_iter()
                        .map(|(name, objs)| {
                            let leaves = objs
                                .iter()
                                .map(|&i| Node {
                                    label: format!("{name}  #{}", s.obj_id[i]),
                                    children: Vec::new(),
                                    members: vec![Sel::Obj(i)],
                                    leaf: Some(Sel::Obj(i)),
                                })
                                .collect();
                            Node::branch(name, leaves)
                        })
                        .collect();
                    Node::branch(folder, kids)
                })
                .collect();
            Node::branch(label, children)
        })
        .collect()
}

/// Does `node` (or anything under it) match a lower-case filter?
pub fn matches(node: &Node, f: &str) -> bool {
    f.is_empty() || node.label.to_lowercase().contains(f) || node.children.iter().any(|c| matches(c, f))
}
