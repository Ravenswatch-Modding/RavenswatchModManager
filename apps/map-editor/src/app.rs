//! The editor window: menu bar, outliner tree, 3D view with Blender-style
//! tools, inspector, status bar.
//!
//! Every action (menu item, shortcut, context menu, command palette) goes
//! through [`Cmd`] and [`MapEditor::run`], so they all behave the same.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use glam::{Mat4, Quat, Vec3};
use serde_json::{json, Value};

use crate::assets::{Job, Loaded, Loader, MeshData};
use crate::backend::{q, Backend, Response};
use crate::camera::Camera;
use crate::brush::{self, Brush, Mode as BrushMode};
use crate::edit::{Add, Edits, History, Sel, Stroke, NEW_SPOT};
use crate::recipe::{self, KindVals, PoolTile, Recipe};
use crate::icons::{self, Icon};
use crate::outliner::{self, Node};
use crate::render::{DrawPart, Frame, Instance, LineVertex, Renderer};
use crate::scene::{Environment, PartKey, Scenery, Terrain, TerrainLook, Xf};
use crate::session::{self, Autosave, Group, Prefs};

const SELECT_TINT: [f32; 4] = [1.0, 0.62, 0.18, 0.30];
const ACTIVE_TINT: [f32; 4] = [1.0, 0.78, 0.35, 0.42];
const MOVED_TINT: [f32; 4] = [0.35, 0.75, 1.0, 0.14];
const ADDED_TINT: [f32; 4] = [0.45, 1.0, 0.55, 0.14];
const AXIS_COL: [egui::Color32; 3] =
    [egui::Color32::from_rgb(235, 72, 72), egui::Color32::from_rgb(120, 205, 60), egui::Color32::from_rgb(70, 130, 245)];
const AUTOSAVE_EVERY: Duration = Duration::from_secs(45);

/// Command-line choices.
#[derive(Default)]
pub struct Options {
    pub chapter: Option<String>,
    /// Save the 3D view here once everything has loaded, then quit.
    pub snapshot: Option<std::path::PathBuf>,
    /// Make a few scenery edits, run them through /map/api/check, print the
    /// answer and quit (exercises the whole edit -> save document path).
    pub self_check: bool,
    /// Snapshot camera: target x, z and distance (else the whole map).
    pub view: Option<[f32; 3]>,
    /// Screenshot the whole window (UI included) once loaded, then quit.
    pub ui_snapshot: Option<std::path::PathBuf>,
    /// Open a pool tile in the POI editor, edit it, run the tile check, quit.
    pub tile_check: bool,
    /// Open the first pool tile whose name contains this, once loaded.
    pub open_tile: Option<String>,
}

struct Chapter {
    key: String,
    label: String,
}

#[derive(Clone)]
struct Prop {
    entity: String,
    name: String,
    group: String,
    scenery: bool,
}

/// What one added entity draws, in its own space.
type Template = Vec<(PartKey, Vec<Mat4>)>;

enum Event {
    State(Vec<Chapter>, Vec<String>),
    Terrain(String, anyhow::Result<Terrain>),
    Env(String, anyhow::Result<Environment>),
    Look(String, anyhow::Result<TerrainLook>),
    Scenery(String, anyhow::Result<Scenery>),
    Props(String, anyhow::Result<Vec<Prop>>),
    Template(String, anyhow::Result<Template>),
    Opened(String, anyhow::Result<Value>),
    Recipe(String, anyhow::Result<Recipe>),
    Pool(String, anyhow::Result<Vec<PoolTile>>),
    TileObjects(String, anyhow::Result<Value>),
    TileProps(anyhow::Result<Vec<Prop>>),
    Error(String),
}

/// The chapter's state, parked while a tile (POI) is being edited.
struct Stash {
    scenery: Option<Scenery>,
    edits: Edits,
    history: History,
    sel: BTreeSet<Sel>,
    active: Option<Sel>,
    hidden: HashSet<Sel>,
    locked: HashSet<Sel>,
    groups: Vec<(String, Vec<Sel>)>,
    tree: Vec<Node>,
    props: Vec<Prop>,
    mod_id: String,
    mod_name: String,
    unsaved: bool,
    result: Option<Result<Vec<String>, String>>,
    terrain: Option<Terrain>,
    camera: Camera,
    tile: Option<TileSession>,
}

/// Editing one tile's own objects (a POI), alone in its own tab.
struct TileSession {
    tile: String,
    origin: Vec3,
    /// `(entity, pos in tile space, rot, scale)` as the game ships them.
    objects: Vec<(String, Vec3, Quat, Vec3)>,
    built: bool,
}

/// A document tab: the map, or one tile.
struct Tab {
    title: String,
    tile: Option<String>,
    /// Its state while another tab is shown.
    parked: Option<Stash>,
}

#[derive(PartialEq, Clone)]
enum Tool {
    Select,
    Place(String),
    /// Place a new tile spot of this footprint group.
    AddSpot(String),
    Brush,
}

#[derive(PartialEq, Clone, Copy)]
enum GizmoMode {
    Move,
    Rotate,
    Scale,
}

#[derive(PartialEq, Clone, Copy)]
enum LeftTab {
    Outliner,
    Assets,
    Pois,
    Spots,
    Terrain,
    Changes,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum Kind {
    Grab,
    Rotate,
    Scale,
}

/// Blender's axis constraint: one axis, or (Shift) everything but one.
#[derive(PartialEq, Clone, Copy, Debug)]
enum Lock {
    Free,
    Axis(usize),
    Plane(usize),
}

impl Lock {
    fn mask(self) -> Vec3 {
        let unit = |a: usize| Vec3::from_array(std::array::from_fn(|i| if i == a { 1.0 } else { 0.0 }));
        match self {
            Lock::Free => Vec3::ONE,
            Lock::Axis(a) => unit(a),
            Lock::Plane(a) => Vec3::ONE - unit(a),
        }
    }

    fn label(self) -> String {
        let n = ["X", "Y", "Z"];
        match self {
            Lock::Free => String::new(),
            Lock::Axis(a) => format!(" along {}", n[a]),
            Lock::Plane(a) => format!(" except {}", n[a]),
        }
    }
}

/// An in-progress transform of the whole selection.
#[derive(Clone)]
struct Modal {
    kind: Kind,
    lock: Lock,
    starts: Vec<(Sel, Xf)>,
    pivot: Vec3,
    /// Pointer where the transform began (screen) and the ground under it.
    p0: egui::Pos2,
    g0: Option<Vec3>,
    /// Ended by releasing the button (a drag) rather than by a click.
    by_drag: bool,
    /// Typed exact value (Blender's numeric input).
    typed: String,
}

#[derive(PartialEq)]
enum Pending {
    Check,
    Save,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Cmd {
    New,
    Save,
    SaveAs,
    Check,
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    Duplicate,
    Delete,
    SelectAll,
    DeselectAll,
    Invert,
    Hide,
    HideUnselected,
    UnhideAll,
    Lock,
    UnlockAll,
    Group,
    Grab,
    Rotate,
    Scale,
    DropToGround,
    Align(usize, i8),
    Distribute(usize),
    ResetTransform,
    FrameAll,
    FrameSelected,
    ViewFront,
    ViewSide,
    ViewTop,
    ToggleOrtho,
    ToggleGrid,
    ToggleSnap,
    ToggleShadows,
    ToggleMinimap,
    ToggleEnv,
    ToggleLog,
    ToggleTheme,
    Palette,
    Shortcuts,
    ToolMove,
    ToolRotate,
    ToolScale,
}

/// Name, shortcut, command: the palette's list and the shortcut sheet.
const COMMANDS: &[(&str, &str, Cmd)] = &[
    ("Save", "Ctrl+S", Cmd::Save),
    ("Save As…", "Ctrl+Shift+S", Cmd::SaveAs),
    ("Check the mod builds", "", Cmd::Check),
    ("New map from a chapter…", "Ctrl+N", Cmd::New),
    ("Undo", "Ctrl+Z", Cmd::Undo),
    ("Redo", "Ctrl+Shift+Z / Ctrl+Y", Cmd::Redo),
    ("Copy", "Ctrl+C", Cmd::Copy),
    ("Cut", "Ctrl+X", Cmd::Cut),
    ("Paste at cursor", "Ctrl+V", Cmd::Paste),
    ("Duplicate and move", "Shift+D", Cmd::Duplicate),
    ("Delete", "X / Delete", Cmd::Delete),
    ("Select all", "A", Cmd::SelectAll),
    ("Deselect all", "Alt+A", Cmd::DeselectAll),
    ("Invert selection", "Ctrl+I", Cmd::Invert),
    ("Hide selected", "H", Cmd::Hide),
    ("Hide unselected", "Shift+H", Cmd::HideUnselected),
    ("Show all", "Alt+H", Cmd::UnhideAll),
    ("Lock selected", "L", Cmd::Lock),
    ("Unlock all", "Alt+L", Cmd::UnlockAll),
    ("Group selected", "Ctrl+G", Cmd::Group),
    ("Move (grab)", "G  then X/Y/Z, a number, Enter", Cmd::Grab),
    ("Rotate", "R  then X/Y/Z, a number, Enter", Cmd::Rotate),
    ("Scale", "S  then X/Y/Z, a number, Enter", Cmd::Scale),
    ("Drop to ground", "End", Cmd::DropToGround),
    ("Align X: min", "", Cmd::Align(0, -1)),
    ("Align X: centre", "", Cmd::Align(0, 0)),
    ("Align X: max", "", Cmd::Align(0, 1)),
    ("Align Z: min", "", Cmd::Align(2, -1)),
    ("Align Z: centre", "", Cmd::Align(2, 0)),
    ("Align Z: max", "", Cmd::Align(2, 1)),
    ("Align Y: same height", "", Cmd::Align(1, 0)),
    ("Distribute along X", "", Cmd::Distribute(0)),
    ("Distribute along Z", "", Cmd::Distribute(2)),
    ("Reset transform", "Alt+G", Cmd::ResetTransform),
    ("Frame whole map", "Home", Cmd::FrameAll),
    ("Frame selection", "F / Numpad .", Cmd::FrameSelected),
    ("Front view", "1", Cmd::ViewFront),
    ("Side view", "3", Cmd::ViewSide),
    ("Top view", "7", Cmd::ViewTop),
    ("Perspective / orthographic", "5", Cmd::ToggleOrtho),
    ("Move tool (gizmo)", "W", Cmd::ToolMove),
    ("Rotate tool (gizmo)", "E", Cmd::ToolRotate),
    ("Scale tool (gizmo)", "T", Cmd::ToolScale),
    ("Show grid", "", Cmd::ToggleGrid),
    ("Snap to grid", "Ctrl held = invert", Cmd::ToggleSnap),
    ("Shadows", "", Cmd::ToggleShadows),
    ("Minimap", "M", Cmd::ToggleMinimap),
    ("Environment (light & fog)", "", Cmd::ToggleEnv),
    ("Log", "", Cmd::ToggleLog),
    ("Light / dark theme", "", Cmd::ToggleTheme),
    ("Command palette", "F3 / Ctrl+P", Cmd::Palette),
    ("Keyboard shortcuts", "F1", Cmd::Shortcuts),
];

pub struct MapEditor {
    backend: Backend,
    loader: Loader,
    events: Receiver<Event>,
    events_tx: Sender<Event>,
    renderer: Option<Renderer>,

    chapters: Vec<Chapter>,
    editor_mods: Vec<String>,
    chapter: Option<String>,
    terrain: Option<Terrain>,
    scenery: Option<Scenery>,
    tree: Vec<Node>,
    props: Vec<Prop>,
    templates: HashMap<String, Option<Template>>,
    requested: HashSet<String>,
    pending_assets: usize,
    failed_assets: Vec<String>,
    look_pending: bool,
    minimap_tex: Option<egui::TextureHandle>,

    // generation recipe, tile pool, run preview
    recipe: Option<Recipe>,
    pool: Vec<PoolTile>,
    layout: std::collections::BTreeMap<i64, String>,
    seed: u32,
    scen: usize,
    show_spots: bool,
    show_tiles: bool,
    kind_focus: Option<String>,
    tile: Option<TileSession>,
    tabs: Vec<Tab>,
    active_tab: usize,
    tile_props: Vec<Prop>,
    close_confirm: Option<usize>,
    poi_filter: String,
    poi_size: Option<i64>,

    // terrain brushes
    terrain_base: Option<Terrain>,
    look_base: Option<crate::scene::TerrainLook>,
    weights_now: Vec<u8>,
    color_now: Option<Vec<u8>>,
    brush: Brush,
    last_dab: Option<Vec3>,

    // edits and view state
    edits: Edits,
    history: History,
    sel: BTreeSet<Sel>,
    active: Option<Sel>,
    hidden: HashSet<Sel>,
    locked: HashSet<Sel>,
    groups: Vec<(String, Vec<Sel>)>,
    clipboard: Vec<(String, String, Xf)>,
    tool: Tool,
    gizmo: GizmoMode,
    modal: Option<Modal>,
    box_start: Option<egui::Pos2>,
    keep_edits: Value,
    keep_map: Option<Value>,
    opened_doc: Option<Value>,
    unsaved: bool,
    last_autosave: Instant,
    restore_offer: Option<Autosave>,

    // drawing caches
    dirty: bool,
    shipped_draw: Vec<Vec<Instance>>,
    add_draw: Vec<(PartKey, Vec<Instance>)>,
    pickables: Vec<(String, Mat4, Sel)>,

    // view
    camera: Camera,
    left_tab: LeftTab,
    tree_filter: String,
    prop_filter: String,
    all_props: bool,
    show_grid: bool,
    snap: bool,
    grid_size: f32,
    snap_ground: bool,
    show_minimap: bool,
    min_px: f32,
    haze: f32,
    exposure: f32,
    shadows: bool,
    env_shipped: Environment,
    env: Environment,
    show_env: bool,
    show_log: bool,
    show_palette: bool,
    palette_query: String,
    show_shortcuts: bool,
    show_save_as: bool,
    light_theme: bool,
    recent: Vec<String>,
    status: String,
    frame_ms: f32,
    cursor: Option<Vec3>,

    // save
    mod_id: String,
    mod_name: String,
    as_map: bool,
    map_id: String,
    map_play: bool,
    pending: Option<(Pending, Receiver<Response>)>,
    result: Option<Result<Vec<String>, String>>,
    saved_next: Option<String>,

    opts: Options,
    settled_frames: u32,
    self_check_sent: bool,
    ui_shot_sent: bool,
}

impl MapEditor {
    pub fn new(cc: &eframe::CreationContext, backend: Backend, opts: Options) -> MapEditor {
        let renderer = cc.wgpu_render_state.as_ref().map(Renderer::new);
        let ctx = cc.egui_ctx.clone();
        let workers = thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
        let loader = Loader::new(backend.clone(), workers, move || ctx.request_repaint());
        let (tx, rx) = channel();
        let prefs = session::load_prefs();
        style(&cc.egui_ctx, prefs.light_theme);
        let me = MapEditor {
            backend,
            loader,
            events: rx,
            events_tx: tx,
            renderer,
            chapters: Vec::new(),
            editor_mods: Vec::new(),
            chapter: None,
            terrain: None,
            scenery: None,
            tree: Vec::new(),
            props: Vec::new(),
            templates: HashMap::new(),
            requested: HashSet::new(),
            pending_assets: 0,
            failed_assets: Vec::new(),
            look_pending: false,
            minimap_tex: None,
            recipe: None,
            pool: Vec::new(),
            layout: std::collections::BTreeMap::new(),
            seed: 1,
            scen: 0,
            show_spots: true,
            show_tiles: true,
            kind_focus: None,
            tile: None,
            tabs: vec![Tab { title: "Map".into(), tile: None, parked: None }],
            active_tab: 0,
            tile_props: Vec::new(),
            close_confirm: None,
            poi_filter: String::new(),
            poi_size: None,
            terrain_base: None,
            look_base: None,
            weights_now: Vec::new(),
            color_now: None,
            brush: Brush::default(),
            last_dab: None,
            edits: Edits::default(),
            history: History::default(),
            sel: BTreeSet::new(),
            active: None,
            hidden: HashSet::new(),
            locked: HashSet::new(),
            groups: Vec::new(),
            clipboard: Vec::new(),
            tool: Tool::Select,
            gizmo: GizmoMode::Move,
            modal: None,
            box_start: None,
            keep_edits: json!({}),
            keep_map: None,
            opened_doc: None,
            unsaved: false,
            last_autosave: Instant::now(),
            restore_offer: None,
            dirty: true,
            shipped_draw: Vec::new(),
            add_draw: Vec::new(),
            pickables: Vec::new(),
            camera: Camera::default(),
            left_tab: LeftTab::Outliner,
            tree_filter: String::new(),
            prop_filter: String::new(),
            all_props: false,
            show_grid: true,
            snap: false,
            grid_size: prefs.grid_size.unwrap_or(1.0),
            snap_ground: true,
            show_minimap: true,
            min_px: 1.5,
            haze: 0.00025,
            exposure: 1.0,
            shadows: true,
            env_shipped: Environment::default(),
            env: Environment::default(),
            show_env: false,
            show_log: false,
            show_palette: false,
            palette_query: String::new(),
            show_shortcuts: false,
            show_save_as: false,
            light_theme: prefs.light_theme,
            recent: prefs.recent,
            status: "Reading chapters…".into(),
            frame_ms: 0.0,
            cursor: None,
            mod_id: String::new(),
            mod_name: String::new(),
            as_map: false,
            map_id: String::new(),
            map_play: true,
            pending: None,
            result: None,
            saved_next: None,
            opts,
            settled_frames: 0,
            self_check_sent: false,
            ui_shot_sent: false,
        };
        me.refresh_state(&cc.egui_ctx);
        me
    }

    fn save_prefs(&self) {
        if self.headless() {
            return;
        }
        session::save_prefs(&Prefs { recent: self.recent.clone(), light_theme: self.light_theme, grid_size: Some(self.grid_size) });
    }

    fn refresh_state(&self, ctx: &egui::Context) {
        self.spawn(ctx, |be| {
            let v = be.get_json("/map/api/state")?;
            let chapters = v["chapters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| Some(Chapter { key: c["key"].as_str()?.to_owned(), label: c["label"].as_str()?.to_owned() }))
                .collect();
            let mods = v["mods"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(str::to_owned)).collect();
            Ok(Event::State(chapters, mods))
        });
    }

    /// Run a backend read on its own thread; its event arrives in `ui`.
    fn spawn(&self, ctx: &egui::Context, f: impl FnOnce(&Backend) -> anyhow::Result<Event> + Send + 'static) {
        let (be, tx, ctx) = (self.backend.clone(), self.events_tx.clone(), ctx.clone());
        thread::spawn(move || {
            let ev = f(&be).unwrap_or_else(|e| Event::Error(format!("{e:#}")));
            let _ = tx.send(ev);
            ctx.request_repaint();
        });
    }

    // ---- loading ------------------------------------------------------------------

    fn open_chapter(&mut self, ctx: &egui::Context, key: String) {
        self.chapter = Some(key.clone());
        self.tabs[0].title = key.replace('_', " ");
        self.terrain = None;
        self.scenery = None;
        self.tree.clear();
        self.props.clear();
        self.shipped_draw.clear();
        self.edits = Edits::default();
        self.history.clear();
        self.sel.clear();
        self.active = None;
        self.hidden.clear();
        self.locked.clear();
        self.groups.clear();
        self.modal = None;
        self.tool = Tool::Select;
        self.result = None;
        self.unsaved = false;
        self.minimap_tex = None;
        self.recipe = None;
        self.pool.clear();
        self.layout.clear();
        self.kind_focus = None;
        self.scen = 0;
        self.terrain_base = None;
        self.look_base = None;
        self.weights_now.clear();
        self.color_now = None;
        self.last_dab = None;
        self.tile = None;
        self.tabs.truncate(1);
        self.tabs[0].parked = None;
        self.active_tab = 0;
        self.dirty = true;
        self.restore_offer = if self.headless() { None } else { session::load_autosave(&key) }.filter(|a| a.scenery.as_object().is_some_and(|o| !o.is_empty()) || !a.groups.is_empty());
        if let Some(r) = &mut self.renderer {
            r.clear_scene();
        }
        self.status = format!("Loading {key}…");
        let k = key.clone();
        self.spawn(ctx, move |be| {
            let t = be.get_json(&format!("/map/api/terrain?grid=512&chapter={}", q(&k))).and_then(|v| Terrain::from_json(&v));
            Ok(Event::Terrain(k, t))
        });
        let k = key.clone();
        self.spawn(ctx, move |be| {
            let r = be.get_json(&format!("/map/api/recipe?chapter={}", q(&k))).map(|v| Recipe::from_json(&v));
            Ok(Event::Recipe(k, r))
        });
        let k = key.clone();
        self.spawn(ctx, move |be| {
            let r = be.get_json(&format!("/map/api/tiles?chapter={}", q(&k))).map(|v| recipe::pool_from_json(&v));
            Ok(Event::Pool(k, r))
        });
        let k = key.clone();
        self.spawn(ctx, move |be| {
            let e = be.get_json(&format!("/map/api/environment?chapter={}", q(&k))).map(|v| Environment::from_json(&v));
            Ok(Event::Env(k, e))
        });
        let k = key.clone();
        self.look_pending = true;
        self.spawn(ctx, move |be| {
            let l = be.get_json(&format!("/map/api/terrain/look?chapter={}", q(&k))).and_then(|v| TerrainLook::from_json(&v));
            Ok(Event::Look(k, l))
        });
        let k = key.clone();
        self.spawn(ctx, move |be| {
            let s = be.get_json(&format!("/map/api/scenery?chapter={}", q(&k))).and_then(|v| Scenery::from_json(&v));
            Ok(Event::Scenery(k, s))
        });
        self.spawn(ctx, move |be| {
            let p = be.get_json(&format!("/map/api/scenery/props?chapter={}", q(&key))).map(|v| {
                v["props"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| {
                        Some(Prop {
                            entity: p["entity"].as_str()?.to_owned(),
                            name: p["name"].as_str().unwrap_or_default().to_owned(),
                            group: p["group"].as_str().unwrap_or_default().to_owned(),
                            scenery: p["scenery"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            });
            Ok(Event::Props(key, p))
        });
    }

    fn request_part_assets(&mut self, key: &PartKey) {
        if key.decal {
            if let Some(r) = &mut self.renderer {
                if !r.has_mesh(&key.mesh) {
                    r.add_mesh(key.mesh.clone(), MeshData::decal_quad());
                }
            }
        } else if self.requested.insert(key.mesh.clone()) {
            self.loader.request(Job::Mesh(key.mesh.clone()));
            self.pending_assets += 1;
        }
        if let Some(t) = &key.texture {
            if self.requested.insert(format!("{t}|{}", key.swap_rb)) {
                self.loader.request(Job::Texture(t.clone(), key.swap_rb));
                self.pending_assets += 1;
            }
        }
    }

    fn ensure_template(&mut self, ctx: &egui::Context, entity: &str) {
        if self.templates.contains_key(entity) {
            return;
        }
        self.templates.insert(entity.to_owned(), None);
        let e = entity.to_owned();
        self.spawn(ctx, move |be| {
            let t = be.get_json(&format!("/map/api/t3/entity?ref={}", q(&e))).and_then(|v| parse_template(&v));
            Ok(Event::Template(e, t))
        });
    }

    fn shipped_kinds(&self) -> Vec<(String, KindVals)> {
        self.recipe.as_ref().map(|r| r.kinds.iter().map(|k| (k.name.clone(), k.vals.clone())).collect()).unwrap_or_default()
    }

    fn apply_opened(&mut self, ctx: &egui::Context) {
        if self.scenery.is_none() || self.recipe.is_none() || self.opened_doc.is_none() {
            return; // waits for the scenery AND the recipe
        }
        let kinds = self.shipped_kinds();
        let (Some(doc), Some(s)) = (self.opened_doc.take(), &self.scenery) else { return };
        let (mut edits, skipped) = Edits::from_doc(&doc["scenery"], s);
        let rest = edits.load_recipe(&doc["edits"], s, &kinds);
        self.keep_edits = Value::Object(rest);
        self.keep_map = doc.get("map").filter(|m| !m.is_null()).cloned();
        if let Some(st) = self.keep_map.as_ref().and_then(|m| m.get("terrain")) {
            edits.strokes = serde_json::from_value(st.clone()).unwrap_or_default();
        }
        if let Some(m) = &self.keep_map {
            self.as_map = true;
            self.map_id = m["id"].as_str().unwrap_or_default().to_owned();
            self.map_play = m["play"].as_bool().unwrap_or(true);
        }
        if let Some(n) = doc["name"].as_str() {
            self.mod_name = n.to_owned();
        }
        let ents: Vec<String> = edits.adds.iter().map(|a| a.entity.clone()).collect();
        self.edits = edits;
        self.history.clear();
        self.dirty = true;
        self.replay_terrain();
        for e in ents {
            self.ensure_template(ctx, &e);
        }
        self.status = format!(
            "Opened {}: {} scenery, {} recipe edits, {} brush dabs",
            self.mod_id,
            self.edits.scenery_count(),
            self.edits.recipe_count(),
            self.edits.strokes.len()
        );
        if !skipped.is_empty() {
            self.status += &format!("; {} no longer match the chapter", skipped.len());
            self.failed_assets.extend(skipped);
        }
    }

    fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::State(list, mods) => {
                    self.editor_mods = mods;
                    if self.chapters.is_empty() {
                        self.status = format!("{} chapters", list.len());
                        let want = self.opts.chapter.clone().unwrap_or_else(|| "DarkHills".into());
                        let first = list.iter().find(|c| c.key == want).or(list.first()).map(|c| c.key.clone());
                        self.chapters = list;
                        if let (None, Some(k)) = (&self.chapter, first) {
                            self.open_chapter(ctx, k);
                        }
                    }
                }
                Event::Error(e) => self.status = format!("Error: {e}"),
                Event::Terrain(k, r) if Some(&k) == self.chapter.as_ref() => match r {
                    Ok(t) => {
                        self.frame_map(&t);
                        if let Some(r) = &mut self.renderer {
                            r.set_terrain(&t);
                        }
                        self.minimap_tex = Some(minimap_texture(ctx, &t));
                        self.terrain_base = Some(t.clone());
                        self.terrain = Some(t);
                        self.replay_terrain();
                    }
                    Err(e) => self.status = format!("Terrain: {e:#}"),
                },
                Event::Env(k, r) if Some(&k) == self.chapter.as_ref() => match r {
                    Ok(e) => {
                        self.env_shipped = e.clone();
                        self.env = e;
                    }
                    Err(e) => self.failed_assets.push(format!("environment: {e:#}")),
                },
                Event::Look(k, r) if Some(&k) == self.chapter.as_ref() => match r {
                    Ok(look) => {
                        self.weights_now = look.weights.clone();
                        self.color_now = look.color.clone();
                        self.look_base = Some(look.clone());
                        for t in look.textures() {
                            if self.requested.insert(format!("{t}|false")) {
                                self.loader.request(Job::Texture(t, false));
                                self.pending_assets += 1;
                            }
                        }
                        if let Some(r) = &mut self.renderer {
                            r.set_look(look);
                        }
                    }
                    Err(e) => {
                        self.look_pending = false;
                        self.failed_assets.push(format!("terrain material: {e:#}"));
                    }
                },
                Event::Scenery(k, r) if Some(&k) == self.chapter.as_ref() => match r {
                    Ok(s) => {
                        self.status = format!("{} objects, {} meshes", s.obj_xf.len(), s.parts.len());
                        let keys: Vec<PartKey> = s.parts.iter().map(|p| p.key.clone()).collect();
                        for k in &keys {
                            self.request_part_assets(k);
                        }
                        let mut s = s;
                        if let Some(r) = &self.recipe {
                            s.spots = r.spots.clone();
                        }
                        self.tree = outliner::build(&s);
                        self.scenery = Some(s);
                        self.dirty = true;
                        self.apply_opened(ctx);
                    }
                    Err(e) => self.status = format!("Scenery: {e:#}"),
                },
                Event::Recipe(k, r) if Some(&k) == self.chapter.as_ref() => match r {
                    Ok(r) => {
                        if let Some(s) = &mut self.scenery {
                            s.spots = r.spots.clone();
                        }
                        self.recipe = Some(r);
                        self.dirty = true;
                        self.apply_opened(ctx);
                    }
                    Err(e) => self.failed_assets.push(format!("recipe: {e:#}")),
                },
                Event::Pool(k, r) if Some(&k) == self.chapter.as_ref() => match r {
                    Ok(p) => self.pool = p,
                    Err(e) => self.failed_assets.push(format!("tile pool: {e:#}")),
                },
                Event::TileObjects(tile, r) => match r {
                    Ok(v) => {
                        if let Some(ts) = self.tile.as_mut().filter(|t| t.tile == tile) {
                            let v3 = |x: &Value| {
                                let a = x.as_array();
                                let f = |i: usize| a.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                                Vec3::new(f(0), f(1), f(2))
                            };
                            ts.objects = v["objects"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|o| {
                                    let q = o["quat"].as_array();
                                    let f = |i: usize, d: f64| q.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(d) as f32;
                                    let sc = if o["scale"].is_array() { v3(&o["scale"]) } else { Vec3::ONE };
                                    (o["entity"].as_str().unwrap_or_default().to_owned(), v3(&o["pos"]), Quat::from_xyzw(f(0, 0.0), f(1, 0.0), f(2, 0.0), f(3, 1.0)).normalize(), sc)
                                })
                                .collect();
                            let ents: BTreeSet<String> = ts.objects.iter().map(|o| o.0.clone()).collect();
                            for e in ents {
                                self.ensure_template(ctx, &e);
                            }
                            self.status = format!("Loading tile objects…");
                        }
                    }
                    Err(e) => {
                        self.status = format!("Tile: {e:#}");
                        self.tile = None;
                    }
                },
                Event::TileProps(r) => {
                    if let Ok(p) = r {
                        self.tile_props = p;
                        if self.tile.is_some() {
                            self.props = self.tile_props.clone();
                        }
                    }
                }
                Event::Props(k, r) if Some(&k) == self.chapter.as_ref() => match r {
                    Ok(p) => self.props = p,
                    Err(e) => self.failed_assets.push(format!("props: {e:#}")),
                },
                Event::Template(e, r) => match r {
                    Ok(t) => {
                        for (k, _) in &t {
                            let k = k.clone();
                            self.request_part_assets(&k);
                        }
                        self.templates.insert(e, Some(t));
                        self.dirty = true;
                        self.try_build_tile();
                    }
                    Err(err) => {
                        // Draw nothing for it, but do not wait on it forever.
                        self.failed_assets.push(format!("{e}: {err:#}"));
                        self.templates.insert(e, Some(Vec::new()));
                        self.try_build_tile();
                    }
                },
                Event::Opened(id, r) => match r {
                    Ok(doc) => {
                        let ch = doc["chapter"].as_str().unwrap_or_default().to_owned();
                        self.mod_id = id.clone();
                        self.remember_recent(&id);
                        if self.chapter.as_deref() != Some(&ch) || self.scenery.is_none() {
                            self.open_chapter(ctx, ch);
                            self.mod_id = id;
                            self.opened_doc = Some(doc);
                            self.restore_offer = None;
                        } else {
                            self.opened_doc = Some(doc);
                            self.apply_opened(ctx);
                        }
                    }
                    Err(e) => self.status = format!("Open {id}: {e:#}"),
                },
                _ => {} // for a chapter that is no longer open
            }
        }
        // Upload finished assets, a few milliseconds' worth per frame.
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(12) {
            let Ok(done) = self.loader.done.try_recv() else { break };
            self.pending_assets = self.pending_assets.saturating_sub(1);
            let Some(r) = &mut self.renderer else { continue };
            match done {
                Loaded::Mesh(p, Ok(m)) => r.add_mesh(p, m),
                // `_col` meshes are collision shapes: nothing to draw, not an error.
                Loaded::Mesh(p, Err(_)) if p.contains("_col.") => {}
                Loaded::Mesh(p, Err(e)) => self.failed_assets.push(format!("{p}: {e:#}")),
                Loaded::Texture(p, swap, res) => {
                    if let Err(e) = &res {
                        self.failed_assets.push(format!("{p}: {e:#}"));
                    }
                    r.add_texture(p, swap, res.ok());
                }
            }
            self.dirty = true;
        }
        if self.tile.as_ref().is_some_and(|t| !t.built) {
            self.try_build_tile();
        }
        if self.look_pending {
            if let Some(r) = &mut self.renderer {
                if r.finish_look() {
                    self.look_pending = false;
                    if self.tile.is_some() {
                        r.set_look_enabled(false); // a tile tab keeps its plain stage
                    }
                }
            }
        }
        if self.pending_assets > 0 || self.look_pending {
            ctx.request_repaint();
        }
        // A check or save finishing.
        if let Some((kind, rx)) = &self.pending {
            if let Ok(r) = rx.try_recv() {
                let saving = *kind == Pending::Save;
                self.pending = None;
                let body = r.json().unwrap_or(Value::Null);
                if r.ok() {
                    let changes: Vec<String> = body["changes"]
                        .as_array()
                        .or(body["files"].as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c.as_str().map(str::to_owned))
                        .collect();
                    if saving {
                        self.status = format!("Saved {}", body["path"].as_str().unwrap_or("the mod"));
                        self.saved_next = body["next"].as_str().map(str::to_owned);
                        self.unsaved = false;
                        let id = self.mod_id.clone();
                        self.remember_recent(&id);
                        if let Some(ch) = &self.chapter {
                            session::clear_autosave(ch);
                        }
                        self.refresh_state(ctx);
                    } else {
                        self.status = format!("Check passed: {} change(s) build cleanly", changes.len());
                    }
                    self.result = Some(Ok(changes));
                } else {
                    self.result = Some(Err(r.error_text()));
                    self.status = if saving { "Save refused — see the Save panel".into() } else { "Check failed".into() };
                }
                if self.opts.self_check || self.opts.tile_check {
                    println!("self-check body: {}", self.save_body(false).map(|b| b["scenery"].to_string()).unwrap_or_default());
                    match &self.result {
                        Some(Ok(c)) => {
                            println!("self-check OK: {c:?}");
                            std::process::exit(0);
                        }
                        Some(Err(e)) => {
                            println!("self-check FAILED: {e}");
                            std::process::exit(1);
                        }
                        None => {}
                    }
                }
            }
        }
        if self.opts.self_check && !self.self_check_sent && self.pending.is_none() {
            self.run_self_check(ctx);
        }
        if let Some(want) = self.opts.open_tile.clone() {
            if self.tile.is_none() && self.scenery.is_some() && self.terrain.is_some() && !self.pool.is_empty() {
                if let Some(t) = self.pool.iter().find(|t| t.name.contains(&want)).map(|t| t.path.clone()) {
                    self.opts.open_tile = None;
                    self.enter_tile(ctx, t, Vec3::ZERO);
                }
            }
        }
        if self.opts.tile_check && self.pending.is_none() && !self.self_check_sent {
            self.run_tile_check(ctx);
        }
        // Autosave (crash recovery) while there is unsaved work.
        if self.unsaved && self.last_autosave.elapsed() > AUTOSAVE_EVERY {
            self.autosave();
        }
    }

    fn run_tile_check(&mut self, ctx: &egui::Context) {
        match &self.tile {
            None => {
                // A camp tile: it has objects to move and remove.
                let Some(t) = self.pool.iter().find(|t| t.width >= 40).or(self.pool.first()).map(|t| t.path.clone()) else { return };
                if self.scenery.is_none() || self.terrain.is_none() {
                    return;
                }
                self.enter_tile(ctx, t, Vec3::new(10.0, 0.0, 10.0));
            }
            Some(ts) if !(ts.built && !self.props.is_empty() && self.pending_assets == 0) => {
                if ctx.input(|i| (i.time * 2.0).fract() < 0.02) {
                    let waiting = ts.objects.iter().filter(|(e, ..)| !matches!(self.templates.get(e), Some(Some(_)))).count();
                    eprintln!("tile-check waiting: built {} objects {} templates missing {} props {} assets {} status {}", ts.built, ts.objects.len(), waiting, self.props.len(), self.pending_assets, self.status);
                }
                ctx.request_repaint();
            }
            Some(ts) if ts.built && !self.props.is_empty() && self.pending_assets == 0 => {
                self.self_check_sent = true;
                let n = self.scenery.as_ref().map(|s| s.obj_xf.len()).unwrap_or(0);
                println!("tile-check: {} objects in {}", n, ts.tile);
                let xf0 = self.scenery.as_ref().map(|s| s.obj_xf[0]);
                if let Some(x) = xf0 {
                    self.set_many(vec![(Sel::Obj(0), Xf { pos: x.pos + Vec3::new(1.0, 0.0, 0.0), ..x })]);
                }
                if n > 1 {
                    self.select_only([Sel::Obj(1)]);
                    self.run(ctx, Cmd::Delete);
                }
                let prop = self.props.first().map(|p| p.entity.clone());
                if let (Some(p), Some(x)) = (prop, xf0) {
                    self.place(ctx, p, x.pos + Vec3::new(3.0, 0.0, 3.0));
                }
                println!("tile-check body: {}", self.tile_body(false).map(|b| b.0.to_string()).unwrap_or_default().chars().take(600).collect::<String>());
                self.submit(Pending::Check);
            }
            _ => {}
        }
    }

    fn run_self_check(&mut self, ctx: &egui::Context) {
        if self.recipe.is_none() || self.look_base.is_none() || self.terrain.is_none() || self.look_pending {
            return;
        }
        let (Some(s), false) = (&self.scenery, self.props.is_empty()) else { return };
        let prop = self.props.iter().find(|p| p.scenery).or(self.props.first()).map(|p| p.entity.clone());
        let a = s.obj_xf[0];
        self.self_check_sent = true;
        self.change(|e, s| {
            e.set_xf(s, Sel::Obj(0), Xf { pos: a.pos + Vec3::new(3.0, 0.0, 0.0), ..a });
            e.delete(Sel::Obj(1));
        });
        if let Some(p) = prop {
            self.place(ctx, p, a.pos + Vec3::new(6.0, 0.0, 6.0));
        }
        // Recipe: move one spot, add one, change a kind; terrain: two dabs.
        let spot = self.spots_now().into_iter().find(|p| !p.whole_map);
        let kind = self.kinds_now().into_iter().next();
        if let Some(sp) = spot {
            let to = sp.pos + Vec3::new(4.0, 0.0, -3.0);
            self.change(|e, s| {
                e.set_xf(s, Sel::Spot(sp.id), Xf { pos: to, rot: Quat::IDENTITY, scale: Vec3::ONE });
                let allow = [(String::from("Ruin"), true)].into_iter().collect();
                e.add_spot(sp.group.clone(), sp.pos + Vec3::new(20.0, 0.0, 20.0), allow);
            });
        }
        if let Some((k, mut v)) = kind {
            v.count += 1;
            self.change(|e, _| {
                e.kinds.insert(k.name.clone(), v);
            });
        }
        self.history.push(&self.edits);
        self.dab(a.pos, false, false);
        self.brush.mode = BrushMode::Paint(1);
        self.dab(a.pos + Vec3::new(5.0, 0.0, 0.0), false, false);
        self.map_id = "Map_Check".into();
        // Copy / paste / undo / redo must keep the document consistent.
        self.select_only([Sel::Obj(2)]);
        self.run(ctx, Cmd::Copy);
        self.cursor = Some(a.pos + Vec3::new(9.0, 0.0, 9.0));
        let before = self.edits.adds.len();
        self.run(ctx, Cmd::Paste);
        assert_eq!(self.edits.adds.len(), before + usize::from(!self.clipboard.is_empty()), "paste adds the copied object");
        let doc = self.edits.clone();
        self.undo();
        self.redo();
        assert_eq!(doc, self.edits, "undo/redo round trip");
        self.submit(Pending::Check);
    }

    fn remember_recent(&mut self, id: &str) {
        if id.is_empty() {
            return;
        }
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_owned());
        self.recent.truncate(10);
        self.save_prefs();
    }

    fn frame_map(&mut self, t: &Terrain) {
        let c = (t.min + t.max) * 0.5;
        let rad = (t.max - t.min).length() * 0.42;
        self.camera.frame(Vec3::new(c.x, t.hmin + (t.hmax - t.hmin) * 0.3, c.z), rad);
    }

    // ---- session ------------------------------------------------------------------

    fn sel_key(s: Sel) -> String {
        match s {
            Sel::Obj(i) => format!("o{i}"),
            Sel::Add(u) => format!("a{u}"),
            Sel::Spot(id) => format!("s{id}"),
        }
    }

    fn key_sel(k: &str) -> Option<Sel> {
        let (t, n) = k.split_at(1);
        match t {
            "o" => n.parse().ok().map(Sel::Obj),
            "a" => n.parse().ok().map(Sel::Add),
            "s" => n.parse().ok().map(Sel::Spot),
            _ => None,
        }
    }

    /// A scripted run (snapshot / self-check) must never touch the author's files.
    fn headless(&self) -> bool {
        self.opts.self_check || self.opts.tile_check || self.opts.snapshot.is_some() || self.opts.ui_snapshot.is_some()
    }

    fn autosave(&mut self) {
        self.last_autosave = Instant::now();
        if self.headless() || self.tile.is_some() {
            return;
        }
        let (Some(ch), Some(s)) = (&self.chapter, &self.scenery) else { return };
        let a = Autosave {
            chapter: ch.clone(),
            mod_id: self.mod_id.clone(),
            mod_name: self.mod_name.clone(),
            saved_at: session::now(),
            scenery: self.edits.scenery_doc(s),
            keep_edits: self.keep_edits.clone(),
            keep_map: self.keep_map.clone(),
            hidden: self.hidden.iter().map(|s| Self::sel_key(*s)).collect(),
            locked: self.locked.iter().map(|s| Self::sel_key(*s)).collect(),
            groups: self
                .groups
                .iter()
                .map(|(n, m)| Group { name: n.clone(), members: m.iter().map(|s| Self::sel_key(*s)).collect() })
                .collect(),
            recipe: Value::Object(self.edits.recipe_doc(s, &self.shipped_kinds())),
            strokes: serde_json::to_value(&self.edits.strokes).unwrap_or_default(),
        };
        match session::save_autosave(&a) {
            Ok(()) => self.status = "Autosaved".into(),
            Err(e) => self.status = format!("Autosave failed: {e}"),
        }
    }

    fn restore(&mut self, ctx: &egui::Context, a: Autosave) {
        let Some(s) = &self.scenery else { return };
        let (mut edits, _) = Edits::from_doc(&a.scenery, s);
        let kinds = self.shipped_kinds();
        edits.load_recipe(&a.recipe, s, &kinds);
        edits.strokes = serde_json::from_value(a.strokes.clone()).unwrap_or_default();
        let ents: Vec<String> = edits.adds.iter().map(|x| x.entity.clone()).collect();
        self.edits = edits;
        self.keep_edits = a.keep_edits;
        self.keep_map = a.keep_map;
        self.mod_id = a.mod_id;
        self.mod_name = a.mod_name;
        self.hidden = a.hidden.iter().filter_map(|k| Self::key_sel(k)).collect();
        self.locked = a.locked.iter().filter_map(|k| Self::key_sel(k)).collect();
        self.groups = a.groups.into_iter().map(|g| (g.name, g.members.iter().filter_map(|k| Self::key_sel(k)).collect())).collect();
        self.history.clear();
        self.unsaved = true;
        self.dirty = true;
        for e in ents {
            self.ensure_template(ctx, &e);
        }
        self.replay_terrain();
        self.status = "Restored the unsaved work".into();
    }

    // ---- edits --------------------------------------------------------------------

    fn change(&mut self, f: impl FnOnce(&mut Edits, &Scenery)) {
        let Some(s) = &self.scenery else { return };
        let before = self.edits.clone();
        f(&mut self.edits, s);
        if self.edits != before {
            self.history.push(&before);
            self.touched();
        }
    }

    fn touched(&mut self) {
        self.dirty = true;
        self.result = None;
        self.unsaved = true;
    }

    fn undo(&mut self) {
        if self.history.undo(&mut self.edits) {
            self.after_history();
        }
    }

    fn redo(&mut self) {
        if self.history.redo(&mut self.edits) {
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.touched();
        self.replay_terrain();
        self.modal = None;
        self.prune_selection();
    }

    fn prune_selection(&mut self) {
        if let Some(s) = &self.scenery {
            let e = &self.edits;
            self.sel.retain(|x| e.exists(s, *x));
            if self.active.is_some_and(|a| !self.sel.contains(&a)) {
                self.active = self.sel.iter().next_back().copied();
            }
        }
        self.dirty = true;
    }

    fn selectable(&self, sel: Sel) -> bool {
        !self.hidden.contains(&sel) && !self.locked.contains(&sel)
    }

    fn select_only(&mut self, it: impl IntoIterator<Item = Sel>) {
        self.sel = it.into_iter().collect();
        self.active = self.sel.iter().next_back().copied();
        self.dirty = true;
    }

    fn all_objects(&self) -> Vec<Sel> {
        let Some(s) = &self.scenery else { return vec![] };
        let spots: Vec<Sel> = if self.show_spots && self.tile.is_none() {
            self.edits.spots(s).into_iter().filter(|p| !p.whole_map).map(|p| Sel::Spot(p.id)).collect()
        } else {
            Vec::new()
        };
        (0..s.obj_xf.len())
            .filter(|i| !self.edits.removed.contains(i))
            .map(Sel::Obj)
            .chain(self.edits.adds.iter().map(|a| Sel::Add(a.uid)))
            .chain(spots)
            .collect()
    }

    fn pivot(&self) -> Option<Vec3> {
        let s = self.scenery.as_ref()?;
        let pts: Vec<Vec3> = self.sel.iter().filter_map(|x| self.edits.xf(s, *x)).map(|x| x.pos).collect();
        (!pts.is_empty()).then(|| pts.iter().copied().sum::<Vec3>() / pts.len() as f32)
    }

    fn entity_of(&self, sel: Sel) -> Option<(String, String)> {
        let s = self.scenery.as_ref()?;
        match sel {
            Sel::Obj(i) => Some((s.entities[s.obj_entity[i] as usize].clone(), s.levels[s.obj_level[i] as usize].clone())),
            Sel::Add(u) => self.edits.add(u).map(|a| (a.entity.clone(), a.level.clone())),
            Sel::Spot(_) => None,
        }
    }

    fn name_of(&self, sel: Sel) -> String {
        match (sel, &self.scenery) {
            (Sel::Obj(i), Some(s)) => s.object_name(i),
            (Sel::Add(u), _) => self.edits.add(u).map(Add::name).unwrap_or_default(),
            (Sel::Spot(id), Some(s)) => self
                .edits
                .spot(s, id)
                .map(|p| format!("{} spot #{}{}", p.group, if id >= NEW_SPOT { id - NEW_SPOT + 1 } else { id }, if p.new { " (new)" } else { "" }))
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// Paste `items` (entity, level, transform relative to their centre) with
    /// their centre at `at`; selects what was pasted.
    fn paste_items(&mut self, ctx: &egui::Context, items: Vec<(String, String, Xf)>, at: Vec3) -> usize {
        let allowed: HashSet<&str> = self.props.iter().map(|p| p.entity.as_str()).collect();
        let (ok, refused): (Vec<_>, Vec<_>) = items.into_iter().partition(|(e, _, _)| allowed.contains(e.as_str()));
        for (e, _, _) in &ok {
            self.ensure_template(ctx, e);
        }
        let mut new = Vec::new();
        let snap_ground = self.snap_ground;
        let ground: Vec<Option<f32>> = ok.iter().map(|(_, _, x)| self.ground_y(at.x + x.pos.x, at.z + x.pos.z)).collect();
        self.change(|e, _| {
            for ((entity, level, x), gy) in ok.into_iter().zip(ground) {
                let (yaw, _, _) = x.rot.to_euler(glam::EulerRot::YXZ);
                let mut pos = at + x.pos;
                if snap_ground {
                    if let Some(g) = gy {
                        pos.y = g + x.pos.y;
                    }
                }
                new.push(e.push_add(Add { uid: 0, entity, level, pos, yaw: yaw.to_degrees(), scale: x.scale.x.clamp(0.05, 20.0) }));
            }
        });
        let n = new.len();
        if n > 0 {
            self.select_only(new);
        }
        if !refused.is_empty() {
            self.status = format!(
                "{} object(s) skipped: this chapter's resource cache does not preload them, so the game would not show them",
                refused.len()
            );
        }
        n
    }

    fn copy_selection(&mut self) {
        let Some(p) = self.pivot() else { return };
        let Some(s) = &self.scenery else { return };
        // Heights are kept relative to the ground under each object.
        self.clipboard = self
            .sel
            .iter()
            .filter_map(|x| {
                let xf = self.edits.xf(s, *x)?;
                let (e, l) = self.entity_of(*x)?;
                let lift = xf.pos.y - self.ground_y(xf.pos.x, xf.pos.z).unwrap_or(xf.pos.y);
                Some((e, l, Xf { pos: Vec3::new(xf.pos.x - p.x, lift, xf.pos.z - p.z), ..xf }))
            })
            .collect();
        self.status = format!("Copied {} object(s)", self.clipboard.len());
    }

    fn place(&mut self, ctx: &egui::Context, entity: String, at: Vec3) {
        let Some(s) = &self.scenery else { return };
        // The level of the nearest shipped object, as the browser editor does.
        let best = (0..s.obj_xf.len())
            .min_by(|&a, &b| {
                let d = |i: usize| (s.obj_xf[i].pos - at).length_squared();
                d(a).total_cmp(&d(b))
            })
            .map(|i| s.levels[s.obj_level[i] as usize].clone());
        let Some(level) = best else { return };
        self.ensure_template(ctx, &entity);
        let mut sel = None;
        self.change(|e, _| sel = Some(e.push_add(Add { uid: 0, entity, level, pos: at, yaw: 0.0, scale: 1.0 })));
        if let Some(s) = sel {
            self.select_only([s]);
        }
    }

    fn ground_y(&self, x: f32, z: f32) -> Option<f32> {
        self.terrain.as_ref()?.height_at(x, z)
    }

    /// Apply new transforms to several objects as ONE undo step.
    fn set_many(&mut self, xfs: Vec<(Sel, Xf)>) {
        self.change(|e, s| {
            for (sel, xf) in xfs {
                e.set_xf(s, sel, xf);
            }
        });
    }

    fn set_live(&mut self, xfs: &[(Sel, Xf)]) {
        if let Some(s) = &self.scenery {
            for (sel, xf) in xfs {
                self.edits.set_xf(s, *sel, *xf);
            }
            self.touched();
        }
    }


    // ---- terrain brushes --------------------------------------------------------------

    /// Rebuild the preview ground from the vanilla grids plus every stroke
    /// (after undo / redo / open / restore).
    fn replay_terrain(&mut self) {
        let Some(base) = &self.terrain_base else { return };
        let mut t = base.clone();
        let mut w = self.look_base.as_ref().map(|l| l.weights.clone()).unwrap_or_default();
        let mut c = self.look_base.as_ref().and_then(|l| l.color.clone());
        for st in &self.edits.strokes {
            Self::apply_stroke_to(&mut t, self.look_base.as_ref(), &mut w, c.as_mut(), st);
        }
        if let Some(r) = &mut self.renderer {
            r.set_terrain(&t);
            if !w.is_empty() {
                r.update_look_maps(&w, c.as_deref());
            }
        }
        self.terrain = Some(t);
        self.weights_now = w;
        self.color_now = c;
    }

    /// Apply one stroke to the preview grids; returns height rows touched.
    fn apply_stroke_to(
        t: &mut Terrain,
        look: Option<&crate::scene::TerrainLook>,
        weights: &mut [u8],
        color: Option<&mut Vec<u8>>,
        st: &Stroke,
    ) -> Option<(usize, usize)> {
        if st.layer == brush::HEIGHT {
            return brush::apply_height(t, st);
        }
        let look = look?;
        if st.op == "tint" {
            if let Some(c) = color {
                brush::apply_tint(t, look.n, c, st);
            }
            return None;
        }
        let snapshot = t.clone();
        for (ch, src) in look.channel_sources.iter().enumerate() {
            if src.as_deref() == Some(st.layer.as_str()) {
                brush::apply_channel(&snapshot, look.n, weights, ch, st);
            }
        }
        None
    }

    /// One dab of the current brush at `at`.
    fn dab(&mut self, at: Vec3, invert: bool, smooth: bool) {
        let sources = self.look_base.as_ref().map(|l| l.channel_sources.clone()).unwrap_or_default();
        let color_layer = self.look_base.as_ref().and_then(|l| l.color_layer.clone());
        let mut b = self.brush.clone();
        if smooth {
            b.mode = BrushMode::Smooth;
        }
        let Some(st) = b.stroke(at, invert, &sources, color_layer.as_deref()) else {
            self.status = "This brush needs the chapter's terrain material (still loading?)".into();
            return;
        };
        let Some(t) = &mut self.terrain else { return };
        let rows = Self::apply_stroke_to(t, self.look_base.as_ref(), &mut self.weights_now, self.color_now.as_mut(), &st);
        if let Some(r) = &mut self.renderer {
            match rows {
                Some((r0, r1)) => r.update_terrain_rows(t, r0, r1),
                None => r.update_look_maps(&self.weights_now, self.color_now.as_deref()),
            }
        }
        self.edits.strokes.push(st);
        self.touched();
    }

    // ---- recipe: spots and the run preview --------------------------------------------

    fn kinds_now(&self) -> Vec<(recipe::Kind, KindVals)> {
        self.recipe
            .as_ref()
            .map(|r| r.kinds.iter().map(|k| (k.clone(), self.edits.kinds.get(&k.name).cloned().unwrap_or_else(|| k.vals.clone()))).collect())
            .unwrap_or_default()
    }

    fn spots_now(&self) -> Vec<recipe::SpotNow> {
        self.scenery.as_ref().map(|s| self.edits.spots(s)).unwrap_or_default()
    }

    fn roll(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.recipe else { return };
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        self.layout = recipe::sample(&self.spots_now(), &self.kinds_now(), &r.quotas, &self.pool, self.scen, self.seed);
        let paths: BTreeSet<String> = self.layout.values().cloned().collect();
        for p in paths {
            self.ensure_tile_template(ctx, &p);
        }
        self.show_tiles = true;
        self.dirty = true;
        self.status = format!("Preview: {} tiles rolled, roughly as a run would (approximate)", self.layout.len());
    }

    fn ensure_tile_template(&mut self, ctx: &egui::Context, path: &str) {
        let key = format!("tile:{path}");
        if self.templates.contains_key(&key) {
            return;
        }
        let Some(ch) = self.chapter.clone() else { return };
        self.templates.insert(key.clone(), None);
        let p = path.to_owned();
        self.spawn(ctx, move |be| {
            let t = be.get_json(&format!("/map/api/tile?chapter={}&path={}", q(&ch), q(&p))).and_then(|v| parse_template(&v));
            Ok(Event::Template(key, t))
        });
    }

    fn spot_lines(&self) -> Vec<LineVertex> {
        let mut out = Vec::new();
        if let Some(ts) = &self.tile {
            // The tile's footprint on its stage ("40x40_…" = 40 m square).
            let size = tile_name(&ts.tile).split('_').next().map(recipe::group_size).unwrap_or(40.0);
            let h = size * 0.5;
            let col = [1.0, 0.75, 0.3, 0.9];
            let c = [(-h, -h), (h, -h), (h, h), (-h, h)];
            for i in 0..4 {
                let (a, b) = (c[i], c[(i + 1) % 4]);
                out.push(LineVertex { pos: [a.0, 0.05, a.1], col });
                out.push(LineVertex { pos: [b.0, 0.05, b.1], col });
            }
            return out;
        }
        if !self.show_spots {
            return out;
        }
        let kinds = self.kinds_now();
        let focus = self.kind_focus.as_ref().and_then(|f| kinds.iter().find(|(k, _)| &k.name == f));
        for sp in self.spots_now() {
            if sp.whole_map || self.hidden.contains(&Sel::Spot(sp.id)) {
                continue;
            }
            let sel = self.sel.contains(&Sel::Spot(sp.id));
            let mut col = recipe::group_color(&sp.group);
            if let Some((k, v)) = focus {
                col = match recipe::eligible(&sp, k, v, self.scen) {
                    "yes" => [0.3, 1.0, 0.45, 1.0],
                    "blocked" => [1.0, 0.75, 0.2, 0.8],
                    _ => [0.5, 0.5, 0.5, 0.35],
                };
            }
            if sel {
                col = [1.0, 0.62, 0.18, 1.0];
            }
            let h = recipe::group_size(&sp.group) * 0.5;
            let y = |x: f32, z: f32| self.ground_y(x, z).unwrap_or(sp.pos.y) + 0.25;
            let corners = [(-h, -h), (h, -h), (h, h), (-h, h)];
            for i in 0..4 {
                let (a, b) = (corners[i], corners[(i + 1) % 4]);
                let steps = ((h / 3.0) as usize).clamp(2, 16);
                for t in 0..steps {
                    let f0 = t as f32 / steps as f32;
                    let f1 = (t + 1) as f32 / steps as f32;
                    let p = |f: f32| {
                        let (x, z) = (sp.pos.x + a.0 + (b.0 - a.0) * f, sp.pos.z + a.1 + (b.1 - a.1) * f);
                        [x, y(x, z), z]
                    };
                    out.push(LineVertex { pos: p(f0), col });
                    out.push(LineVertex { pos: p(f1), col });
                }
            }
            // A post in the middle so a spot reads from far away.
            let gy = y(sp.pos.x, sp.pos.z);
            out.push(LineVertex { pos: [sp.pos.x, gy, sp.pos.z], col });
            out.push(LineVertex { pos: [sp.pos.x, gy + 2.0 + h * 0.15, sp.pos.z], col });
        }
        out
    }

    /// The spot under a screen ray (smallest first, so a 3x3 inside a camp wins).
    fn pick_spot(&self, o: Vec3, d: Vec3) -> Option<Sel> {
        if !self.show_spots || self.tile.is_some() {
            return None;
        }
        let g = self.terrain.as_ref().and_then(|t| t.raycast(o, d, 9000.0))?;
        self.spots_now()
            .into_iter()
            .filter(|sp| !sp.whole_map && !self.hidden.contains(&Sel::Spot(sp.id)) && !self.locked.contains(&Sel::Spot(sp.id)))
            .filter(|sp| {
                let h = recipe::group_size(&sp.group) * 0.5;
                (g.x - sp.pos.x).abs() <= h && (g.z - sp.pos.z).abs() <= h
            })
            .min_by(|a, b| recipe::group_size(&a.group).total_cmp(&recipe::group_size(&b.group)))
            .map(|sp| Sel::Spot(sp.id))
    }

    /// Objects first, then spots — or spots first while the Spots tab is open.
    fn pick_any(&self, o: Vec3, d: Vec3) -> Option<Sel> {
        if self.left_tab == LeftTab::Spots {
            self.pick_spot(o, d).or_else(|| self.pick(o, d))
        } else {
            self.pick(o, d).or_else(|| self.pick_spot(o, d))
        }
    }

    // ---- tabs and tile (POI) editing ------------------------------------------------------

    /// Everything that belongs to the shown tab, taken out (the tab is left empty).
    fn park(&mut self) -> Stash {
        Stash {
            scenery: self.scenery.take(),
            edits: std::mem::take(&mut self.edits),
            history: std::mem::take(&mut self.history),
            sel: std::mem::take(&mut self.sel),
            active: self.active.take(),
            hidden: std::mem::take(&mut self.hidden),
            locked: std::mem::take(&mut self.locked),
            groups: std::mem::take(&mut self.groups),
            tree: std::mem::take(&mut self.tree),
            props: std::mem::take(&mut self.props),
            mod_id: std::mem::take(&mut self.mod_id),
            mod_name: std::mem::take(&mut self.mod_name),
            unsaved: std::mem::replace(&mut self.unsaved, false),
            result: self.result.take(),
            terrain: self.terrain.take(),
            camera: self.camera,
            tile: self.tile.take(),
        }
    }

    fn unpark(&mut self, st: Stash) {
        self.scenery = st.scenery;
        self.edits = st.edits;
        self.history = st.history;
        self.sel = st.sel;
        self.active = st.active;
        self.hidden = st.hidden;
        self.locked = st.locked;
        self.groups = st.groups;
        self.tree = st.tree;
        self.props = st.props;
        self.mod_id = st.mod_id;
        self.mod_name = st.mod_name;
        self.unsaved = st.unsaved;
        self.result = st.result;
        self.terrain = st.terrain;
        self.camera = st.camera;
        self.tile = st.tile;
    }

    /// Show tab `i`; the ground is re-uploaded for what it shows.
    fn switch_tab(&mut self, i: usize) {
        if i == self.active_tab || i >= self.tabs.len() {
            return;
        }
        self.modal = None;
        self.box_start = None;
        self.tool = Tool::Select;
        let cur = self.park();
        self.tabs[self.active_tab].parked = Some(cur);
        self.active_tab = i;
        if let Some(st) = self.tabs[i].parked.take() {
            self.unpark(st);
        }
        if self.tile.is_none() {
            // Back on the map: its terrain, material and water.
            self.replay_terrain();
            if let Some(r) = &mut self.renderer {
                r.set_look_enabled(true);
            }
        } else {
            self.stage_ground();
        }
        self.dirty = true;
    }

    /// A tile stands alone on plain ground sized to it.
    fn stage_ground(&mut self) {
        let half = self
            .tile
            .as_ref()
            .map(|ts| ts.objects.iter().map(|o| o.1.x.abs().max(o.1.z.abs())).fold(20.0f32, f32::max) + 15.0)
            .unwrap_or(40.0);
        let flat = Terrain::flat(half);
        if let Some(r) = &mut self.renderer {
            r.set_terrain(&flat);
            r.set_look_enabled(false);
        }
        self.terrain = Some(flat);
    }

    /// Open `tile` in its own tab (or show its tab if already open).
    fn enter_tile(&mut self, ctx: &egui::Context, tile: String, _near: Vec3) {
        if let Some(i) = self.tabs.iter().position(|t| t.tile.as_deref() == Some(&tile)) {
            self.switch_tab(i);
            return;
        }
        let name = tile_name(&tile);
        self.tabs.push(Tab { title: name.replace('_', " "), tile: Some(tile.clone()), parked: None });
        let i = self.tabs.len() - 1;
        // An empty tab to switch into, then fill it.
        self.tabs[i].parked = Some(Stash {
            scenery: None,
            edits: Edits::default(),
            history: History::default(),
            sel: BTreeSet::new(),
            active: None,
            hidden: HashSet::new(),
            locked: HashSet::new(),
            groups: Vec::new(),
            tree: Vec::new(),
            props: self.tile_props.clone(),
            mod_id: format!("{}-poi", name.to_lowercase().replace('_', "-")),
            mod_name: String::new(),
            unsaved: false,
            result: None,
            terrain: None,
            camera: Camera { pitch: 0.85, ..Camera::default() },
            tile: Some(TileSession { tile: tile.clone(), origin: Vec3::ZERO, objects: Vec::new(), built: false }),
        });
        self.switch_tab(i);
        self.status = format!("Opening tile {name}…");
        let t = tile.clone();
        self.spawn(ctx, move |be| {
            let r = be.get_json(&format!("/map/api/t3/objects?tile={}", q(&t)));
            Ok(Event::TileObjects(t, r))
        });
        if self.tile_props.is_empty() {
            self.spawn(ctx, move |be| {
                let r = be.get_json("/map/api/t3/props").map(|v| {
                    v["props"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| {
                            Some(Prop {
                                entity: p["entity"].as_str()?.to_owned(),
                                name: p["name"].as_str().unwrap_or_default().to_owned(),
                                group: p["group"].as_str().unwrap_or_default().to_owned(),
                                scenery: true,
                            })
                        })
                        .collect()
                });
                Ok(Event::TileProps(r))
            });
        }
    }

    /// Once every entity template is in: build the shown tile as a scene.
    fn try_build_tile(&mut self) {
        let Some(ts) = &self.tile else { return };
        if ts.built || ts.objects.is_empty() {
            return;
        }
        if ts.objects.iter().any(|(e, ..)| !matches!(self.templates.get(e), Some(Some(_)))) {
            return;
        }
        let tile_path = ts.tile.clone();
        let origin = Mat4::from_translation(ts.origin);
        let mut entities: Vec<String> = Vec::new();
        let (mut lvl, mut ids, mut ents, mut xfs) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut parts: Vec<crate::scene::ScPart> = Vec::new();
        let mut index: HashMap<PartKey, usize> = HashMap::new();
        for (i, (e, pos, rot, scale)) in ts.objects.iter().enumerate() {
            let ei = entities.iter().position(|x| x == e).unwrap_or_else(|| {
                entities.push(e.clone());
                entities.len() - 1
            });
            lvl.push(0u16);
            ids.push(i as u32);
            ents.push(ei as u32);
            xfs.push(Xf { pos: *pos + ts.origin, rot: *rot, scale: *scale });
            let world = origin * Xf { pos: *pos, rot: *rot, scale: *scale }.mat();
            if let Some(Some(t)) = self.templates.get(e) {
                for (key, mats) in t {
                    let pi = *index.entry(key.clone()).or_insert_with(|| {
                        parts.push(crate::scene::ScPart { key: key.clone(), base: Vec::new(), owners: Vec::new() });
                        parts.len() - 1
                    });
                    for m in mats {
                        parts[pi].base.push(world * *m);
                        parts[pi].owners.push(i as u32);
                    }
                }
            }
        }
        let mut obj_instances = vec![Vec::new(); xfs.len()];
        for (pi, p) in parts.iter().enumerate() {
            for (j, ow) in p.owners.iter().enumerate() {
                obj_instances[*ow as usize].push((pi as u32, j as u32));
            }
        }
        // Frame the whole tile, not one corner of it.
        let n = xfs.len().max(1) as f32;
        let center = xfs.iter().map(|x: &Xf| x.pos).sum::<Vec3>() / n;
        let radius = xfs.iter().map(|x| (x.pos - center).length()).fold(10.0f32, f32::max);
        let scene = Scenery {
            levels: vec![tile_path.clone()],
            entities,
            obj_level: lvl,
            obj_id: ids,
            obj_entity: ents,
            obj_xf: xfs,
            parts,
            obj_instances,
            spots: Vec::new(),
        };
        let keys: Vec<PartKey> = scene.parts.iter().map(|p| p.key.clone()).collect();
        for k in &keys {
            self.request_part_assets(k);
        }
        self.tree = outliner::build(&scene);
        let count = scene.obj_xf.len();
        self.scenery = Some(scene);
        self.props = self.tile_props.clone();
        if let Some(ts) = &mut self.tile {
            ts.built = true;
        }
        self.stage_ground();
        self.camera.target = center.with_y(0.0);
        self.camera.pitch = 0.85;
        self.camera.frame(center.with_y(0.0), radius * 1.15);
        self.left_tab = LeftTab::Outliner;
        self.dirty = true;
        self.status = format!("Tile {}: {count} objects", tile_name(&tile_path));
    }

    /// Close tab `i` (never the map).
    fn close_tab(&mut self, i: usize) {
        if i == 0 || i >= self.tabs.len() {
            return;
        }
        if i == self.active_tab {
            self.switch_tab(0);
        }
        self.tabs.remove(i);
        if self.active_tab > i {
            self.active_tab -= 1;
        }
        self.close_confirm = None;
    }

    fn exit_tile(&mut self) {
        let i = self.active_tab;
        self.close_tab(i);
    }

    /// The tile editor's save body (`tile_editor.clean_edit` form).
    fn tile_body(&self, with_id: bool) -> Option<(Value, Vec<String>)> {
        let ts = self.tile.as_ref()?;
        let s = self.scenery.as_ref()?;
        let mut notes = Vec::new();
        let r3 = |v: Vec3| json!([v.x, v.y, v.z]);
        let places: Vec<Value> = self
            .edits
            .adds
            .iter()
            .map(|a| json!({"entity": a.entity, "pos": r3(a.pos - ts.origin), "yaw": a.yaw, "scale": a.scale}))
            .collect();
        let mut moves = Vec::new();
        for (&i, xf) in &self.edits.moves {
            let (e, p0, r0, s0) = &ts.objects[i];
            if (xf.rot.dot(*r0)).abs() < 0.9999 || (xf.scale - *s0).length() > 1e-3 {
                notes.push(format!("{}: a tile object can be moved, not rotated or scaled — only the move is saved", self.name_of(Sel::Obj(i))));
            }
            moves.push(json!({"entity": e, "at": r3(*p0), "pos": r3(xf.pos - ts.origin)}));
        }
        let removes: Vec<Value> = self.edits.removed.iter().map(|&i| json!({"entity": ts.objects[i].0, "at": r3(ts.objects[i].1)})).collect();
        let own = !moves.is_empty() || !removes.is_empty();
        let mut b = json!({"tile": ts.tile, "places": places, "own": own});
        if own {
            b["copies"] = json!(1);
            b["clear"] = json!(false);
            b["remove"] = Value::Array(removes);
            b["move"] = Value::Array(moves);
        }
        if with_id {
            b["mod_id"] = json!(self.mod_id.trim().to_lowercase());
            b["name"] = json!(self.mod_name.trim());
        }
        let _ = s;
        Some((b, notes))
    }

    // ---- commands -----------------------------------------------------------------

    fn run(&mut self, ctx: &egui::Context, cmd: Cmd) {
        match cmd {
            Cmd::New => {
                self.show_save_as = false;
                self.status = "File > New map: pick the chapter to start from".into();
            }
            Cmd::Save => {
                if self.mod_id.trim().is_empty() {
                    self.show_save_as = true;
                } else {
                    self.submit(Pending::Save);
                }
            }
            Cmd::SaveAs => self.show_save_as = true,
            Cmd::Check => self.submit(Pending::Check),
            Cmd::Undo => self.undo(),
            Cmd::Redo => self.redo(),
            Cmd::Copy => self.copy_selection(),
            Cmd::Cut => {
                self.copy_selection();
                self.run(ctx, Cmd::Delete);
            }
            Cmd::Paste => {
                let at = self.cursor.or_else(|| Some(self.camera.target)).unwrap();
                let items = self.clipboard.clone();
                let n = self.paste_items(ctx, items, at);
                if n > 0 {
                    self.status = format!("Pasted {n} object(s)");
                }
            }
            Cmd::Duplicate => {
                let Some(p) = self.pivot() else { return };
                self.copy_selection();
                let items = self.clipboard.clone();
                if self.paste_items(ctx, items, p + Vec3::new(1.0, 0.0, 1.0)) > 0 {
                    // Blender: the copy follows the mouse until a click.
                    self.begin(ctx, Kind::Grab, false);
                }
            }
            Cmd::Delete => {
                let sel: Vec<Sel> = self.sel.iter().copied().filter(|s| !self.locked.contains(s)).collect();
                self.change(|e, _| {
                    for s in sel {
                        e.delete(s);
                    }
                });
                self.prune_selection();
            }
            Cmd::SelectAll => {
                let all: Vec<Sel> = self.all_objects().into_iter().filter(|s| self.selectable(*s)).collect();
                self.select_only(all);
            }
            Cmd::DeselectAll => self.select_only([]),
            Cmd::Invert => {
                let all: Vec<Sel> =
                    self.all_objects().into_iter().filter(|s| self.selectable(*s) && !self.sel.contains(s)).collect();
                self.select_only(all);
            }
            Cmd::Hide => {
                self.hidden.extend(self.sel.iter().copied());
                self.select_only([]);
                self.unsaved = true;
            }
            Cmd::HideUnselected => {
                let others: Vec<Sel> = self.all_objects().into_iter().filter(|s| !self.sel.contains(s)).collect();
                self.hidden.extend(others);
                self.dirty = true;
            }
            Cmd::UnhideAll => {
                self.hidden.clear();
                self.dirty = true;
            }
            Cmd::Lock => {
                self.locked.extend(self.sel.iter().copied());
                self.select_only([]);
            }
            Cmd::UnlockAll => self.locked.clear(),
            Cmd::Group => {
                if !self.sel.is_empty() {
                    let name = format!("Group {}", self.groups.len() + 1);
                    self.groups.push((name.clone(), self.sel.iter().copied().collect()));
                    self.status = format!("{name}: {} object(s)", self.sel.len());
                    self.left_tab = LeftTab::Outliner;
                    self.unsaved = true;
                }
            }
            Cmd::Grab => self.begin(ctx, Kind::Grab, false),
            Cmd::Rotate => self.begin(ctx, Kind::Rotate, false),
            Cmd::Scale => self.begin(ctx, Kind::Scale, false),
            Cmd::DropToGround => {
                let Some(s) = &self.scenery else { return };
                let xfs: Vec<(Sel, Xf)> = self
                    .sel
                    .iter()
                    .filter_map(|x| {
                        let xf = self.edits.xf(s, *x)?;
                        let y = self.ground_y(xf.pos.x, xf.pos.z)?;
                        Some((*x, Xf { pos: Vec3::new(xf.pos.x, y, xf.pos.z), ..xf }))
                    })
                    .collect();
                self.set_many(xfs);
            }
            Cmd::Align(axis, side) => {
                let Some(s) = &self.scenery else { return };
                let cur: Vec<(Sel, Xf)> = self.sel.iter().filter_map(|x| Some((*x, self.edits.xf(s, *x)?))).collect();
                if cur.len() < 2 {
                    self.status = "Align needs two or more objects selected".into();
                    return;
                }
                let vals: Vec<f32> = cur.iter().map(|(_, x)| x.pos[axis]).collect();
                let (lo, hi) = vals.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
                let to = match side {
                    -1 => lo,
                    1 => hi,
                    _ => vals.iter().sum::<f32>() / vals.len() as f32,
                };
                let xfs = cur
                    .into_iter()
                    .map(|(sel, mut x)| {
                        x.pos[axis] = to;
                        (sel, x)
                    })
                    .collect();
                self.set_many(xfs);
            }
            Cmd::Distribute(axis) => {
                let Some(s) = &self.scenery else { return };
                let mut cur: Vec<(Sel, Xf)> = self.sel.iter().filter_map(|x| Some((*x, self.edits.xf(s, *x)?))).collect();
                if cur.len() < 3 {
                    self.status = "Distribute needs three or more objects selected".into();
                    return;
                }
                cur.sort_by(|a, b| a.1.pos[axis].total_cmp(&b.1.pos[axis]));
                let (lo, hi) = (cur[0].1.pos[axis], cur[cur.len() - 1].1.pos[axis]);
                let n = cur.len() - 1;
                let xfs = cur
                    .into_iter()
                    .enumerate()
                    .map(|(k, (sel, mut x))| {
                        x.pos[axis] = lo + (hi - lo) * k as f32 / n as f32;
                        (sel, x)
                    })
                    .collect();
                self.set_many(xfs);
            }
            Cmd::ResetTransform => {
                let objs: Vec<usize> = self.sel.iter().filter_map(|s| if let Sel::Obj(i) = s { Some(*i) } else { None }).collect();
                self.change(|e, _| {
                    for i in objs {
                        e.moves.remove(&i);
                    }
                });
            }
            Cmd::FrameAll => {
                if let Some(t) = self.terrain.take() {
                    self.frame_map(&t);
                    self.terrain = Some(t);
                }
            }
            Cmd::FrameSelected => {
                if let Some(p) = self.pivot() {
                    self.camera.target = p;
                    self.camera.dist = self.camera.dist.min(60.0);
                }
            }
            Cmd::ViewFront => self.camera.view_front(),
            Cmd::ViewSide => self.camera.view_side(),
            Cmd::ViewTop => self.camera.view_top(),
            Cmd::ToggleOrtho => self.camera.ortho = !self.camera.ortho,
            Cmd::ToggleGrid => self.show_grid = !self.show_grid,
            Cmd::ToggleSnap => self.snap = !self.snap,
            Cmd::ToggleShadows => self.shadows = !self.shadows,
            Cmd::ToggleMinimap => self.show_minimap = !self.show_minimap,
            Cmd::ToggleEnv => self.show_env = !self.show_env,
            Cmd::ToggleLog => self.show_log = !self.show_log,
            Cmd::ToggleTheme => {
                self.light_theme = !self.light_theme;
                style(ctx, self.light_theme);
                self.save_prefs();
            }
            Cmd::Palette => {
                self.show_palette = !self.show_palette;
                self.palette_query.clear();
            }
            Cmd::Shortcuts => self.show_shortcuts = !self.show_shortcuts,
            Cmd::ToolMove => self.gizmo = GizmoMode::Move,
            Cmd::ToolRotate => self.gizmo = GizmoMode::Rotate,
            Cmd::ToolScale => self.gizmo = GizmoMode::Scale,
        }
        self.dirty = true;
    }

    // ---- modal transforms -----------------------------------------------------------

    /// Start G / R / S on the whole selection (locked objects stay put).
    fn begin(&mut self, ctx: &egui::Context, kind: Kind, by_drag: bool) {
        self.begin_locked(ctx, kind, Lock::Free, by_drag);
    }

    fn begin_locked(&mut self, ctx: &egui::Context, kind: Kind, lock: Lock, by_drag: bool) {
        let (Some(s), Some(pivot)) = (&self.scenery, self.pivot()) else { return };
        let starts: Vec<(Sel, Xf)> = self
            .sel
            .iter()
            .filter(|x| !self.locked.contains(x))
            .filter_map(|x| Some((*x, self.edits.xf(s, *x)?)))
            .collect();
        if starts.is_empty() {
            return;
        }
        let p0 = ctx.input(|i| i.pointer.latest_pos()).unwrap_or_default();
        self.history.push(&self.edits);
        self.modal = Some(Modal { kind, lock, starts, pivot, p0, g0: self.cursor, by_drag, typed: String::new() });
    }

    fn cancel_modal(&mut self) {
        if let Some(m) = self.modal.take() {
            self.set_live(&m.starts);
            // The history entry pushed when the transform began is now a no-op.
            self.undo();
        }
    }

    /// The new transforms for the current pointer / typed value.
    #[allow(clippy::too_many_arguments)]
    fn modal_xfs(
        &self,
        m: &Modal,
        pointer: egui::Pos2,
        ground: Option<Vec3>,
        pivot_scr: Option<egui::Pos2>,
        axis_px: [egui::Vec2; 3],
        snap: bool,
    ) -> Vec<(Sel, Xf)> {
        let typed: Option<f32> = m.typed.parse().ok();
        let mask = m.lock.mask();
        let drag = pointer - m.p0;
        // Metres along screen-projected axes: the mouse movement is projected
        // onto the axis as it appears on screen (Blender), so a locked move
        // stays exactly on its axis from any view, Y included.
        let along = |a: usize| {
            let v = axis_px[a];
            let l2 = v.length_sq();
            if l2 < 1e-6 { 0.0 } else { drag.dot(v) / l2 }
        };
        match m.kind {
            Kind::Grab => {
                let mut d = match m.lock {
                    Lock::Free => match (ground, m.g0) {
                        (Some(g), Some(g0)) => Vec3::new(g.x - g0.x, 0.0, g.z - g0.z),
                        _ => Vec3::ZERO,
                    },
                    Lock::Axis(a) => {
                        let mut d = Vec3::ZERO;
                        d[a] = along(a);
                        d
                    }
                    Lock::Plane(1) => match (ground, m.g0) {
                        (Some(g), Some(g0)) => Vec3::new(g.x - g0.x, 0.0, g.z - g0.z),
                        _ => Vec3::ZERO,
                    },
                    Lock::Plane(skip) => {
                        // Two free axes: solve drag = s*axis_i + t*axis_j on screen.
                        let (i, j) = if skip == 0 { (1, 2) } else { (0, 1) };
                        let (vi, vj) = (axis_px[i], axis_px[j]);
                        let det = vi.x * vj.y - vi.y * vj.x;
                        let mut d = Vec3::ZERO;
                        if det.abs() > 1e-6 {
                            d[i] = (drag.x * vj.y - drag.y * vj.x) / det;
                            d[j] = (vi.x * drag.y - vi.y * drag.x) / det;
                        }
                        d
                    }
                };
                if let Some(v) = typed {
                    let axis = match m.lock {
                        Lock::Axis(a) => a,
                        _ => 0,
                    };
                    d = Vec3::ZERO;
                    d[axis] = v;
                }
                d *= mask;
                if snap && typed.is_none() {
                    let g = self.grid_size.max(0.01);
                    d = (d / g).round() * g;
                }
                let follow = self.snap_ground && m.lock == Lock::Free && typed.is_none();
                m.starts
                    .iter()
                    .map(|(sel, st)| {
                        let mut pos = st.pos + d;
                        if follow {
                            let lift = st.pos.y - self.ground_y(st.pos.x, st.pos.z).unwrap_or(st.pos.y);
                            if let Some(gy) = self.ground_y(pos.x, pos.z) {
                                pos.y = gy + lift;
                            }
                        }
                        (*sel, Xf { pos, ..*st })
                    })
                    .collect()
            }
            Kind::Rotate => {
                let mut a = match pivot_scr {
                    Some(c) => -((pointer - c).angle() - (m.p0 - c).angle()),
                    None => 0.0,
                };
                if let Some(v) = typed {
                    a = v.to_radians();
                }
                if snap && typed.is_none() {
                    let st = 15f32.to_radians();
                    a = (a / st).round() * st;
                }
                let r = match m.lock {
                    Lock::Axis(0) => Quat::from_rotation_x(a),
                    Lock::Axis(2) => Quat::from_rotation_z(a),
                    _ => Quat::from_rotation_y(a),
                };
                m.starts
                    .iter()
                    .map(|(sel, st)| (*sel, Xf { pos: m.pivot + r * (st.pos - m.pivot), rot: r * st.rot, ..*st }))
                    .collect()
            }
            Kind::Scale => {
                let mut k = match pivot_scr {
                    Some(c) => (pointer - c).length() / (m.p0 - c).length().max(4.0),
                    None => 1.0,
                };
                if let Some(v) = typed {
                    k = v;
                }
                if snap && typed.is_none() {
                    k = (k * 10.0).round() / 10.0;
                }
                let kv = Vec3::ONE + (Vec3::splat(k) - Vec3::ONE) * mask;
                m.starts
                    .iter()
                    .map(|(sel, st)| {
                        let scale = (st.scale * kv).clamp(Vec3::splat(0.05), Vec3::splat(20.0));
                        (*sel, Xf { pos: m.pivot + (st.pos - m.pivot) * kv, scale, ..*st })
                    })
                    .collect()
            }
        }
    }

    // ---- drawing caches -------------------------------------------------------------

    fn rebuild(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(s) = &self.scenery else { return };
        let renderer = self.renderer.as_ref();
        let meshes_ready = |mesh: &str| renderer.is_some_and(|r| r.has_mesh(mesh));
        self.pickables.clear();
        let tint_for = |sel: Sel, base: [f32; 4]| {
            if self.active == Some(sel) {
                ACTIVE_TINT
            } else if self.sel.contains(&sel) {
                SELECT_TINT
            } else {
                base
            }
        };
        let deltas: HashMap<usize, Mat4> =
            self.edits.moves.iter().map(|(&i, xf)| (i, xf.mat() * s.obj_xf[i].mat().inverse())).collect();
        let mut shipped = Vec::with_capacity(s.parts.len());
        for p in &s.parts {
            let mut v = Vec::with_capacity(p.base.len());
            for (j, base) in p.base.iter().enumerate() {
                let ow = p.owners[j] as usize;
                let sel = Sel::Obj(ow);
                if self.edits.removed.contains(&ow) || self.hidden.contains(&sel) {
                    continue;
                }
                let (m, tint) = match deltas.get(&ow) {
                    Some(d) => (*d * *base, MOVED_TINT),
                    None => (*base, [0.0; 4]),
                };
                v.push(Instance { m: m.to_cols_array_2d(), tint: tint_for(sel, tint) });
                if !p.key.decal && meshes_ready(&p.key.mesh) && !self.locked.contains(&sel) {
                    self.pickables.push((p.key.mesh.clone(), m, sel));
                }
            }
            shipped.push(v);
        }
        self.shipped_draw = shipped;
        let mut by_key: HashMap<PartKey, Vec<Instance>> = HashMap::new();
        for a in &self.edits.adds {
            let sel = Sel::Add(a.uid);
            if self.hidden.contains(&sel) {
                continue;
            }
            let Some(Some(t)) = self.templates.get(&a.entity) else { continue };
            let world = a.xf().mat();
            for (key, mats) in t {
                for local in mats {
                    let m = world * *local;
                    by_key.entry(key.clone()).or_default().push(Instance { m: m.to_cols_array_2d(), tint: tint_for(sel, ADDED_TINT) });
                    if !key.decal && meshes_ready(&key.mesh) && !self.locked.contains(&sel) {
                        self.pickables.push((key.mesh.clone(), m, sel));
                    }
                }
            }
        }
        // The run preview: rolled tiles standing on their spots.
        if self.show_tiles && self.tile.is_none() {
            for (id, path) in &self.layout {
                let Some(sp) = self.edits.spot(s, *id) else { continue };
                if self.hidden.contains(&Sel::Spot(*id)) {
                    continue;
                }
                let Some(Some(t)) = self.templates.get(&format!("tile:{path}")) else { continue };
                let gy = self.terrain.as_ref().and_then(|t| t.height_at(sp.pos.x, sp.pos.z)).unwrap_or(sp.pos.y);
                let world = Mat4::from_translation(Vec3::new(sp.pos.x, gy, sp.pos.z));
                let sel = Sel::Spot(*id);
                for (key, mats) in t {
                    for local in mats {
                        let m = world * *local;
                        by_key.entry(key.clone()).or_default().push(Instance { m: m.to_cols_array_2d(), tint: tint_for(sel, [0.0; 4]) });
                        if !key.decal && meshes_ready(&key.mesh) && !self.locked.contains(&sel) {
                            self.pickables.push((key.mesh.clone(), m, sel));
                        }
                    }
                }
            }
        }
        self.add_draw = by_key.into_iter().collect();
    }

    /// The selectable thing under a screen ray: bounding spheres first, then
    /// the triangles of each candidate, nearest first.
    fn pick(&self, o: Vec3, d: Vec3) -> Option<Sel> {
        let r = self.renderer.as_ref()?;
        let mut cands: Vec<(f32, usize)> = Vec::new();
        for (k, (mesh, m, _)) in self.pickables.iter().enumerate() {
            let Some(g) = r.meshes.get(mesh) else { continue };
            let c = m.transform_point3(g.center);
            let sc = m.x_axis.truncate().length().max(m.y_axis.truncate().length()).max(m.z_axis.truncate().length());
            let rad = g.radius * sc;
            let oc = c - o;
            let tca = oc.dot(d);
            let d2 = oc.length_squared() - tca * tca;
            if d2 > rad * rad || tca + rad < 0.0 {
                continue;
            }
            cands.push(((tca - (rad * rad - d2).sqrt()).max(0.0), k));
        }
        cands.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut best: Option<(Sel, f32)> = None;
        for (t_in, k) in cands {
            if best.is_some_and(|(_, bt)| t_in > bt) {
                break;
            }
            let (mesh, m, sel) = &self.pickables[k];
            let g = &r.meshes[mesh];
            let inv = m.inverse();
            let (lo, ld) = (inv.transform_point3(o), inv.transform_vector3(d));
            if let Some(tl) = ray_mesh(lo, ld, &g.cpu.vertices, &g.cpu.indices) {
                let t = (m.transform_point3(lo + ld * tl) - o).length();
                if best.is_none_or(|(_, bt)| t < bt) {
                    best = Some((*sel, t));
                }
            }
        }
        best.map(|(s, _)| s)
    }

    fn selection_lines(&self) -> Vec<LineVertex> {
        let Some(r) = &self.renderer else { return vec![] };
        let mut out = Vec::new();
        for (mesh, m, s) in &self.pickables {
            if !self.sel.contains(s) {
                continue;
            }
            if let Some(g) = r.meshes.get(mesh) {
                let (lo, hi) = aabb(&g.cpu.vertices);
                let col = if self.active == Some(*s) { [1.0, 0.85, 0.4, 0.95] } else { [1.0, 0.6, 0.2, 0.8] };
                box_lines(&mut out, *m, lo, hi, col);
            }
        }
        out
    }

    fn grid_lines(&self) -> Vec<LineVertex> {
        let mut out = Vec::new();
        if !self.show_grid {
            return out;
        }
        // A grid around the focus, cells of `grid_size` (every 10th brighter),
        // laid on the ground so it reads on slopes too.
        let g = self.grid_size.max(0.1);
        let cells = ((self.camera.dist * 0.9 / g) as i32).clamp(8, 120);
        let c = self.camera.target;
        let (cx, cz) = ((c.x / g).round() * g, (c.z / g).round() * g);
        let half = cells as f32 * g;
        let y = |x: f32, z: f32| self.ground_y(x, z).unwrap_or(c.y) + 0.06;
        let steps = 24;
        for k in -cells..=cells {
            let major = k.rem_euclid(10) == 0;
            let col = if major { [1.0, 1.0, 1.0, 0.22] } else { [1.0, 1.0, 1.0, 0.07] };
            let off = k as f32 * g;
            for (ax, bz) in [(true, false), (false, true)] {
                for t in 0..steps {
                    let a = -half + 2.0 * half * t as f32 / steps as f32;
                    let b = -half + 2.0 * half * (t + 1) as f32 / steps as f32;
                    let (p, q) = if ax {
                        ((cx + off, cz + a), (cx + off, cz + b))
                    } else {
                        let _ = bz;
                        ((cx + a, cz + off), (cx + b, cz + off))
                    };
                    out.push(LineVertex { pos: [p.0, y(p.0, p.1), p.1], col });
                    out.push(LineVertex { pos: [q.0, y(q.0, q.1), q.1], col });
                }
            }
        }
        out
    }

    // ---- viewport -------------------------------------------------------------------

    fn ui_viewport(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        let ppp = ctx.pixels_per_point();
        let size = ((rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32);
        let aspect = rect.width() / rect.height().max(1.0);
        let cam = self.camera;
        let ray_at = |p: egui::Pos2| {
            let x = (p.x - rect.left()) / rect.width() * 2.0 - 1.0;
            let y = 1.0 - (p.y - rect.top()) / rect.height() * 2.0;
            cam.ray(x, y, aspect)
        };
        let vp = cam.proj(aspect) * cam.view();
        let to_screen = |p: Vec3| -> Option<egui::Pos2> {
            let c = vp * p.extend(1.0);
            (c.w > 0.0).then(|| {
                egui::pos2(rect.left() + (c.x / c.w * 0.5 + 0.5) * rect.width(), rect.top() + (0.5 - c.y / c.w * 0.5) * rect.height())
            })
        };
        let pointer = resp.hover_pos().or(ctx.input(|i| i.pointer.latest_pos()));
        let ground_at = |terrain: Option<&Terrain>, p: egui::Pos2| -> Option<Vec3> {
            let (o, d) = ray_at(p);
            terrain.and_then(|t| t.raycast(o, d, 9000.0)).or_else(|| (d.y < -1e-4).then(|| o + d * ((cam.target.y - o.y) / d.y)))
        };
        let hover_ground = pointer.filter(|p| rect.contains(*p)).and_then(|p| ground_at(self.terrain.as_ref(), p));
        if hover_ground.is_some() {
            self.cursor = hover_ground;
        }
        let (shift, cmd_key, alt) = ctx.input(|i| (i.modifiers.shift, i.modifiers.command, i.modifiers.alt));
        let snap_now = self.snap ^ cmd_key;

        // ---- camera ----
        let before = self.camera;
        if resp.dragged_by(egui::PointerButton::Secondary) && self.modal.is_none() {
            let d = resp.drag_delta();
            self.camera.orbit(d.x, d.y);
        }
        if resp.dragged_by(egui::PointerButton::Middle) {
            let d = resp.drag_delta();
            if shift {
                // Shift + middle drag: straight up / down.
                self.camera.lift(d.y * 2.0 * self.camera.half_height() / rect.height());
            } else {
                self.camera.pan(d.x, d.y, rect.height());
            }
        }
        if resp.hovered() {
            // Shift+wheel arrives as a horizontal scroll on most systems.
            let scroll = ctx.input(|i| i.smooth_scroll_delta);
            let amount = if scroll.y != 0.0 { scroll.y } else { scroll.x };
            if amount != 0.0 {
                if shift {
                    // Shift + wheel: move the view up / down.
                    self.camera.lift(amount * self.camera.dist * 0.002);
                } else {
                    self.camera.zoom(amount);
                }
            }
        }
        let typing = ctx.egui_wants_keyboard_input();
        if !typing && self.modal.is_none() {
            let (dt, rmb) = ctx.input(|i| (i.stable_dt.min(0.1), i.pointer.secondary_down()));
            let key = |k: egui::Key| ctx.input(|i| i.key_down(k));
            // WASD + Q/E fly while the right button is held (Blender / Unreal
            // style), arrows always; with nothing selected WASD flies too.
            let fly = rmb || self.sel.is_empty();
            let speed = self.camera.dist * 1.2 * dt * if shift { 3.0 } else { 1.0 };
            let f = Vec3::new(self.camera.forward().x, 0.0, self.camera.forward().z).normalize_or(Vec3::Z);
            let rgt = self.camera.right();
            let mut mv = Vec3::ZERO;
            if key(egui::Key::ArrowUp) || (fly && key(egui::Key::W)) {
                mv += f;
            }
            if key(egui::Key::ArrowDown) || (fly && key(egui::Key::S) && !cmd_key) {
                mv -= f;
            }
            if key(egui::Key::ArrowLeft) || (fly && key(egui::Key::A) && !alt) {
                mv -= rgt;
            }
            if key(egui::Key::ArrowRight) || (fly && key(egui::Key::D) && !cmd_key) {
                mv += rgt;
            }
            if rmb && key(egui::Key::E) || key(egui::Key::PageUp) {
                mv += Vec3::Y;
            }
            if rmb && key(egui::Key::Q) || key(egui::Key::PageDown) {
                mv -= Vec3::Y;
            }
            if mv != Vec3::ZERO {
                self.camera.target += mv.normalize() * speed;
            }
        }
        if self.camera != before {
            ctx.request_repaint();
        }

        // ---- gizmo hit test (before clicks) ----
        let pivot = self.pivot();
        let pivot_scr = pivot.and_then(&to_screen);
        let px_m = 2.0 * self.camera.half_height() / rect.height();
        let handle_len = 90.0 * px_m; // ~90 px on screen whatever the zoom
        let gizmo_hover = match (pivot, pivot_scr, pointer, self.modal.is_none() && self.tool == Tool::Select) {
            (Some(p), Some(c), Some(ptr), true) => gizmo_hit(self.gizmo, p, c, ptr, handle_len, &to_screen),
            _ => None,
        };

        // ---- pointer ----
        if resp.drag_started_by(egui::PointerButton::Primary) && self.modal.is_none() && self.tool == Tool::Select {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(h) = gizmo_hover {
                    let kind = match self.gizmo {
                        GizmoMode::Move => Kind::Grab,
                        GizmoMode::Rotate => Kind::Rotate,
                        GizmoMode::Scale => Kind::Scale,
                    };
                    self.cursor = ground_at(self.terrain.as_ref(), p);
                    self.begin_locked(&ctx, kind, h, true);
                } else {
                    let (o, d) = ray_at(p);
                    match self.pick_any(o, d) {
                        Some(h) => {
                            if !self.sel.contains(&h) {
                                if shift {
                                    self.sel.insert(h);
                                    self.active = Some(h);
                                } else {
                                    self.select_only([h]);
                                }
                            }
                            self.cursor = ground_at(self.terrain.as_ref(), p);
                            self.begin(&ctx, Kind::Grab, true);
                        }
                        None => self.box_start = Some(p),
                    }
                }
            }
        }
        // Terrain brush: paint while the left button is held.
        if self.tool == Tool::Brush && self.modal.is_none() {
            let down = ctx.input(|i| i.pointer.primary_down()) && resp.hovered() || resp.dragged_by(egui::PointerButton::Primary);
            if down && !ctx.input(|i| i.pointer.secondary_down()) {
                if let Some(g) = hover_ground {
                    if self.last_dab.is_none() {
                        self.history.push(&self.edits); // one undo step per stroke
                    }
                    let spacing = (self.brush.radius * 0.3).max(0.25);
                    if self.last_dab.is_none_or(|l| (l - g).with_y(0.0).length() >= spacing) {
                        self.dab(g, cmd_key, shift);
                        self.last_dab = Some(g);
                    }
                }
                ctx.request_repaint();
            } else {
                self.last_dab = None;
            }
            if !typing {
                if ctx.input(|i| i.key_pressed(egui::Key::OpenBracket)) {
                    self.brush.radius = (self.brush.radius / 1.25).max(1.0);
                }
                if ctx.input(|i| i.key_pressed(egui::Key::CloseBracket)) {
                    self.brush.radius = (self.brush.radius * 1.25).min(120.0);
                }
            }
        }
        // Box select.
        if let Some(b0) = self.box_start {
            if let Some(p) = pointer {
                let r = egui::Rect::from_two_pos(b0, p);
                ui.painter().rect(
                    r,
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(255, 170, 60, 24),
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 170, 60)),
                    egui::StrokeKind::Inside,
                );
                if !ctx.input(|i| i.pointer.primary_down()) {
                    self.box_start = None;
                    if let Some(s) = &self.scenery {
                        let hit: Vec<Sel> = self
                            .all_objects()
                            .into_iter()
                            .filter(|x| self.selectable(*x))
                            .filter(|x| self.edits.xf(s, *x).and_then(|xf| to_screen(xf.pos)).is_some_and(|sp| r.contains(sp)))
                            .collect();
                        if !shift {
                            self.sel.clear();
                        }
                        let n = hit.len();
                        self.sel.extend(hit);
                        self.active = self.sel.iter().next_back().copied();
                        self.dirty = true;
                        self.status = format!("Box: {n} object(s)");
                    }
                }
            }
        }
        // Modal transform in progress.
        if let (Some(m), Some(p)) = (self.modal.clone(), pointer) {
            let mut m = m;
            if !typing {
                ctx.input(|i| {
                    for ev in &i.events {
                        if let egui::Event::Text(t) = ev {
                            for ch in t.chars() {
                                if ch.is_ascii_digit() || ch == '.' || (ch == '-' && m.typed.is_empty()) {
                                    m.typed.push(ch);
                                }
                            }
                        }
                    }
                });
                if ctx.input(|i| i.key_pressed(egui::Key::Backspace)) {
                    m.typed.pop();
                }
                for (k, a) in [(egui::Key::X, 0), (egui::Key::Y, 1), (egui::Key::Z, 2)] {
                    if ctx.input(|i| i.key_pressed(k)) {
                        let want = if shift { Lock::Plane(a) } else { Lock::Axis(a) };
                        m.lock = if m.lock == want { Lock::Free } else { want };
                        if m.kind == Kind::Grab {
                            m.g0 = ground_at(self.terrain.as_ref(), m.p0).or(m.g0);
                        }
                    }
                }
            }
            let g = ground_at(self.terrain.as_ref(), p);
            let mut g0_fixed = m.clone();
            if g0_fixed.g0.is_none() {
                g0_fixed.g0 = g;
                m.g0 = g;
            }
            // The pivot where the transform began, not the live centre.
            let ps = to_screen(m.pivot).or(pivot_scr);
            let axis_px: [egui::Vec2; 3] = std::array::from_fn(|a| {
                let mut dir = Vec3::ZERO;
                dir[a] = 1.0;
                match (ps, to_screen(m.pivot + dir * 10.0)) {
                    (Some(c), Some(t)) => (t - c) / 10.0,
                    _ => egui::Vec2::ZERO,
                }
            });
            let xfs = self.modal_xfs(&g0_fixed, p, g, ps, axis_px, snap_now);
            self.set_live(&xfs);
            self.modal = Some(m.clone());
            let confirm = ctx.input(|i| i.key_pressed(egui::Key::Enter))
                || (m.by_drag && !ctx.input(|i| i.pointer.primary_down()))
                || (!m.by_drag && resp.clicked());
            if confirm {
                self.modal = None;
            } else if resp.secondary_clicked() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.cancel_modal();
            }
            ctx.request_repaint();
        } else if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                match self.tool.clone() {
                    Tool::Place(entity) => {
                        if let Some(g) = ground_at(self.terrain.as_ref(), p) {
                            self.place(&ctx, entity, g);
                            if !shift {
                                self.tool = Tool::Select;
                            }
                        }
                    }
                    Tool::Brush => {}
                    Tool::AddSpot(group) => {
                        if let Some(g) = ground_at(self.terrain.as_ref(), p) {
                            // A new spot allows every kind that fits its footprint.
                            let allow = self
                                .kinds_now()
                                .iter()
                                .map(|(k, v)| (k.name.clone(), v.footprints.get(&group).copied().unwrap_or(false)))
                                .collect();
                            let mut sel = None;
                            self.change(|e, _| sel = Some(e.add_spot(group.clone(), g, allow)));
                            if let Some(s) = sel {
                                self.select_only([s]);
                            }
                            if !shift {
                                self.tool = Tool::Select;
                            }
                        }
                    }
                    Tool::Select => {
                        let (o, d) = ray_at(p);
                        match self.pick_any(o, d) {
                            Some(h) if shift => {
                                if !self.sel.remove(&h) {
                                    self.sel.insert(h);
                                    self.active = Some(h);
                                }
                                self.dirty = true;
                            }
                            Some(h) => self.select_only([h]),
                            None if !shift => self.select_only([]),
                            None => {}
                        }
                    }
                }
            }
        }
        if self.modal.is_none() {
            resp.context_menu(|ui| self.ui_context_menu(ui));
        }

        // ---- draw ----
        self.rebuild();
        let Some(rs) = frame.wgpu_render_state() else {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "wgpu is not available", Default::default(), egui::Color32::RED);
            return;
        };
        let mut lines = self.grid_lines();
        lines.extend(self.spot_lines());
        lines.extend(self.selection_lines());
        let mut top = Vec::new();
        match (&self.tool, hover_ground) {
            (Tool::Place(_), Some(g)) => ring(&mut top, g, 1.5, [0.5, 1.0, 0.6, 1.0], |x, z| self.ground_y(x, z)),
            (Tool::Brush, Some(g)) => {
                ring(&mut top, g, self.brush.radius, [1.0, 0.85, 0.4, 1.0], |x, z| self.ground_y(x, z));
                ring(&mut top, g, self.brush.radius * 0.5, [1.0, 0.85, 0.4, 0.35], |x, z| self.ground_y(x, z));
            }
            (Tool::AddSpot(grp), Some(g)) => {
                let h = recipe::group_size(grp) * 0.5;
                ring(&mut top, g, h * 1.41, [0.4, 1.0, 0.6, 0.8], |x, z| self.ground_y(x, z));
            }
            _ => {}
        }
        let mut parts: Vec<DrawPart> = Vec::new();
        if let Some(s) = &self.scenery {
            parts.extend(s.parts.iter().zip(&self.shipped_draw).map(|(p, inst)| DrawPart { key: &p.key, instances: inst }));
        }
        parts.extend(self.add_draw.iter().map(|(k, inst)| DrawPart { key: k, instances: inst }));
        let Some(renderer) = self.renderer.as_mut() else { return };
        if let (true, Some([x, z, d])) = (self.tile.is_none() && (self.opts.snapshot.is_some() || (self.opts.ui_snapshot.is_some() && self.settled_frames < 3)), self.opts.view) {
            let y = self.terrain.as_ref().and_then(|t| t.height_at(x, z)).unwrap_or(0.0);
            self.camera.target = Vec3::new(x, y, z);
            self.camera.dist = d;
            self.camera.pitch = 0.95;
        }
        // Snapshots render at a fixed size, whatever the window got.
        let size = if self.opts.snapshot.is_some() { (1600, 1000) } else { size };
        let t0 = Instant::now();
        let tex = renderer.render(
            rs,
            size,
            &Frame {
                camera: &self.camera,
                parts,
                lines: &lines,
                top_lines: &top,
                min_px: self.min_px,
                haze: self.haze,
                exposure: self.exposure,
                shadows: self.shadows,
                env: &self.env,
                draw_water: self.tile.is_none(),
            },
        );
        self.frame_ms = self.frame_ms * 0.9 + t0.elapsed().as_secs_f32() * 1000.0 * 0.1;
        ui.painter().image(tex, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);

        if let Some(path) = self.opts.ui_snapshot.clone() {
            let tile_ok = self.opts.open_tile.is_none() && self.tile.as_ref().is_none_or(|t| t.built);
            let ready = tile_ok && self.terrain.is_some() && self.scenery.is_some() && self.pending_assets == 0 && !self.look_pending;
            self.settled_frames = if ready { self.settled_frames + 1 } else { 0 };
            ctx.request_repaint();
            if self.settled_frames == 3 {
                // Something to show: the objects nearest the view centre.
                if let (Some(s), Some([x, z, _])) = (&self.scenery, self.opts.view) {
                    let c = Vec3::new(x, 0.0, z);
                    let mut near: Vec<usize> = (0..s.obj_xf.len()).collect();
                    near.sort_by(|a, b| {
                        let d = |i: usize| (s.obj_xf[i].pos - c).with_y(0.0).length_squared();
                        d(*a).total_cmp(&d(*b))
                    });
                    self.sel = near.into_iter().take(3).map(Sel::Obj).collect();
                    self.active = self.sel.iter().next_back().copied();
                    self.dirty = true;
                }
            }
            if self.settled_frames == 12 && !self.ui_shot_sent {
                self.ui_shot_sent = true;
                let path = path.clone();
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                    let [w, h] = img.size;
                    let px: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    match image::RgbaImage::from_raw(w as u32, h as u32, px).map(|i| i.save(&path)) {
                        Some(Ok(())) => println!("ui snapshot: {} ({w}x{h})", path.display()),
                        other => eprintln!("ui snapshot failed: {other:?}"),
                    }
                    std::process::exit(0);
                })));
            }
        }
        if let Some(path) = self.opts.snapshot.clone() {
            // A part that failed to load counts as settled.
            let settled = |loaded: bool, failed: &str| loaded || self.status.starts_with(failed);
            let ready = settled(self.terrain.is_some(), "Terrain:")
                && settled(self.scenery.is_some(), "Scenery:")
                && self.pending_assets == 0
                && !self.look_pending;
            self.settled_frames = if ready { self.settled_frames + 1 } else { 0 };
            ctx.request_repaint();
            if self.settled_frames >= 5 {
                match renderer.capture().map(|img| img.save(&path)) {
                    Some(Ok(())) => println!("snapshot: {} ({})", path.display(), self.status),
                    other => eprintln!("snapshot failed: {other:?}"),
                }
                println!(
                    "stats: {} draws, {} instances, {} tris, {:.1} ms, {} failed assets",
                    renderer.stats.draws,
                    renderer.stats.drawn_instances,
                    renderer.stats.drawn_triangles,
                    self.frame_ms,
                    self.failed_assets.len()
                );
                std::process::exit(0);
            }
        }

        // ---- overlays: gizmo, modal hint, minimap ----
        let painter = ui.painter_at(rect);
        if let (Some(p), Some(c), None, Tool::Select) = (pivot, pivot_scr, &self.modal, &self.tool) {
            draw_gizmo(&painter, self.gizmo, p, c, handle_len, gizmo_hover, &to_screen);
        }
        if let (Some(m), Some(c)) = (&self.modal, pivot_scr) {
            // Axis guide line through the pivot.
            if let Lock::Axis(a) = m.lock {
                let mut dir = Vec3::ZERO;
                dir[a] = 1.0;
                if let (Some(p1), Some(p2)) = (to_screen(m.pivot - dir * 1000.0), to_screen(m.pivot + dir * 1000.0)) {
                    painter.line_segment([p1, p2], egui::Stroke::new(1.5, AXIS_COL[a]));
                }
            }
            let what = match m.kind {
                Kind::Grab => "Move",
                Kind::Rotate => "Rotate",
                Kind::Scale => "Scale",
            };
            let val = if m.typed.is_empty() { String::new() } else { format!("  = {}", m.typed) };
            let txt = format!("{what}{}{val}   (X/Y/Z axis, Shift+X/Y/Z plane, type a number, Ctrl snap, Enter / click confirm, Esc cancel)", m.lock.label());
            painter.text(c + egui::vec2(16.0, -16.0), egui::Align2::LEFT_BOTTOM, &txt, egui::FontId::proportional(13.0), egui::Color32::WHITE);
        }
        if self.show_minimap && self.tile.is_none() {
            self.draw_minimap(ui, rect, aspect);
        }
        // View label (top-left) like Blender.
        let view = format!("{} · {}", if self.camera.ortho { "Orthographic" } else { "Perspective" }, view_name(&self.camera));
        painter.text(rect.left_top() + egui::vec2(10.0, 8.0), egui::Align2::LEFT_TOP, view, egui::FontId::proportional(13.0), egui::Color32::from_white_alpha(210));
    }

    fn draw_minimap(&mut self, ui: &mut egui::Ui, rect: egui::Rect, aspect: f32) {
        let (Some(tex), Some(t)) = (&self.minimap_tex, &self.terrain) else { return };
        let side = 190.0;
        let r = egui::Rect::from_min_size(rect.right_bottom() - egui::vec2(side + 12.0, side + 12.0), egui::vec2(side, side));
        let resp = ui.interact(r, ui.scope_id().with("minimap"), egui::Sense::click_and_drag());
        let p = ui.painter();
        p.rect_filled(r.expand(3.0), 4.0, egui::Color32::from_black_alpha(160));
        p.image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        // World (x, z) -> minimap; rows run along +Z downward in the texture.
        let to_mm = |x: f32, z: f32| {
            egui::pos2(r.left() + (x - t.min.x) / (t.max.x - t.min.x) * r.width(), r.top() + (z - t.min.z) / (t.max.z - t.min.z) * r.height())
        };
        if let Some(s) = &self.scenery {
            for x in &self.sel {
                if let Some(xf) = self.edits.xf(s, *x) {
                    p.circle_filled(to_mm(xf.pos.x, xf.pos.z), 2.0, egui::Color32::from_rgb(255, 170, 60));
                }
            }
        }
        // The camera's footprint: where the four screen corners hit the ground plane.
        let cam = self.camera;
        let corners: Vec<egui::Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .iter()
            .filter_map(|&(x, y)| {
                let (o, d) = cam.ray(x, y, aspect);
                (d.y < -1e-3).then(|| {
                    let q = o + d * ((cam.target.y - o.y) / d.y);
                    to_mm(q.x, q.z)
                })
            })
            .collect();
        if corners.len() == 4 {
            p.add(egui::Shape::closed_line(corners, egui::Stroke::new(1.5, egui::Color32::WHITE)));
        }
        p.circle_filled(to_mm(cam.target.x, cam.target.z), 3.0, egui::Color32::WHITE);
        if resp.clicked() || resp.dragged() {
            if let Some(q) = resp.interact_pointer_pos() {
                let x = t.min.x + (q.x - r.left()) / r.width() * (t.max.x - t.min.x);
                let z = t.min.z + (q.y - r.top()) / r.height() * (t.max.z - t.min.z);
                self.camera.target = Vec3::new(x, t.height_at(x, z).unwrap_or(self.camera.target.y), z);
            }
        }
    }

    fn ui_context_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let has = !self.sel.is_empty();
        let mut pick = None;
        let mut item = |ui: &mut egui::Ui, on: bool, label: &str, key: &str, c: Cmd| {
            if ui.add_enabled(on, egui::Button::new(label).shortcut_text(key)).clicked() {
                pick = Some(c);
                ui.close();
            }
        };
        item(ui, has, "Move", "G", Cmd::Grab);
        item(ui, has, "Rotate", "R", Cmd::Rotate);
        item(ui, has, "Scale", "S", Cmd::Scale);
        ui.separator();
        item(ui, has, "Duplicate", "Shift+D", Cmd::Duplicate);
        item(ui, has, "Copy", "Ctrl+C", Cmd::Copy);
        item(ui, has, "Cut", "Ctrl+X", Cmd::Cut);
        item(ui, !self.clipboard.is_empty(), "Paste here", "Ctrl+V", Cmd::Paste);
        item(ui, has, "Delete", "X", Cmd::Delete);
        ui.separator();
        item(ui, has, "Drop to ground", "End", Cmd::DropToGround);
        item(ui, has, "Hide", "H", Cmd::Hide);
        item(ui, has, "Lock", "L", Cmd::Lock);
        item(ui, has, "Group", "Ctrl+G", Cmd::Group);
        item(ui, has, "Frame selection", "F", Cmd::FrameSelected);
        if let Some(c) = pick {
            self.run(&ctx, c);
        }
    }

    // ---- shortcuts ------------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (cmd, shift, alt) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift, i.modifiers.alt));
        let p = |k: egui::Key| ctx.input(|i| i.key_pressed(k));
        // A running transform owns X / Y / Z / digits / Enter / Esc.
        if self.modal.is_some() {
            return;
        }
        let rmb = ctx.input(|i| i.pointer.secondary_down());
        let c = if p(egui::Key::F1) {
            Some(Cmd::Shortcuts)
        } else if p(egui::Key::F3) || (cmd && p(egui::Key::P)) {
            Some(Cmd::Palette)
        } else if cmd && shift && p(egui::Key::S) {
            Some(Cmd::SaveAs)
        } else if cmd && p(egui::Key::S) {
            Some(Cmd::Save)
        } else if cmd && p(egui::Key::N) {
            Some(Cmd::New)
        } else if cmd && shift && p(egui::Key::Z) || cmd && p(egui::Key::Y) {
            Some(Cmd::Redo)
        } else if cmd && p(egui::Key::Z) {
            Some(Cmd::Undo)
        } else if cmd && p(egui::Key::C) {
            Some(Cmd::Copy)
        } else if cmd && p(egui::Key::X) {
            Some(Cmd::Cut)
        } else if cmd && p(egui::Key::V) {
            Some(Cmd::Paste)
        } else if cmd && p(egui::Key::D) || shift && p(egui::Key::D) {
            Some(Cmd::Duplicate)
        } else if cmd && p(egui::Key::I) {
            Some(Cmd::Invert)
        } else if cmd && p(egui::Key::G) {
            Some(Cmd::Group)
        } else if p(egui::Key::B) && !cmd && self.tile.is_none() {
            if self.tool == Tool::Brush {
                self.tool = Tool::Select;
            } else {
                self.tool = Tool::Brush;
                self.left_tab = LeftTab::Terrain;
            }
            None
        } else if rmb {
            None // right button held: WASD / Q / E fly the camera
        } else if alt && p(egui::Key::A) {
            Some(Cmd::DeselectAll)
        } else if p(egui::Key::A) && !cmd && !self.sel.is_empty() && self.sel.len() == self.all_objects().len() {
            Some(Cmd::DeselectAll)
        } else if p(egui::Key::A) && !cmd && !alt {
            Some(Cmd::SelectAll)
        } else if alt && p(egui::Key::H) {
            Some(Cmd::UnhideAll)
        } else if shift && p(egui::Key::H) {
            Some(Cmd::HideUnselected)
        } else if p(egui::Key::H) {
            Some(Cmd::Hide)
        } else if alt && p(egui::Key::L) {
            Some(Cmd::UnlockAll)
        } else if p(egui::Key::L) {
            Some(Cmd::Lock)
        } else if alt && p(egui::Key::G) {
            Some(Cmd::ResetTransform)
        } else if p(egui::Key::G) && !self.sel.is_empty() {
            Some(Cmd::Grab)
        } else if p(egui::Key::R) && !self.sel.is_empty() {
            Some(Cmd::Rotate)
        } else if p(egui::Key::S) && !self.sel.is_empty() && !cmd {
            Some(Cmd::Scale)
        } else if p(egui::Key::X) || p(egui::Key::Delete) {
            Some(Cmd::Delete)
        } else if p(egui::Key::End) {
            Some(Cmd::DropToGround)
        } else if p(egui::Key::F) || p(egui::Key::Period) {
            Some(Cmd::FrameSelected)
        } else if p(egui::Key::Home) {
            Some(Cmd::FrameAll)
        } else if p(egui::Key::Num1) {
            Some(Cmd::ViewFront)
        } else if p(egui::Key::Num3) {
            Some(Cmd::ViewSide)
        } else if p(egui::Key::Num7) {
            Some(Cmd::ViewTop)
        } else if p(egui::Key::Num5) {
            Some(Cmd::ToggleOrtho)
        } else if p(egui::Key::M) {
            Some(Cmd::ToggleMinimap)
        } else if p(egui::Key::W) && !self.sel.is_empty() {
            Some(Cmd::ToolMove)
        } else if p(egui::Key::E) && !self.sel.is_empty() {
            Some(Cmd::ToolRotate)
        } else if p(egui::Key::T) {
            Some(Cmd::ToolScale)
        } else if p(egui::Key::Escape) {
            if self.tool != Tool::Select {
                self.tool = Tool::Select;
                None
            } else {
                Some(Cmd::DeselectAll)
            }
        } else {
            None
        };
        if let Some(c) = c {
            if matches!(c, Cmd::Grab | Cmd::Rotate | Cmd::Scale) {
                // G/R/S from the keyboard measure from the ground under the cursor.
                self.modal = None;
            }
            self.run(ctx, c);
        }
    }

    // ---- panels ---------------------------------------------------------------------

    fn ui_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let pick: std::cell::Cell<Option<Cmd>> = std::cell::Cell::new(None);
        let mut open_chapter: Option<String> = None;
        let mut open_mod: Option<String> = None;
        egui::MenuBar::new().ui(ui, |ui| {
            let item = |ui: &mut egui::Ui, label: &str, key: &str, c: Cmd| {
                if ui.add(egui::Button::new(label).shortcut_text(key)).clicked() {
                    pick.set(Some(c));
                    ui.close();
                }
            };
            ui.menu_button("File", |ui| {
                ui.menu_button("New map from chapter", |ui| {
                    for c in &self.chapters {
                        if ui.button(&c.label).clicked() {
                            open_chapter = Some(c.key.clone());
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Open mod", |ui| {
                    if self.editor_mods.is_empty() {
                        ui.label("No map-editor mods yet");
                    }
                    for m in &self.editor_mods {
                        if ui.button(m).clicked() {
                            open_mod = Some(m.clone());
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Open recent", |ui| {
                    if self.recent.is_empty() {
                        ui.label("Nothing yet");
                    }
                    for m in &self.recent {
                        if ui.button(m).clicked() {
                            open_mod = Some(m.clone());
                            ui.close();
                        }
                    }
                });
                ui.separator();
                item(ui, "Save", "Ctrl+S", Cmd::Save);
                item(ui, "Save As…", "Ctrl+Shift+S", Cmd::SaveAs);
                item(ui, "Check it builds", "", Cmd::Check);
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                item(ui, "Undo", "Ctrl+Z", Cmd::Undo);
                item(ui, "Redo", "Ctrl+Shift+Z", Cmd::Redo);
                ui.separator();
                item(ui, "Copy", "Ctrl+C", Cmd::Copy);
                item(ui, "Cut", "Ctrl+X", Cmd::Cut);
                item(ui, "Paste at cursor", "Ctrl+V", Cmd::Paste);
                item(ui, "Duplicate", "Shift+D", Cmd::Duplicate);
                item(ui, "Delete", "X", Cmd::Delete);
                ui.separator();
                item(ui, "Command palette", "F3", Cmd::Palette);
            });
            ui.menu_button("Select", |ui| {
                item(ui, "All", "A", Cmd::SelectAll);
                item(ui, "None", "Alt+A", Cmd::DeselectAll);
                item(ui, "Invert", "Ctrl+I", Cmd::Invert);
            });
            ui.menu_button("Object", |ui| {
                item(ui, "Move", "G", Cmd::Grab);
                item(ui, "Rotate", "R", Cmd::Rotate);
                item(ui, "Scale", "S", Cmd::Scale);
                item(ui, "Reset transform", "Alt+G", Cmd::ResetTransform);
                item(ui, "Drop to ground", "End", Cmd::DropToGround);
                ui.menu_button("Align", |ui| {
                    for (label, _, c) in COMMANDS.iter().filter(|(_, _, c)| matches!(c, Cmd::Align(..))) {
                        if ui.button(label.trim_start_matches("Align ")).clicked() {
                            pick.set(Some(*c));
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Distribute", |ui| {
                    if ui.button("Evenly along X").clicked() {
                        pick.set(Some(Cmd::Distribute(0)));
                        ui.close();
                    }
                    if ui.button("Evenly along Z").clicked() {
                        pick.set(Some(Cmd::Distribute(2)));
                        ui.close();
                    }
                });
                ui.separator();
                item(ui, "Hide", "H", Cmd::Hide);
                item(ui, "Hide others", "Shift+H", Cmd::HideUnselected);
                item(ui, "Show all", "Alt+H", Cmd::UnhideAll);
                item(ui, "Lock", "L", Cmd::Lock);
                item(ui, "Unlock all", "Alt+L", Cmd::UnlockAll);
                item(ui, "Group", "Ctrl+G", Cmd::Group);
            });
            ui.menu_button("View", |ui| {
                item(ui, "Frame whole map", "Home", Cmd::FrameAll);
                item(ui, "Frame selection", "F", Cmd::FrameSelected);
                ui.separator();
                item(ui, "Front", "1", Cmd::ViewFront);
                item(ui, "Side", "3", Cmd::ViewSide);
                item(ui, "Top", "7", Cmd::ViewTop);
                item(ui, "Perspective / Orthographic", "5", Cmd::ToggleOrtho);
                ui.separator();
                ui.checkbox(&mut self.show_grid, "Grid");
                ui.checkbox(&mut self.snap, "Snap to grid");
                ui.add(egui::Slider::new(&mut self.grid_size, 0.25..=16.0).logarithmic(true).text("grid m"));
                ui.checkbox(&mut self.snap_ground, "Objects follow the ground");
                ui.checkbox(&mut self.shadows, "Shadows");
                ui.checkbox(&mut self.show_minimap, "Minimap");
                ui.add(egui::Slider::new(&mut self.min_px, 0.0..=8.0).text("hide props under px"));
                ui.separator();
                item(ui, "Environment (light & fog)…", "", Cmd::ToggleEnv);
                item(ui, "Log", "", Cmd::ToggleLog);
                item(ui, if self.light_theme { "Dark theme" } else { "Light theme" }, "", Cmd::ToggleTheme);
            });
            ui.menu_button("Help", |ui| {
                item(ui, "Keyboard shortcuts", "F1", Cmd::Shortcuts);
                item(ui, "Command palette", "F3", Cmd::Palette);
            });
            ui.separator();
            // Chapter + tools, right in the bar.
            let cur = self
                .chapter
                .as_ref()
                .and_then(|k| self.chapters.iter().find(|c| &c.key == k))
                .map(|c| c.label.clone())
                .unwrap_or_else(|| "Chapter…".into());
            ui.label(egui::RichText::new(cur).strong());
            if !self.mod_id.is_empty() {
                ui.label(egui::RichText::new(format!("· {}{}", self.mod_id, if self.unsaved { " •" } else { "" })).weak());
            } else if self.unsaved {
                ui.label(egui::RichText::new("· unsaved").weak());
            }
            ui.separator();
            for (mode, icon, tip) in [
                (GizmoMode::Move, Icon::Move, "Move tool (W)"),
                (GizmoMode::Rotate, Icon::Rotate, "Rotate tool (E)"),
                (GizmoMode::Scale, Icon::Scale, "Scale tool (T)"),
            ] {
                if icons::button(ui, icon, self.gizmo == mode, tip).clicked() {
                    self.gizmo = mode;
                }
            }
            ui.separator();
            if icons::button(ui, Icon::Magnet, self.snap, "Snap to grid (hold Ctrl to invert)").clicked() {
                self.snap = !self.snap;
            }
            if icons::button(ui, Icon::Grid, self.show_grid, "Grid").clicked() {
                self.show_grid = !self.show_grid;
            }
            if icons::button(ui, Icon::Sun, self.show_env, "Environment: light & fog").clicked() {
                self.show_env = !self.show_env;
            }
        });
        if let Some(k) = open_chapter {
            if self.unsaved {
                self.autosave();
            }
            self.mod_id.clear();
            self.mod_name.clear();
            self.keep_edits = json!({});
            self.keep_map = None;
            self.as_map = true;
            self.map_id = format!("Map_{}_Custom", k);
            self.open_chapter(&ctx, k);
            self.restore_offer = None;
            self.status = "New map: a copy of this chapter. Edit, then File > Save As".into();
        }
        if let Some(id) = open_mod {
            if self.unsaved {
                self.autosave();
            }
            self.spawn(&ctx, move |be| {
                let r = be.get_json(&format!("/map/api/mod?id={}", q(&id)));
                Ok(Event::Opened(id, r))
            });
        }
        if let Some(c) = pick.get() {
            self.run(&ctx, c);
        }
    }

    fn ui_tabs(&mut self, ui: &mut egui::Ui) {
        let mut go = None;
        let mut close = None;
        ui.horizontal(|ui| {
            for (i, t) in self.tabs.iter().enumerate() {
                let on = i == self.active_tab;
                let dirty = if on { self.unsaved } else { t.parked.as_ref().is_some_and(|p| p.unsaved) };
                let label = format!("{}{}{}", if t.tile.is_some() { "Tile: " } else { "" }, t.title, if dirty { " •" } else { "" });
                egui::Frame::new()
                    .fill(if on { ui.visuals().selection.bg_fill.gamma_multiply(0.5) } else { ui.visuals().faint_bg_color })
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui.add(egui::Label::new(egui::RichText::new(label).strong()).sense(egui::Sense::click())).clicked() {
                                go = Some(i);
                            }
                            if i > 0 && ui.small_button("x").on_hover_text("Close tab").clicked() {
                                close = Some((i, dirty));
                            }
                        });
                    });
            }
        });
        if let Some(i) = go {
            self.switch_tab(i);
        }
        match close {
            Some((i, true)) => self.close_confirm = Some(i),
            Some((i, false)) => self.close_tab(i),
            None => {}
        }
    }

    fn ui_left(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.left_tab, LeftTab::Outliner, "Outliner");
            ui.selectable_value(&mut self.left_tab, LeftTab::Assets, "Assets");
            if self.tile.is_none() {
                ui.selectable_value(&mut self.left_tab, LeftTab::Spots, "Spots");
                ui.selectable_value(&mut self.left_tab, LeftTab::Pois, "POIs");
                ui.selectable_value(&mut self.left_tab, LeftTab::Terrain, "Terrain");
            }
            let n = self.edits.count();
            ui.selectable_value(&mut self.left_tab, LeftTab::Changes, if n > 0 { format!("Changes ({n})") } else { "Changes".into() });
        });
        ui.separator();
        if self.tile.is_some() && matches!(self.left_tab, LeftTab::Spots | LeftTab::Terrain | LeftTab::Pois) {
            self.left_tab = LeftTab::Outliner;
        }
        // Leaving the Terrain tab puts the brush down.
        if self.left_tab != LeftTab::Terrain && self.tool == Tool::Brush {
            self.tool = Tool::Select;
        }
        match self.left_tab {
            LeftTab::Outliner => self.ui_outliner(ui),
            LeftTab::Assets => self.ui_assets(ui),
            LeftTab::Spots => self.ui_spots(ui),
            LeftTab::Pois => self.ui_pois(ui),
            LeftTab::Terrain => self.ui_terrain(ui),
            LeftTab::Changes => self.ui_changes(ui),
        }
    }

    /// One outliner row: expander (branches), eye, lock, label. Returns
    /// (clicked label, double-clicked).
    fn tree_row(&mut self, ui: &mut egui::Ui, members: &[Sel], label: &str, count: Option<usize>, selected: bool) -> (bool, bool) {
        let all_hidden = !members.is_empty() && members.iter().all(|m| self.hidden.contains(m));
        let all_locked = !members.is_empty() && members.iter().all(|m| self.locked.contains(m));
        let mut out = (false, false);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let eye = icons::button(ui, Icon::Eye(!all_hidden), false, "Show / hide");
            if eye.clicked() {
                if all_hidden {
                    for m in members {
                        self.hidden.remove(m);
                    }
                } else {
                    self.hidden.extend(members.iter().copied());
                    for m in members {
                        self.sel.remove(m);
                    }
                }
                self.dirty = true;
            }
            let lock = icons::button(ui, Icon::Lock(all_locked), false, "Lock / unlock");
            if lock.clicked() {
                if all_locked {
                    for m in members {
                        self.locked.remove(m);
                    }
                } else {
                    self.locked.extend(members.iter().copied());
                    for m in members {
                        self.sel.remove(m);
                    }
                }
                self.dirty = true;
            }
            let text = match count {
                Some(n) => format!("{label}  ({n})"),
                None => label.to_owned(),
            };
            let mut rt = egui::RichText::new(text);
            if all_hidden {
                rt = rt.weak();
            }
            let r = ui.selectable_label(selected, rt);
            out = (r.clicked(), r.double_clicked());
        });
        out
    }

    fn select_members(&mut self, ui: &egui::Ui, members: &[Sel]) {
        let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        let pick: Vec<Sel> = members.iter().copied().filter(|m| self.selectable(*m)).collect();
        if add {
            self.sel.extend(pick);
            self.active = self.sel.iter().next_back().copied();
            self.dirty = true;
        } else {
            self.select_only(pick);
        }
    }

    fn ui_outliner(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.tree_filter).hint_text("Search objects…").desired_width(f32::INFINITY));
        });
        if self.scenery.is_none() {
            ui.label("Loading…");
            return;
        }
        let filter = self.tree_filter.to_lowercase();
        let tree = std::mem::take(&mut self.tree);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            // Groups the author made.
            if !self.groups.is_empty() {
                let groups = self.groups.clone();
                egui::CollapsingHeader::new(egui::RichText::new("Groups").strong()).id_salt("groups").default_open(true).show(ui, |ui| {
                    let mut remove = None;
                    for (gi, (name, members)) in groups.iter().enumerate() {
                        let selected = !members.is_empty() && members.iter().all(|m| self.sel.contains(m));
                        let (clicked, dbl) = self.tree_row(ui, members, name, Some(members.len()), selected);
                        if clicked {
                            self.select_members(ui, members);
                        }
                        if dbl {
                            self.run(&ui.ctx().clone(), Cmd::FrameSelected);
                        }
                        ui.horizontal(|ui| {
                            ui.add_space(46.0);
                            if ui.small_button("ungroup").clicked() {
                                remove = Some(gi);
                            }
                        });
                    }
                    if let Some(gi) = remove {
                        self.groups.remove(gi);
                    }
                });
            }
            // Objects the author added.
            if !self.edits.adds.is_empty() {
                let adds: Vec<(Sel, String)> = self.edits.adds.iter().map(|a| (Sel::Add(a.uid), a.name())).collect();
                let members: Vec<Sel> = adds.iter().map(|(s, _)| *s).collect();
                egui::CollapsingHeader::new(egui::RichText::new(format!("Added  ({})", adds.len())).strong())
                    .id_salt("adds")
                    .default_open(true)
                    .show(ui, |ui| {
                        let (c, _) = self.tree_row(ui, &members, "all added", None, false);
                        if c {
                            self.select_members(ui, &members);
                        }
                        for (s, n) in adds.iter().filter(|(_, n)| filter.is_empty() || n.to_lowercase().contains(&filter)) {
                            let (c, d) = self.tree_row(ui, &[*s], n, None, self.sel.contains(s));
                            if c {
                                self.select_members(ui, &[*s]);
                            }
                            if d {
                                self.run(&ui.ctx().clone(), Cmd::FrameSelected);
                            }
                        }
                    });
            }
            // Tile spots, by footprint.
            if self.show_spots && self.tile.is_none() {
                let mut by_group: std::collections::BTreeMap<String, Vec<(Sel, String)>> = std::collections::BTreeMap::new();
                for sp in self.spots_now().into_iter().filter(|p| !p.whole_map) {
                    let sel = Sel::Spot(sp.id);
                    let label = self.name_of(sel);
                    if filter.is_empty() || label.to_lowercase().contains(&filter) {
                        by_group.entry(sp.group.clone()).or_default().push((sel, label));
                    }
                }
                if !by_group.is_empty() {
                    let all: Vec<Sel> = by_group.values().flatten().map(|(s, _)| *s).collect();
                    egui::CollapsingHeader::new(egui::RichText::new(format!("Tile spots  ({})", all.len())).strong()).id_salt("spots").show(ui, |ui| {
                        for (g, list) in &by_group {
                            let members: Vec<Sel> = list.iter().map(|(s, _)| *s).collect();
                            egui::CollapsingHeader::new(format!("{g}  ({})", list.len())).id_salt(("spotg", g)).show(ui, |ui| {
                                let (c, _) = self.tree_row(ui, &members, "all", None, false);
                                if c {
                                    self.select_members(ui, &members);
                                }
                                for (sel, label) in list {
                                    let (c, d) = self.tree_row(ui, &[*sel], label, None, self.sel.contains(sel));
                                    if c {
                                        self.select_members(ui, &[*sel]);
                                    }
                                    if d {
                                        self.run(&ui.ctx().clone(), Cmd::FrameSelected);
                                    }
                                }
                            });
                        }
                    });
                }
            }
            for (li, level) in tree.iter().enumerate() {
                if !outliner::matches(level, &filter) {
                    continue;
                }
                self.tree_node(ui, level, &filter, &format!("L{li}"), 0);
            }
        });
        self.tree = tree;
    }

    fn tree_node(&mut self, ui: &mut egui::Ui, node: &Node, filter: &str, id: &str, depth: usize) {
        let live: Vec<Sel> = node.members.iter().copied().filter(|m| !matches!(m, Sel::Obj(i) if self.edits.removed.contains(i))).collect();
        if let Some(leaf) = node.leaf {
            if live.is_empty() {
                return;
            }
            let (c, d) = self.tree_row(ui, &[leaf], &node.label, None, self.sel.contains(&leaf));
            if c {
                self.select_members(ui, &[leaf]);
            }
            if d {
                self.run(&ui.ctx().clone(), Cmd::FrameSelected);
            }
            return;
        }
        let open_default = depth == 0 && !filter.is_empty();
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(id), open_default);
        let state = if !filter.is_empty() {
            let mut s = state;
            s.set_open(true);
            s
        } else {
            state
        };
        let selected = !live.is_empty() && live.iter().all(|m| self.sel.contains(m));
        let header = state.show_header(ui, |ui| {
            let (c, d) = self.tree_row(ui, &live, &node.label, Some(live.len()), selected);
            if c {
                self.select_members(ui, &live);
            }
            if d {
                self.run(&ui.ctx().clone(), Cmd::FrameSelected);
            }
        });
        header.body(|ui| {
            // Instance lists can run to thousands (grass): show the first ones.
            const MAX_ROWS: usize = 300;
            let shown: Vec<&Node> = node.children.iter().filter(|c| outliner::matches(c, filter)).collect();
            for (ci, child) in shown.iter().take(MAX_ROWS).enumerate() {
                self.tree_node(ui, child, filter, &format!("{id}/{ci}"), depth + 1);
            }
            if shown.len() > MAX_ROWS {
                ui.label(egui::RichText::new(format!("… {} more — search to narrow, or click the row above to select all", shown.len() - MAX_ROWS)).weak());
            }
        });
    }


    fn ui_spots(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(r) = self.recipe.clone() else {
            ui.label("Loading the generation recipe…");
            return;
        };
        ui.label(egui::RichText::new("Spots are where a run can place a tile (a POI). Clicks pick spots first while this tab is open.").small().weak());
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_spots, "Show spots");
            if ui.checkbox(&mut self.show_tiles, "Show tiles").changed() {
                self.dirty = true;
            }
        });
        if r.scenarios.len() > 1 {
            egui::ComboBox::from_label("Scenario").selected_text(r.scenarios.get(self.scen).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                for (i, sc) in r.scenarios.iter().enumerate() {
                    ui.selectable_value(&mut self.scen, i, sc);
                }
            });
        }
        ui.horizontal(|ui| {
            if ui.button(if self.layout.is_empty() { "Preview a run" } else { "Roll again" }).on_hover_text("Fill the spots with tiles roughly the way a run would").clicked() {
                self.roll(&ctx);
            }
            if !self.layout.is_empty() && ui.button("Clear").clicked() {
                self.layout.clear();
                self.dirty = true;
            }
        });
        ui.separator();
        ui.label(egui::RichText::new("Add a spot").strong());
        ui.horizontal_wrapped(|ui| {
            for g in &r.groups {
                let on = self.tool == Tool::AddSpot(g.clone());
                if ui.selectable_label(on, g).on_hover_text("Click the ground to place (Shift keeps placing)").clicked() {
                    self.tool = if on { Tool::Select } else { Tool::AddSpot(g.clone()) };
                }
            }
        });
        ui.separator();
        ui.label(egui::RichText::new("Tile kinds per run").strong());
        ui.label(egui::RichText::new("Click a name to light up the spots it can use (green fits, amber allowed but wrong size).").small().weak());
        let mut changes: Vec<(String, KindVals)> = Vec::new();
        egui::ScrollArea::vertical().id_salt("kinds").max_height(ui.available_height() * 0.55).show(ui, |ui| {
            for (k, v) in self.kinds_now() {
                let focus = self.kind_focus.as_deref() == Some(&k.name);
                egui::CollapsingHeader::new(egui::RichText::new(format!("{}  ×{}", k.name.replace('_', " "), v.count)).strong())
                    .id_salt(("kind", &k.name))
                    .show(ui, |ui| {
                        let mut nv = v.clone();
                        if ui.selectable_label(focus, "Show where it can go").clicked() {
                            self.kind_focus = if focus { None } else { Some(k.name.clone()) };
                        }
                        ui.horizontal(|ui| {
                            ui.label("Per run");
                            ui.add(egui::DragValue::new(&mut nv.count).range(0..=200));
                            ui.label("Spacing");
                            ui.add(egui::DragValue::new(&mut nv.min_distance).range(0.0..=500.0).suffix(" m"));
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Fits");
                            for g in &r.groups {
                                let mut on = nv.footprints.get(g).copied().unwrap_or(false);
                                if ui.checkbox(&mut on, g).changed() {
                                    nv.footprints.insert(g.clone(), on);
                                }
                            }
                        });
                        if nv != v {
                            changes.push((k.name.clone(), nv));
                        }
                    });
            }
        });
        for (name, nv) in changes {
            let shipped = r.kinds.iter().find(|k| k.name == name).map(|k| k.vals.clone());
            self.change(|e, _| {
                if shipped.as_ref() == Some(&nv) {
                    e.kinds.remove(&name);
                } else {
                    e.kinds.insert(name.clone(), nv.clone());
                }
            });
        }
        ui.label(egui::RichText::new("The tiles themselves are in the POIs tab.").small().weak());
    }

    /// The tile pool as an overview: cards grouped by what the tile is,
    /// filterable by name and footprint; Open edits a tile in its own tab.
    fn ui_pois(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.pool.is_empty() {
            ui.label("Loading the tile pool…");
            return;
        }
        ui.add(egui::TextEdit::singleline(&mut self.poi_filter).hint_text("Search tiles…").desired_width(f32::INFINITY));
        let mut sizes: Vec<i64> = self.pool.iter().map(|t| t.width).collect();
        sizes.sort_unstable();
        sizes.dedup();
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.poi_size, None, "All sizes");
            for sz in sizes {
                ui.selectable_value(&mut self.poi_size, Some(sz), format!("{sz}x{sz}"));
            }
        });
        // How many of each the current preview placed.
        let mut on_map: HashMap<&str, usize> = HashMap::new();
        for p in self.layout.values() {
            *on_map.entry(p.as_str()).or_default() += 1;
        }
        let f = self.poi_filter.to_lowercase();
        let mut groups: std::collections::BTreeMap<String, Vec<&PoolTile>> = std::collections::BTreeMap::new();
        for t in self.pool.iter().filter(|t| self.poi_size.is_none_or(|s| t.width == s)) {
            if !f.is_empty() && !t.name.to_lowercase().contains(&f) && !t.flags.iter().any(|x| x.to_lowercase().contains(&f)) {
                continue;
            }
            let g = t.flags.iter().find(|x| x.as_str() != "Blocker").or(t.flags.first()).cloned().unwrap_or_else(|| "Other".into());
            groups.entry(g.replace('_', " ")).or_default().push(t);
        }
        let mut open = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (g, list) in &groups {
                egui::CollapsingHeader::new(egui::RichText::new(format!("{g}  ({})", list.len())).strong())
                    .id_salt(("poi", g))
                    .default_open(groups.len() <= 6 || !f.is_empty())
                    .show(ui, |ui| {
                        for t in list {
                            let col = recipe::group_color(&format!("{}x{}", t.width, t.height));
                            let c32 = egui::Color32::from_rgb((col[0] * 255.0) as u8, (col[1] * 255.0) as u8, (col[2] * 255.0) as u8);
                            egui::Frame::group(ui.style()).inner_margin(6.0).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(format!("{}x{}", t.width, t.height)).color(c32).monospace().strong());
                                    ui.label(egui::RichText::new(pretty_tile(&t.name)).strong());
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button("Open").on_hover_text("Edit this tile in its own tab").clicked() {
                                            open = Some(t.path.clone());
                                        }
                                    });
                                });
                                let mut info = t.flags.join(" · ");
                                if let Some(n) = on_map.get(t.path.as_str()) {
                                    info += &format!("   ·   {n} in the preview");
                                }
                                ui.label(egui::RichText::new(info).small().weak());
                            });
                        }
                    });
            }
        });
        if let Some(path) = open {
            self.enter_tile(&ctx, path, Vec3::ZERO);
        }
    }

    fn ui_terrain(&mut self, ui: &mut egui::Ui) {
        let painting = self.tool == Tool::Brush;
        if ui.add(egui::Button::new(if painting { "Brush on (B)" } else { "Pick up the brush (B)" }).selected(painting)).clicked() {
            self.tool = if painting { Tool::Select } else { Tool::Brush };
        }
        ui.label(egui::RichText::new("Left-drag paints · Ctrl inverts · Shift smooths · [ ] size · right-drag still orbits").small().weak());
        ui.separator();
        let slots: Vec<(usize, String)> = self
            .look_base
            .as_ref()
            .map(|l| (1..l.slots.len()).filter(|k| l.channel_sources.get(k - 1).is_some_and(Option::is_some)).map(|k| (k, l.slots[k].label.clone())).collect())
            .unwrap_or_default();
        let b = &mut self.brush;
        ui.label(egui::RichText::new("Shape").strong());
        ui.horizontal_wrapped(|ui| {
            for (m, l) in [(BrushMode::Raise, "Raise"), (BrushMode::Lower, "Lower"), (BrushMode::Flatten, "Flatten"), (BrushMode::Smooth, "Smooth"), (BrushMode::Level, "Set height")] {
                ui.selectable_value(&mut b.mode, m, l);
            }
        });
        if b.mode == BrushMode::Level {
            ui.add(egui::Slider::new(&mut b.level, -50.0..=50.0).text("height m"));
        }
        ui.label(egui::RichText::new("Paint the ground").strong());
        ui.horizontal_wrapped(|ui| {
            for (k, label) in &slots {
                ui.selectable_value(&mut b.mode, BrushMode::Paint(*k), label);
            }
        });
        if !slots.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label("Erase");
                for (k, label) in &slots {
                    ui.selectable_value(&mut b.mode, BrushMode::Erase(*k), label);
                }
            });
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut b.mode, BrushMode::Tint, "Tint");
            if b.mode == BrushMode::Tint {
                ui.color_edit_button_rgb(&mut b.color);
            }
        });
        ui.separator();
        ui.add(egui::Slider::new(&mut b.radius, 1.0..=120.0).logarithmic(true).text("size m"));
        let (lo, hi, unit) = match b.mode {
            BrushMode::Raise | BrushMode::Lower => (0.02, 3.0, " m/dab"),
            _ => (0.01, 1.0, ""),
        };
        ui.add(egui::Slider::new(&mut b.strength, lo..=hi).logarithmic(true).text(format!("strength{unit}")));
        egui::ComboBox::from_label("falloff").selected_text(b.falloff).show_ui(ui, |ui| {
            for f in ["smooth", "linear", "constant"] {
                ui.selectable_value(&mut b.falloff, f, f);
            }
        });
        ui.separator();
        let n = self.edits.strokes.len();
        ui.label(format!("{n} brush dabs"));
        if n > 15000 {
            ui.colored_label(egui::Color32::LIGHT_RED, "Close to the 20000-dab limit: use a bigger brush");
        }
        ui.label(egui::RichText::new("Terrain edits save as a new map (a copy of this chapter). Height, paths and walkability follow the painted height in game.").small().weak());
        if n > 0 && ui.button("Remove all terrain edits").clicked() {
            self.change(|e, _| e.strokes.clear());
            self.replay_terrain();
        }
    }

    fn ui_assets(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.prop_filter).hint_text("Search assets…").desired_width(f32::INFINITY));
        ui.checkbox(&mut self.all_props, "Include gameplay objects (spawners, FX, chests…)");
        ui.label(egui::RichText::new("Click an asset, then click the ground (Shift-click keeps placing). Only what this chapter preloads can be added.").small().weak());
        if self.props.is_empty() {
            ui.label("Loading…");
            return;
        }
        let f = self.prop_filter.to_lowercase();
        let mut groups: std::collections::BTreeMap<String, Vec<Prop>> = std::collections::BTreeMap::new();
        for p in self.props.iter().filter(|p| (self.all_props || p.scenery) && (f.is_empty() || p.name.to_lowercase().contains(&f) || p.group.to_lowercase().contains(&f))) {
            groups.entry(p.group.replace('_', " ")).or_default().push(p.clone());
        }
        let mut pick = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (g, list) in groups {
                egui::CollapsingHeader::new(format!("{g}  ({})", list.len())).default_open(!f.is_empty()).show(ui, |ui| {
                    for p in list {
                        let on = self.tool == Tool::Place(p.entity.clone());
                        if ui.selectable_label(on, &p.name).on_hover_text(&p.entity).clicked() {
                            pick = Some(p.entity.clone());
                        }
                    }
                });
            }
        });
        if let Some(e) = pick {
            let ctx = ui.ctx().clone();
            self.ensure_template(&ctx, &e);
            self.tool = Tool::Place(e);
        }
    }

    fn ui_changes(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &self.scenery else { return };
        if self.edits.is_empty() {
            ui.label("No scenery changes yet.");
        }
        let mut revert: Option<Sel> = None;
        let mut unremove: Option<usize> = None;
        let mut select: Option<Sel> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for &i in self.edits.moves.keys() {
                ui.horizontal(|ui| {
                    if ui.link(format!("Moved {} #{}", s.object_name(i), s.obj_id[i])).clicked() {
                        select = Some(Sel::Obj(i));
                    }
                    if ui.small_button("revert").clicked() {
                        revert = Some(Sel::Obj(i));
                    }
                });
            }
            for &i in &self.edits.removed {
                ui.horizontal(|ui| {
                    ui.label(format!("Removed {} #{}", s.object_name(i), s.obj_id[i]));
                    if ui.small_button("restore").clicked() {
                        unremove = Some(i);
                    }
                });
            }
            for a in &self.edits.adds {
                ui.horizontal(|ui| {
                    if ui.link(format!("Added {}", a.name())).clicked() {
                        select = Some(Sel::Add(a.uid));
                    }
                    if ui.small_button("remove").clicked() {
                        revert = Some(Sel::Add(a.uid));
                    }
                });
            }
            if let Some(k) = self.keep_edits.as_object().filter(|o| !o.is_empty()) {
                ui.separator();
                ui.label(format!("Kept from the opened mod: recipe edits ({})", k.keys().cloned().collect::<Vec<_>>().join(", ")));
            }
            if self.keep_map.is_some() {
                ui.label("Kept from the opened mod: map clone (pool / pinned tiles / terrain)");
            }
        });
        if let Some(sel) = revert {
            self.change(|e, _| match sel {
                Sel::Obj(i) => {
                    e.moves.remove(&i);
                }
                Sel::Add(_) | Sel::Spot(_) => e.delete(sel),
            });
            self.prune_selection();
        }
        if let Some(i) = unremove {
            self.change(|e, _| {
                e.removed.remove(&i);
            });
        }
        if let Some(sel) = select {
            self.select_only([sel]);
            self.run(&ui.ctx().clone(), Cmd::FrameSelected);
        }
    }

    fn ui_inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("Properties");
        let Some(s) = &self.scenery else { return };
        if self.sel.len() > 1 {
            ui.label(format!("{} objects selected", self.sel.len()));
            if let Some(p) = self.pivot() {
                ui.label(egui::RichText::new(format!("centre  {:.2}  {:.2}  {:.2}", p.x, p.y, p.z)).monospace());
            }
            ui.label(egui::RichText::new("G / R / S move, rotate and scale them together around their centre.").small().weak());
            ui.horizontal_wrapped(|ui| {
                let ctx = ui.ctx().clone();
                for (label, c) in [
                    ("Align X", Cmd::Align(0, 0)),
                    ("Align Z", Cmd::Align(2, 0)),
                    ("Same height", Cmd::Align(1, 0)),
                    ("Spread X", Cmd::Distribute(0)),
                    ("Spread Z", Cmd::Distribute(2)),
                    ("Drop to ground", Cmd::DropToGround),
                    ("Group", Cmd::Group),
                ] {
                    if ui.button(label).clicked() {
                        self.run(&ctx, c);
                    }
                }
            });
            return;
        }
        let Some(sel) = self.active.filter(|a| self.sel.contains(a)) else {
            ui.label(egui::RichText::new("Click an object, drag a box on empty ground, or pick one in the outliner.").weak());
            return;
        };
        if let Sel::Spot(id) = sel {
            self.ui_spot_inspector(ui, id);
            return;
        }
        let Some(xf) = self.edits.xf(s, sel) else { return };
        let (entity, where_) = match sel {
            Sel::Obj(i) => (
                s.entities.get(s.obj_entity[i] as usize).cloned().unwrap_or_default(),
                format!("{} · id {}", s.levels.get(s.obj_level[i] as usize).map(|l| outliner::level_label(l)).unwrap_or_default(), s.obj_id[i]),
            ),
            Sel::Add(u) => {
                let a = self.edits.add(u).cloned();
                (a.as_ref().map(|a| a.entity.clone()).unwrap_or_default(), format!("{} · new", a.map(|a| outliner::level_label(&a.level)).unwrap_or_default()))
            }
            Sel::Spot(_) => return,
        };
        ui.label(egui::RichText::new(self.name_of(sel)).strong().size(15.0));
        ui.label(egui::RichText::new(entity).small().weak());
        ui.label(egui::RichText::new(where_).small().weak());
        ui.separator();
        let mut new = xf;
        let (yaw, pitch, roll) = xf.rot.to_euler(glam::EulerRot::YXZ);
        let mut ypr = [yaw.to_degrees(), pitch.to_degrees(), roll.to_degrees()];
        let mut started = false;
        let mut changed = false;
        let mut track = |r: egui::Response| {
            started |= r.drag_started() || r.gained_focus();
            changed |= r.changed();
        };
        egui::Grid::new("xf").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            ui.label("");
            for (a, c) in ["X", "Y", "Z"].iter().zip(AXIS_COL) {
                ui.label(egui::RichText::new(*a).color(c).strong());
            }
            ui.end_row();
            ui.label("Location");
            for k in 0..3 {
                track(ui.add(egui::DragValue::new(&mut new.pos[k]).speed(0.05).fixed_decimals(2).suffix(" m")));
            }
            ui.end_row();
            ui.label("Rotation");
            let yaw_only = matches!(sel, Sel::Add(_));
            // Euler YXZ: show as X (pitch), Y (yaw), Z (roll).
            for (k, idx) in [1usize, 0, 2].into_iter().enumerate() {
                if yaw_only && idx != 0 {
                    ui.label("—");
                    continue;
                }
                let _ = k;
                track(ui.add(egui::DragValue::new(&mut ypr[idx]).speed(0.5).fixed_decimals(1).suffix("°")));
            }
            ui.end_row();
            ui.label("Scale");
            match sel {
                Sel::Add(_) => {
                    let mut k = new.scale.x;
                    track(ui.add(egui::DragValue::new(&mut k).speed(0.01).range(0.05..=20.0).fixed_decimals(2)));
                    new.scale = Vec3::splat(k);
                }
                Sel::Obj(_) => {
                    for k in 0..3 {
                        track(ui.add(egui::DragValue::new(&mut new.scale[k]).speed(0.01).range(0.05..=20.0).fixed_decimals(2)));
                    }
                }
                Sel::Spot(_) => {}
            }
            ui.end_row();
        });
        if started {
            self.history.push(&self.edits);
        }
        if changed {
            new.rot = Quat::from_euler(glam::EulerRot::YXZ, ypr[0].to_radians(), ypr[1].to_radians(), ypr[2].to_radians());
            self.set_live(&[(sel, new)]);
        }
        let locked = self.locked.contains(&sel);
        let hidden = self.hidden.contains(&sel);
        ui.horizontal(|ui| {
            let mut l = locked;
            if ui.checkbox(&mut l, "Locked").changed() {
                if l { self.locked.insert(sel); } else { self.locked.remove(&sel); }
                self.dirty = true;
            }
            let mut h = hidden;
            if ui.checkbox(&mut h, "Hidden").changed() {
                if h { self.hidden.insert(sel); } else { self.hidden.remove(&sel); }
                self.dirty = true;
            }
        });
        ui.horizontal_wrapped(|ui| {
            let ctx = ui.ctx().clone();
            for (label, c) in [
                ("Drop to ground", Cmd::DropToGround),
                ("Reset", Cmd::ResetTransform),
                ("Duplicate", Cmd::Duplicate),
                ("Delete", Cmd::Delete),
            ] {
                if ui.button(label).clicked() {
                    self.run(&ctx, c);
                }
            }
        });
    }

    /// Terrain brushes can only ship on a copy of the chapter's map.
    fn needs_map(&self) -> bool {
        self.as_map || self.keep_map.is_some() || !self.edits.strokes.is_empty()
    }

    fn ui_spot_inspector(&mut self, ui: &mut egui::Ui, id: i64) {
        let ctx = ui.ctx().clone();
        let (Some(s), Some(sp)) = (&self.scenery, self.scenery.as_ref().and_then(|s| self.edits.spot(s, id))) else { return };
        let _ = s;
        ui.label(egui::RichText::new(self.name_of(Sel::Spot(id))).strong().size(15.0));
        ui.label(egui::RichText::new(format!("{} m footprint · {:.1}, {:.1}", recipe::group_size(&sp.group), sp.pos.x, sp.pos.z)).small().weak());
        let mut p = sp.pos;
        let mut changed = false;
        let mut started = false;
        ui.horizontal(|ui| {
            ui.label("Position");
            for k in [0, 2] {
                let r = ui.add(egui::DragValue::new(&mut p[k]).speed(0.1).fixed_decimals(1).prefix(if k == 0 { "X " } else { "Z " }));
                started |= r.drag_started() || r.gained_focus();
                changed |= r.changed();
            }
        });
        if started {
            self.history.push(&self.edits);
        }
        if changed {
            p.y = self.ground_y(p.x, p.z).unwrap_or(p.y);
            self.set_live(&[(Sel::Spot(id), Xf { pos: p, rot: Quat::IDENTITY, scale: Vec3::ONE })]);
        }
        ui.separator();
        ui.label(egui::RichText::new("Tiles this spot may take").strong());
        let kinds = self.kinds_now();
        let mut toggles = Vec::new();
        egui::Grid::new("allow").num_columns(2).striped(true).show(ui, |ui| {
            for (k, v) in &kinds {
                let mut on = sp.allow.get(&k.name).copied().unwrap_or(false);
                let fits = v.footprints.get(&sp.group).copied().unwrap_or(false);
                if ui.checkbox(&mut on, k.name.replace('_', " ")).changed() {
                    toggles.push((k.name.clone(), on));
                }
                ui.label(egui::RichText::new(if fits { "fits" } else { "wrong size" }).small().color(if fits { egui::Color32::LIGHT_GREEN } else { egui::Color32::from_gray(140) }));
                ui.end_row();
            }
        });
        for (k, on) in toggles {
            self.change(|e, s| e.set_allow(s, id, &k, on));
        }
        ui.separator();
        match self.layout.get(&id).cloned() {
            Some(path) => {
                let name = path.rsplit('\\').next().unwrap_or(&path).trim_end_matches(".tiledef.ot").to_owned();
                ui.label(format!("Preview tile here: {name}"));
                if ui.button("Edit this tile (POI)").clicked() {
                    let gy = self.ground_y(sp.pos.x, sp.pos.z).unwrap_or(sp.pos.y);
                    self.enter_tile(&ctx, path, Vec3::new(sp.pos.x, gy, sp.pos.z));
                }
            }
            None => {
                ui.label(egui::RichText::new("Spots & POIs → Preview a run to see a tile here.").small().weak());
            }
        }
        if ui.button("Delete spot").clicked() {
            self.run(&ctx, Cmd::Delete);
        }
    }

    fn save_body(&self, with_id: bool) -> Option<Value> {
        let s = self.scenery.as_ref()?;
        let mut edits = self.keep_edits.as_object().cloned().unwrap_or_default();
        edits.extend(self.edits.recipe_doc(s, &self.shipped_kinds()));
        let mut b = json!({
            "chapter": self.chapter.clone()?,
            "edits": Value::Object(edits),
            "scenery": self.edits.scenery_doc(s),
        });
        if self.needs_map() {
            let mut m = self.keep_map.clone().unwrap_or_else(|| json!({"pool": {"add": [], "remove": []}, "place": {}, "terrain": []}));
            m["terrain"] = serde_json::to_value(&self.edits.strokes).unwrap_or(json!([]));
            let id = self.map_id.trim();
            m["id"] = json!(if id.is_empty() { "Map_Check" } else { id });
            m["play"] = json!(self.map_play);
            b["map"] = m;
        }
        if with_id {
            b["mod_id"] = json!(self.mod_id.trim().to_lowercase());
            b["name"] = json!(self.mod_name.trim());
        }
        Some(b)
    }

    fn submit(&mut self, kind: Pending) {
        let saving = kind == Pending::Save;
        let (b, target) = if self.tile.is_some() {
            let Some((b, notes)) = self.tile_body(saving) else { return };
            if !notes.is_empty() {
                self.failed_assets.extend(notes);
            }
            (b, if saving { "/map/api/t3/save" } else { "/map/api/t3/check" })
        } else {
            if self.edits.strokes.len() > 20000 {
                self.status = format!("{} brush dabs: the map kind takes at most 20000 — undo some, or use a bigger brush", self.edits.strokes.len());
                return;
            }
            let Some(b) = self.save_body(saving) else { return };
            (b, if saving { "/map/api/save" } else { "/map/api/check" })
        };
        match self.backend.send("POST", target, Some(&b)) {
            Ok(rx) => {
                self.pending = Some((kind, rx));
                self.status = if saving { "Saving…".into() } else { "Checking…".into() };
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    fn ui_save_panel(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(egui::RichText::new("Save").strong()).default_open(true).show(ui, |ui| {
            if self.mod_id.is_empty() {
                ui.label(egui::RichText::new("Not saved yet: File > Save As").weak());
            } else {
                ui.label(format!("mods/{}", self.mod_id));
            }
            let busy = self.pending.is_some();
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Check")).clicked() {
                    self.submit(Pending::Check);
                }
                if ui.add_enabled(!busy, egui::Button::new("Save")).clicked() {
                    self.run(&ui.ctx().clone(), Cmd::Save);
                }
                if ui.add_enabled(!busy, egui::Button::new("Save As…")).clicked() {
                    self.show_save_as = true;
                }
                if busy {
                    ui.spinner();
                }
            });
            match &self.result {
                Some(Ok(changes)) => {
                    ui.colored_label(egui::Color32::LIGHT_GREEN, format!("{} change(s) build cleanly", changes.len()));
                    egui::ScrollArea::vertical().id_salt("changes").max_height(140.0).show(ui, |ui| {
                        for c in changes {
                            ui.label(egui::RichText::new(c).small());
                        }
                    });
                    if let Some(n) = &self.saved_next {
                        ui.label("Close the game, then install it with:");
                        ui.code(n);
                    }
                }
                Some(Err(e)) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, e.to_string());
                }
                None => {}
            }
        });
    }

    fn ui_save_as(&mut self, ctx: &egui::Context) {
        let mut open = self.show_save_as;
        let mut go = false;
        egui::Window::new("Save As").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            egui::Grid::new("saveas").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label("Mod id");
                ui.add(egui::TextEdit::singleline(&mut self.mod_id).hint_text("my-map-mod"));
                ui.end_row();
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.mod_name).hint_text("Optional"));
                ui.end_row();
            });
            let locked = self.keep_map.is_some() || !self.edits.strokes.is_empty();
            if !self.edits.strokes.is_empty() {
                ui.label(egui::RichText::new("Terrain brushes ship as a new map (a copy of this chapter).").small().weak());
            }
            ui.add_enabled_ui(!locked, |ui| {
                ui.radio_value(&mut self.as_map, false, "Change this chapter for everyone who installs the mod");
                ui.radio_value(&mut self.as_map, true, "As a new map (a copy of this chapter)");
            });
            if self.needs_map() {
                ui.horizontal(|ui| {
                    ui.label("Map id");
                    ui.add(egui::TextEdit::singleline(&mut self.map_id).hint_text("Map_My_Hills"));
                });
                ui.checkbox(&mut self.map_play, "The chapter plays this map");
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ok = !self.mod_id.trim().is_empty();
                if ui.add_enabled(ok, egui::Button::new("Save")).clicked() || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    go = true;
                }
                ui.label(egui::RichText::new("2–64 lower-case letters, digits, - or _").small().weak());
            });
        });
        if go {
            open = false;
            self.submit(Pending::Save);
        }
        self.show_save_as = open;
    }

    fn ui_palette(&mut self, ctx: &egui::Context) {
        let mut open = self.show_palette;
        let mut run = None;
        egui::Window::new("Command palette")
            .open(&mut open)
            .collapsible(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .fixed_size([460.0, 380.0])
            .show(ctx, |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.palette_query).hint_text("Type a command or an object name…").desired_width(f32::INFINITY));
                r.request_focus();
                let q = self.palette_query.to_lowercase();
                let cmds: Vec<&(&str, &str, Cmd)> = COMMANDS.iter().filter(|(n, _, _)| q.is_empty() || n.to_lowercase().contains(&q)).collect();
                // Objects by name too ("search everything").
                let objs: Vec<(Sel, String)> = if q.len() >= 2 {
                    self.all_objects().into_iter().map(|s| (s, self.name_of(s))).filter(|(_, n)| n.to_lowercase().contains(&q)).take(40).collect()
                } else {
                    Vec::new()
                };
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for (i, (name, key, c)) in cmds.iter().enumerate() {
                        let b = ui.add(egui::Button::new(*name).shortcut_text(*key).frame(i == 0).min_size(egui::vec2(440.0, 0.0)));
                        if b.clicked() || (i == 0 && enter) {
                            run = Some(Ok(*c));
                        }
                    }
                    if !objs.is_empty() {
                        ui.separator();
                        ui.label(egui::RichText::new("Objects").weak());
                        for (s, n) in &objs {
                            if ui.button(n).clicked() || (cmds.is_empty() && enter) {
                                run = Some(Err(*s));
                                break;
                            }
                        }
                    }
                });
            });
        match run {
            Some(Ok(c)) => {
                open = false;
                self.run(ctx, c);
            }
            Some(Err(s)) => {
                open = false;
                self.select_only([s]);
                self.run(ctx, Cmd::FrameSelected);
            }
            None => {}
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        self.show_palette = open && self.show_palette;
    }

    fn ui_shortcuts(&mut self, ctx: &egui::Context) {
        let mut open = self.show_shortcuts;
        egui::Window::new("Keyboard shortcuts").open(&mut open).default_width(520.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                ui.strong("Camera");
                egui::Grid::new("cam").num_columns(2).striped(true).show(ui, |ui| {
                    for (a, b) in [
                        ("Orbit", "Right-drag"),
                        ("Pan", "Middle-drag"),
                        ("Zoom", "Wheel"),
                        ("Up / down", "Shift+wheel, Shift+middle-drag, PageUp / PageDown"),
                        ("Fly", "Hold right button + W A S D, Q / E down / up (Shift faster)"),
                        ("Views", "1 front · 3 side · 7 top · 5 ortho"),
                        ("Minimap", "M; click it to jump"),
                    ] {
                        ui.label(a);
                        ui.label(b);
                        ui.end_row();
                    }
                });
                ui.add_space(8.0);
                ui.strong("Editing");
                egui::Grid::new("keys").num_columns(2).striped(true).show(ui, |ui| {
                    ui.label("Select");
                    ui.label("Click · Shift+click add/remove · drag a box on empty ground");
                    ui.end_row();
                    ui.label("Move by dragging");
                    ui.label("Drag an object, or a gizmo arrow / ring / handle");
                    ui.end_row();
                    for (n, k, _) in COMMANDS.iter().filter(|(_, k, _)| !k.is_empty()) {
                        ui.label(*n);
                        ui.label(*k);
                        ui.end_row();
                    }
                });
            });
        });
        self.show_shortcuts = open;
    }

    /// The chapter's light, read from its day-cycle manager and fog settings.
    fn ui_env(&mut self, ui: &mut egui::Ui) {
        let e = &mut self.env;
        ui.label(egui::RichText::new("The chapter's own daytime light. Change it to compare with the game; it is not saved.").small().weak());
        let color = |ui: &mut egui::Ui, label: &str, c: &mut [f32; 3]| {
            ui.horizontal(|ui| {
                ui.color_edit_button_rgb(c);
                ui.label(label);
            });
        };
        egui::CollapsingHeader::new("Sun").default_open(true).show(ui, |ui| {
            color(ui, "colour", &mut e.sun_color);
            ui.add(egui::Slider::new(&mut e.sun_intensity, 0.0..=30.0).text("intensity"));
            ui.add(egui::Slider::new(&mut e.sun_zenith, 0.02..=1.0).text("zenith"));
            ui.add(egui::Slider::new(&mut e.sun_yaw, -180.0..=180.0).text("yaw °"));
        });
        egui::CollapsingHeader::new("Ambient").default_open(true).show(ui, |ui| {
            color(ui, "top", &mut e.amb_top);
            color(ui, "horizon", &mut e.amb_horizon);
            color(ui, "bottom", &mut e.amb_bottom);
            ui.add(egui::Slider::new(&mut e.amb_mul, 0.0..=4.0).text("multiplier"));
            ui.add(egui::Slider::new(&mut e.amb_pow, 0.1..=4.0).text("horizon power"));
        });
        egui::CollapsingHeader::new("Fog").default_open(true).show(ui, |ui| {
            ui.checkbox(&mut e.fog_on, "Height fog");
            for (label, c) in [("bottom", &mut e.fog_bottom), ("mid", &mut e.fog_mid), ("top", &mut e.fog_top)] {
                ui.horizontal(|ui| {
                    ui.color_edit_button_rgba_unmultiplied(c);
                    ui.label(label);
                });
            }
            ui.add(egui::Slider::new(&mut e.fog_intensity, 0.0..=3.0).text("intensity"));
            ui.horizontal(|ui| {
                ui.label("heights");
                for h in e.fog_heights.iter_mut() {
                    ui.add(egui::DragValue::new(h).speed(0.1));
                }
            });
            ui.horizontal(|ui| {
                ui.label("distances");
                for d in e.fog_distances.iter_mut() {
                    ui.add(egui::DragValue::new(d).speed(0.5));
                }
            });
        });
        ui.add(egui::Slider::new(&mut self.exposure, 0.25..=4.0).text("exposure"));
        if ui.add_enabled(self.env != self.env_shipped, egui::Button::new("Reset to the chapter's values")).clicked() {
            self.env = self.env_shipped.clone();
        }
    }

    fn ui_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let n = self.sel.len();
            ui.label(match n {
                0 => "Nothing selected".to_owned(),
                1 => format!("1 selected: {}", self.active.map(|a| self.name_of(a)).unwrap_or_default()),
                _ => format!("{n} selected"),
            });
            ui.separator();
            if let Some(c) = self.cursor {
                ui.monospace(format!("X {:7.2}  Y {:6.2}  Z {:7.2}", c.x, c.y, c.z));
                ui.separator();
            }
            if let Some(r) = &self.renderer {
                let st = r.stats;
                ui.label(egui::RichText::new(format!(
                    "{} objects · {} drawn · {:.1}M tris · {:.1} ms",
                    self.all_objects().len(),
                    st.drawn_instances,
                    st.drawn_triangles as f64 / 1e6,
                    self.frame_ms
                ))
                .weak());
            }
            if !self.hidden.is_empty() || !self.locked.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new(format!("{} hidden · {} locked", self.hidden.len(), self.locked.len())).weak());
            }
            if self.pending_assets > 0 {
                ui.separator();
                ui.spinner();
                ui.label(format!("loading {} assets", self.pending_assets));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(&self.status);
            });
        });
    }
}

impl eframe::App for MapEditor {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events(&ctx);
        self.handle_keys(&ctx);

        egui::Panel::top("menu").show(ui, |ui| self.ui_menu(ui));
        if self.tabs.len() > 1 {
            egui::Panel::top("tabs").show(ui, |ui| self.ui_tabs(ui));
        }
        egui::Panel::bottom("status").show(ui, |ui| self.ui_status(ui));
        if self.show_log {
            egui::Panel::bottom("log").resizable(true).default_size(160.0).show(ui, |ui| {
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
                    for l in self.backend.log.lock().unwrap().iter() {
                        ui.monospace(l);
                    }
                    for l in &self.failed_assets {
                        ui.colored_label(egui::Color32::LIGHT_RED, l);
                    }
                });
            });
        }
        egui::Panel::left("left").resizable(true).default_size(300.0).size_range(220.0..=420.0).show(ui, |ui| self.ui_left(ui));
        egui::Panel::right("right").resizable(true).default_size(320.0).size_range(240.0..=440.0).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("right").show(ui, |ui| {
                if let Some(ts) = &self.tile {
                    let name = ts.tile.rsplit('\\').next().unwrap_or(&ts.tile).trim_end_matches(".tiledef.ot").to_owned();
                    let built = ts.built;
                    let mut exit = false;
                    egui::Frame::group(ui.style()).fill(egui::Color32::from_rgb(60, 42, 20)).show(ui, |ui| {
                        ui.label(egui::RichText::new(format!("Tile: {}", pretty_tile(&name))).strong());
                        ui.label(egui::RichText::new(if built {
                            "Added objects appear in every spawn of this tile. Moving or removing the game's own objects saves an own copy of the tile (the original still spawns)."
                        } else {
                            "Loading its objects…"
                        }).small());
                        exit = ui.button("Close this tab").clicked();
                    });
                    if exit {
                        if self.unsaved {
                            self.close_confirm = Some(self.active_tab);
                        } else {
                            self.exit_tile();
                        }
                    }
                    ui.add_space(8.0);
                }
                if let Some(a) = self.restore_offer.clone() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.label(egui::RichText::new("Unsaved work from your last session").strong());
                        ui.label(egui::RichText::new(format!("{} scenery edits", a.scenery.as_object().map(|o| o.len()).unwrap_or(0))).small());
                        ui.horizontal(|ui| {
                            if ui.button("Restore").clicked() {
                                self.restore_offer = None;
                                self.restore(&ui.ctx().clone(), a.clone());
                            }
                            if ui.button("Discard").clicked() {
                                self.restore_offer = None;
                                if let Some(ch) = &self.chapter {
                                    session::clear_autosave(ch);
                                }
                            }
                        });
                    });
                    ui.add_space(8.0);
                }
                self.ui_inspector(ui);
                ui.add_space(10.0);
                ui.separator();
                self.ui_save_panel(ui);
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.ui_viewport(ui, frame));

        if self.show_env {
            let mut open = true;
            egui::Window::new("Environment").open(&mut open).default_width(320.0).show(&ctx, |ui| self.ui_env(ui));
            self.show_env = open;
        }
        if self.show_save_as {
            self.ui_save_as(&ctx);
        }
        if let Some(i) = self.close_confirm {
            let title = self.tabs.get(i).map(|t| t.title.clone()).unwrap_or_default();
            let mut decide = None;
            egui::Window::new("Close tab?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(&ctx, |ui| {
                ui.label(format!("{title} has unsaved changes."));
                ui.horizontal(|ui| {
                    if ui.button("Discard and close").clicked() {
                        decide = Some(true);
                    }
                    if ui.button("Keep editing").clicked() {
                        decide = Some(false);
                    }
                });
            });
            match decide {
                Some(true) => self.close_tab(i),
                Some(false) => self.close_confirm = None,
                None => {}
            }
        }
        if self.show_palette {
            self.ui_palette(&ctx);
        }
        if self.show_shortcuts {
            self.ui_shortcuts(&ctx);
        }
        if self.unsaved {
            ctx.request_repaint_after(AUTOSAVE_EVERY);
        }
    }

    fn on_exit(&mut self) {
        if self.unsaved {
            self.autosave();
        }
    }
}

// ---- helpers ------------------------------------------------------------------------

fn style(ctx: &egui::Context, light: bool) {
    ctx.set_visuals(if light { egui::Visuals::light() } else { egui::Visuals::dark() });
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.visuals.selection.bg_fill = egui::Color32::from_rgb(196, 120, 40);
        s.visuals.selection.stroke.color = egui::Color32::WHITE;
    });
}

fn view_name(c: &Camera) -> &'static str {
    if c.pitch > 1.5 {
        "Top"
    } else if c.pitch.abs() < 0.01 && c.yaw.abs() < 0.01 {
        "Front"
    } else if c.pitch.abs() < 0.01 && (c.yaw + std::f32::consts::FRAC_PI_2).abs() < 0.01 {
        "Side"
    } else {
        "User"
    }
}

fn minimap_texture(ctx: &egui::Context, t: &Terrain) -> egui::TextureHandle {
    let n = t.n;
    let mut px = Vec::with_capacity(n * n);
    for i in 0..n * n {
        let c = t.color(i);
        let g = |v: f32| (v.max(0.0).powf(1.0 / 2.2) * 255.0).round().min(255.0) as u8;
        px.push(egui::Color32::from_rgb(g(c[0]), g(c[1]), g(c[2])));
    }
    ctx.load_texture("minimap", egui::ColorImage::new([n, n], px), egui::TextureOptions::LINEAR)
}

/// Which gizmo handle is under the pointer.
fn gizmo_hit(
    mode: GizmoMode,
    pivot: Vec3,
    c: egui::Pos2,
    ptr: egui::Pos2,
    len: f32,
    to_screen: &impl Fn(Vec3) -> Option<egui::Pos2>,
) -> Option<Lock> {
    const TOL: f32 = 9.0;
    let axes = [Vec3::X, Vec3::Y, Vec3::Z];
    match mode {
        GizmoMode::Rotate => {
            let mut best = None;
            for (a, ax) in axes.iter().enumerate() {
                let ring = ring_points(pivot, *ax, len, to_screen);
                let d = ring.windows(2).map(|w| seg_dist(ptr, w[0], w[1])).fold(f32::MAX, f32::min);
                if d < TOL && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((a, d));
                }
            }
            best.map(|(a, _)| Lock::Axis(a))
        }
        _ => {
            if (ptr - c).length() < 10.0 {
                return Some(Lock::Free);
            }
            for (a, ax) in axes.iter().enumerate() {
                if let Some(tip) = to_screen(pivot + *ax * len) {
                    if seg_dist(ptr, c, tip) < TOL {
                        return Some(Lock::Axis(a));
                    }
                }
            }
            None
        }
    }
}

fn ring_points(pivot: Vec3, axis: Vec3, r: f32, to_screen: &impl Fn(Vec3) -> Option<egui::Pos2>) -> Vec<egui::Pos2> {
    let (u, v) = if axis == Vec3::Y { (Vec3::X, Vec3::Z) } else if axis == Vec3::X { (Vec3::Y, Vec3::Z) } else { (Vec3::X, Vec3::Y) };
    (0..=48)
        .filter_map(|k| {
            let a = k as f32 / 48.0 * std::f32::consts::TAU;
            to_screen(pivot + (u * a.cos() + v * a.sin()) * r)
        })
        .collect()
}

fn draw_gizmo(
    p: &egui::Painter,
    mode: GizmoMode,
    pivot: Vec3,
    c: egui::Pos2,
    len: f32,
    hover: Option<Lock>,
    to_screen: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    let axes = [Vec3::X, Vec3::Y, Vec3::Z];
    let width = |a: usize| if hover == Some(Lock::Axis(a)) { 4.0 } else { 2.5 };
    match mode {
        GizmoMode::Rotate => {
            for (a, ax) in axes.iter().enumerate() {
                let pts = ring_points(pivot, *ax, len, to_screen);
                p.add(egui::Shape::line(pts, egui::Stroke::new(width(a), AXIS_COL[a])));
            }
        }
        _ => {
            for (a, ax) in axes.iter().enumerate() {
                if let Some(tip) = to_screen(pivot + *ax * len) {
                    p.line_segment([c, tip], egui::Stroke::new(width(a), AXIS_COL[a]));
                    if mode == GizmoMode::Move {
                        let d = (tip - c).normalized();
                        let n = egui::vec2(-d.y, d.x);
                        p.add(egui::Shape::convex_polygon(vec![tip + d * 12.0, tip + n * 6.0, tip - n * 6.0], AXIS_COL[a], egui::Stroke::NONE));
                    } else {
                        p.rect_filled(egui::Rect::from_center_size(tip, egui::vec2(10.0, 10.0)), 1.0, AXIS_COL[a]);
                    }
                }
            }
        }
    }
    let center_col = if hover == Some(Lock::Free) { egui::Color32::WHITE } else { egui::Color32::from_white_alpha(170) };
    p.circle_stroke(c, 7.0, egui::Stroke::new(2.0, center_col));
}

fn seg_dist(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

fn aabb(v: &[crate::assets::Vertex]) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in v {
        lo = lo.min(Vec3::from(p.pos));
        hi = hi.max(Vec3::from(p.pos));
    }
    (lo, hi)
}

fn box_lines(out: &mut Vec<LineVertex>, m: Mat4, lo: Vec3, hi: Vec3, col: [f32; 4]) {
    let c = |x: bool, y: bool, z: bool| {
        m.transform_point3(Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z }))
    };
    const EDGES: [((u8, u8, u8), (u8, u8, u8)); 12] = [
        ((0, 0, 0), (1, 0, 0)),
        ((0, 1, 0), (1, 1, 0)),
        ((0, 0, 1), (1, 0, 1)),
        ((0, 1, 1), (1, 1, 1)),
        ((0, 0, 0), (0, 1, 0)),
        ((1, 0, 0), (1, 1, 0)),
        ((0, 0, 1), (0, 1, 1)),
        ((1, 0, 1), (1, 1, 1)),
        ((0, 0, 0), (0, 0, 1)),
        ((1, 0, 0), (1, 0, 1)),
        ((0, 1, 0), (0, 1, 1)),
        ((1, 1, 0), (1, 1, 1)),
    ];
    for ((ax, ay, az), (bx, by, bz)) in EDGES {
        out.push(LineVertex { pos: c(ax == 1, ay == 1, az == 1).into(), col });
        out.push(LineVertex { pos: c(bx == 1, by == 1, bz == 1).into(), col });
    }
}

/// A circle lying on the ground.
fn ring(out: &mut Vec<LineVertex>, c: Vec3, r: f32, col: [f32; 4], ground: impl Fn(f32, f32) -> Option<f32>) {
    const N: usize = 48;
    let at = |k: usize| {
        let a = k as f32 / N as f32 * std::f32::consts::TAU;
        let (x, z) = (c.x + a.cos() * r, c.z + a.sin() * r);
        Vec3::new(x, ground(x, z).unwrap_or(c.y) + 0.15, z)
    };
    for k in 0..N {
        out.push(LineVertex { pos: at(k).into(), col });
        out.push(LineVertex { pos: at(k + 1).into(), col });
    }
}

/// Nearest hit of a ray with a triangle mesh (Möller–Trumbore), in ray units.
fn ray_mesh(o: Vec3, d: Vec3, v: &[crate::assets::Vertex], idx: &[u32]) -> Option<f32> {
    let mut best: Option<f32> = None;
    for tri in idx.chunks_exact(3) {
        let (Some(a), Some(b), Some(c)) = (v.get(tri[0] as usize), v.get(tri[1] as usize), v.get(tri[2] as usize)) else {
            continue;
        };
        let (a, b, c) = (Vec3::from(a.pos), Vec3::from(b.pos), Vec3::from(c.pos));
        let (e1, e2) = (b - a, c - a);
        let p = d.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            continue;
        }
        let inv = 1.0 / det;
        let s = o - a;
        let u = s.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let qv = s.cross(e1);
        let w = d.dot(qv) * inv;
        if w < 0.0 || u + w > 1.0 {
            continue;
        }
        let t = e2.dot(qv) * inv;
        if t > 0.0 && best.is_none_or(|b| t < b) {
            best = Some(t);
        }
    }
    best
}

/// `{parts, matrices}` (map_scene.to_json) as draw-ready parts.
fn parse_template(v: &Value) -> anyhow::Result<Template> {
    let parts = v["parts"].as_array().cloned().unwrap_or_default();
    let mats = v["matrices"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for (p, m) in parts.iter().zip(&mats) {
        let key = PartKey {
            mesh: p["mesh"].as_str().unwrap_or_default().to_owned(),
            texture: p["texture"].as_str().map(str::to_owned),
            swap_rb: p["swap_rb"].as_bool().unwrap_or(false),
            color: p["color"].as_str().map(str::to_owned),
            decal: p["decal"].as_bool().unwrap_or(false),
        };
        let f = crate::scene::f32s_pub(m)?;
        out.push((key, f.chunks_exact(16).map(Mat4::from_cols_slice).collect()));
    }
    Ok(out)
}

/// `Tiles\\Dark_Hills\\40x40_Camp_01.tiledef.ot` -> `40x40_Camp_01`.
fn tile_name(path: &str) -> String {
    path.rsplit('\\').next().unwrap_or(path).trim_end_matches(".tiledef.ot").to_owned()
}

/// `40x40_Boss_Ghoul_Den_Entrance_LiveOps` -> `Boss Ghoul Den Entrance (LiveOps)`.
fn pretty_tile(name: &str) -> String {
    let mut parts: Vec<&str> = name.split('_').collect();
    if parts.first().is_some_and(|p| p.contains('x') && p.chars().next().is_some_and(|c| c.is_ascii_digit())) {
        parts.remove(0);
    }
    let mut out = parts.join(" ");
    for tag in ["LiveOps", "Update3", "Update5"] {
        if let Some(stripped) = out.strip_suffix(&format!(" {tag}")) {
            out = format!("{stripped} ({tag})");
        }
    }
    out
}
