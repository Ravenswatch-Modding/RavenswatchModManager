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
    /// Start on File > New empty chapter (at 0 m) instead of the shipped chapter.
    pub empty: bool,
    /// Make a few scenery edits, run them through /map/api/check, print the
    /// answer and quit (exercises the whole edit -> save document path).
    pub self_check: bool,
    /// Snapshot camera: target x, z and distance (else the whole map).
    pub view: Option<[f32; 3]>,
    /// Screenshot the whole window (UI included) once loaded, then quit.
    pub ui_snapshot: Option<std::path::PathBuf>,
    /// With `ui_snapshot`: click these window points (in order) once loaded,
    /// then take the screenshot. A test hook for the panels.
    pub ui_clicks: Vec<[f32; 2]>,
    /// With `ui_snapshot`: select the first object whose name contains this.
    pub select: Option<String>,
    /// Show every icon in a window (with `ui_snapshot`, to check them).
    pub icon_sheet: bool,
    /// Start with this gizmo: move, rotate or scale.
    pub gizmo: Option<String>,
    /// Open a pool tile in the POI editor, edit it, run the tile check, quit.
    pub tile_check: bool,
    /// Open the first pool tile whose name contains this, once loaded.
    pub open_tile: Option<String>,
    /// Import these files once the scene is loaded and place the first model
    /// at the `--view` point (checks the import path headless).
    pub import: Vec<std::path::PathBuf>,
    /// Open this Project folder at start (`Game/DarkHills/...`).
    pub project: Option<String>,
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
    /// Tagged with the request's number: only the latest open lands.
    Opened(String, u64, anyhow::Result<Value>),
    Recipe(String, anyhow::Result<Recipe>),
    Pool(String, anyhow::Result<Vec<PoolTile>>),
    TileObjects(String, anyhow::Result<Value>),
    TileProps(anyhow::Result<Vec<Prop>>),
    /// Files picked in Import… (or dropped on the window).
    Imported(Vec<std::path::PathBuf>),
    /// A picture chosen as the brush shape.
    StampPicked(Option<std::path::PathBuf>),
    /// A picture chosen for a terrain layer (`base`, `R` .. `A`).
    SurfacePicked(String, Option<std::path::PathBuf>),
    /// A file of the mod's own art reached the backend's staging area.
    /// Tagged with the upload's number: a response for bytes since replaced
    /// (another mod's model at the same path, a re-import) is dropped.
    Staged(String, u64, anyhow::Result<String>),
    /// Play in game finished launching (or failed): rsmm's last words.
    Played(Result<String, String>),
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
/// The Inspector side's tabs (Unity: Inspector + tool windows docked beside it).
enum LeftTab {
    Inspector,
    Pois,
    Spots,
    Terrain,
    Changes,
}

/// The bottom dock (Unity: Project and Console).
#[derive(PartialEq, Clone, Copy)]
enum BottomTab {
    Project,
    Console,
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
    /// The frame the lock's axes are in: world, or the active object's own
    /// axes for scale (an object's size is along its own axes).
    basis: Quat,
    /// Grabbed on a rotate ring: the ring angle under the pointer and the
    /// screen direction along the ring there. The turn follows the drag along
    /// the ring, so an edge-on ring turns as smoothly as a face-on one.
    ring: Option<(f32, egui::Vec2)>,
}

#[derive(PartialEq)]
enum Pending {
    Check,
    Save,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Cmd {
    New,
    Rename,
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
    Play,
    ToolMove,
    ToolRotate,
    ToolScale,
}

/// What a Hierarchy row drag carries: the objects being moved.
struct RowDrag(Vec<Sel>);

/// Chapters a blank map can start from: the tile-generated ones (the `map`
/// kind's `blank` needs a terrain and a recipe).
const BLANK_BASES: &[&str] = &["DarkHills", "Storm_Island", "Avalon"];

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
    ("Rename", "F2", Cmd::Rename),
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
    ("Play in game", "F5", Cmd::Play),
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
    /// The mod's own models (Project > Your models).
    models: Vec<crate::models::Model>,
    /// `art/<name>` -> its staged id at the backend (what a save copies).
    staged: HashMap<String, String>,
    /// `art/<name>` -> the number of its latest upload still in flight.
    staging: HashMap<String, u64>,
    stage_seq: u64,
    /// Bumped by each mod open and chapter switch; an older open's answer is stale.
    open_seq: u64,
    requested: HashSet<String>,
    pending_assets: usize,
    failed_assets: Vec<String>,
    look_pending: bool,
    minimap_tex: Option<egui::TextureHandle>,
    /// The ground changed since the minimap was drawn.
    minimap_stale: bool,

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
    /// Copied objects: entity, level, transform relative to their centre, own collision.
    clipboard: Vec<(String, String, Xf, Option<crate::edit::Add>)>,
    /// Inspector: a copied transform row (location 0, rotation 1, scale 2).
    xf_clip: Option<(usize, Xf)>,
    /// Inspector name field: one undo step per rename.
    renaming_inspector: bool,
    tool: Tool,
    gizmo: GizmoMode,
    modal: Option<Modal>,
    box_start: Option<egui::Pos2>,
    keep_edits: Value,
    keep_map: Option<Value>,
    /// The Project panel's open folder (`Assets`, `Game/DarkHills/...`).
    project_dir: String,
    /// Project tile size in points (the slider; Ctrl+wheel over the grid).
    tile_size: f32,
    /// Rendered Project thumbnails, by entity.
    thumbs: HashMap<String, egui::TextureId>,
    /// Tiles on screen this frame still waiting for a thumbnail.
    thumb_want: Vec<String>,
    /// The Project item picked with a click (placing is a drag).
    asset_sel: Option<String>,
    /// Effects play in the Scene (View menu).
    animate_fx: bool,
    started: Instant,
    /// The Hierarchy scrolls this row into view the next time it draws it.
    scroll_to_sel: Option<Sel>,
    /// The Scene's last hover pick: cursor position and what is under it.
    hover_pick: Option<(egui::Pos2, Option<Sel>)>,
    /// The window title last sent (sent again only when it changes).
    title: String,
    /// Open (true) or close (false) every Hierarchy section this frame.
    tree_open: Option<bool>,
    /// Right-click on selected text: where its Copy menu stands.
    copy_menu_at: Option<egui::Pos2>,
    /// The "Copied" note stays until then.
    copied_until: Option<Instant>,
    /// The mod's own files removed in the Project panel: deleted from its folder on save.
    dropped_art: Vec<String>,
    /// Project folders under Assets made by the author, kept while empty (`Assets/rocks`).
    asset_dirs: BTreeSet<String>,
    /// Terrain layers drawn with the mod's own picture: slot -> (art path, file on disk).
    surfaces: std::collections::BTreeMap<String, (String, std::path::PathBuf)>,
    /// Save also draws the in-game minimap from the edited ground (`map.minimap`).
    export_minimap: bool,
    /// A Project folder being renamed: its path and the name being typed.
    folder_rename: Option<(String, String)>,
    /// Double-clicked Project item shown large: entity, yaw, pitch.
    preview: Option<(String, f32, f32)>,
    preview_tex: Option<egui::TextureId>,
    /// Until when the viewport shows the zoom readout.
    zoom_shown: Option<Instant>,
    /// Metres per second while flying (right button + WASD); the wheel sets it.
    fly_speed: f32,
    /// Ground height the File > New blank map menu starts from.
    new_blank_height: f32,
    /// File > New chapter: keep the run's tile kinds and the fill pass, and
    /// wall off the ground's edges.
    new_keep_kinds: bool,
    new_edge_walls: bool,
    /// View > Walkability: blocked cells and every collision box.
    show_walk: bool,
    opened_doc: Option<Value>,
    unsaved: bool,
    last_autosave: Instant,
    restore_offer: Option<Autosave>,
    /// `unsaved` when the running save was sent, put back if it fails.
    save_dirty: bool,

    // drawing caches
    dirty: bool,
    shipped_draw: Vec<Vec<Instance>>,
    add_draw: Vec<(PartKey, Vec<Instance>)>,
    pickables: Vec<(String, Mat4, Sel)>,

    // view
    camera: Camera,
    left_tab: LeftTab,
    bottom_tab: BottomTab,
    show_bottom: bool,
    /// Inline rename in the Hierarchy: the object and the text being typed.
    renaming: Option<(Sel, String)>,
    /// Inline rename of a group: its index and the text being typed.
    renaming_group: Option<(usize, String)>,
    /// Single-object rows in on-screen order, last frame and this one
    /// (Shift-click selects a range of them).
    row_order: Vec<Sel>,
    row_order_now: Vec<Sel>,
    /// Where a Shift-click range starts.
    row_anchor: Option<Sel>,
    /// A drop on a Hierarchy row this frame: (the row's members, what was dropped).
    row_dropped: Option<(Vec<Sel>, Vec<Sel>)>,
    /// File > New empty chapter: strip the recipe once it has loaded.
    clean_pending: bool,
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
    show_palette: bool,
    palette_query: String,
    show_shortcuts: bool,
    show_save_as: bool,
    /// Play in game: launch once the save in flight (or the Save As) succeeds.
    play_after_save: bool,
    /// Play in game: rsmm is applying / launching.
    playing: bool,
    /// Every colour of the editor (View > Theme…).
    theme: crate::theme::Theme,
    show_theme: bool,
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
    /// --ui-click: press / release steps sent so far.
    ui_click_step: usize,
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
        let theme = prefs.theme.unwrap_or_else(|| legacy_theme(prefs.light_theme, prefs.accent.as_deref()));
        theme.apply(&cc.egui_ctx);
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
            models: Vec::new(),
            staged: HashMap::new(),
            staging: HashMap::new(),
            stage_seq: 0,
            open_seq: 0,
            requested: HashSet::new(),
            pending_assets: 0,
            failed_assets: Vec::new(),
            look_pending: false,
            minimap_tex: None,
            minimap_stale: false,
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
            xf_clip: None,
            renaming_inspector: false,
            tool: Tool::Select,
            gizmo: match opts.gizmo.as_deref() {
                Some("rotate") => GizmoMode::Rotate,
                Some("scale") => GizmoMode::Scale,
                _ => GizmoMode::Move,
            },
            modal: None,
            box_start: None,
            keep_edits: json!({}),
            keep_map: None,
            new_blank_height: 0.0,
            new_keep_kinds: false,
            new_edge_walls: true,
            show_walk: false,
            fly_speed: 25.0,
            project_dir: opts.project.clone().unwrap_or_else(|| crate::project::ASSETS.to_owned()),
            tile_size: 92.0,
            thumbs: HashMap::new(),
            thumb_want: Vec::new(),
            asset_sel: None,
            animate_fx: true,
            started: Instant::now(),
            scroll_to_sel: None,
            hover_pick: None,
            title: String::new(),
            tree_open: None,
            copy_menu_at: None,
            copied_until: None,
            dropped_art: Vec::new(),
            asset_dirs: BTreeSet::new(),
            surfaces: std::collections::BTreeMap::new(),
            export_minimap: false,
            folder_rename: None,
            preview: None,
            preview_tex: None,
            zoom_shown: None,
            opened_doc: None,
            unsaved: false,
            last_autosave: Instant::now(),
            restore_offer: None,
            save_dirty: false,
            dirty: true,
            shipped_draw: Vec::new(),
            add_draw: Vec::new(),
            pickables: Vec::new(),
            camera: Camera::default(),
            left_tab: LeftTab::Inspector,
            bottom_tab: BottomTab::Project,
            show_bottom: true,
            renaming: None,
            renaming_group: None,
            row_order: Vec::new(),
            row_order_now: Vec::new(),
            row_anchor: None,
            row_dropped: None,
            clean_pending: false,
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
            show_palette: false,
            palette_query: String::new(),
            show_shortcuts: false,
            show_save_as: false,
            play_after_save: false,
            playing: false,
            theme,
            show_theme: false,
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
            ui_click_step: 0,
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
        session::save_prefs(&Prefs { recent: self.recent.clone(), light_theme: self.theme.light, grid_size: Some(self.grid_size), accent: None, theme: Some(self.theme) });
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
        // A mod still opening would replace whatever this opens.
        self.open_seq += 1;
        self.chapter = Some(key.clone());
        self.tabs[0].title = key.replace('_', " ");
        self.dropped_art.clear();
        self.asset_dirs.clear();
        self.surfaces.clear();
        self.export_minimap = false;
        self.asset_sel = None;
        self.preview = None;
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
        self.restore_offer = if self.headless() { None } else { session::load_autosave(&key) }.filter(Autosave::has_edits);
        if let Some(r) = &mut self.renderer {
            r.clear_scene();
        }
        self.status = format!("Loading {key}…");
        let k = key.clone();
        self.spawn(ctx, move |be| {
            let t = be.get_json(&format!("/map/api/terrain?grid=1024&chapter={}", q(&k))).and_then(|v| Terrain::from_json(&v));
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
        if key.decal || key.vfx {
            if let Some(r) = &mut self.renderer {
                if !r.has_mesh(&key.mesh) {
                    r.add_mesh(key.mesh.clone(), if key.vfx { MeshData::particle_quads() } else { MeshData::decal_quad() });
                }
                if key.vfx && !r.has_mesh(VFX_PICK) {
                    r.add_mesh(VFX_PICK.to_owned(), MeshData::pick_box());
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
        if entity.starts_with(crate::edit::COLLIDER) {
            // Only collision boxes: drawn as lines, nothing to load.
            self.templates.insert(entity.to_owned(), Some(Vec::new()));
            self.dirty = true;
            return;
        }
        if let Some(rel) = entity.strip_prefix(crate::models::PREFIX) {
            // One of the mod's own models: drawn from the file on disk.
            let t = match self.models.iter().find(|m| m.mesh.rel == rel) {
                Some(m) => {
                    let key = m.part();
                    self.request_part_assets(&key);
                    vec![(key, vec![Mat4::IDENTITY])]
                }
                None => Vec::new(),
            };
            self.templates.insert(entity.to_owned(), Some(t));
            self.dirty = true;
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
        // A map clone's moves / removes live in `map.scenery`.
        let own = doc["map"]["scenery"].as_object().filter(|o| !o.is_empty()).map(|o| Value::Object(o.clone()));
        let (mut edits, skipped) = Edits::from_doc(own.as_ref().unwrap_or(&doc["scenery"]), s);
        let rest = edits.load_recipe(&doc["edits"], s, &kinds);
        self.keep_edits = Value::Object(rest);
        self.keep_map = doc.get("map").filter(|m| !m.is_null()).cloned();
        if let Some(st) = self.keep_map.as_ref().and_then(|m| m.get("terrain")) {
            edits.strokes = serde_json::from_value(st.clone()).unwrap_or_default();
        }
        if let Some(m) = self.keep_map.as_mut().and_then(|m| m.as_object_mut()) {
            m.remove("scenery");
        }
        if let Some(o) = self.keep_map.as_mut().and_then(|m| m.as_object_mut()).and_then(|m| m.remove("objects")) {
            let folder = std::path::PathBuf::from(doc["folder"].as_str().unwrap_or_default());
            self.register_mod_models(&o, &folder);
            self.asset_dirs.extend(doc["art_dirs"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|d| crate::project::folder_of_art(&format!("{d}/x"))));
            edits.load_objects(&o);
        }
        self.export_minimap = self.keep_map.as_mut().and_then(|m| m.as_object_mut()).and_then(|m| m.remove("minimap")).is_some();
        if let Some(sf) = self.keep_map.as_mut().and_then(|m| m.as_object_mut()).and_then(|m| m.remove("surfaces")) {
            let folder = std::path::PathBuf::from(doc["folder"].as_str().unwrap_or_default());
            for (slot, rel) in sf.as_object().into_iter().flatten() {
                let Some(rel) = rel.as_str() else { continue };
                let file = folder.join(rel);
                match std::fs::read(&file) {
                    Ok(b) => {
                        self.stage_now(rel.to_owned(), b);
                        self.surfaces.insert(slot.clone(), (rel.to_owned(), file));
                    }
                    Err(e) => self.failed_assets.push(format!("{rel}: {e}")),
                }
            }
            self.refresh_look();
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
                            if self.opts.empty {
                                self.start_empty(ctx, k, 0.0);
                            } else {
                                self.open_chapter(ctx, k);
                            }
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
                        self.terrain_base = Some(t.clone());
                        self.terrain = Some(t);
                        self.replay_terrain();
                        self.minimap_tex = self.terrain.as_ref().map(|t| minimap_texture(ctx, t));
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
                        if self.blank().is_some() || !self.edits.strokes.is_empty() {
                            self.replay_terrain(); // the paint follows the edits, not the shipped grids
                        }
                        for t in look.textures() {
                            if self.requested.insert(format!("{t}|false")) {
                                self.loader.request(Job::Texture(t, false));
                                self.pending_assets += 1;
                            }
                        }
                        self.refresh_look();
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
                        // The active tab, or the parked one the user switched away from.
                        let parked = self.tabs.iter_mut().filter_map(|t| t.parked.as_mut()).filter_map(|p| p.tile.as_mut()).find(|t| t.tile == tile);
                        if let Some(ts) = self.tile.as_mut().filter(|t| t.tile == tile).or(parked) {
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
                Event::Imported(paths) => self.import_files(ctx, paths),
                Event::Played(r) => {
                    self.playing = false;
                    self.status = match r {
                        Ok(msg) => format!("Ravenswatch is starting with {}: {msg}", self.mod_id),
                        Err(e) => format!("Play in game failed: {e}"),
                    };
                }
                Event::StampPicked(Some(f)) => match std::fs::read(&f).map_err(anyhow::Error::from).and_then(|b| crate::brush::Stamp::from_png(&crate::models::clean_stem(&f), &b)) {
                    Ok(st) => {
                        self.status = format!("Brush shape: {}", st.name);
                        self.brush.stamp = Some(st);
                    }
                    Err(e) => self.status = format!("{}: {e:#}", f.display()),
                },
                Event::StampPicked(None) | Event::SurfacePicked(_, None) => {}
                Event::SurfacePicked(slot, Some(f)) => self.set_surface(ctx, &slot, f),
                Event::Staged(rel, n, r) if self.staging.get(&rel) == Some(&n) => {
                    self.staging.remove(&rel);
                    match r {
                        Ok(id) => {
                            self.staged.insert(rel, id);
                        }
                        Err(e) => self.failed_assets.push(format!("import: {e:#}")),
                    }
                }
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
                Event::Opened(id, n, r) if n == self.open_seq => match r {
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
                        let id = self.mod_id.clone();
                        self.remember_recent(&id);
                        if !self.unsaved {
                            if let Some(ch) = &self.chapter {
                                session::clear_autosave(ch);
                            }
                        }
                        self.refresh_state(ctx);
                        if std::mem::take(&mut self.play_after_save) {
                            self.start_play(ctx);
                        }
                    } else {
                        self.status = format!("Check passed: {} change(s) build cleanly", changes.len());
                    }
                    self.result = Some(Ok(changes));
                } else {
                    if saving {
                        self.unsaved |= self.save_dirty;
                        self.play_after_save = false;
                    }
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
        if self.opts.self_check && !self.self_check_sent && self.pending.is_none() && !self.clean_pending {
            self.run_self_check(ctx);
        }
        if !self.opts.import.is_empty() && self.scenery.is_some() && self.terrain.is_some() && !self.clean_pending {
            let files = std::mem::take(&mut self.opts.import);
            self.import_files(ctx, files);
            if let Some(e) = self.models.first().map(|m| m.entity()) {
                let [x, z, _] = self.opts.view.unwrap_or([0.0, 0.0, 0.0]);
                let y = self.ground_y(x, z).unwrap_or(0.0);
                self.place(ctx, e, Vec3::new(x, y, z));
            }
        }
        if let Some(want) = self.opts.open_tile.clone() {
            if self.tile.is_none() && self.scenery.is_some() && self.terrain.is_some() && !self.pool.is_empty() {
                if let Some(t) = self.pool.iter().find(|t| t.name.contains(&want)).map(|t| t.path.clone()) {
                    self.opts.open_tile = None;
                    self.enter_tile(ctx, t, Vec3::ZERO);
                }
            }
        }
        if let (Some(want), Some(s)) = (self.opts.select.clone(), &self.scenery) {
            let tile_ready = self.opts.open_tile.is_none() && self.tile.as_ref().is_none_or(|t| t.built);
            if tile_ready {
                let want = want.to_lowercase();
                let hit = (0..s.obj_xf.len()).map(Sel::Obj).find(|x| self.name_of(*x).to_lowercase().contains(&want));
                if let Some(h) = hit {
                    self.opts.select = None;
                    self.select_only([h]);
                    self.scroll_to_sel = Some(h);
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
        // A wandering enemy camp, and a tilted walkable floor (a collider).
        if let Some(c) = self.props.iter().find(|p| p.entity.contains("Enemy_Camp_Wandering")).map(|p| p.entity.clone()) {
            self.place(ctx, c, a.pos + Vec3::new(12.0, 0.0, 6.0));
            if let Some(last) = self.edits.adds.last_mut() {
                last.camp = Some(crate::edit::Camp { difficulty: "Very_Hard".into(), power: [150.0, 300.0] });
            }
        }
        // Painted blocked ground and edge walls.
        self.edits.blocked.extend((0..6).flat_map(|x| (0..3).map(move |z| (x + 20, z + 20))));
        self.add_edge_walls();
        self.place(ctx, format!("{}floor", crate::edit::COLLIDER), a.pos + Vec3::new(6.0, 0.0, 12.0));
        if let Some(f) = self.edits.adds.last_mut() {
            f.col.get_or_insert_with(Vec::new)[0].rot = Vec3::new(12.0, 0.0, 0.0);
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
            scenery: self.edits.scenery_doc(s, true),
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
            blocked: self.edits.blocked.iter().copied().collect(),
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
        edits.blocked = a.blocked.iter().copied().collect();
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
        self.minimap_stale = true;
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
        let blank = self.blank_hides_shipped();
        (0..s.obj_xf.len())
            .filter(|i| !blank && !self.edits.removed.contains(i))
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

    /// File > New empty chapter on `k`, flat at `h` metres.
    fn start_empty(&mut self, ctx: &egui::Context, k: String, h: f32) {
        self.mod_id.clear();
        self.mod_name.clear();
        self.keep_edits = json!({});
        self.keep_map = Some(json!({"pool": {"add": [], "remove": []}, "place": {}, "terrain": [], "blank": h}));
        self.as_map = true;
        self.map_id = format!("Map_{}_Empty", k);
        self.clean_pending = true;
        self.open_chapter(ctx, k);
        self.restore_offer = None;
        self.status = format!("New empty chapter: flat ground at {h} m. Paint it, place props and spots, then File > Save As");
    }

    /// File > New chapter: once the recipe has loaded, place only the start
    /// and the boss. Every spot stays, so the start lands wherever the game
    /// normally puts it (removing spots left Dark Hills with no start: the
    /// hero never spawned); the fill pass that puts camps, blockers and props
    /// on leftover spots is turned off. One undo step, like any other edit.
    fn make_clean(&mut self) {
        if !self.clean_pending {
            return;
        }
        let (true, Some(r)) = (self.scenery.is_some(), self.recipe.clone()) else { return };
        if self.terrain.is_none() {
            return;
        }
        self.clean_pending = false;
        if self.new_edge_walls {
            self.add_edge_walls();
        }
        if self.new_keep_kinds {
            self.status = "New chapter: flat ground; the run's tiles still roll on the chapter's spots".into();
            return;
        }
        const KEEP: [&str; 2] = ["Start", "Map_Boss"];
        let kinds: Vec<(String, KindVals)> = r
            .kinds
            .iter()
            .map(|k| {
                let mut v = k.vals.clone();
                v.count = if KEEP.contains(&k.name.as_str()) { v.count.min(1) } else { 0 };
                (k.name.clone(), v)
            })
            .collect();
        let shipped: HashMap<String, KindVals> = r.kinds.iter().map(|k| (k.name.clone(), k.vals.clone())).collect();
        let fill: serde_json::Map<String, Value> = r.groups.iter().map(|g| (g.clone(), json!([]))).collect();
        self.change(|e, _| {
            for (name, v) in kinds {
                if shipped.get(&name) != Some(&v) {
                    e.kinds.insert(name, v);
                }
            }
        });
        if let Some(o) = self.keep_edits.as_object_mut() {
            o.insert("fill".into(), Value::Object(fill));
        }
        self.history.clear();
        self.layout.clear();
        self.show_spots = false;
        self.dirty = true;
        self.status = "Empty chapter: only the start and the boss are placed; no camps, crystals or fill".into();
    }

    /// The flat ground height of a blank map (`map.blank`), kept in the map
    /// clone document so it saves, autosaves and reopens with it.
    fn blank(&self) -> Option<f32> {
        self.keep_map.as_ref()?.get("blank")?.as_f64().map(|v| v as f32)
    }

    /// A blank map sinks the chapter's shipped objects; a tile tab keeps its own.
    fn blank_hides_shipped(&self) -> bool {
        self.tile.is_none() && self.blank().is_some()
    }

    /// Paste `items` (entity, level, transform relative to their centre) with
    /// their centre at `at`; selects what was pasted.
    fn paste_items(&mut self, ctx: &egui::Context, items: Vec<(String, String, Xf, Option<crate::edit::Add>)>, at: Vec3) -> usize {
        let allowed: HashSet<&str> = self.props.iter().map(|p| p.entity.as_str()).collect();
        let (ok, refused): (Vec<_>, Vec<_>) = items
            .into_iter()
            .partition(|(e, _, _, _)| allowed.contains(e.as_str()) || e.starts_with(crate::models::PREFIX) || e.starts_with(crate::edit::COLLIDER));
        for (e, _, _, _) in &ok {
            self.ensure_template(ctx, e);
        }
        let mut new = Vec::new();
        let snap_ground = self.snap_ground;
        let ground: Vec<Option<f32>> = ok.iter().map(|(_, _, x, _)| self.ground_y(at.x + x.pos.x, at.z + x.pos.z)).collect();
        self.change(|e, _| {
            for ((entity, level, x, src), gy) in ok.into_iter().zip(ground) {
                let (col, camp) = src.map(|a| (a.col, a.camp)).unwrap_or_default();
                let mut pos = at + x.pos;
                if snap_ground {
                    if let Some(g) = gy {
                        pos.y = g + x.pos.y;
                    }
                }
                new.push(e.push_add(Add { uid: 0, entity, level, pos, rot: x.rot, scale: x.scale.clamp(Vec3::splat(0.05), Vec3::splat(20.0)), name: String::new(), col, camp }));
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
                let col = match x {
                    Sel::Add(u) => self.edits.add(*u).cloned(),
                    _ => None,
                };
                Some((e, l, Xf { pos: Vec3::new(xf.pos.x - p.x, lift, xf.pos.z - p.z), ..xf }, col))
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
        let col = crate::edit::default_cols(&entity);
        self.change(|e, _| sel = Some(e.push_add(Add { uid: 0, entity, level, pos: at, rot: Quat::IDENTITY, scale: Vec3::ONE, name: String::new(), col, camp: None })));
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
        if self.tile.is_some() {
            return; // a tile tab stands on its own flat ground, not the map's
        }
        let Some(base) = &self.terrain_base else { return };
        let mut t = base.clone();
        if let Some(h) = self.blank() {
            // What `blank` writes: flat ground, no water, nothing blocked.
            let h = h.clamp(t.min.y, t.max.y);
            t.h.fill(h);
            t.water = None;
            t.block = None;
            t.path = None;
        }
        let mut w = self.look_base.as_ref().map(|l| l.weights.clone()).unwrap_or_default();
        let mut c = self.look_base.as_ref().and_then(|l| l.color.clone());
        if self.blank().is_some() {
            // What `blank` writes: no painted surfaces (the base material
            // everywhere) and a white vertex colour.
            w.fill(0);
            if let Some(c) = c.as_mut() {
                c.fill(255);
            }
        }
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
        if self.brush.mode == BrushMode::Block && !smooth {
            // Blocked ground: cells under the brush on (Ctrl: off).
            let (c, r) = (crate::edit::BLOCK_CELL, self.brush.radius);
            let k = |v: f32| (v / c).floor() as i32;
            let mut n = 0;
            for x in k(at.x - r)..=k(at.x + r) {
                for z in k(at.z - r)..=k(at.z + r) {
                    let mid = Vec3::new((x as f32 + 0.5) * c, at.y, (z as f32 + 0.5) * c);
                    if (mid - at).with_y(0.0).length() <= r {
                        n += usize::from(if invert { self.edits.blocked.remove(&(x, z)) } else { self.edits.blocked.insert((x, z)) });
                    }
                }
            }
            if n > 0 {
                self.unsaved = true;
            }
            return;
        }
        let sources = self.look_base.as_ref().map(|l| l.channel_sources.clone()).unwrap_or_default();
        let color_layer = self.look_base.as_ref().and_then(|l| l.color_layer.clone());
        let mut b = self.brush.clone();
        if smooth {
            b.mode = BrushMode::Smooth;
        }
        let dabs = b.strokes(at, invert, &sources, color_layer.as_deref());
        if dabs.is_empty() {
            if b.stroke(at, invert, &sources, color_layer.as_deref()).is_none() {
                self.status = "This brush needs the chapter's terrain material (still loading?)".into();
            }
            return;
        }
        let Some(t) = &mut self.terrain else { return };
        for st in dabs {
            let rows = Self::apply_stroke_to(t, self.look_base.as_ref(), &mut self.weights_now, self.color_now.as_mut(), &st);
            if let Some(r) = &mut self.renderer {
                match rows {
                    Some((r0, r1)) => r.update_terrain_rows(t, r0, r1),
                    None => r.update_look_maps(&self.weights_now, self.color_now.as_deref()),
                }
            }
            self.edits.strokes.push(st);
        }
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
        self.left_tab = LeftTab::Inspector;
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
            .map(|a| json!({"entity": a.entity, "pos": r3(a.pos - ts.origin), "yaw": a.yaw(), "scale": a.scale.x}))
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
            Cmd::Rename => match self.active.or_else(|| self.sel.iter().next().copied()) {
                Some(Sel::Add(u)) => {
                    let name = self.edits.add(u).map(Add::name).unwrap_or_default();
                    self.renaming = Some((Sel::Add(u), name));
                }
                Some(_) => self.status = "The game's own objects keep their names: rename the objects you added".into(),
                None => self.status = "Select an object you added to rename it".into(),
            },
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
                    self.status = format!("Pasted {n} object(s): move the mouse, click to drop");
                    if !self.headless() {
                        // The paste follows the mouse until a click, like Duplicate.
                        self.begin(ctx, Kind::Grab, false);
                    }
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
                    // Type its name, Enter to keep it.
                    self.renaming_group = Some((self.groups.len() - 1, name.clone()));
                    self.status = format!("{name}: {} object(s)", self.sel.len());
                    self.left_tab = LeftTab::Inspector;
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
            Cmd::ToggleLog => {
                self.show_bottom = !(self.show_bottom && self.bottom_tab == BottomTab::Console);
                self.bottom_tab = BottomTab::Console;
            }
            Cmd::ToggleTheme => {
                // Dark <-> light, keeping the accent; View > Theme… for the rest.
                let accent = self.theme.accent;
                self.theme = crate::theme::PRESETS[if self.theme.light { 0 } else { 1 }].1;
                self.theme.accent = accent;
                self.theme.apply(ctx);
                self.save_prefs();
            }
            Cmd::Palette => {
                self.show_palette = !self.show_palette;
                self.palette_query.clear();
            }
            Cmd::Shortcuts => self.show_shortcuts = !self.show_shortcuts,
            Cmd::Play => {
                if self.playing || self.headless() {
                    return;
                }
                if self.tile.is_some() {
                    self.status = "Play in game: go back to the map tab first".into();
                    return;
                }
                // Always save first: the game runs what is on disk.
                self.play_after_save = true;
                self.status = "Play in game: saving…".into();
                self.run(ctx, Cmd::Save);
            }
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
        let basis = if kind == Kind::Scale { self.gizmo_spec(GizmoMode::Scale).basis } else { Quat::IDENTITY };
        self.modal = Some(Modal { kind, lock, starts, pivot, p0, g0: self.cursor, by_drag, typed: String::new(), basis, ring: None });
    }

    /// The gizmo's handles for the current selection (see `GizmoSpec`).
    fn gizmo_spec(&self, mode: GizmoMode) -> GizmoSpec {
        // A tile's `places` save a yaw and one scale only; the map's own objects take any.
        let adds_only = self.tile.is_some() && !self.sel.is_empty() && self.sel.iter().all(|s| matches!(s, Sel::Add(_)));
        let lead = self.active.filter(|a| self.sel.contains(a)).or_else(|| self.sel.iter().next().copied());
        let basis = match (mode, &self.scenery, lead) {
            (GizmoMode::Scale, Some(s), Some(l)) => self.edits.xf(s, l).map_or(Quat::IDENTITY, |x| x.rot.normalize()),
            _ => Quat::IDENTITY,
        };
        let axes = match mode {
            GizmoMode::Move => [true; 3],
            GizmoMode::Rotate if adds_only => [false, true, false],
            GizmoMode::Rotate => [true; 3],
            GizmoMode::Scale => [!adds_only; 3],
        };
        GizmoSpec { mode, basis, axes, planes: mode == GizmoMode::Move || (mode == GizmoMode::Scale && !adds_only) }
    }

    /// The live scale factor: typed, else along the handle grabbed (a gizmo
    /// drag, `GIZMO_PX` further = twice the size), else (S key) the pointer's
    /// distance from the pivot against where it began.
    fn scale_factor(m: &Modal, pointer: egui::Pos2, pivot_scr: Option<egui::Pos2>, axis_px: [egui::Vec2; 3], snap: bool) -> f32 {
        if let Ok(v) = m.typed.parse::<f32>() {
            return v;
        }
        let unit = |v: egui::Vec2| if v.length() > 1e-3 { v.normalized() } else { egui::Vec2::ZERO };
        let mut k = if m.by_drag {
            let dir = match m.lock {
                Lock::Axis(a) => unit(axis_px[a]),
                Lock::Plane(skip) => {
                    let (i, j) = others(skip);
                    unit(unit(axis_px[i]) + unit(axis_px[j]))
                }
                // Up / right grows, down / left shrinks.
                Lock::Free => egui::vec2(1.0, -1.0).normalized(),
            };
            1.0 + (pointer - m.p0).dot(dir) / GIZMO_PX
        } else {
            match pivot_scr {
                Some(c) => (pointer - c).length() / (m.p0 - c).length().max(4.0),
                None => 1.0,
            }
        };
        if snap {
            k = (k * 10.0).round() / 10.0;
        }
        k.max(0.01)
    }

    /// The live turn in radians about the locked axis (+axis, right-handed).
    fn rotate_angle(&self, m: &Modal, pointer: egui::Pos2, pivot_scr: Option<egui::Pos2>, snap: bool) -> f32 {
        if let Ok(v) = m.typed.parse::<f32>() {
            return v.to_radians();
        }
        let axis = match m.lock {
            Lock::Axis(k) | Lock::Plane(k) => k.min(2),
            Lock::Free => 1,
        };
        let mut a = if let Some((_, tan)) = m.ring {
            // Along the ring: one ring radius of drag = one radian.
            let sign = GizmoSpec { mode: GizmoMode::Rotate, basis: m.basis, axes: [true; 3], planes: false }.ring_frame(axis).2;
            (pointer - m.p0).dot(tan) / GIZMO_PX * sign
        } else {
            // The R key: the mouse circling the pivot, turned the way it circles
            // on screen (flipped when the axis points away from the camera).
            let mut a = match pivot_scr {
                Some(c) => -((pointer - c).angle() - (m.p0 - c).angle()),
                None => 0.0,
            };
            let mut dir = Vec3::ZERO;
            dir[axis] = 1.0;
            if dir.dot(self.camera.eye() - m.pivot) < 0.0 {
                a = -a;
            }
            a
        };
        if snap {
            let st = 15f32.to_radians();
            a = (a / st).round() * st;
        }
        a
    }

    fn cancel_modal(&mut self) {
        if let Some(m) = self.modal.take() {
            self.set_live(&m.starts);
            // The history entry pushed when the transform began is now a no-op.
            self.undo();
        }
    }

    /// The new transforms for the current pointer / typed value. `rays` are
    /// the view rays through where the drag began and the pointer now.
    #[allow(clippy::too_many_arguments)]
    fn modal_xfs(
        &self,
        m: &Modal,
        pointer: egui::Pos2,
        ground: Option<Vec3>,
        pivot_scr: Option<egui::Pos2>,
        axis_px: [egui::Vec2; 3],
        rays: [(Vec3, Vec3); 2],
        snap: bool,
    ) -> Vec<(Sel, Xf)> {
        let typed: Option<f32> = m.typed.parse().ok();
        let mask = m.lock.mask();
        let drag = pointer - m.p0;
        // Fallback for an axis seen end-on: the drag projected on its screen
        // direction (Blender).
        let along_screen = |a: usize| {
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
                    // The point on the axis nearest each view ray: the grabbed
                    // arm stays under the pointer, in perspective too.
                    Lock::Axis(a) => {
                        let mut dir = Vec3::ZERO;
                        dir[a] = 1.0;
                        let mut d = Vec3::ZERO;
                        d[a] = match (line_ray_param(m.pivot, dir, rays[0]), line_ray_param(m.pivot, dir, rays[1])) {
                            (Some(s0), Some(s1)) => s1 - s0,
                            _ => along_screen(a),
                        };
                        d
                    }
                    // Where each view ray meets the plane through the pivot.
                    Lock::Plane(skip) => {
                        let mut n = Vec3::ZERO;
                        n[skip] = 1.0;
                        let hit = |(o, dv): (Vec3, Vec3)| {
                            let den = dv.dot(n);
                            (den.abs() > 0.02).then(|| o + dv * ((m.pivot - o).dot(n) / den)).filter(|p| (*p - o).dot(dv) > 0.0)
                        };
                        match (hit(rays[0]), hit(rays[1])) {
                            (Some(a), Some(b)) => b - a,
                            _ => Vec3::ZERO,
                        }
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
                let a = self.rotate_angle(m, pointer, pivot_scr, snap && typed.is_none());
                let axis = match m.lock {
                    Lock::Axis(k) | Lock::Plane(k) => k.min(2),
                    Lock::Free => 1,
                };
                let r = Quat::from_axis_angle(m.basis * [Vec3::X, Vec3::Y, Vec3::Z][axis], a);
                m.starts
                    .iter()
                    .map(|(sel, st)| (*sel, Xf { pos: m.pivot + r * (st.pos - m.pivot), rot: (r * st.rot).normalize(), ..*st }))
                    .collect()
            }
            Kind::Scale => {
                let k = Self::scale_factor(m, pointer, pivot_scr, axis_px, snap && typed.is_none());
                // Per axis of `basis` (the active object's own axes).
                let kv = Vec3::ONE + (Vec3::splat(k) - Vec3::ONE) * mask;
                let inv = m.basis.inverse();
                m.starts
                    .iter()
                    .map(|(sel, st)| {
                        let scale = (st.scale * kv).clamp(Vec3::splat(0.05), Vec3::splat(20.0));
                        (*sel, Xf { pos: m.pivot + m.basis * ((inv * (st.pos - m.pivot)) * kv), scale, ..*st })
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
        // A blank map sinks every shipped object (`map.blank`).
        let blank = self.blank_hides_shipped();
        for p in &s.parts {
            let mut v = Vec::with_capacity(p.base.len());
            for (j, base) in p.base.iter().enumerate() {
                let ow = p.owners[j] as usize;
                let sel = Sel::Obj(ow);
                if blank || self.edits.removed.contains(&ow) || self.hidden.contains(&sel) {
                    continue;
                }
                let (m, tint) = match deltas.get(&ow) {
                    Some(d) => (*d * *base, MOVED_TINT),
                    None => (*base, [0.0; 4]),
                };
                v.push(Instance { m: m.to_cols_array_2d(), tint: tint_for(sel, tint) });
                if !p.key.decal && !p.key.vfx && meshes_ready(&p.key.mesh) && !self.locked.contains(&sel) {
                    self.pickables.push((p.key.mesh.clone(), m, sel));
                } else if p.key.vfx && meshes_ready(VFX_PICK) && !self.locked.contains(&sel) {
                    // An effect is clicked through an invisible box around it.
                    self.pickables.push((VFX_PICK.to_owned(), vfx_pick_mat(m), sel));
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
            if meshes_ready(VFX_PICK) && !self.locked.contains(&sel) {
                if a.entity.starts_with(crate::edit::COLLIDER) {
                    // Clicked through its boxes (the pick box stands on y = 0).
                    for b in a.col.iter().flatten() {
                        let m = world * b.mat() * Mat4::from_scale_rotation_translation(b.size, Quat::IDENTITY, Vec3::new(0.0, -b.size.y * 0.5, 0.0));
                        self.pickables.push((VFX_PICK.to_owned(), m, sel));
                    }
                } else if !t.iter().any(|(k, _)| !k.decal && !k.vfx) {
                    // A spawner / camp: clicked through a box at its marker.
                    let m = Mat4::from_scale_rotation_translation(Vec3::new(3.0, 3.0, 3.0), Quat::IDENTITY, a.pos);
                    self.pickables.push((VFX_PICK.to_owned(), m, sel));
                }
            }
            for (key, mats) in t {
                for local in mats {
                    let m = world * *local;
                    by_key.entry(key.clone()).or_default().push(Instance { m: m.to_cols_array_2d(), tint: tint_for(sel, ADDED_TINT) });
                    if !key.decal && !key.vfx && meshes_ready(&key.mesh) && !self.locked.contains(&sel) {
                        self.pickables.push((key.mesh.clone(), m, sel));
                    } else if key.vfx && meshes_ready(VFX_PICK) && !self.locked.contains(&sel) {
                        self.pickables.push((VFX_PICK.to_owned(), vfx_pick_mat(m), sel));
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
                        if !key.decal && !key.vfx && meshes_ready(&key.mesh) && !self.locked.contains(&sel) {
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
            // Effects and colliders are clicked through stand-in boxes: no outline.
            if !self.sel.contains(s) || mesh == VFX_PICK {
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
        if let (Some(entity), Some(g)) = (resp.dnd_release_payload::<String>().filter(|e| !e.starts_with("folder:")), hover_ground) {
            self.place(&ctx, (*entity).clone(), g);
        }
        // The object under a still cursor, named beside it (re-picked only when the cursor moves).
        let idle = self.modal.is_none() && self.tool == Tool::Select && self.box_start.is_none() && !ctx.input(|i| i.pointer.any_down());
        match pointer.filter(|p| idle && resp.hovered() && rect.contains(*p)) {
            Some(p) => {
                if self.hover_pick.is_none_or(|(q, _)| (q - p).length() > 1.0) {
                    let (o, d) = ray_at(p);
                    self.hover_pick = Some((p, self.pick_any(o, d)));
                }
                if let Some((_, Some(h))) = self.hover_pick {
                    let name = self.name_of(h).replace('_', " ");
                    let g = ui.painter().layout_no_wrap(name, egui::FontId::proportional(12.5), egui::Color32::WHITE);
                    let at = p + egui::vec2(16.0, 14.0);
                    ui.painter().rect_filled(egui::Rect::from_min_size(at - egui::vec2(5.0, 3.0), g.size() + egui::vec2(10.0, 6.0)), 4.0, egui::Color32::from_black_alpha(170));
                    ui.painter().galley(at, g, egui::Color32::WHITE);
                }
            }
            None => self.hover_pick = None,
        }
        if hover_ground.is_some() {
            self.cursor = hover_ground;
        }
        let (shift, cmd_key, alt) = ctx.input(|i| (i.modifiers.shift, i.modifiers.command, i.modifiers.alt));
        let snap_now = self.snap ^ cmd_key;

        // ---- camera ----
        let before = self.camera;
        // Unity's Scene view: right-drag looks around, Alt+left-drag orbits,
        // middle-drag pans, Alt+right-drag and the wheel zoom.
        if resp.dragged_by(egui::PointerButton::Secondary) && self.modal.is_none() {
            let d = resp.drag_delta();
            if alt {
                self.camera.zoom(-d.y * 3.0 + d.x * 3.0);
            } else {
                self.camera.look(d.x, d.y);
            }
        }
        if alt && resp.dragged_by(egui::PointerButton::Primary) && self.modal.is_none() {
            let d = resp.drag_delta();
            self.camera.orbit(d.x, d.y);
        }
        if resp.dragged_by(egui::PointerButton::Middle) {
            let d = resp.drag_delta();
            self.camera.pan_screen(d.x, d.y, rect.height());
        }
        if resp.hovered() {
            let scroll = ctx.input(|i| i.smooth_scroll_delta);
            let amount = if scroll.y != 0.0 { scroll.y } else { scroll.x };
            if amount != 0.0 {
                if ctx.input(|i| i.pointer.secondary_down()) {
                    // Unity: the wheel while flying sets the fly speed.
                    self.fly_speed = (self.fly_speed * (1.0 + amount * 0.002)).clamp(1.0, 500.0);
                    self.status = format!("Fly speed {:.0} m/s", self.fly_speed);
                } else {
                    self.camera.zoom(amount);
                    self.zoom_shown = Some(Instant::now() + Duration::from_millis(1200));
                }
            }
        }
        let typing = ctx.egui_wants_keyboard_input();
        if !typing && self.modal.is_none() {
            let (dt, rmb) = ctx.input(|i| (i.stable_dt.min(0.1), i.pointer.secondary_down()));
            let key = |k: egui::Key| ctx.input(|i| i.key_down(k));
            // Unity: hold the right button, then WASD flies where you look,
            // Q / E go down / up, Shift is faster. Arrows move on the ground
            // plane at any time.
            let fly = rmb && !alt;
            let speed = if fly { self.fly_speed } else { self.camera.dist * 1.2 } * dt * if shift { 3.0 } else { 1.0 };
            let ground_f = Vec3::new(self.camera.forward().x, 0.0, self.camera.forward().z).normalize_or(Vec3::Z);
            let f = if fly { self.camera.forward() } else { ground_f };
            let rgt = self.camera.right();
            let mut mv = Vec3::ZERO;
            if key(egui::Key::ArrowUp) || (fly && key(egui::Key::W)) {
                mv += f;
            }
            if key(egui::Key::ArrowDown) || (fly && key(egui::Key::S) && !cmd_key) {
                mv -= f;
            }
            if key(egui::Key::ArrowLeft) || (fly && key(egui::Key::A)) {
                mv -= rgt;
            }
            if key(egui::Key::ArrowRight) || (fly && key(egui::Key::D) && !cmd_key) {
                mv += rgt;
            }
            if fly && key(egui::Key::E) || key(egui::Key::PageUp) {
                mv += Vec3::Y;
            }
            if fly && key(egui::Key::Q) || key(egui::Key::PageDown) {
                mv -= Vec3::Y;
            }
            if mv != Vec3::ZERO {
                self.camera.fly(mv.normalize() * speed);
            }
        }
        if self.camera != before {
            ctx.request_repaint();
        }

        // ---- gizmo hit test (before clicks) ----
        let pivot = self.pivot();
        let pivot_scr = pivot.and_then(&to_screen);
        // ~90 px on screen whatever the zoom: metres per pixel at the pivot's depth.
        let depth_half = match pivot {
            Some(p) if !self.camera.ortho => (p - self.camera.eye()).dot(self.camera.forward()).max(NEAR_GIZMO) * (self.camera.fov_y * 0.5).tan(),
            _ => self.camera.half_height(),
        };
        let px_m = 2.0 * depth_half / rect.height();
        let handle_len = GIZMO_PX * px_m;
        let spec = self.gizmo_spec(self.gizmo);
        let gizmo_hover = match (pivot, pivot_scr, pointer, self.modal.is_none() && self.tool == Tool::Select) {
            (Some(p), Some(c), Some(ptr), true) => gizmo_hit(&spec, p, c, ptr, handle_len, self.camera.eye(), &to_screen),
            _ => None,
        };
        // Around the gizmo a near miss of a handle must not grab or select
        // whatever stands behind it: the handles' own area (90 px + a margin).
        let gizmo_live = !self.sel.is_empty() && self.modal.is_none() && self.tool == Tool::Select;
        let in_gizmo_zone = move |ptr: egui::Pos2| -> bool { gizmo_live && pivot_scr.is_some_and(|c| (ptr - c).length() < GIZMO_PX + 22.0) };

        // ---- pointer ----
        if resp.drag_started_by(egui::PointerButton::Primary) && self.modal.is_none() && self.tool == Tool::Select && !alt {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(h) = gizmo_hover {
                    let kind = match self.gizmo {
                        GizmoMode::Move => Kind::Grab,
                        GizmoMode::Rotate => Kind::Rotate,
                        GizmoMode::Scale => Kind::Scale,
                    };
                    self.cursor = ground_at(self.terrain.as_ref(), p);
                    self.begin_locked(&ctx, kind, h, true);
                    // A ring: the turn follows the drag along it from here.
                    if let (Kind::Rotate, Lock::Axis(a), Some(pv), Some(c), Some(m)) = (kind, h, pivot, pivot_scr, self.modal.as_mut()) {
                        let v = gizmo_view(&spec, pv, c, handle_len, self.camera.eye(), &to_screen);
                        m.p0 = p;
                        m.ring = ring_grab(&v, a, p);
                    }
                } else {
                    let (o, d) = ray_at(p);
                    match self.pick_any(o, d) {
                        // Dragging what is already selected moves it.
                        Some(h) if self.sel.contains(&h) => {
                            self.cursor = ground_at(self.terrain.as_ref(), p);
                            self.begin(&ctx, Kind::Grab, true);
                        }
                        // A near miss of a gizmo handle: nothing behind it moves or gets picked.
                        _ if in_gizmo_zone(p) => {
                            self.status = "Missed the handle: drag an arrow, a ring or the centre".into();
                        }
                        // Dragging something else selects it; it moves on the next drag
                        // (or with G / the gizmo), so a stray drag never moves a neighbour.
                        Some(h) => {
                            if shift {
                                self.sel.insert(h);
                                self.active = Some(h);
                            } else {
                                self.select_only([h]);
                            }
                        }
                        None => self.box_start = Some(p),
                    }
                }
            }
        }
        // Terrain brush: paint while the left button is held.
        if self.tool == Tool::Brush && self.modal.is_none() {
            let down = !alt && (ctx.input(|i| i.pointer.primary_down()) && resp.hovered() || resp.dragged_by(egui::PointerButton::Primary));
            if down && !ctx.input(|i| i.pointer.secondary_down()) {
                if let Some(g) = hover_ground {
                    if self.last_dab.is_none() {
                        self.history.push(&self.edits); // one undo step per stroke
                        if self.brush.mode == BrushMode::Layer {
                            self.brush.level = self.brush.layer_target(g.y, cmd_key);
                            self.status = format!("Layer: painting a flat terrace at {:.1} m", self.brush.level);
                        }
                    }
                    let spacing = (self.brush.radius * 0.3).max(0.25);
                    if self.last_dab.is_none_or(|l| (l - g).with_y(0.0).length() >= spacing) {
                        self.dab(g, cmd_key, shift);
                        self.last_dab = Some(g);
                    }
                }
                ctx.request_repaint();
            } else {
                if self.last_dab.take().is_some() {
                    self.minimap_stale = true; // a stroke ended: redraw the minimap
                }
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
                    ui.visuals().selection.bg_fill.gamma_multiply(0.1),
                    egui::Stroke::new(1.0, ui.visuals().selection.bg_fill),
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
            // Pixels per metre along each axis of the transform's frame.
            let axis_px: [egui::Vec2; 3] = std::array::from_fn(|a| {
                let dir = m.basis * [Vec3::X, Vec3::Y, Vec3::Z][a];
                match (ps, to_screen(m.pivot + dir * 0.1 * handle_len)) {
                    (Some(c), Some(t)) => (t - c) / (0.1 * handle_len),
                    _ => egui::Vec2::ZERO,
                }
            });
            let xfs = self.modal_xfs(&g0_fixed, p, g, ps, axis_px, [ray_at(m.p0), ray_at(p)], snap_now);
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
        } else if resp.clicked() && !alt {
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
                        let hit = self.pick_any(o, d);
                        // A click just off a gizmo handle keeps the selection.
                        let near_miss = in_gizmo_zone(p) && hit.is_none_or(|h| !self.sel.contains(&h));
                        match hit {
                            _ if near_miss => {}
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
        // Collision boxes: a selected model copy's, and every collider's (they
        // have nothing else to show). Things with nothing to draw (enemy
        // camps, spawners) get a marker.
        for a in &self.edits.adds {
            let sel = Sel::Add(a.uid);
            if self.hidden.contains(&sel) {
                continue;
            }
            let on = self.sel.contains(&sel);
            let collider = a.entity.starts_with(crate::edit::COLLIDER);
            if on || collider {
                if let Some((boxes, xf, _)) = self.obj_cols(a.uid) {
                    for b in &boxes {
                        let c = if b.ground { [0.35, 1.0, 0.45, 1.0] } else { [1.0, 0.55, 0.15, 1.0] };
                        let c = if on { c } else { [c[0], c[1], c[2], 0.55] };
                        box_lines(&mut top, xf * b.mat(), -b.size * 0.5, b.size * 0.5, c);
                    }
                }
            } else if self.draws_nothing(&a.entity) {
                let c = if on { [1.0, 0.85, 0.4, 1.0] } else { [1.0, 0.3, 0.25, 0.9] };
                ring(&mut top, a.pos, 2.0, c, |_, _| Some(a.pos.y + 0.1));
                top.push(LineVertex { pos: a.pos.into(), col: c });
                top.push(LineVertex { pos: (a.pos + Vec3::Y * 3.0).into(), col: c });
            }
        }
        // Blocked ground, and (View > Walkability) every copy's collision too.
        let walk = self.show_walk || (self.tool == Tool::Brush && self.brush.mode == BrushMode::Block);
        if walk {
            let c = crate::edit::BLOCK_CELL;
            let col = [1.0, 0.2, 0.2, 0.9];
            for &(x, z) in &self.edits.blocked {
                let (x0, z0) = (x as f32 * c, z as f32 * c);
                let y = |px: f32, pz: f32| self.ground_y(px, pz).unwrap_or(0.0) + 0.15;
                let q = [(x0, z0), (x0 + c, z0), (x0 + c, z0 + c), (x0, z0 + c)];
                for k in 0..4 {
                    let (a, b) = (q[k], q[(k + 1) % 4]);
                    top.push(LineVertex { pos: [a.0, y(a.0, a.1), a.1], col });
                    top.push(LineVertex { pos: [b.0, y(b.0, b.1), b.1], col });
                }
                top.push(LineVertex { pos: [x0, y(x0, z0), z0], col });
                top.push(LineVertex { pos: [x0 + c, y(x0 + c, z0 + c), z0 + c], col });
            }
            if self.show_walk {
                for a in &self.edits.adds {
                    if self.sel.contains(&Sel::Add(a.uid)) || a.entity.starts_with(crate::edit::COLLIDER) {
                        continue; // drawn above
                    }
                    if let Some((boxes, xf, _)) = self.obj_cols(a.uid) {
                        for b in &boxes {
                            let c = if b.ground { [0.35, 1.0, 0.45, 0.6] } else { [1.0, 0.55, 0.15, 0.6] };
                            box_lines(&mut top, xf * b.mat(), -b.size * 0.5, b.size * 0.5, c);
                        }
                    }
                }
            }
        }
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
        self.make_thumbs(rs);
        let mut parts: Vec<DrawPart> = Vec::new();
        if let Some(s) = &self.scenery {
            parts.extend(s.parts.iter().zip(&self.shipped_draw).map(|(p, inst)| DrawPart { key: &p.key, instances: inst }));
        }
        parts.extend(self.add_draw.iter().map(|(k, inst)| DrawPart { key: k, instances: inst }));
        let play_fx = self.animate_fx && !self.headless();
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
                no_terrain: false,
                sky: self.theme.scene_linear(),
                time: if play_fx { self.started.elapsed().as_secs_f32() } else { 1.0 },
            },
        );
        if play_fx && renderer.stats.vfx > 0 {
            ctx.request_repaint(); // effects move
        }
        self.frame_ms = self.frame_ms * 0.9 + t0.elapsed().as_secs_f32() * 1000.0 * 0.1;
        ui.painter().image(tex, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);

        if let Some(path) = self.opts.ui_snapshot.clone() {
            let tile_ok = self.opts.open_tile.is_none() && self.tile.as_ref().is_none_or(|t| t.built);
            let ready = tile_ok && self.terrain.is_some() && self.scenery.is_some() && self.pending_assets == 0 && !self.look_pending;
            self.settled_frames = if ready { self.settled_frames + 1 } else { 0 };
            ctx.request_repaint();
            if self.settled_frames == 3 {
                // Something to show: the objects nearest the view centre.
                if let (Some(s), Some([x, z, _]), true) = (&self.scenery, self.opts.view, self.sel.is_empty()) {
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
            if self.settled_frames >= 12 && self.ui_click_step >= 20 * self.opts.ui_clicks.len() + 4 && !self.ui_shot_sent {
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
        // What the mouse is doing, as a small badge beside the cursor.
        let zooming = ctx.input(|i| i.smooth_scroll_delta.y != 0.0 || i.zoom_delta() != 1.0) && resp.hovered();
        let badge = if let Some(m) = &self.modal {
            Some(match m.kind {
                Kind::Grab => Icon::Move,
                Kind::Rotate => Icon::Rotate,
                Kind::Scale => Icon::Scale,
            })
        } else if egui::DragAndDrop::has_any_payload(&ctx) && resp.contains_pointer() {
            Some(Icon::Pin)
        } else if resp.dragged_by(egui::PointerButton::Secondary) {
            Some(if alt { Icon::Search } else { Icon::Eye(true) })
        } else if resp.dragged_by(egui::PointerButton::Middle) {
            Some(Icon::Hand)
        } else if resp.dragged_by(egui::PointerButton::Primary) && alt {
            Some(Icon::Orbit)
        } else if zooming {
            Some(Icon::Search)
        } else if resp.hovered() && self.tool == Tool::Brush {
            Some(Icon::Brush)
        } else if resp.hovered() && matches!(self.tool, Tool::Place(_) | Tool::AddSpot(_)) {
            Some(Icon::Pin)
        } else {
            None
        };
        // Moving the camera: the icon takes the pointer's place (hidden), drawn
        // at its newest position so it does not trail behind.
        // Every mode icon takes the pointer's place (hidden), outlined like a cursor.
        if let (Some(icon), Some(at)) = (badge, ctx.input(|i| i.pointer.latest_pos()).or(pointer)) {
            ctx.set_cursor_icon(egui::CursorIcon::None);
            icons::cursor_badge(&painter, at, icon, ui.visuals().selection.bg_fill, true);
        }
        // Live values of the transform in progress (the gizmo stretches by `k`).
        let live = self.modal.as_ref().zip(pointer).map(|(m, ptr)| {
            let ps = to_screen(m.pivot).or(pivot_scr);
            let axis_px: [egui::Vec2; 3] = std::array::from_fn(|a| {
                let dir = m.basis * [Vec3::X, Vec3::Y, Vec3::Z][a];
                match (ps, to_screen(m.pivot + dir * 0.1 * handle_len)) {
                    (Some(c), Some(t)) => (t - c) / (0.1 * handle_len),
                    _ => egui::Vec2::ZERO,
                }
            });
            let snap = snap_now && m.typed.is_empty();
            (Self::scale_factor(m, ptr, ps, axis_px, snap), self.rotate_angle(m, ptr, ps, snap))
        });
        if let (Some(p), Some(c), Tool::Select) = (pivot, pivot_scr, &self.tool) {
            // Kept on screen while dragging, the handle in use lit up.
            let hot = self.modal.as_ref().map(|m| m.lock).or(gizmo_hover);
            let mode = match self.modal.as_ref().map(|m| m.kind) {
                Some(Kind::Grab) => GizmoMode::Move,
                Some(Kind::Rotate) => GizmoMode::Rotate,
                Some(Kind::Scale) => GizmoMode::Scale,
                None => self.gizmo,
            };
            let mut g = self.gizmo_spec(mode);
            if let Some(m) = &self.modal {
                g.basis = m.basis;
            }
            let k = match (&self.modal, live) {
                (Some(m), Some((k, _))) if m.kind == Kind::Scale => k,
                _ => 1.0,
            };
            draw_gizmo(&painter, &g, p, c, handle_len, hot, self.modal.is_some(), k, self.camera.eye(), &to_screen);
        }
        if let (Some(m), Some(c), Some((k, angle))) = (&self.modal, pivot_scr, live) {
            let ptr = pointer.unwrap_or(c);
            // Guide lines through the pivot: the locked axis, or a plane's two.
            let axes: Vec<usize> = match m.lock {
                Lock::Axis(a) if m.kind != Kind::Rotate => vec![a],
                Lock::Plane(skip) if m.kind != Kind::Rotate => (0..3).filter(|a| *a != skip).collect(),
                _ => Vec::new(),
            };
            for a in axes {
                let dir = m.basis * [Vec3::X, Vec3::Y, Vec3::Z][a];
                let reach = handle_len * 12.0;
                if let (Some(p1), Some(p2)) = (to_screen(m.pivot - dir * reach), to_screen(m.pivot + dir * reach)) {
                    painter.line_segment([p1, p2], egui::Stroke::new(1.5, AXIS_COL[a].gamma_multiply(0.85)));
                }
            }
            let readout = match m.kind {
                Kind::Rotate => {
                    let axis = match m.lock {
                        Lock::Axis(a) | Lock::Plane(a) => a.min(2),
                        Lock::Free => 1,
                    };
                    let col = AXIS_COL[axis];
                    // The swept angle as a slice in the ring's own plane, from where it was grabbed.
                    let g = GizmoSpec { mode: GizmoMode::Rotate, basis: m.basis, axes: [true; 3], planes: false };
                    let (u, v, sign) = g.ring_frame(axis);
                    let t0 = m.ring.map(|r| r.0).unwrap_or_else(|| {
                        // The R key: start from the ring point nearest where the drag began.
                        let mut best = (0.0, f32::MAX);
                        for k in 0..64 {
                            let t = k as f32 / 64.0 * std::f32::consts::TAU;
                            if let Some(q) = to_screen(m.pivot + (u * t.cos() + v * t.sin()) * handle_len) {
                                let d = (q - m.p0).length();
                                if d < best.1 {
                                    best = (t, d);
                                }
                            }
                        }
                        best.0
                    });
                    let sweep = angle * sign;
                    let n = ((sweep.abs() / std::f32::consts::TAU * 64.0).ceil() as usize).clamp(1, 128);
                    let pts: Vec<egui::Pos2> = (0..=n)
                        .filter_map(|k| {
                            let t = t0 + sweep * k as f32 / n as f32;
                            to_screen(m.pivot + (u * t.cos() + v * t.sin()) * handle_len)
                        })
                        .collect();
                    if let Some(pc) = to_screen(m.pivot) {
                        for w in pts.windows(2) {
                            painter.add(egui::Shape::convex_polygon(vec![pc, w[0], w[1]], col.gamma_multiply(0.22), egui::Stroke::NONE));
                        }
                        if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
                            painter.line_segment([pc, *a], egui::Stroke::new(1.5, col));
                            painter.line_segment([pc, *b], egui::Stroke::new(2.0, GIZMO_HOT));
                        }
                    }
                    format!("{:.1}°", angle.to_degrees())
                }
                Kind::Scale => format!("×{k:.2}"),
                Kind::Grab => {
                    let d = m.starts.first().and_then(|(sel, st)| Some(self.edits.xf(self.scenery.as_ref()?, *sel)?.pos - st.pos)).unwrap_or(Vec3::ZERO);
                    format!("{:+.2}  {:+.2}  {:+.2} m", d.x, d.y, d.z)
                }
            };
            // The value by the pointer, on a dark pill so it reads over any ground.
            let gl = painter.layout_no_wrap(readout, egui::FontId::proportional(14.0), egui::Color32::WHITE);
            let at = ptr + egui::vec2(22.0, -24.0);
            painter.rect_filled(egui::Rect::from_min_size(at - egui::vec2(6.0, 3.0), gl.size() + egui::vec2(12.0, 6.0)), 5.0, egui::Color32::from_black_alpha(170));
            painter.galley(at, gl, egui::Color32::WHITE);
            // What is going on and the keys, along the bottom of the view.
            let what = match m.kind {
                Kind::Grab => "Move",
                Kind::Rotate => "Rotate",
                Kind::Scale => "Scale",
            };
            let val = if m.typed.is_empty() { String::new() } else { format!(" = {}", m.typed) };
            let txt = format!("{what}{}{val}    X / Y / Z axis · Shift+X / Y / Z plane · type a number · Ctrl snap · Enter or release to confirm · Esc cancel", m.lock.label());
            let gl = painter.layout_no_wrap(txt, egui::FontId::proportional(13.0), egui::Color32::WHITE);
            let at = rect.center_bottom() - egui::vec2(gl.size().x * 0.5, gl.size().y + 14.0);
            painter.rect_filled(egui::Rect::from_min_size(at - egui::vec2(10.0, 5.0), gl.size() + egui::vec2(20.0, 10.0)), 6.0, egui::Color32::from_black_alpha(180));
            painter.galley(at, gl, egui::Color32::WHITE);
        }
        if self.show_minimap && self.tile.is_none() {
            self.draw_minimap(ui, rect, aspect);
        }
        // View label (top-left) like Blender.
        let view = format!("{} · {}", if self.camera.ortho { "Orthographic" } else { "Perspective" }, view_name(&self.camera));
        painter.text(rect.left_top() + egui::vec2(10.0, 8.0), egui::Align2::LEFT_TOP, view, egui::FontId::proportional(13.0), egui::Color32::from_white_alpha(210));
        // Zoom readout while zooming: how far in, against the whole-map view.
        if let Some(until) = self.zoom_shown {
            if Instant::now() < until {
                let factor = 640.0 / self.camera.dist.max(0.1);
                let text = format!("Zoom {}  ·  {:.0} m away", if factor >= 10.0 { format!("{factor:.0}×") } else { format!("{factor:.1}×") }, self.camera.dist);
                let at = rect.center_top() + egui::vec2(0.0, 14.0);
                let g = painter.layout_no_wrap(text, egui::FontId::proportional(15.0), egui::Color32::WHITE);
                let bg = egui::Rect::from_center_size(at + egui::vec2(0.0, g.size().y * 0.5), g.size() + egui::vec2(18.0, 8.0));
                painter.rect_filled(bg, 6.0, egui::Color32::from_black_alpha(170));
                painter.galley(bg.center() - g.size() * 0.5, g, egui::Color32::WHITE);
                ui.ctx().request_repaint();
            } else {
                self.zoom_shown = None;
            }
        }
    }

    fn draw_minimap(&mut self, ui: &mut egui::Ui, rect: egui::Rect, aspect: f32) {
        if std::mem::take(&mut self.minimap_stale) && self.tile.is_none() {
            self.minimap_tex = self.terrain.as_ref().map(|t| minimap_texture(ui.ctx(), t));
        }
        let (Some(tex), Some(t)) = (&self.minimap_tex, &self.terrain) else { return };
        let side = 190.0;
        let r = egui::Rect::from_min_size(rect.right_bottom() - egui::vec2(side + 12.0, side + 12.0), egui::vec2(side, side));
        let resp = ui.interact(r, ui.scope_id().with("minimap"), egui::Sense::click_and_drag());
        // Clipped: the camera's footprint runs far past the box near the horizon.
        let p = ui.painter().with_clip_rect(r.expand(3.0));
        p.rect_filled(r.expand(3.0), 4.0, egui::Color32::from_black_alpha(160));
        p.image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        // World (x, z) -> minimap; rows run along +Z downward in the texture.
        let to_mm = |x: f32, z: f32| {
            egui::pos2(r.left() + (x - t.min.x) / (t.max.x - t.min.x) * r.width(), r.top() + (z - t.min.z) / (t.max.z - t.min.z) * r.height())
        };
        if let Some(s) = &self.scenery {
            for x in &self.sel {
                if let Some(xf) = self.edits.xf(s, *x) {
                    p.circle_filled(to_mm(xf.pos.x, xf.pos.z), 2.0, self.accent32());
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
        } else if p(egui::Key::F5) {
            Some(Cmd::Play)
        } else if p(egui::Key::F2) {
            Some(Cmd::Rename)
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
        } else if cmd && (p(egui::Key::C) || p(egui::Key::X)) && ctx.plugin::<egui::text_selection::LabelSelectionState>().lock().has_selection() {
            None // text is selected: egui copies it, the objects stay put
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
        let mut open_blank: Option<(String, f32)> = None;
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
                ui.menu_button("New chapter…", |ui| {
                    ui.label(egui::RichText::new("An empty chapter: flat unpainted ground, no scenery, no invisible walls, one start and one boss spot.").small().weak());
                    ui.label(egui::RichText::new("Pick its world: the sky, light, music, enemies and tiles it uses.").small().weak());
                    ui.add(egui::Slider::new(&mut self.new_blank_height, -40.0..=40.0).step_by(2.0).text("ground m"));
                    ui.checkbox(&mut self.new_keep_kinds, "Keep the run's tiles").on_hover_text("Camps, crystals, shrines, chests… on the chapter's spots, as the shipped chapter rolls them; off = only the start and the boss");
                    ui.checkbox(&mut self.new_edge_walls, "Edge walls").on_hover_text("Invisible walls around the ground, so heroes cannot walk off it");
                    for c in &self.chapters {
                        if BLANK_BASES.contains(&c.key.as_str()) && ui.button(format!("{} world", c.label)).clicked() {
                            open_blank = Some((c.key.clone(), self.new_blank_height));
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
                item(ui, "Play in game", "F5", Cmd::Play);
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
                ui.checkbox(&mut self.show_walk, "Walkability").on_hover_text("Blocked ground in red, every wall (orange) and walkable box (green)");
                ui.checkbox(&mut self.snap, "Snap to grid");
                ui.add(egui::Slider::new(&mut self.grid_size, 0.25..=16.0).logarithmic(true).text("grid m"));
                ui.checkbox(&mut self.snap_ground, "Objects follow the ground");
                ui.checkbox(&mut self.shadows, "Shadows");
                ui.checkbox(&mut self.show_minimap, "Minimap");
                ui.checkbox(&mut self.animate_fx, "Play effects").on_hover_text("Fire, smoke and glows move (keeps the view redrawing)");
                ui.add(egui::Slider::new(&mut self.min_px, 0.0..=8.0).text("hide props under px"));
                ui.separator();
                item(ui, "Environment (light & fog)…", "", Cmd::ToggleEnv);
                item(ui, "Log", "", Cmd::ToggleLog);
                if ui.button("Theme…").on_hover_text("Every colour, roundness and text size").clicked() {
                    self.show_theme = true;
                    ui.close();
                }
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
        if let Some((k, h)) = open_blank {
            if self.unsaved {
                self.autosave();
            }
            self.start_empty(&ctx, k, h);
        }
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
            self.open_seq += 1;
            let n = self.open_seq;
            self.status = format!("Opening {id}…");
            self.spawn(&ctx, move |be| {
                let r = be.get_json(&format!("/map/api/mod?id={}", q(&id)));
                Ok(Event::Opened(id, n, r))
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

    /// Unity's Hierarchy: the scene's objects, nothing else.
    fn ui_hierarchy(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.menu_button(egui::RichText::new(" + ").strong(), |ui| self.ui_create_menu(ui)).response.on_hover_text("Create");
            ui.label(egui::RichText::new("Hierarchy").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Collapse").on_hover_text("Close every section").clicked() {
                    self.tree_open = Some(false);
                }
                if ui.small_button("Expand").on_hover_text("Open the top sections").clicked() {
                    self.tree_open = Some(true);
                }
                if !self.sel.is_empty() {
                    ui.label(egui::RichText::new(format!("{} selected", self.sel.len())).small().color(ui.visuals().selection.bg_fill));
                }
            });
        });
        self.ui_outliner(ui);
        self.tree_open = None;
    }

    /// The Inspector side: the selection's properties, and the tool windows
    /// docked beside it as tabs.
    fn ui_inspector_side(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::horizontal().id_salt("inspector_tabs").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.selectable_value(&mut self.left_tab, LeftTab::Inspector, "Inspector");
            if self.tile.is_none() {
                ui.selectable_value(&mut self.left_tab, LeftTab::Terrain, "Terrain");
                ui.selectable_value(&mut self.left_tab, LeftTab::Spots, "Spots");
                ui.selectable_value(&mut self.left_tab, LeftTab::Pois, "Tiles");
            }
            let n = self.edits.count();
            ui.selectable_value(&mut self.left_tab, LeftTab::Changes, if n > 0 { format!("Changes ({n})") } else { "Changes".into() });
        }));
        ui.separator();
        if self.tile.is_some() && matches!(self.left_tab, LeftTab::Spots | LeftTab::Terrain | LeftTab::Pois) {
            self.left_tab = LeftTab::Inspector;
        }
        // Leaving the Terrain tab puts the brush down.
        if self.left_tab != LeftTab::Terrain && self.tool == Tool::Brush {
            self.tool = Tool::Select;
        }
        match self.left_tab {
            LeftTab::Inspector => self.ui_inspector_tab(ui),
            LeftTab::Spots => self.ui_spots(ui),
            LeftTab::Pois => self.ui_pois(ui),
            LeftTab::Terrain => {
                egui::ScrollArea::vertical().id_salt("terrain").show(ui, |ui| self.ui_terrain(ui));
            }
            LeftTab::Changes => self.ui_changes(ui),
        }
    }

    /// Unity's Project + Console, docked at the bottom.
    fn ui_bottom(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Project, "Project");
            let errs = self.failed_assets.len();
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Console, if errs > 0 { format!("Console ({errs})") } else { "Console".into() });
        });
        ui.separator();
        match self.bottom_tab {
            BottomTab::Project => self.ui_assets(ui),
            BottomTab::Console => {
                ui.horizontal(|ui| {
                    if icons::text_button(ui, Icon::Copy, "Copy all").on_hover_text("Or select any text and press Ctrl+C").clicked() {
                        let mut all: Vec<String> = self.backend.log.lock().unwrap().iter().cloned().collect();
                        all.extend(self.failed_assets.iter().cloned());
                        ui.ctx().copy_text(all.join("\n"));
                        self.status = "Console copied".into();
                    }
                });
                egui::ScrollArea::vertical().id_salt("console").stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
                    for l in self.backend.log.lock().unwrap().iter() {
                        ui.monospace(l);
                    }
                    for l in &self.failed_assets {
                        ui.colored_label(egui::Color32::LIGHT_RED, l);
                    }
                });
            }
        }
    }

    /// Create menu (Hierarchy "+", right-click on empty space or a row).
    fn ui_create_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("Import model…").on_hover_text(".glb models and .png textures; or drop the files on the window").clicked() {
            self.pick_files(ui.ctx());
            ui.close();
        }
        if ui.button("Prop…").on_hover_text("Pick one in the Project panel, then click the ground, or drag it into the scene").clicked() {
            self.show_bottom = true;
            self.bottom_tab = BottomTab::Project;
            self.status = "Project panel: click a prop then the ground, or drag it into the scene".into();
            ui.close();
        }
        // Enemy camps the chapter already preloads: a camp stands its monsters
        // there (the `_Wandering` ones roam).
        let mut camps: Vec<(String, String)> = self
            .props
            .iter()
            .filter(|p| p.entity.contains("Enemy_Camp") && !p.entity.contains("Reward"))
            .map(|p| (p.entity.clone(), p.name.replace('_', " ")))
            .collect();
        // The wandering camps (what a chapter scatters) first.
        camps.sort_by_key(|(e, n)| (!e.contains("Wandering"), n.clone()));
        ui.add_enabled_ui(!camps.is_empty(), |ui| {
            ui.menu_button("Enemy camp", |ui| {
                for (e, name) in &camps {
                    if ui.button(name).clicked() {
                        self.tool = Tool::Place(e.clone());
                        self.status = format!("Click the ground to place {name} (Shift keeps placing)");
                        ui.close();
                    }
                }
            });
        });
        for (k, label, tip) in [
            ("wall", "Invisible wall", "A box heroes and enemies cannot pass; nothing is drawn in game"),
            ("floor", "Walkable floor", "A box heroes can stand on: a deck, a bridge, a ramp (tilt it)"),
        ] {
            let ok = self.needs_map() && self.tile.is_none();
            let r = ui.add_enabled(ok, egui::Button::new(label)).on_hover_text(tip).on_disabled_hover_text("Only on a map of your own: File > New chapter or New map from chapter");
            if r.clicked() {
                self.tool = Tool::Place(format!("{}{k}", crate::edit::COLLIDER));
                self.status = format!("Click the ground to place: {label} (Shift keeps placing)");
                ui.close();
            }
        }
        let ok = self.needs_map() && self.tile.is_none() && self.terrain.is_some();
        if ui.add_enabled(ok, egui::Button::new("Edge walls")).on_hover_text("Invisible walls all around the ground, so heroes cannot walk off it").clicked() {
            self.add_edge_walls();
            ui.close();
        }
        ui.separator();
        if self.tile.is_none() {
            if let Some(r) = self.recipe.clone() {
                ui.menu_button("Tile spot", |ui| {
                    for g in &r.groups {
                        if ui.button(format!("{g} spot")).clicked() {
                            self.tool = Tool::AddSpot(g.clone());
                            self.status = format!("Click the ground to place a {g} spot (Shift keeps placing)");
                            ui.close();
                        }
                    }
                });
            }
        }
        if ui.add_enabled(!self.clipboard.is_empty(), egui::Button::new("Paste at cursor")).clicked() {
            let ctx = ui.ctx().clone();
            self.run(&ctx, Cmd::Paste);
            ui.close();
        }
    }

    /// Invisible walls along the four edges of the ground (2 m in), in 120 m
    /// pieces (a collision box is at most 128 m a side). What a blank map
    /// lost with the chapter's own `_Collisions` walls. One undo step.
    fn add_edge_walls(&mut self) {
        let Some(t) = &self.terrain else { return };
        let (lo, hi) = (t.min + Vec3::new(2.0, 0.0, 2.0), t.max - Vec3::new(2.0, 0.0, 2.0));
        let level = self.edits.adds.first().map(|a| a.level.clone()).or_else(|| self.scenery.as_ref().and_then(|s| s.levels.first().cloned())).unwrap_or_default();
        let mut walls = Vec::new();
        for (a0, a1, fixed, along_x) in [(lo.x, hi.x, lo.z, true), (lo.x, hi.x, hi.z, true), (lo.z, hi.z, lo.x, false), (lo.z, hi.z, hi.x, false)] {
            let n = ((a1 - a0) / 120.0).ceil().max(1.0) as usize;
            let len = (a1 - a0) / n as f32;
            for k in 0..n {
                let m = a0 + len * (k as f32 + 0.5);
                let (x, z) = if along_x { (m, fixed) } else { (fixed, m) };
                let y = self.ground_y(x, z).unwrap_or(0.0);
                let rot = if along_x { Quat::IDENTITY } else { Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) };
                walls.push((Vec3::new(x, y, z), rot, len));
            }
        }
        let mut new = Vec::new();
        self.change(|e, _| {
            for (pos, rot, len) in walls {
                let col = Some(vec![crate::edit::ColBox::wall(Vec3::new(len, 12.0, 1.0), Vec3::new(0.0, 5.0, 0.0))]);
                new.push(e.push_add(Add { uid: 0, entity: format!("{}wall", crate::edit::COLLIDER), level: level.clone(), pos, rot, scale: Vec3::ONE, name: "Edge wall".into(), col, camp: None }));
            }
        });
        let n = new.len();
        // One Hierarchy group, not twenty rows.
        self.groups.push(("Edge walls".into(), new));
        self.ensure_template_now(&format!("{}wall", crate::edit::COLLIDER));
        self.status = format!("Edge walls: {n} invisible walls around the ground");
    }

    /// `ensure_template` for a collider (needs no backend).
    fn ensure_template_now(&mut self, entity: &str) {
        if entity.starts_with(crate::edit::COLLIDER) {
            self.templates.entry(entity.to_owned()).or_insert(Some(Vec::new()));
            self.dirty = true;
        }
    }

    /// An entity with no mesh to draw (an enemy camp, a spawner), once loaded.
    fn draws_nothing(&self, entity: &str) -> bool {
        matches!(self.templates.get(entity), Some(Some(t)) if !t.iter().any(|(k, _)| !k.decal && !k.vfx))
    }

    /// A placed model copy's or collider's collision boxes as they will save
    /// (a model copy without its own: the model's default), with its world
    /// matrix and the model it is a copy of.
    fn obj_cols(&self, u: u64) -> Option<(Vec<crate::edit::ColBox>, Mat4, Option<crate::models::Model>)> {
        let a = self.edits.add(u)?;
        if a.entity.starts_with(crate::edit::COLLIDER) {
            return Some((a.col.clone().unwrap_or_default(), a.xf().mat(), None));
        }
        let rel = a.entity.strip_prefix(crate::models::PREFIX)?;
        let m = self.models.iter().find(|m| m.mesh.rel == rel)?.clone();
        Some((a.col.clone().unwrap_or_else(|| m.default_cols()), a.xf().mat(), Some(m)))
    }

    /// Inspector > Enemy camp: a placed camp's own difficulty and power (the
    /// map's tribe decides which monsters; these decide how hard a group).
    fn ui_camp(&mut self, ui: &mut egui::Ui, u: u64) {
        use crate::edit::{Camp, CAMP_DIFFICULTIES};
        let Some(a) = self.edits.add(u) else { return };
        if !a.entity.contains("Enemy_Camp") || a.entity.contains("Reward") {
            return;
        }
        let before = a.camp.clone();
        let mut camp = before.clone();
        ui.separator();
        ui.label(egui::RichText::new("Enemy camp").strong());
        let mut own = camp.is_some();
        ui.checkbox(&mut own, "Own spawn settings").on_hover_text("Off: the camp spawns as the shipped chapter's do");
        if own != camp.is_some() {
            camp = own.then(|| Camp { difficulty: "Medium".into(), power: [100.0, 200.0] });
        }
        let mut started = false;
        if let Some(c) = &mut camp {
            egui::Grid::new("camp").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
                ui.label("Difficulty");
                egui::ComboBox::from_id_salt("camp_diff").selected_text(c.difficulty.replace('_', " ")).show_ui(ui, |ui| {
                    for d in CAMP_DIFFICULTIES {
                        ui.selectable_value(&mut c.difficulty, d.to_owned(), d.replace('_', " "));
                    }
                });
                ui.end_row();
                ui.label("Power").on_hover_text("The two numbers the camp rolls its group from (Dark Hills' wandering camps ship 100–200, a Ghouls camp 10–20); higher = more or stronger monsters. Read from the game data, not yet confirmed in game.");
                let r0 = ui.add(egui::DragValue::new(&mut c.power[0]).speed(1.0).range(1.0..=10000.0).prefix("min "));
                let r1 = ui.add(egui::DragValue::new(&mut c.power[1]).speed(1.0).range(1.0..=10000.0).prefix("max "));
                started |= r0.drag_started() || r0.gained_focus() || r1.drag_started() || r1.gained_focus();
                c.power[1] = c.power[1].max(c.power[0]);
                ui.end_row();
            });
            ui.label(egui::RichText::new("Which monsters come from the map's tribe.").small().weak());
        }
        if camp != before {
            if started || !ui.ctx().egui_is_using_pointer() {
                self.history.push(&self.edits);
            }
            if let Some(a) = self.edits.adds.iter_mut().find(|a| a.uid == u) {
                a.camp = camp;
            }
            self.unsaved = true;
        }
    }

    /// Inspector > Collision: the boxes of one of the mod's own models or of
    /// a collider. Walls block heroes and enemies; ground can be stood on.
    fn ui_collision(&mut self, ui: &mut egui::Ui, u: u64) {
        use crate::edit::ColBox;
        let Some((mut boxes, _, model)) = self.obj_cols(u) else { return };
        ui.separator();
        ui.label(egui::RichText::new("Collision").strong());
        let before = boxes.clone();
        let mut started = false;
        let mut drop = None;
        for (i, b) in boxes.iter_mut().enumerate() {
            ui.push_id(("colbox", i), |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("kind")
                        .width(80.0)
                        .selected_text(if b.ground { "Ground" } else { "Wall" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut b.ground, false, "Wall").on_hover_text("Blocks heroes and enemies");
                            ui.selectable_value(&mut b.ground, true, "Ground").on_hover_text("Can be walked on (a deck, a ramp)");
                        });
                    if icons::button(ui, Icon::Trash, false, "Remove this box").clicked() {
                        drop = Some(i);
                    }
                });
                egui::Grid::new("col").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
                    for (label, v, lo, hi, suffix) in [
                        ("Size", &mut b.size, 0.05f32, 128.0f32, " m"),
                        ("Centre", &mut b.at, -128.0, 128.0, " m"),
                        ("Rotation", &mut b.rot, -180.0, 180.0, "°"),
                    ] {
                        ui.label(label);
                        for k in 0..3 {
                            let r = ui.add(egui::DragValue::new(&mut v[k]).speed(if suffix == "°" { 0.5 } else { 0.05 }).range(lo..=hi).fixed_decimals(if suffix == "°" { 1 } else { 2 }).suffix(suffix));
                            started |= r.drag_started() || r.gained_focus();
                        }
                        ui.end_row();
                    }
                });
            });
            ui.add_space(4.0);
        }
        if let Some(i) = drop {
            boxes.remove(i);
        }
        ui.horizontal(|ui| {
            if ui.small_button("+ Wall box").clicked() {
                boxes.push(ColBox::wall(Vec3::new(2.0, 2.0, 2.0), Vec3::new(0.0, 1.0, 0.0)));
            }
            if ui.small_button("+ Ground box").clicked() {
                boxes.push(ColBox { ground: true, size: Vec3::new(4.0, 0.5, 4.0), at: Vec3::new(0.0, -0.25, 0.0), rot: Vec3::ZERO });
            }
        });
        let empty = if model.is_some() { "No boxes: heroes walk through it." } else { "No boxes: this collider does nothing." };
        let note = if boxes.is_empty() { empty } else { "In the object's own space: orange walls, green ground in the scene." };
        ui.label(egui::RichText::new(note).small().weak());
        let mut reset = false;
        if let Some(m) = &model {
            if self.edits.add(u).is_some_and(|a| a.col.is_some()) {
                reset = ui.small_button(format!("Reset to {}'s default", m.name())).clicked();
            }
        }
        let changed = boxes != before;
        if started || reset || drop.is_some() || (changed && !ui.ctx().egui_is_using_pointer()) {
            self.history.push(&self.edits);
        }
        if changed || reset {
            if let Some(a) = self.edits.adds.iter_mut().find(|a| a.uid == u) {
                a.col = if reset { None } else { Some(boxes) };
            }
            self.unsaved = true;
            self.dirty = true;
        }
    }

    /// Render up to a few Project thumbnails whose item is fully loaded.
    fn make_thumbs(&mut self, rs: &eframe::egui_wgpu::RenderState) {
        const SIZE: u32 = 160;
        let mut made = 0;
        for e in std::mem::take(&mut self.thumb_want) {
            if made >= 3 || self.thumbs.contains_key(&e) {
                continue;
            }
            match self.render_entity(rs, &e, SIZE, 0.65, 0.45, false) {
                Some(Some(id)) => {
                    self.thumbs.insert(e, id);
                    made += 1;
                }
                Some(None) => {
                    self.thumbs.insert(e, egui::TextureId::default());
                }
                None => {}
            }
        }
        // The preview window, redrawn while it is open (it turns with the mouse).
        if let Some((e, yaw, pitch)) = self.preview.clone() {
            if let Some(Some(id)) = self.render_entity(rs, &e, 512, yaw, pitch, true) {
                self.preview_tex = Some(id);
            }
        }
    }

    /// Render `e` alone, framed: `None` while its meshes / textures load,
    /// `Some(None)` for nothing drawable.
    fn render_entity(&mut self, rs: &eframe::egui_wgpu::RenderState, e: &str, size: u32, yaw: f32, pitch: f32, preview: bool) -> Option<Option<egui::TextureId>> {
        let r = self.renderer.as_mut()?;
        let Some(Some(t)) = self.templates.get(e) else { return None };
        // Wait for every mesh and texture, so the preview is the real thing.
        let ready = t.iter().all(|(k, _)| (k.decal || k.vfx || r.has_mesh(&k.mesh)) && k.texture.as_ref().is_none_or(|tx| r.texture_known(tx, k.swap_rb)));
        if !ready {
            return None;
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let inst: Vec<Vec<Instance>> = t.iter().map(|(_, ms)| ms.iter().map(|m| Instance { m: m.to_cols_array_2d(), tint: [0.0; 4] }).collect()).collect();
        for (k, ms) in t {
            let Some((c, rad)) = r.mesh_bounds(&k.mesh) else { continue };
            for m in ms {
                let wc = m.transform_point3(c);
                let wr = rad * m.x_axis.length().max(m.y_axis.length()).max(m.z_axis.length());
                lo = lo.min(wc - Vec3::splat(wr));
                hi = hi.max(wc + Vec3::splat(wr));
            }
        }
        if lo.x > hi.x {
            return Some(None);
        }
        let mut cam = Camera { target: (lo + hi) * 0.5, yaw, pitch, dist: 1.0, ortho: false, ..self.camera };
        cam.frame((lo + hi) * 0.5, (hi - lo).length() * 0.3);
        let parts: Vec<DrawPart> = t.iter().zip(&inst).map(|((k, _), i)| DrawPart { key: k, instances: i }).collect();
        let f = Frame {
            camera: &cam,
            parts,
            lines: &[],
            top_lines: &[],
            min_px: 0.0,
            haze: 0.0,
            exposure: self.exposure,
            shadows: false,
            env: &self.env,
            draw_water: false,
            no_terrain: true,
            sky: None,
            // The preview window plays the effects; thumbnails hold them still.
            time: if preview { self.started.elapsed().as_secs_f32() } else { 1.0 },
        };
        Some(Some(if preview { r.render_preview(rs, (size, size), &f) } else { r.render_thumb(rs, (size, size), &f) }))
    }

    /// Import… : the system's file picker, off the UI thread.
    fn pick_files(&mut self, ctx: &egui::Context) {
        self.show_bottom = true;
        self.bottom_tab = BottomTab::Project;
        self.spawn(ctx, |_| {
            let files = rfd::FileDialog::new().set_title("Import models and textures").add_filter("Models and textures", &["glb", "png"]).pick_files();
            Ok(Event::Imported(files.unwrap_or_default()))
        });
    }

    /// Add `.glb` models and `.png` textures to the mod. A texture goes on
    /// the model with the same file name, else on the last model imported.
    fn import_files(&mut self, ctx: &egui::Context, paths: Vec<std::path::PathBuf>) {
        use crate::models::{clean_stem, read_glb, Art, Model};
        let (mut glbs, mut pngs): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("glb")));
        pngs.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")));
        let mut done = Vec::new();
        for file in glbs.drain(..) {
            match read_glb(&file) {
                Ok((bytes, lo, hi)) => {
                    let mut name = clean_stem(&file);
                    while self.models.iter().any(|m| m.name() == name && m.mesh.file != file) {
                        name.push('_');
                    }
                    let rel = crate::project::rel_in(&self.project_dir, &format!("{name}.glb"));
                    self.dropped_art.retain(|r| r != &rel);
                    self.models.retain(|m| m.mesh.rel != rel);
                    self.templates.remove(&format!("{}{rel}", crate::models::PREFIX));
                    self.models.push(Model { mesh: Art { rel: rel.clone(), file: file.clone() }, texture: None, solid: true, lo, hi });
                    self.stage(ctx, rel, bytes);
                    done.push(name);
                }
                Err(e) => self.failed_assets.push(format!("import: {e:#}")),
            }
        }
        for file in pngs {
            let stem = clean_stem(&file);
            let Some(i) = self.models.iter().position(|m| m.name() == stem).or(self.models.len().checked_sub(1)) else {
                self.status = format!("{}: import a model first, then its texture", file.display());
                continue;
            };
            // Beside its model, in the same folder.
            let rel = format!("{}.png", self.models[i].mesh.rel.trim_end_matches(".glb"));
            self.dropped_art.retain(|r| r != &rel);
            match std::fs::read(&file) {
                Ok(bytes) if bytes.starts_with(b"\x89PNG") => {
                    self.models[i].texture = Some(Art { rel: rel.clone(), file: file.clone() });
                    self.templates.remove(&self.models[i].entity());
                    self.stage(ctx, rel, bytes);
                    done.push(format!("{} (texture)", self.models[i].name()));
                }
                _ => self.failed_assets.push(format!("import: {} is not a .png", file.display())),
            }
        }
        // Placed copies pick up a new texture.
        let ents: Vec<String> = self.models.iter().map(|m| m.entity()).collect();
        for e in ents {
            self.ensure_template(ctx, &e);
        }
        self.dirty = true;
        if !done.is_empty() {
            self.unsaved = true;
            self.status = format!("Imported {}: drag it from Project into the scene", done.join(", "));
        }
    }

    /// Play in game, after a save: enable this mod and `rsmm run` (which
    /// restores, applies every enabled mod and launches through Steam, then
    /// leaves a watcher that restores the game files when it closes).
    fn start_play(&mut self, ctx: &egui::Context) {
        let id = self.mod_id.trim().to_owned();
        if id.is_empty() {
            return;
        }
        self.playing = true;
        self.status = format!("Play in game: applying {id} and starting Ravenswatch…");
        let (cmd, tx, ctx) = (self.backend.cmd.clone(), self.events_tx.clone(), ctx.clone());
        thread::spawn(move || {
            let rsmm = |args: &[&str]| -> Result<String, String> {
                let (prog, pre) = cmd.split_first().ok_or("no rsmm command")?;
                let out = std::process::Command::new(prog).args(pre).args(args).output().map_err(|e| format!("rsmm {}: {e}", args[0]))?;
                let text = String::from_utf8_lossy(if out.status.success() { &out.stdout } else { &out.stderr }).trim().to_owned();
                let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_owned();
                if out.status.success() { Ok(last) } else { Err(format!("rsmm {}: {}", args[0], if last.is_empty() { out.status.to_string() } else { last })) }
            };
            let r = rsmm(&["enable", &id, "--no-apply"]).and_then(|_| rsmm(&["run"]));
            let _ = tx.send(Event::Played(r));
            ctx.request_repaint();
        });
    }

    fn stage(&mut self, ctx: &egui::Context, rel: String, bytes: Vec<u8>) {
        self.stage_now(rel, bytes);
        ctx.request_repaint();
    }

    /// A reopened mod's own models, read from its folder.
    fn register_mod_models(&mut self, objects: &Value, folder: &std::path::Path) {
        use crate::models::{read_glb, Art, Model};
        for o in objects.as_array().into_iter().flatten() {
            let Some(rel) = o["model"].as_str() else { continue };
            let file = folder.join(rel);
            if self.models.iter().any(|m| m.mesh.rel == rel && m.mesh.file == file) {
                continue;
            }
            // Another mod's model at the same path: this mod's own file wins.
            self.models.retain(|m| m.mesh.rel != rel);
            self.staged.remove(rel);
            self.staging.remove(rel);
            self.templates.remove(&format!("{}{rel}", crate::models::PREFIX));
            match read_glb(&file) {
                Ok((bytes, lo, hi)) => {
                    let texture = o["texture"].as_str().map(|t| Art { rel: t.to_owned(), file: folder.join(t) });
                    if let Some(t) = &texture {
                        if let Ok(b) = std::fs::read(&t.file) {
                            self.stage_now(t.rel.clone(), b);
                        }
                    }
                    self.models.push(Model { mesh: Art { rel: rel.to_owned(), file }, texture, solid: o.get("collision").is_some(), lo, hi });
                    self.stage_now(rel.to_owned(), bytes);
                }
                Err(e) => self.failed_assets.push(format!("{rel}: {e:#}")),
            }
        }
    }

    /// `stage` without a context (a reopened mod's art, read at load).
    fn stage_now(&mut self, rel: String, bytes: Vec<u8>) {
        self.stage_seq += 1;
        let n = self.stage_seq;
        // The old id is for other bytes: a save waits for this upload instead.
        self.staged.remove(&rel);
        self.staging.insert(rel.clone(), n);
        let (be, tx) = (self.backend.clone(), self.events_tx.clone());
        thread::spawn(move || {
            let r = crate::models::upload(&be, &rel, &bytes);
            let _ = tx.send(Event::Staged(rel, n, r));
        });
    }

    /// Right-click on a Hierarchy row (Unity: the row's context menu).
    fn ui_row_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let has = !self.sel.is_empty();
        let one_add = self.sel.len() == 1 && matches!(self.sel.iter().next(), Some(Sel::Add(_)));
        let mut pick = None;
        let mut item = |ui: &mut egui::Ui, on: bool, label: &str, key: &str, c: Cmd| {
            if ui.add_enabled(on, egui::Button::new(label).shortcut_text(key)).clicked() {
                pick = Some(c);
                ui.close();
            }
        };
        item(ui, one_add, "Rename", "F2", Cmd::Rename);
        item(ui, has, "Duplicate", "Shift+D", Cmd::Duplicate);
        item(ui, has, "Copy", "Ctrl+C", Cmd::Copy);
        item(ui, !self.clipboard.is_empty(), "Paste", "Ctrl+V", Cmd::Paste);
        item(ui, has, "Delete", "Del", Cmd::Delete);
        ui.separator();
        item(ui, has, "Frame selected", "F", Cmd::FrameSelected);
        item(ui, has, "Drop to ground", "End", Cmd::DropToGround);
        item(ui, has, "Hide", "H", Cmd::Hide);
        item(ui, has, "Lock", "L", Cmd::Lock);
        item(ui, has, "Group", "Ctrl+G", Cmd::Group);
        ui.separator();
        ui.menu_button("Create", |ui| self.ui_create_menu(ui));
        if let Some(c) = pick {
            self.run(&ctx, c);
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
            // What the row is, at a glance.
            let (gr, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            self.row_glyph(ui.painter(), gr, members);
            let vis = ui.visuals();
            let mut job = egui::text::LayoutJob::default();
            let font = egui::TextStyle::Button.resolve(ui.style());
            let main = if all_hidden { vis.weak_text_color() } else { vis.widgets.inactive.text_color() };
            job.append(label, 0.0, egui::TextFormat::simple(font.clone(), main));
            if let Some(n) = count {
                job.append(&format!("  {n}"), 0.0, egui::TextFormat::simple(egui::FontId::proportional(font.size - 1.5), vis.weak_text_color()));
            }
            // Long names end in "…" rather than widening the panel.
            job.wrap = egui::text::TextWrapping::truncate_at_width((ui.available_width() - 12.0).max(40.0));
            let rt = job;
            if let (Some((rs, buf)), [m]) = (self.renaming.as_mut(), members) {
                if rs == m {
                    let te = ui.add(egui::TextEdit::singleline(buf).desired_width(f32::INFINITY));
                    let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    // Focus first, then check: a request_focus before lost_focus takes the focus back.
                    let done = te.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if !(done || cancel) {
                        te.request_focus();
                    }
                    if done || cancel {
                        let (sel, name) = self.renaming.take().unwrap();
                        if let (Sel::Add(u), false) = (sel, cancel) {
                            let name = name.trim().to_owned();
                            self.change(|e, _| {
                                if let Some(a) = e.adds.iter_mut().find(|a| a.uid == u) {
                                    a.name = name;
                                }
                            });
                        }
                    }
                    return;
                }
            }
            let r = ui.add(egui::Button::selectable(selected, rt).sense(egui::Sense::click_and_drag()));
            if let [m] = members {
                self.row_order_now.push(*m);
                if self.scroll_to_sel == Some(*m) {
                    r.scroll_to_me(Some(egui::Align::Center));
                    self.scroll_to_sel = None;
                }
            }
            // Drag rows onto a group (or empty space) to group them.
            let dragged = if !members.is_empty() && members.iter().all(|m| self.sel.contains(m)) {
                self.sel.iter().copied().collect()
            } else {
                members.to_vec()
            };
            r.dnd_set_drag_payload(RowDrag(dragged));
            if r.dnd_hover_payload::<RowDrag>().is_some() {
                ui.painter().rect_stroke(r.rect.expand(2.0), 3.0, egui::Stroke::new(1.5, ui.visuals().selection.bg_fill), egui::StrokeKind::Outside);
            }
            if let Some(p) = r.dnd_release_payload::<RowDrag>() {
                self.row_dropped = Some((members.to_vec(), p.0.clone()));
            }
            if r.secondary_clicked() && !members.iter().all(|m| self.sel.contains(m)) {
                let pick: Vec<Sel> = members.iter().copied().filter(|m| self.selectable(*m)).collect();
                self.select_only(pick);
            }
            let row_text = label.to_owned();
            r.context_menu(|ui| {
                if ui.button("Copy name").clicked() {
                    ui.ctx().copy_text(row_text.clone());
                    ui.close();
                }
                if let [m] = members {
                    if let Some((e, _)) = self.entity_of(*m) {
                        if ui.button("Copy entity path").clicked() {
                            ui.ctx().copy_text(e);
                            ui.close();
                        }
                    }
                }
                ui.separator();
                self.ui_row_menu(ui);
            });
            out = (r.clicked(), r.double_clicked());
        });
        out
    }

    /// A small mark before a Hierarchy row: a folder for a group of rows, a
    /// cube for a shipped object, a teal cube for the mod's own model, an
    /// orange-cornered cube for an added prop, a footprint for a tile spot.
    fn row_glyph(&self, p: &egui::Painter, r: egui::Rect, members: &[Sel]) {
        let c = |r: u8, g: u8, b: u8| egui::Color32::from_rgb(r, g, b);
        let cube = |col: egui::Color32| {
            let q = r.shrink(1.5);
            p.rect_filled(q, 2.0, col);
            p.line_segment([q.left_top() + egui::vec2(0.0, 3.0), q.right_top() + egui::vec2(0.0, 3.0)], egui::Stroke::new(1.0, egui::Color32::from_black_alpha(70)));
        };
        match members {
            [Sel::Obj(_)] => cube(c(150, 146, 138)),
            [Sel::Add(u)] => {
                let own = self.edits.adds.iter().find(|a| a.uid == *u).is_some_and(|a| a.entity.starts_with(crate::models::PREFIX));
                cube(if own { c(90, 175, 165) } else { c(170, 165, 155) });
                p.circle_filled(r.right_top() + egui::vec2(-2.0, 2.0), 2.5, c(240, 150, 50));
            }
            [Sel::Spot(_)] => {
                p.rect_stroke(r.shrink(2.0), 1.0, egui::Stroke::new(1.5, c(110, 170, 230)), egui::StrokeKind::Inside);
            }
            _ => folder_glyph(p, r.shrink2(egui::vec2(0.0, 1.5))),
        }
    }

    /// Unity's Hierarchy clicks: plain = select, Ctrl = toggle, Shift = the
    /// range from the last clicked row.
    fn select_members(&mut self, ui: &egui::Ui, members: &[Sel]) {
        let (cmd, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pick: Vec<Sel> = members.iter().copied().filter(|m| self.selectable(*m)).collect();
        let range = match (shift, self.row_anchor, pick.as_slice()) {
            (true, Some(a), [b]) => {
                let pos = |x: Sel| self.row_order.iter().position(|r| *r == x);
                pos(a).zip(pos(*b)).map(|(i, j)| self.row_order[i.min(j)..=i.max(j)].to_vec())
            }
            _ => None,
        };
        if let Some(r) = range {
            let r: Vec<Sel> = r.into_iter().filter(|m| self.selectable(*m)).collect();
            self.sel.extend(r);
            self.active = pick.first().copied();
            self.dirty = true;
            return;
        }
        if cmd || shift {
            for m in &pick {
                if !self.sel.remove(m) {
                    self.sel.insert(*m);
                }
            }
            self.active = self.sel.iter().next_back().copied();
            self.dirty = true;
        } else {
            self.select_only(pick.clone());
        }
        if let [m] = pick.as_slice() {
            self.row_anchor = Some(*m);
        }
    }

    /// Put `objs` in group `gi` (out of any other group); empty groups go.
    fn move_to_group(&mut self, gi: usize, objs: &[Sel]) {
        for (i, (_, g)) in self.groups.iter_mut().enumerate() {
            if i != gi {
                g.retain(|m| !objs.contains(m));
            }
        }
        if let Some((_, g)) = self.groups.get_mut(gi) {
            for m in objs {
                if !g.contains(m) {
                    g.push(*m);
                }
            }
        }
        let name = self.groups.get(gi).map(|g| g.0.clone()).unwrap_or_default();
        self.groups.retain(|(_, g)| !g.is_empty());
        self.status = format!("{} object(s) in {name}", objs.len());
        self.unsaved = true;
    }

    fn ui_outliner(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let clear = !self.tree_filter.is_empty();
            let w = ui.available_width() - if clear { 24.0 } else { 0.0 };
            ui.add(egui::TextEdit::singleline(&mut self.tree_filter).hint_text("Search objects…").desired_width(w));
            if clear && ui.small_button("×").on_hover_text("Clear the search").clicked() {
                self.tree_filter.clear();
            }
        });
        if self.scenery.is_none() {
            ui.label("Loading…");
            return;
        }
        let filter = self.tree_filter.to_lowercase();
        let tree = std::mem::take(&mut self.tree);
        self.row_order = std::mem::take(&mut self.row_order_now);
        self.row_dropped = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            // Groups the author made: drop rows on one to add them to it.
            if !self.groups.is_empty() {
                let groups = self.groups.clone();
                egui::CollapsingHeader::new(egui::RichText::new("Groups").strong()).id_salt("groups").default_open(true).open(self.tree_open).show(ui, |ui| {
                    let mut remove = None;
                    for (gi, (name, members)) in groups.iter().enumerate() {
                        if let Some((g, buf)) = self.renaming_group.as_mut().filter(|(g, _)| *g == gi) {
                            let te = ui.add(egui::TextEdit::singleline(buf).desired_width(f32::INFINITY));
                            let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
                            if te.lost_focus() || cancel || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                let (g, name) = (*g, buf.trim().to_owned());
                                if !cancel && !name.is_empty() {
                                    self.groups[g].0 = name;
                                    self.unsaved = true;
                                }
                                self.renaming_group = None;
                            } else {
                                te.request_focus();
                            }
                            continue;
                        }
                        let selected = !members.is_empty() && members.iter().all(|m| self.sel.contains(m));
                        // A group is a folder: its objects are listed inside it (and nowhere else).
                        let id = ui.make_persistent_id(("group", gi));
                        let mut st = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
                        if let Some(o) = self.tree_open {
                            st.set_open(o);
                        }
                        st.show_header(ui, |ui| {
                                let (clicked, dbl) = self.tree_row(ui, members, name, Some(members.len()), selected);
                                if clicked {
                                    self.select_members(ui, members);
                                }
                                if dbl {
                                    self.run(&ui.ctx().clone(), Cmd::FrameSelected);
                                }
                                if let Some((target, objs)) = self.row_dropped.take() {
                                    if &target == members {
                                        self.move_to_group(gi, &objs);
                                    } else {
                                        self.row_dropped = Some((target, objs));
                                    }
                                }
                            })
                            .body(|ui| {
                                for m in members {
                                    if !self.exists(*m) {
                                        continue;
                                    }
                                    let label = self.name_of(*m);
                                    if !filter.is_empty() && !label.to_lowercase().contains(&filter) {
                                        continue;
                                    }
                                    let (c, d) = self.tree_row(ui, &[*m], &label, None, self.sel.contains(m));
                                    if c {
                                        self.select_members(ui, &[*m]);
                                    }
                                    if d {
                                        self.run(&ui.ctx().clone(), Cmd::FrameSelected);
                                    }
                                }
                                ui.horizontal(|ui| {
                                    if ui.small_button("rename").clicked() {
                                        self.renaming_group = Some((gi, name.clone()));
                                    }
                                    if ui.small_button("ungroup").clicked() {
                                        remove = Some(gi);
                                    }
                                });
                            });
                    }
                    if let Some(gi) = remove {
                        self.groups.remove(gi);
                    }
                });
            }
            // Objects the author added.
            if !self.edits.adds.is_empty() {
                let grouped = self.grouped();
                let adds: Vec<(Sel, String)> = self.edits.adds.iter().map(|a| (Sel::Add(a.uid), a.name())).filter(|(s, _)| !grouped.contains(s)).collect();
                let members: Vec<Sel> = adds.iter().map(|(s, _)| *s).collect();
                egui::CollapsingHeader::new(egui::RichText::new(format!("Added  ({})", adds.len())).strong()).open(self.tree_open)
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
                    egui::CollapsingHeader::new(egui::RichText::new(format!("Tile spots  ({})", all.len())).strong()).id_salt("spots").open(self.tree_open).show(ui, |ui| {
                        for (g, list) in &by_group {
                            let members: Vec<Sel> = list.iter().map(|(s, _)| *s).collect();
                            egui::CollapsingHeader::new(format!("{g}  ({})", list.len())).id_salt(("spotg", g)).open(self.tree_open.filter(|o| !o)).show(ui, |ui| {
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
            // A blank map has sunk every shipped object: list none of them.
            let blank = self.blank_hides_shipped();
            for (li, level) in tree.iter().enumerate().filter(|_| !blank) {
                if !outliner::matches(level, &filter) {
                    continue;
                }
                self.tree_node(ui, level, &filter, &format!("L{li}"), 0);
            }
            // Unity: right-click the empty part of the Hierarchy to create.
            let rest = ui.allocate_response(ui.available_size().max(egui::vec2(0.0, 40.0)), egui::Sense::click());
            if rest.clicked() {
                self.select_only([]);
            }
            if rest.dnd_hover_payload::<RowDrag>().is_some() {
                ui.painter().text(rest.rect.left_top() + egui::vec2(8.0, 6.0), egui::Align2::LEFT_TOP, "Drop here: new group", egui::FontId::proportional(12.0), ui.visuals().selection.bg_fill);
            }
            if let Some(p) = rest.dnd_release_payload::<RowDrag>() {
                let name = format!("Group {}", self.groups.len() + 1);
                self.groups.push((name, Vec::new()));
                let gi = self.groups.len() - 1;
                self.move_to_group(gi, &p.0);
            }
            rest.context_menu(|ui| self.ui_create_menu(ui));
        });
        self.tree = tree;
    }

    /// Every object in a group (listed inside the group in the Hierarchy).
    fn grouped(&self) -> HashSet<Sel> {
        self.groups.iter().flat_map(|(_, m)| m.iter().copied()).collect()
    }

    fn exists(&self, sel: Sel) -> bool {
        self.scenery.as_ref().is_some_and(|s| self.edits.exists(s, sel))
    }

    fn tree_node(&mut self, ui: &mut egui::Ui, node: &Node, filter: &str, id: &str, depth: usize) {
        let live: Vec<Sel> = node.members.iter().copied().filter(|m| !matches!(m, Sel::Obj(i) if self.edits.removed.contains(i))).collect();
        if let Some(leaf) = node.leaf {
            if live.is_empty() || self.groups.iter().any(|(_, m)| m.contains(&leaf)) {
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
        // A row picked in the Scene opens the levels it sits in.
        let reveal = self.scroll_to_sel.is_some_and(|x| node.leaf != Some(x) && node.members.contains(&x));
        let state = if reveal {
            let mut s = state;
            s.set_open(true);
            s
        } else if let Some(o) = self.tree_open.filter(|o| !o || depth == 0) {
            // Collapse all closes every level; expand opens the top ones only.
            let mut s = state;
            s.set_open(o);
            s
        } else if !filter.is_empty() {
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
        ui.label(egui::RichText::new("Left-drag paints · Ctrl inverts · Shift smooths · [ ] size · right-drag still looks around").small().weak());
        ui.separator();
        let slots: Vec<(usize, String)> = self
            .look_base
            .as_ref()
            .map(|l| (1..l.slots.len()).filter(|k| l.channel_sources.get(k - 1).is_some_and(Option::is_some)).map(|k| (k, l.slots[k].label.clone())).collect())
            .unwrap_or_default();
        let b = &mut self.brush;
        ui.label(egui::RichText::new("Shape").strong());
        ui.horizontal_wrapped(|ui| {
            for (m, l) in [(BrushMode::Raise, "Raise"), (BrushMode::Lower, "Lower"), (BrushMode::Flatten, "Flatten"), (BrushMode::Smooth, "Smooth"), (BrushMode::Level, "Set height"), (BrushMode::Layer, "Layer")] {
                ui.selectable_value(&mut b.mode, m, l);
            }
        });
        if b.mode == BrushMode::Level {
            ui.add(egui::Slider::new(&mut b.level, -50.0..=50.0).text("height m"));
        }
        if b.mode == BrushMode::Layer {
            ui.add(egui::Slider::new(&mut b.step, 1.0..=8.0).step_by(1.0).text("step m"));
            ui.label(egui::RichText::new("Each drag paints one flat terrace a step above the ground you start on (Ctrl: below). The shipped chapters use 4 m steps, sometimes 2 m.").small().weak());
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
        ui.label(egui::RichText::new("Walking").strong());
        ui.horizontal(|ui| {
            ui.selectable_value(&mut b.mode, BrushMode::Block, "Block").on_hover_text("Paint ground heroes and enemies cannot enter (Ctrl clears). Saved as invisible walls.");
            if b.mode == BrushMode::Block {
                ui.label(egui::RichText::new("Ctrl clears · red in the scene").small().weak());
            }
        });
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
        let mut pick_stamp = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Shape");
            ui.selectable_value(&mut b.stamp, None, "Round");
            for st in crate::brush::Stamp::builtin() {
                let on = b.stamp.as_ref().is_some_and(|s| s.name == st.name);
                if ui.selectable_label(on, st.name.as_str()).clicked() {
                    b.stamp = Some(st);
                }
            }
            if let Some(st) = b.stamp.as_ref().filter(|s| !crate::brush::Stamp::builtin().iter().any(|x| x.name == s.name)) {
                let _ = ui.selectable_label(true, st.name.as_str());
            }
            pick_stamp = ui.button("Picture…").on_hover_text("Any .png: bright, opaque parts paint, dark or clear parts do not").clicked();
        });
        if pick_stamp {
            self.spawn(ui.ctx(), |_| Ok(Event::StampPicked(rfd::FileDialog::new().set_title("Brush shape").add_filter("Picture", &["png"]).pick_file())));
        }
        ui.separator();
        ui.label(egui::RichText::new("Layer textures").strong());
        ui.label(egui::RichText::new("Draw a layer with your own picture (a tiling .png). It saves into the mod's art/terrain/.").small().weak());
        let names: Vec<(String, String)> = self
            .look_base
            .as_ref()
            .map(|l| ["base", "R", "G", "B", "A"].iter().enumerate().filter_map(|(k, s)| l.slots.get(k).map(|sl| (s.to_string(), if k == 0 { format!("Ground ({})", sl.label) } else { sl.label.clone() }))).collect())
            .unwrap_or_default();
        let mut pick_surface = None;
        let mut reset = None;
        for (slot, label) in &names {
            ui.horizontal(|ui| {
                ui.label(label);
                match self.surfaces.get(slot) {
                    Some((rel, _)) => {
                        ui.label(egui::RichText::new(rel).small());
                        if ui.small_button("Shipped").on_hover_text("Back to the game's texture").clicked() {
                            reset = Some(slot.clone());
                        }
                    }
                    None => {
                        ui.label(egui::RichText::new("shipped").small().weak());
                    }
                }
                if ui.small_button("Own texture…").clicked() {
                    pick_surface = Some(slot.clone());
                }
            });
        }
        if let Some(slot) = pick_surface {
            self.spawn(ui.ctx(), move |_| {
                let f = rfd::FileDialog::new().set_title("Layer texture").add_filter("Picture", &["png"]).pick_file();
                Ok(Event::SurfacePicked(slot, f))
            });
        }
        if let Some(slot) = reset {
            if let Some((rel, _)) = self.surfaces.remove(&slot) {
                if !self.surfaces.values().any(|(r, _)| r == &rel) && !self.dropped_art.contains(&rel) {
                    self.dropped_art.push(rel);
                }
            }
            self.unsaved = true;
            self.refresh_look();
        }
        ui.separator();
        let n = self.edits.strokes.len();
        ui.label(format!("{n} brush dabs"));
        if n > 15000 {
            ui.colored_label(egui::Color32::LIGHT_RED, "Close to the 20000-dab limit: use a bigger brush");
        }
        ui.label(egui::RichText::new("Terrain edits save as a new map (a copy of this chapter). Height, paths and walkability follow the painted height in game.").small().weak());
        if ui.checkbox(&mut self.export_minimap, "Redraw the in-game minimap on Save").on_hover_text("Save draws the edited ground as the chapter's minimap (art/minimap.png). Untested in game: check it draws the right way round.").changed() {
            self.unsaved = true;
        }
        if n > 0 && ui.button("Remove all terrain edits").clicked() {
            self.change(|e, _| e.strokes.clear());
            self.replay_terrain();
        }
    }

    /// Unity's Project window: a folder tree on the left, the open folder's
    /// contents as tiles on the right. `Assets` = the mod's own models,
    /// `Game` = the game's props by where they live in its files.
    fn ui_assets(&mut self, ui: &mut egui::Ui) {
        use crate::project as P;
        let all_props = self.all_props;
        let props: Vec<&Prop> = self.props.iter().filter(|p| all_props || p.scenery).collect();
        let item_dirs: Vec<String> = props
            .iter()
            .map(|p| P::folder_of(&p.entity))
            .chain(self.models.iter().map(|m| P::folder_of_art(&m.mesh.rel)))
            .chain(self.asset_dirs.iter().cloned())
            .collect();
        let folders = P::folders(item_dirs.iter().map(String::as_str));
        if !self.props.is_empty() && !folders.contains(&self.project_dir) {
            self.project_dir = P::ASSETS.to_owned();
        }
        let mut open_dir: Option<String> = None;
        // (model entity, folder) dropped onto a folder; folder actions from menus.
        let mut move_to: Option<(String, String)> = None;
        let mut new_in: Option<String> = None;
        let mut rename: Option<String> = None;
        let mut delete_dir: Option<String> = None;
        let mut pick: Option<(String, bool)> = None;
        let mut remove: Option<usize> = None;
        let mut import = false;
        let mut want: Vec<String> = Vec::new();
        let tile = self.tile_size;
        {
            // ---- folder tree, in its own resizable column
            egui::Panel::left("project_tree").resizable(true).default_size(230.0).size_range(150.0..=420.0).show(ui, |ui| {
              egui::ScrollArea::vertical().id_salt("project_tree").auto_shrink([false, false]).show(ui, |ui| {
                fn tree(ui: &mut egui::Ui, all: &std::collections::BTreeSet<String>, dir: &str, cur: &str, open: &mut Option<String>, act: &mut FolderActs) {
                    let kids = crate::project::children(all, dir);
                    let label = crate::project::leaf(dir).replace('_', " ");
                    let row = |ui: &mut egui::Ui, open: &mut Option<String>, act: &mut FolderActs| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 12.0), egui::Sense::hover());
                            folder_glyph(ui.painter(), r);
                            let r = ui.selectable_label(cur == dir, label.as_str());
                            if r.clicked() {
                                *open = Some(dir.to_owned());
                            }
                            folder_actions(&r, dir, act);
                        });
                    };
                    if kids.is_empty() {
                        row(ui, open, act);
                        return;
                    }
                    let id = ui.make_persistent_id(("ptree", dir));
                    let default_open = cur.starts_with(dir) || dir == crate::project::ASSETS || dir == crate::project::GAME;
                    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open)
                        .show_header(ui, |ui| row(ui, open, act))
                        .body(|ui| {
                            for k in kids {
                                tree(ui, all, k, cur, open, act);
                            }
                        });
                }
                let mut act = FolderActs::default();
                tree(ui, &folders, P::ASSETS, &self.project_dir, &mut open_dir, &mut act);
                tree(ui, &folders, P::GAME, &self.project_dir, &mut open_dir, &mut act);
                act.take_into(&mut move_to, &mut new_in, &mut rename, &mut delete_dir);
              });
            });
            // ---- the open folder
            egui::CentralPanel::default().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.prop_filter).hint_text("Search all…").desired_width(180.0));
                    ui.checkbox(&mut self.all_props, "Gameplay objects").on_hover_text("Also list spawners, FX, chests…");
                    if icons::text_button(ui, Icon::Import, "Import…").on_hover_text(".glb models (metres, Y up) and .png textures; or drop files on the window").clicked() {
                        import = true;
                    }
                    ui.add(egui::Slider::new(&mut self.tile_size, 56.0..=220.0).show_value(false).text("Icon size"))
                        .on_hover_text("Or Ctrl + mouse wheel over the icons");
                });
                ui.horizontal_wrapped(|ui| {
                    // Breadcrumb: each part opens that folder.
                    let mut acc = String::new();
                    for (i, seg) in self.project_dir.split('/').enumerate() {
                        if i > 0 {
                            ui.label("›");
                            acc.push('/');
                        }
                        acc.push_str(seg);
                        if ui.link(seg).clicked() {
                            open_dir = Some(acc.clone());
                        }
                    }
                });
                ui.separator();
                let f = self.prop_filter.to_lowercase();
                let searching = !f.is_empty();
                if ui.ui_contains_pointer() {
                    // Ctrl + wheel arrives as egui's zoom gesture.
                    let z = ui.input(|i| i.zoom_delta());
                    if z != 1.0 {
                        self.tile_size = (self.tile_size * z).clamp(56.0, 220.0);
                    }
                }
                egui::ScrollArea::vertical().id_salt("project_grid").auto_shrink(false).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                        // Sub-folders (not while searching: a search lists every match).
                        if !searching {
                            let mut act = FolderActs::default();
                            for d in P::children(&folders, &self.project_dir) {
                                let r = project_tile(ui, P::leaf(d), TileIcon::Folder, false, d, tile, None);
                                if r.double_clicked() || r.clicked() {
                                    open_dir = Some(d.to_owned());
                                }
                                folder_actions(&r, d, &mut act);
                            }
                            act.take_into(&mut move_to, &mut new_in, &mut rename, &mut delete_dir);
                        }
                        // The mod's own models, each in its folder.
                        if searching || P::art_dir(&self.project_dir).is_some() {
                            for (i, m) in self.models.iter_mut().enumerate() {
                                if if searching { !m.name().to_lowercase().contains(&f) } else { P::folder_of_art(&m.mesh.rel) != self.project_dir } {
                                    continue;
                                }
                                let entity = m.entity();
                                let on = self.asset_sel.as_ref() == Some(&entity);
                                let size = m.hi - m.lo;
                                let tip = format!(
                                    "{}\n{:.1} × {:.1} × {:.1} m · {}{}\nDrag into the scene to place it. Double-click to preview. Right-click for options.",
                                    m.mesh.rel,
                                    size.x,
                                    size.y,
                                    size.z,
                                    m.texture.as_ref().map_or("no texture".to_owned(), |t| t.rel.clone()),
                                    if m.solid { " · solid" } else { "" }
                                );
                                let r = project_tile(ui, m.name(), TileIcon::Model, on, &tip, tile, self.thumbs.get(&entity).copied());
                                if !self.thumbs.contains_key(&entity) && ui.is_rect_visible(r.rect) {
                                    want.push(entity.clone());
                                }
                                r.dnd_set_drag_payload(entity.clone());
                                if r.clicked() || r.double_clicked() {
                                    pick = Some((entity.clone(), r.double_clicked()));
                                }
                                r.context_menu(|ui| {
                                    if ui.button("Copy path").clicked() {
                                        ui.ctx().copy_text(m.mesh.rel.clone());
                                        ui.close();
                                    }
                                    ui.checkbox(&mut m.solid, "Solid (heroes cannot walk through)");
                                    if ui.button("Remove from the mod").clicked() {
                                        remove = Some(i);
                                        ui.close();
                                    }
                                });
                            }
                        }
                        // The game's props in this folder (or every match).
                        let mut shown = 0;
                        for p in &props {
                            let here = if searching {
                                p.name.to_lowercase().contains(&f) || p.group.to_lowercase().contains(&f)
                            } else {
                                P::folder_of(&p.entity) == self.project_dir
                            };
                            if !here {
                                continue;
                            }
                            shown += 1;
                            if shown > 600 {
                                break;
                            }
                            let on = self.asset_sel.as_ref() == Some(&p.entity);
                            let icon = if p.scenery { TileIcon::Prop } else { TileIcon::Gameplay };
                            let tip = format!("{}\nDrag into the scene to place it. Double-click to preview.", p.entity);
                            let r = project_tile(ui, &p.name, icon, on, &tip, tile, self.thumbs.get(&p.entity).copied());
                            if !self.thumbs.contains_key(&p.entity) && ui.is_rect_visible(r.rect) {
                                want.push(p.entity.clone());
                            }
                            r.dnd_set_drag_payload(p.entity.clone());
                            if r.clicked() || r.double_clicked() {
                                pick = Some((p.entity.clone(), r.double_clicked()));
                            }
                            r.context_menu(|ui| {
                                if ui.button("Copy path").clicked() {
                                    ui.ctx().copy_text(p.entity.clone());
                                    ui.close();
                                }
                                if ui.button("Copy name").clicked() {
                                    ui.ctx().copy_text(p.name.clone());
                                    ui.close();
                                }
                            });
                        }
                        if self.props.is_empty() && self.project_dir.starts_with(P::GAME) {
                            ui.label("Loading the game's props…");
                        }
                        if self.project_dir == P::ASSETS && self.models.is_empty() && !searching {
                            ui.label(egui::RichText::new("Your models go here: press Import… or drop .glb / .png files on the window. A .png goes on the model with the same name. Right-click for a new folder.").weak());
                        }
                    });
                    // Empty space below the icons: a new folder here.
                    let rest = ui.allocate_response(ui.available_size().max(egui::vec2(10.0, 40.0)), egui::Sense::click());
                    if P::art_dir(&self.project_dir).is_some() {
                        rest.context_menu(|ui| {
                            if ui.button("New folder").clicked() {
                                new_in = Some(self.project_dir.clone());
                                ui.close();
                            }
                        });
                    }
                });
            });
        }
        // Previews for what is on screen: load the item, then render it.
        let ctx = ui.ctx().clone();
        for e in &want {
            self.ensure_template(&ctx, e);
        }
        self.thumb_want.extend(want); // the Inspector asks for its own too
        if let Some(d) = open_dir {
            self.project_dir = d;
            self.prop_filter.clear();
        }
        if import {
            self.pick_files(ui.ctx());
        }
        if let Some(d) = new_in {
            self.new_folder(&d);
        }
        if let Some(d) = rename {
            self.folder_rename = Some((d.clone(), P::leaf(&d).to_owned()));
        }
        if let Some(d) = delete_dir {
            let inside = |x: &str| x == d || x.starts_with(&format!("{d}/"));
            let used = self.models.iter().any(|m| inside(&P::folder_of_art(&m.mesh.rel)));
            if used {
                self.status = format!("{d} is not empty: move or remove its models first");
            } else {
                self.asset_dirs.retain(|x| x != &d && !x.starts_with(&format!("{d}/")));
                if inside(&self.project_dir) {
                    self.project_dir = P::parent(&d).unwrap_or(P::ASSETS).to_owned();
                }
                self.unsaved = true;
            }
        }
        if let Some((e, d)) = move_to {
            if let Some(from) = e.strip_prefix("folder:") {
                // A folder dragged onto another.
                let to = format!("{d}/{}", P::leaf(from));
                if let Err(err) = self.rename_folder(from, &to) {
                    self.status = err;
                }
            } else if let Err(err) = self.move_model(&e, &d) {
                self.status = err;
            } else {
                self.status = format!("Moved to {d}");
            }
        }
        self.ui_folder_rename(ui.ctx());
        if let Some(i) = remove {
            // Its placed copies go too (one undo step); its files leave the mod on save.
            let m = self.models.remove(i);
            let e = m.entity();
            let placed = self.edits.adds.iter().filter(|a| a.entity == e).count();
            if placed > 0 {
                self.change(|ed, _| ed.adds.retain(|a| a.entity != e));
                self.prune_selection();
            }
            for a in std::iter::once(&m.mesh).chain(&m.texture) {
                self.staged.remove(&a.rel);
                self.staging.remove(&a.rel);
                if !self.dropped_art.contains(&a.rel) {
                    self.dropped_art.push(a.rel.clone());
                }
            }
            self.templates.remove(&e);
            self.thumbs.remove(&e);
            if self.asset_sel.as_ref() == Some(&e) {
                self.asset_sel = None;
            }
            if self.tool == Tool::Place(e.clone()) {
                self.tool = Tool::Select;
            }
            self.unsaved = true;
            self.status = format!("Removed {} from the mod{}; Save deletes its files", m.name(), if placed > 0 { format!(" and {placed} placed copy(s)") } else { String::new() });
        }
        if let Some((e, dbl)) = pick {
            let ctx = ui.ctx().clone();
            self.ensure_template(&ctx, &e);
            if dbl {
                self.preview = Some((e.clone(), 0.65, 0.35));
                self.preview_tex = None;
            }
            self.asset_sel = Some(e);
        }
        self.ui_preview(ui.ctx());
    }

    /// The ground's look with the mod's own layer pictures in place of the shipped ones.
    fn refresh_look(&mut self) {
        let Some(mut look) = self.look_base.clone() else { return };
        for (k, slot) in ["base", "R", "G", "B", "A"].iter().enumerate() {
            if let (Some((_, file)), Some(s)) = (self.surfaces.get(*slot), look.slots.get_mut(k)) {
                let t = format!("file:{}", file.display());
                if self.requested.insert(format!("{t}|false")) {
                    self.loader.request(Job::Texture(t.clone(), false));
                    self.pending_assets += 1;
                }
                s.albedo = Some(t);
                s.color = [1.0, 1.0, 1.0, 1.0]; // the picture as drawn, not tinted by the shipped colour
            }
        }
        if let Some(r) = &mut self.renderer {
            r.set_look(look);
        }
        self.look_pending = true;
    }

    /// Draw terrain layer `slot` with the picture `file` (saved into the mod's art/terrain/).
    fn set_surface(&mut self, ctx: &egui::Context, slot: &str, file: std::path::PathBuf) {
        let bytes = match std::fs::read(&file) {
            Ok(b) if b.starts_with(b"\x89PNG") => b,
            _ => {
                self.status = format!("{}: pick a .png", file.display());
                return;
            }
        };
        let rel = format!("art/terrain/{}.png", crate::models::clean_stem(&file));
        self.dropped_art.retain(|r| r != &rel);
        self.stage(ctx, rel.clone(), bytes);
        self.surfaces.insert(slot.to_owned(), (rel.clone(), file));
        self.unsaved = true;
        self.refresh_look();
        self.status = format!("Layer {slot} drawn with {rel}");
    }

    /// Move one of the mod's own models (and its texture) into the Project folder `dir`.
    fn move_model(&mut self, entity: &str, dir: &str) -> Result<(), String> {
        let Some(i) = self.models.iter().position(|m| m.entity() == entity) else { return Err("not one of the mod's models".into()) };
        let Some(_) = crate::project::art_dir(dir) else { return Err("models go in Assets".into()) };
        let file = |rel: &str| rel.rsplit('/').next().unwrap_or(rel).to_owned();
        let old_mesh = self.models[i].mesh.rel.clone();
        let new_mesh = crate::project::rel_in(dir, &file(&old_mesh));
        if new_mesh == old_mesh {
            return Ok(());
        }
        if self.models.iter().any(|m| m.mesh.rel == new_mesh) {
            return Err(format!("{dir} already has a {}", file(&old_mesh)));
        }
        let arts: Vec<String> = std::iter::once(old_mesh.clone()).chain(self.models[i].texture.as_ref().map(|t| t.rel.clone())).collect();
        if let Some(w) = arts.iter().find(|r| !self.staged.contains_key(*r)) {
            return Err(format!("still importing {w}: try again in a moment"));
        }
        for old in arts {
            let new = crate::project::rel_in(dir, &file(&old));
            let id = self.staged.remove(&old).unwrap_or_default();
            self.staged.insert(new.clone(), id);
            self.dropped_art.retain(|r| r != &new);
            if !self.dropped_art.contains(&old) {
                self.dropped_art.push(old.clone());
            }
            let m = &mut self.models[i];
            if m.mesh.rel == old {
                m.mesh.rel = new;
            } else if let Some(t) = m.texture.as_mut() {
                t.rel = new;
            }
        }
        // Placed copies name the model by its path: follow it, in every undo step too.
        let new_ent = self.models[i].entity();
        let old_ent = entity.to_owned();
        let fix = |e: &mut Edits| e.adds.iter_mut().filter(|a| a.entity == old_ent).for_each(|a| a.entity = new_ent.clone());
        fix(&mut self.edits);
        self.history.for_each_mut(fix);
        if let Some(t) = self.templates.remove(&old_ent) {
            self.templates.insert(new_ent.clone(), t);
        }
        if let Some(t) = self.thumbs.remove(&old_ent) {
            self.thumbs.insert(new_ent.clone(), t);
        }
        if self.asset_sel.as_deref() == Some(&old_ent) {
            self.asset_sel = Some(new_ent.clone());
        }
        if self.tool == Tool::Place(old_ent) {
            self.tool = Tool::Place(new_ent);
        }
        self.unsaved = true;
        self.dirty = true;
        Ok(())
    }

    /// Rename (or move) the Project folder `from` to `to`, with everything in it.
    fn rename_folder(&mut self, from: &str, to: &str) -> Result<(), String> {
        use crate::project as P;
        if from == P::ASSETS || P::art_dir(from).is_none() || P::art_dir(to).is_none() {
            return Err("only folders inside Assets can be renamed".into());
        }
        if to == from {
            return Ok(());
        }
        if to.starts_with(&format!("{from}/")) {
            return Err("a folder cannot go inside itself".into());
        }
        let under = |d: &str| d == from || d.starts_with(&format!("{from}/"));
        let moving: Vec<(String, String)> = self
            .models
            .iter()
            .filter(|m| under(&P::folder_of_art(&m.mesh.rel)))
            .map(|m| (m.entity(), format!("{to}{}", &P::folder_of_art(&m.mesh.rel)[from.len()..])))
            .collect();
        for (e, d) in moving {
            self.move_model(&e, &d)?;
        }
        let dirs: Vec<String> = self.asset_dirs.iter().filter(|d| under(d)).cloned().collect();
        for d in dirs {
            self.asset_dirs.remove(&d);
            self.asset_dirs.insert(format!("{to}{}", &d[from.len()..]));
        }
        self.asset_dirs.insert(to.to_owned());
        if under(&self.project_dir) {
            self.project_dir = format!("{to}{}", &self.project_dir[from.len()..]);
        }
        self.unsaved = true;
        Ok(())
    }

    /// A new empty folder inside `dir` (Assets if `dir` is a game folder), renamed at once.
    fn new_folder(&mut self, dir: &str) {
        let base = if crate::project::art_dir(dir).is_some() { dir.to_owned() } else { crate::project::ASSETS.to_owned() };
        let mut name = "New_folder".to_owned();
        let mut k = 1;
        while self.asset_dirs.contains(&format!("{base}/{name}")) || self.models.iter().any(|m| crate::project::folder_of_art(&m.mesh.rel) == format!("{base}/{name}")) {
            k += 1;
            name = format!("New_folder_{k}");
        }
        let path = format!("{base}/{name}");
        self.asset_dirs.insert(path.clone());
        self.project_dir = base;
        self.folder_rename = Some((path, name));
        self.unsaved = true;
    }

    /// The rename box for `folder_rename`.
    fn ui_folder_rename(&mut self, ctx: &egui::Context) {
        let Some((path, mut buf)) = self.folder_rename.clone() else { return };
        let mut close = false;
        let mut ok = false;
        egui::Window::new("Rename folder").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).show(ctx, |ui| {
            ui.label(egui::RichText::new(&path).weak());
            let te = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(260.0));
            if !te.has_focus() && !te.lost_focus() {
                te.request_focus();
            }
            ui.label(egui::RichText::new("Letters, digits, _ . - (spaces become _)").weak().small());
            ui.horizontal(|ui| {
                ok = ui.button("Rename").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                close = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if ok {
            let name = crate::project::clean_folder(&buf);
            let parent = crate::project::parent(&path).unwrap_or(crate::project::ASSETS).to_owned();
            if name.is_empty() {
                self.status = "A folder needs a name".into();
                return;
            }
            match self.rename_folder(&path, &format!("{parent}/{name}")) {
                Ok(()) => self.folder_rename = None,
                Err(e) => self.status = e,
            }
        } else if close {
            self.folder_rename = None;
        } else {
            self.folder_rename = Some((path, buf));
        }
    }

    /// Selected text anywhere: Ctrl+C or a right-click copies it. The
    /// right-click replays egui's own copy event in the same frame, before
    /// the labels draw; a menu would not do, since a press off the label
    /// ends egui's selection before its button could copy.
    fn text_copy_menu(&mut self, ctx: &egui::Context) {
        let selected = ctx.plugin::<egui::text_selection::LabelSelectionState>().lock().has_selection();
        if selected && ctx.input(|i| i.pointer.secondary_clicked()) {
            ctx.input_mut(|i| i.events.push(egui::Event::Copy));
            self.copy_menu_at = ctx.pointer_latest_pos();
            self.copied_until = Some(Instant::now() + std::time::Duration::from_millis(900));
            self.status = "Copied the selected text".into();
        }
        match (self.copy_menu_at, self.copied_until) {
            (Some(at), Some(t)) if Instant::now() < t => {
                egui::Area::new(egui::Id::unique("text_copied")).order(egui::Order::Tooltip).fixed_pos(at + egui::vec2(12.0, -24.0)).interactable(false).show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| ui.label("Copied"));
                });
                ctx.request_repaint();
            }
            _ => {
                self.copy_menu_at = None;
                self.copied_until = None;
            }
        }
    }

    /// The double-clicked Project item, large; drag to turn it.
    fn ui_preview(&mut self, ctx: &egui::Context) {
        let Some((e, yaw, pitch)) = self.preview.clone() else { return };
        let mut open = true;
        let title = e.rsplit(['\\', '/']).next().unwrap_or(&e).trim_end_matches(".entity.ot").replace('_', " ");
        egui::Window::new("Preview").open(&mut open).resizable(false).default_width(420.0).show(ctx, |ui| {
            ui.strong(&title);
            let side = 400.0;
            let (rect, r) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::drag());
            ui.painter().rect_filled(rect, 4.0, egui::Color32::from_gray(28));
            match self.preview_tex {
                Some(t) if t != egui::TextureId::default() => {
                    ui.painter().image(t, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
                _ => {
                    let msg = if self.thumbs.get(&e) == Some(&egui::TextureId::default()) { "Nothing to draw (a gameplay object)" } else { "Loading…" };
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(14.0), egui::Color32::GRAY);
                }
            }
            if r.dragged() {
                let d = r.drag_delta();
                self.preview = Some((e.clone(), yaw - d.x * 0.01, (pitch + d.y * 0.01).clamp(-1.4, 1.4)));
            }
            ui.label(egui::RichText::new(&e).weak().small());
            ui.label(egui::RichText::new("Drag to turn · drag the item from Project into the scene to place it").weak().small());
        });
        if !open {
            self.preview = None;
            self.preview_tex = None;
        } else {
            ctx.request_repaint();
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

    fn ui_inspector_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().id_salt("inspector").show(ui, |ui| {
                if let Some(ts) = &self.tile {
                    let name = ts.tile.rsplit('\\').next().unwrap_or(&ts.tile).trim_end_matches(".tiledef.ot").to_owned();
                    let built = ts.built;
                    let mut exit = false;
                    egui::Frame::group(ui.style()).fill(ui.visuals().selection.bg_fill.gamma_multiply(0.25)).show(ui, |ui| {
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
                        let n = |v: &Value| v.as_object().map(|o| o.len()).or(v.as_array().map(|a| a.len())).unwrap_or(0);
                        let parts: Vec<String> = [
                            (n(&a.scenery), "scenery edits"),
                            (n(&a.strokes), "brush dabs"),
                            (n(&a.recipe), "recipe changes"),
                            (a.groups.len(), "groups"),
                        ]
                        .into_iter()
                        .filter(|(k, _)| *k > 0)
                        .map(|(k, w)| format!("{k} {w}"))
                        .collect();
                        let when = session::now().saturating_sub(a.saved_at);
                        let ago = if when < 120 { format!("{when} s ago") } else if when < 7200 { format!("{} min ago", when / 60) } else { format!("{} h ago", when / 3600) };
                        ui.label(egui::RichText::new(format!("{} · {ago}", if parts.is_empty() { "map settings".to_owned() } else { parts.join(", ") })).small());
                        ui.horizontal(|ui| {
                            if icons::text_button(ui, Icon::Undo, "Restore").clicked() {
                                self.restore_offer = None;
                                self.restore(&ui.ctx().clone(), a.clone());
                            }
                            if icons::text_button(ui, Icon::Trash, "Discard").clicked() {
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
    }

    fn ui_inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("Properties");
        let Some(s) = &self.scenery else { return };
        if self.sel.len() > 1 {
            ui.label(egui::RichText::new(format!("{} objects selected", self.sel.len())).strong().size(15.0));
            if let Some(p) = self.pivot() {
                // Their centre, editable: moving it moves them all.
                let mut c = p;
                let (mut started, mut changed) = (false, false);
                ui.horizontal(|ui| {
                    ui.label("Centre");
                    for (k, col) in AXIS_COL.iter().enumerate() {
                        let r = ui.add(egui::DragValue::new(&mut c[k]).speed(0.05).fixed_decimals(2).prefix(egui::RichText::new(["X ", "Y ", "Z "][k]).color(*col).text()));
                        started |= r.drag_started() || r.gained_focus();
                        changed |= r.changed();
                    }
                });
                if started {
                    self.history.push(&self.edits);
                }
                if changed {
                    let d = c - p;
                    let xfs: Vec<(Sel, Xf)> = self.sel.iter().filter_map(|x| self.edits.xf(s, *x).map(|xf| (*x, Xf { pos: xf.pos + d, ..xf }))).collect();
                    self.set_live(&xfs);
                    return;
                }
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
                    let icon = match c {
                        Cmd::DropToGround => Some(Icon::Down),
                        Cmd::Group => Some(Icon::Folder),
                        _ => None,
                    };
                    let r = match icon {
                        Some(i) => icons::text_button(ui, i, label),
                        None => ui.button(label),
                    };
                    if r.clicked() {
                        self.run(&ctx, c);
                    }
                }
            });
            return;
        }
        let Some(sel) = self.active.filter(|a| self.sel.contains(a)) else {
            ui.label(egui::RichText::new("Click an object, drag a box on empty ground, or pick one in the Hierarchy.").weak());
            return;
        };
        if let Sel::Spot(id) = sel {
            self.ui_spot_inspector(ui, id);
            return;
        }
        let Some(xf) = self.edits.xf(s, sel) else { return };
        let shipped_xf = match sel {
            Sel::Obj(i) => s.obj_xf.get(i).copied(),
            _ => None,
        };
        let (entity, where_) = match sel {
            Sel::Obj(i) => (
                s.entities.get(s.obj_entity[i] as usize).cloned().unwrap_or_default(),
                format!("{} · id {}", s.levels.get(s.obj_level[i] as usize).map(|l| outliner::level_label(l)).unwrap_or_default(), s.obj_id[i]),
            ),
            Sel::Add(u) => {
                let a = self.edits.add(u).cloned();
                (a.as_ref().map(|a| a.entity.clone()).unwrap_or_default(), match a {
                    Some(a) if a.entity.starts_with(crate::models::PREFIX) => "Your model · new".to_owned(),
                    Some(_) if self.needs_map() => "On this map only · new".to_owned(),
                    a => format!("{} · new", a.map(|a| outliner::level_label(&a.level)).unwrap_or_default()),
                })
            }
            Sel::Spot(_) => return,
        };
        ui.horizontal(|ui| {
            // A picture of it (the Project panel's preview, made on demand).
            let (r, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 6.0, ui.visuals().extreme_bg_color);
            match self.thumbs.get(&entity).copied() {
                Some(t) if t != egui::TextureId::default() => {
                    ui.painter().image(t, r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
                Some(_) => {}
                None => {
                    self.thumb_want.push(entity.clone());
                    self.ensure_template(&ui.ctx().clone(), &entity);
                }
            }
            ui.vertical(|ui| {
                if let Sel::Add(u) = sel {
                    // Its name, typed in place.
                    let mut name = self.edits.add(u).map(|a| a.name.clone()).unwrap_or_default();
                    let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text(self.name_of(sel)).font(egui::TextStyle::Heading).desired_width(f32::INFINITY));
                    if r.changed() {
                        if r.gained_focus() || !self.renaming_inspector {
                            self.history.push(&self.edits);
                            self.renaming_inspector = true;
                        }
                        if let Some(a) = self.edits.adds.iter_mut().find(|a| a.uid == u) {
                            a.name = name;
                        }
                        self.touched();
                    }
                    if r.lost_focus() {
                        self.renaming_inspector = false;
                    }
                } else {
                    ui.label(egui::RichText::new(self.name_of(sel)).strong().size(15.0));
                }
                ui.label(egui::RichText::new(where_).small().weak());
                ui.horizontal(|ui| {
                    let short = entity.rsplit(['\\', '/']).next().unwrap_or(&entity).to_owned();
                    ui.label(egui::RichText::new(short).small().weak()).on_hover_text(&entity);
                    if icons::text_button(ui, Icon::Copy, "Copy path").clicked() {
                        ui.ctx().copy_text(entity.clone());
                        self.status = "Path copied".into();
                    }
                });
            });
        });
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
        let mut row_cmd: Option<(&'static str, usize)> = None; // (reset | copy | paste, row)
        let row_menu = |r: egui::Response, row: usize, cmd: &mut Option<(&'static str, usize)>| {
            r.on_hover_text("Right-click: reset, copy, paste").context_menu(|ui| {
                for (label, what) in [("Reset", "reset"), ("Copy", "copy"), ("Paste", "paste")] {
                    if ui.button(label).clicked() {
                        *cmd = Some((what, row));
                        ui.close();
                    }
                }
            });
        };
        egui::CollapsingHeader::new(egui::RichText::new("Transform").strong()).id_salt("insp_xf").default_open(true).show(ui, |ui| {
        egui::Grid::new("xf").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            ui.label("");
            for (a, c) in ["X", "Y", "Z"].iter().zip(AXIS_COL) {
                ui.label(egui::RichText::new(*a).color(c).strong());
            }
            ui.end_row();
            row_menu(ui.add(egui::Label::new("Location").sense(egui::Sense::click())), 0, &mut row_cmd);
            for k in 0..3 {
                track(ui.add(egui::DragValue::new(&mut new.pos[k]).speed(0.05).fixed_decimals(2).suffix(" m")));
            }
            ui.end_row();
            row_menu(ui.add(egui::Label::new("Rotation").sense(egui::Sense::click())), 1, &mut row_cmd);
            let yaw_only = matches!(sel, Sel::Add(_)) && self.tile.is_some();
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
            row_menu(ui.add(egui::Label::new("Scale").sense(egui::Sense::click())), 2, &mut row_cmd);
            match sel {
                Sel::Add(_) if self.tile.is_some() => {
                    let mut k = new.scale.x;
                    track(ui.add(egui::DragValue::new(&mut k).speed(0.01).range(0.05..=20.0).fixed_decimals(2)));
                    new.scale = Vec3::splat(k);
                }
                Sel::Obj(_) | Sel::Add(_) => {
                    for k in 0..3 {
                        track(ui.add(egui::DragValue::new(&mut new.scale[k]).speed(0.01).range(0.05..=20.0).fixed_decimals(2)));
                    }
                }
                Sel::Spot(_) => {}
            }
            ui.end_row();
        });
        });
        if let Some((what, row)) = row_cmd {
            match what {
                "copy" => {
                    self.xf_clip = Some((row, xf));
                    self.status = format!("Copied the {}", ["location", "rotation", "scale"][row]);
                }
                _ => {
                    let from = match what {
                        "reset" => shipped_xf.or(Some(Xf { pos: xf.pos, rot: Quat::IDENTITY, scale: Vec3::ONE })),
                        _ => self.xf_clip.map(|(_, x)| x),
                    };
                    if let Some(f) = from {
                        let mut out = xf;
                        match row {
                            0 => out.pos = f.pos,
                            1 => out.rot = f.rot,
                            _ => out.scale = f.scale,
                        }
                        self.history.push(&self.edits);
                        self.set_live(&[(sel, out)]);
                        self.touched();
                    }
                }
            }
        }
        if started {
            self.history.push(&self.edits);
        }
        if changed {
            new.rot = Quat::from_euler(glam::EulerRot::YXZ, ypr[0].to_radians(), ypr[1].to_radians(), ypr[2].to_radians());
            self.set_live(&[(sel, new)]);
        }
        // What effects it plays (drawn as particles in the Scene).
        let fx: Vec<(String, Option<String>)> = self
            .templates
            .get(&entity)
            .and_then(|t| t.as_ref())
            .map(|t| t.iter().filter(|(k, _)| k.vfx).map(|(k, _)| (k.texture.clone().unwrap_or_default(), k.color.clone())).collect())
            .unwrap_or_default();
        if !fx.is_empty() {
            egui::CollapsingHeader::new(egui::RichText::new(format!("Effects  {}", fx.len())).strong()).id_salt("insp_fx").default_open(false).show(ui, |ui| {
                for (tex, col) in &fx {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                        let c = col.as_deref().and_then(|h| u32::from_str_radix(h.trim_start_matches('#'), 16).ok()).map(|v| egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)).unwrap_or(egui::Color32::LIGHT_GRAY);
                        ui.painter().circle_filled(r.center(), 5.0, c);
                        ui.label(egui::RichText::new(tex.rsplit('/').next().unwrap_or(tex).trim_end_matches(".png").replace('_', " ")).small());
                    });
                }
                ui.checkbox(&mut self.animate_fx, "Play effects in the Scene");
                ui.label(egui::RichText::new("A preview: the game simulates them in full.").small().weak());
            });
        }
        if let Sel::Add(u) = sel {
            self.ui_camp(ui, u);
            self.ui_collision(ui, u);
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
            for (label, icon, c) in [
                ("Drop to ground", Icon::Down, Cmd::DropToGround),
                ("Reset", Icon::Undo, Cmd::ResetTransform),
                ("Duplicate", Icon::Copy, Cmd::Duplicate),
                ("Delete", Icon::Trash, Cmd::Delete),
            ] {
                if icons::text_button(ui, icon, label).clicked() {
                    self.run(&ctx, c);
                }
            }
        });
    }

    /// Terrain brushes can only ship on a copy of the chapter's map.
    fn needs_map(&self) -> bool {
        self.as_map || self.keep_map.is_some() || !self.edits.strokes.is_empty() || !self.surfaces.is_empty() || self.export_minimap
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
            // A map clone keeps its scenery edits to itself (`map.scenery`).
            "scenery": if self.needs_map() { json!({}) } else { self.edits.scenery_doc(s, true) },
        });
        if self.needs_map() {
            let mut m = self.keep_map.clone().unwrap_or_else(|| json!({"pool": {"add": [], "remove": []}, "place": {}, "terrain": []}));
            m["terrain"] = serde_json::to_value(&self.edits.strokes).unwrap_or(json!([]));
            let id = self.map_id.trim();
            m["id"] = json!(if id.is_empty() { "Map_Check" } else { id });
            m["play"] = json!(self.map_play);
            // Added objects stand in the map's own scenery, never the chapter's.
            m["objects"] = self.edits.objects_doc();
            m["scenery"] = self.edits.scenery_doc(s, false);
            // The mod's own models: `{model, texture, collision}` instead of an
            // entity, and the art a save copies into the mod.
            let mut art = serde_json::Map::new();
            for (o, add) in m["objects"].as_array_mut().into_iter().flatten().zip(&self.edits.adds) {
                if add.entity.starts_with(crate::edit::COLLIDER) {
                    let obj = o.as_object_mut().unwrap();
                    obj.remove("entity");
                    obj.insert("collider".into(), json!(true));
                    continue;
                }
                let Some(rel) = o["entity"].as_str().and_then(|e| e.strip_prefix(crate::models::PREFIX)).map(str::to_owned) else { continue };
                let Some(model) = self.models.iter().find(|x| x.mesh.rel == rel) else { continue };
                o.as_object_mut().unwrap().remove("entity");
                for (k, v) in model.fields().as_object().unwrap() {
                    o[k] = v.clone();
                }
                if let Some(c) = &add.col {
                    let obj = o.as_object_mut().unwrap();
                    obj.remove("collision");
                    if !c.is_empty() {
                        obj.insert("collision".into(), crate::edit::ColBox::list_json(c));
                    }
                }
                for a in std::iter::once(&model.mesh).chain(&model.texture) {
                    if let Some(id) = self.staged.get(&a.rel) {
                        art.insert(a.rel.clone(), json!(id));
                    }
                }
            }
            // Painted blocked ground: one invisible wall per merged rectangle.
            let c = crate::edit::BLOCK_CELL;
            for (x0, z0, x1, z1) in crate::edit::block_rects(&self.edits.blocked) {
                let (w, d) = ((x1 - x0 + 1) as f32 * c, (z1 - z0 + 1) as f32 * c);
                let (cx, cz) = (x0 as f32 * c + w * 0.5, z0 as f32 * c + d * 0.5);
                let gy = [(cx, cz), (x0 as f32 * c, z0 as f32 * c), ((x1 + 1) as f32 * c, (z1 + 1) as f32 * c)]
                    .iter()
                    .filter_map(|&(x, z)| self.ground_y(x, z))
                    .fold(f32::MAX, f32::min);
                let gy = if gy == f32::MAX { 0.0 } else { gy };
                let h = crate::edit::BLOCK_HEIGHT;
                if let Some(a) = m["objects"].as_array_mut() {
                    a.push(json!({"collider": true, "name": crate::edit::BLOCK_NAME, "pos": [cx, gy, cz], "yaw": 0.0, "scale": 1.0,
                        "collision": [{"kind": "wall", "size": [w, h, d], "at": [0.0, h * 0.5 - 1.0, 0.0]}]}));
                }
            }
            if self.export_minimap {
                m["minimap"] = json!(MINIMAP_ART);
                if let Some(id) = self.staged.get(MINIMAP_ART) {
                    art.insert(MINIMAP_ART.to_owned(), json!(id));
                }
            } else {
                m.as_object_mut().map(|o| o.remove("minimap"));
            }
            if self.surfaces.is_empty() {
                m.as_object_mut().map(|o| o.remove("surfaces"));
            } else {
                m["surfaces"] = Value::Object(self.surfaces.iter().map(|(k, (rel, _))| (k.clone(), json!(rel))).collect());
                for (rel, _) in self.surfaces.values() {
                    if let Some(id) = self.staged.get(rel) {
                        art.insert(rel.clone(), json!(id));
                    }
                }
            }
            b["art"] = Value::Object(art);
            b["drop_art"] = json!(self.dropped_art);
            b["art_dirs"] = json!(self.asset_dirs.iter().filter_map(|d| crate::project::art_dir(d)).filter(|d| d != "art").collect::<Vec<_>>());
            b["map"] = m;
        }
        if with_id {
            b["mod_id"] = json!(self.mod_id.trim().to_lowercase());
            b["name"] = json!(self.mod_name.trim());
        }
        Some(b)
    }

    fn submit(&mut self, kind: Pending) {
        // An imported model still on its way to the backend cannot be saved yet.
        let waiting: Vec<&str> = self
            .models
            .iter()
            .filter(|m| self.edits.adds.iter().any(|a| a.entity == m.entity()))
            .flat_map(|m| std::iter::once(&m.mesh).chain(&m.texture))
            .filter(|a| !self.staged.contains_key(&a.rel))
            .map(|a| a.rel.as_str())
            .collect();
        if !waiting.is_empty() {
            self.status = format!("Still importing {}: try again in a moment", waiting.join(", "));
            return;
        }
        let saving = kind == Pending::Save;
        if self.export_minimap && self.tile.is_none() {
            let rect = self.look_base.as_ref().and_then(|l| l.minimap_rect);
            match (rect, &self.terrain) {
                (Some(rect), Some(t)) => {
                    let png = minimap_png(t, rect, 512);
                    // ponytail: a blocking upload (local backend, ~100 KB); async if it ever stalls.
                    match crate::models::upload(&self.backend, MINIMAP_ART, &png) {
                        Ok(id) => {
                            self.staged.insert(MINIMAP_ART.to_owned(), id);
                        }
                        Err(e) => {
                            self.status = format!("Minimap: {e:#}");
                            return;
                        }
                    }
                }
                _ => {
                    self.status = "This chapter has no minimap rectangle to draw (still loading?)".into();
                    return;
                }
            }
        }
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
                if saving {
                    // Edits made while the save runs set `unsaved` again; see the response.
                    self.save_dirty = std::mem::replace(&mut self.unsaved, false);
                }
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
                if ui.add_enabled_ui(!busy, |ui| icons::text_button(ui, Icon::Check, "Check")).inner.clicked() {
                    self.submit(Pending::Check);
                }
                if ui.add_enabled_ui(!busy, |ui| icons::text_button(ui, Icon::Save, "Save")).inner.clicked() {
                    self.run(&ui.ctx().clone(), Cmd::Save);
                }
                if ui.add_enabled_ui(!busy, |ui| icons::text_button(ui, Icon::Save, "Save As…")).inner.clicked() {
                    self.show_save_as = true;
                }
                let can_play = !busy && !self.playing && self.tile.is_none();
                let play = ui.add_enabled_ui(can_play, |ui| icons::text_button(ui, Icon::Play, "Play")).inner;
                if play.on_hover_text("F5: save, apply this mod and start Ravenswatch").clicked() {
                    self.run(&ui.ctx().clone(), Cmd::Play);
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
                    if ui.small_button("Copy error").clicked() {
                        ui.ctx().copy_text(e.to_string());
                    }
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
        // Closed without saving: a Play waiting on it is off.
        if !open && self.pending.is_none() {
            self.play_after_save = false;
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
                        ("Look around", "Right-drag"),
                        ("Fly", "Hold right button + W A S D, Q / E down / up, Shift faster; wheel sets the speed"),
                        ("Orbit", "Alt + left-drag"),
                        ("Pan", "Middle-drag"),
                        ("Zoom", "Wheel, or Alt + right-drag"),
                        ("Frame selection", "F (or double-click it in the Hierarchy)"),
                        ("Up / down", "PageUp / PageDown"),
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
    /// --ui-click: one press / release pair per two settled frames, fed in
    /// before the frame so egui sees a real click.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if self.opts.ui_snapshot.is_none() || self.settled_frames < 4 {
            return;
        }
        // Its own count: a click that starts a load resets `settled_frames`,
        // and the clicks must not replay.
        let k = self.ui_click_step;
        self.ui_click_step += 1;
        // Press, release, then wait out any open / close animation.
        const STEPS: usize = 20;
        if let Some([x, y]) = self.opts.ui_clicks.get(k / STEPS).copied() {
            let pos = egui::pos2(x, y);
            raw.events.push(egui::Event::PointerMoved(pos));
            if k % STEPS < 2 {
                raw.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: k % STEPS == 0, modifiers: Default::default() });
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.text_copy_menu(&ctx);
        // Window title: what is open, and a dot while there is unsaved work.
        let title = {
            let what = self.chapter.as_deref().map(|c| c.replace('_', " ")).unwrap_or_default();
            let m = if self.mod_id.is_empty() { String::new() } else { format!(" · mods/{}", self.mod_id) };
            format!("{}{what}{m} — RSMM Map Editor", if self.unsaved { "● " } else { "" })
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        self.handle_events(&ctx);
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if !dropped.is_empty() {
            self.import_files(&ctx, dropped);
        }
        self.make_clean();
        self.handle_keys(&ctx);
        // Unity's drag preview: what is being dragged follows the cursor.
        if let (Some(what), Some(at)) = (egui::DragAndDrop::payload::<String>(&ctx), ctx.pointer_latest_pos()) {
            let name = what.strip_prefix(crate::models::PREFIX).map(crate::models::stem).unwrap_or_else(|| {
                what.rsplit('\\').next().unwrap_or(&what).trim_end_matches(".entity.ot")
            });
            egui::Area::new(egui::Id::unique("drag_preview")).order(egui::Order::Tooltip).fixed_pos(at + egui::vec2(14.0, 10.0)).interactable(false).show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(format!("Place {}", name.replace('_', " "))));
            });
        }

        egui::Panel::top("menu").show(ui, |ui| self.ui_menu(ui));
        if self.tabs.len() > 1 {
            egui::Panel::top("tabs").show(ui, |ui| self.ui_tabs(ui));
        }
        egui::Panel::bottom("status").show(ui, |ui| self.ui_status(ui));
        // Unity's layout: Hierarchy left, Inspector right, Project/Console below, Scene in the middle.
        egui::Panel::left("hierarchy").resizable(true).default_size(280.0).size_range(200.0..=460.0).show(ui, |ui| self.ui_hierarchy(ui));
        egui::Panel::right("inspector").resizable(true).default_size(330.0).size_range(260.0..=520.0).show(ui, |ui| self.ui_inspector_side(ui));
        if self.show_bottom {
            egui::Panel::bottom("project").resizable(true).default_size(210.0).size_range(90.0..=600.0).show(ui, |ui| self.ui_bottom(ui));
        }
        let active_before = self.active;
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.ui_viewport(ui, frame));
        if self.active != active_before && self.active.is_some() {
            // Picked in the Scene: the Hierarchy shows that row.
            self.scroll_to_sel = self.active;
        }

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
        if self.opts.icon_sheet {
            egui::Window::new("Icons").default_pos([300.0, 120.0]).show(&ctx, icons::sheet);
        }
        self.ui_theme(&ctx); // draws only while View > Theme… is open
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

/// The theme of prefs saved before View > Theme…: dark or light, plus the
/// accent chosen by name in the old menu.
fn legacy_theme(light: bool, accent: Option<&str>) -> crate::theme::Theme {
    let mut t = crate::theme::PRESETS[if light { 1 } else { 0 }].1;
    if let Some(rgb) = accent.and_then(|a| [("Blue", [52, 120, 210]), ("Green", [60, 150, 80]), ("Purple", [140, 90, 200]), ("Red", [190, 60, 60])].into_iter().find(|(n, _)| *n == a)) {
        t.accent = rgb.1;
    }
    t
}

impl MapEditor {
    fn accent32(&self) -> egui::Color32 {
        let [r, g, b] = self.theme.accent;
        egui::Color32::from_rgb(r, g, b)
    }

    /// View > Theme…: every colour, live.
    fn ui_theme(&mut self, ctx: &egui::Context) {
        if !self.show_theme {
            return;
        }
        let mut open = true;
        let mut changed = false;
        egui::Window::new("Theme").open(&mut open).resizable(false).default_width(320.0).show(ctx, |ui| {
            changed = self.theme.ui(ui);
        });
        if changed {
            self.theme.apply(ctx);
            self.dirty = true;
        }
        if !open || changed {
            self.show_theme = open;
            self.save_prefs();
        }
    }
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

/// The ground as the in-game minimap picture: `size` pixels across `rect`
/// (x0, z0, width, depth), row 0 at z0 like the editor's own minimap.
/// ponytail: height colours and water only (no props); in-game orientation unproven.
/// The invisible box a click on an effect hits.
const VFX_PICK: &str = "vfx:pick";

/// An effect sprite's pick box: at its origin, as wide as the sprite (0.6 m at least).
fn vfx_pick_mat(m: Mat4) -> Mat4 {
    let s = m.x_axis.truncate().length().max(0.3) * 2.0;
    Mat4::from_scale_rotation_translation(Vec3::splat(s), Quat::IDENTITY, m.transform_point3(Vec3::ZERO))
}

/// Where Save puts the drawn minimap in the mod.
const MINIMAP_ART: &str = "art/minimap.png";

fn minimap_png(t: &Terrain, rect: [f32; 4], size: u32) -> Vec<u8> {
    let [x0, z0, w, d] = rect;
    let (sx, sz) = ((t.max.x - t.min.x) / t.n as f32, (t.max.z - t.min.z) / t.n as f32);
    let mut img = image::RgbaImage::new(size, size);
    for (px, py, p) in img.enumerate_pixels_mut() {
        let x = x0 + (px as f32 + 0.5) / size as f32 * w;
        let z = z0 + (py as f32 + 0.5) / size as f32 * d;
        let (c, r) = (((x - t.min.x) / sx) as isize, ((z - t.min.z) / sz) as isize);
        if c < 0 || r < 0 || c >= t.n as isize || r >= t.n as isize {
            *p = image::Rgba([0, 0, 0, 0]);
            continue;
        }
        let i = r as usize * t.n + c as usize;
        let mut col = t.color(i);
        if t.water.as_ref().is_some_and(|wa| wa[i] > t.h[i] + 0.05) {
            col = [0.10, 0.18, 0.22];
        }
        let g = |v: f32| (v.max(0.0).powf(1.0 / 2.2) * 255.0).round().min(255.0) as u8;
        *p = image::Rgba([g(col[0]), g(col[1]), g(col[2]), 255]);
    }
    let mut out = Vec::new();
    let _ = img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
    out
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

/// Nearest depth the gizmo is sized for (a pivot behind the eye keeps a sane size).
const NEAR_GIZMO: f32 = 1.0;

/// Hover / active gizmo handles are drawn in this colour (Unity's yellow).
const GIZMO_HOT: egui::Color32 = egui::Color32::from_rgb(255, 214, 60);

/// Gizmo arm length on screen, px. A drag of this far along a scale handle
/// doubles the size; along a rotate ring it turns one radian.
const GIZMO_PX: f32 = 90.0;

/// Which handles the gizmo offers for the current selection, and in which frame.
#[derive(Clone, Copy)]
struct GizmoSpec {
    mode: GizmoMode,
    /// World (move / rotate) or the active object's own axes (scale).
    basis: Quat,
    /// Axes that do something: an added object turns about Y only and scales
    /// uniformly, so its other rings and scale arms are left out.
    axes: [bool; 3],
    planes: bool,
}

impl GizmoSpec {
    fn axis(&self, a: usize) -> Vec3 {
        self.basis * [Vec3::X, Vec3::Y, Vec3::Z][a]
    }

    /// The two axes spanning ring `a` (its points are `u cos t + v sin t`), and
    /// the sign that turns a step in `t` into a turn about `+axis(a)`.
    fn ring_frame(&self, a: usize) -> (Vec3, Vec3, f32) {
        match a {
            0 => (self.axis(1), self.axis(2), 1.0),
            1 => (self.axis(0), self.axis(2), -1.0),
            _ => (self.axis(0), self.axis(1), 1.0),
        }
    }
}

/// The gizmo's handles as they stand on screen this frame.
struct GizmoView {
    /// Each offered axis tip on screen, and whether it can be grabbed (an axis
    /// pointing at the camera, or hiding behind a longer one, is faded).
    tips: [Option<egui::Pos2>; 3],
    usable: [bool; 3],
    /// The plane handle squares, by the axis they leave out (screen polygons);
    /// none when the two axes nearly line up on screen (a sliver, not a handle).
    planes: [Option<[egui::Pos2; 4]>; 3],
    /// Rotate rings: each point, whether it faces the camera, its angle `t`.
    rings: [Vec<(egui::Pos2, bool, f32)>; 3],
}

fn others(skip: usize) -> (usize, usize) {
    match skip {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

fn gizmo_view(g: &GizmoSpec, pivot: Vec3, c: egui::Pos2, len: f32, eye: Vec3, to_screen: &impl Fn(Vec3) -> Option<egui::Pos2>) -> GizmoView {
    let tips: [Option<egui::Pos2>; 3] = std::array::from_fn(|a| if g.axes[a] && g.mode != GizmoMode::Rotate { to_screen(pivot + g.axis(a) * len) } else { None });
    let dir = |a: usize| tips[a].map(|t| t - c).unwrap_or(egui::Vec2::ZERO);
    let cross = |u: egui::Vec2, v: egui::Vec2| (u.x * v.y - u.y * v.x) / (u.length() * v.length()).max(1e-6);
    let usable: [bool; 3] = std::array::from_fn(|a| {
        let d = dir(a);
        // Two arms along the same screen line: only the longer one is grabbable.
        d.length() > 22.0 && (0..3).all(|b| b == a || dir(b).length() <= 22.0 || cross(d, dir(b)).abs() > 0.2 || d.length() > dir(b).length())
    });
    let planes = std::array::from_fn(|skip| {
        let (i, j) = others(skip);
        if !g.planes || !(usable[i] && usable[j]) || cross(dir(i), dir(j)).abs() < 0.4 {
            return None;
        }
        let (u, v) = (g.axis(i), g.axis(j));
        let (lo, hi) = (len * 0.2, len * 0.42);
        let q = [pivot + u * lo + v * lo, pivot + u * hi + v * lo, pivot + u * hi + v * hi, pivot + u * lo + v * hi];
        Some([to_screen(q[0])?, to_screen(q[1])?, to_screen(q[2])?, to_screen(q[3])?])
    });
    let toward = (eye - pivot).normalize_or(Vec3::Y);
    let rings = std::array::from_fn(|a| {
        if g.mode != GizmoMode::Rotate || !g.axes[a] {
            return Vec::new();
        }
        let (u, v, _) = g.ring_frame(a);
        (0..=64)
            .filter_map(|k| {
                let t = k as f32 / 64.0 * std::f32::consts::TAU;
                let off = (u * t.cos() + v * t.sin()) * len;
                to_screen(pivot + off).map(|q| (q, off.dot(toward) >= -0.05 * len, t))
            })
            .collect()
    });
    GizmoView { tips, usable, planes, rings }
}

fn point_in_quad(p: egui::Pos2, q: &[egui::Pos2; 4]) -> bool {
    let side = |a: egui::Pos2, b: egui::Pos2| (b - a).x * (p - a).y - (b - a).y * (p - a).x;
    let s: Vec<f32> = (0..4).map(|i| side(q[i], q[(i + 1) % 4])).collect();
    s.iter().all(|x| *x >= 0.0) || s.iter().all(|x| *x <= 0.0)
}

/// Ring `a`'s point nearest `p`: its angle `t` and the screen direction a
/// growing `t` runs there (what a drag along the ring is measured against).
fn ring_grab(v: &GizmoView, a: usize, p: egui::Pos2) -> Option<(f32, egui::Vec2)> {
    let w = v.rings[a].windows(2).min_by(|x, y| seg_dist(p, x[0].0, x[1].0).total_cmp(&seg_dist(p, y[0].0, y[1].0)))?;
    let t = w[1].0 - w[0].0;
    (t.length() > 1e-3).then(|| (w[0].2, t.normalized()))
}

/// Which gizmo handle is under the pointer: the centre (free / uniform scale),
/// a plane square (two axes), an axis (move / scale arm, rotate ring).
fn gizmo_hit(
    g: &GizmoSpec,
    pivot: Vec3,
    c: egui::Pos2,
    ptr: egui::Pos2,
    len: f32,
    eye: Vec3,
    to_screen: &impl Fn(Vec3) -> Option<egui::Pos2>,
) -> Option<Lock> {
    const TOL: f32 = 13.0; // px: generous, a near miss is worse than a wide grab
    let v = gizmo_view(g, pivot, c, len, eye, to_screen);
    if g.mode == GizmoMode::Rotate {
        // Front halves first: the back of a ring sits behind the others.
        let mut best: Option<(usize, f32)> = None;
        for front in [true, false] {
            for (a, ring) in v.rings.iter().enumerate() {
                let d = ring
                    .windows(2)
                    .filter(|w| w[0].1 == front || w[1].1 == front)
                    .map(|w| seg_dist(ptr, w[0].0, w[1].0))
                    .fold(f32::MAX, f32::min);
                if d < TOL && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((a, d));
                }
            }
            if best.is_some() {
                break;
            }
        }
        return best.map(|(a, _)| Lock::Axis(a));
    }
    if (ptr - c).length() < 12.0 {
        return Some(Lock::Free);
    }
    // An arm wins over a plane square it crosses: the arm is what the eye aims at.
    let arm = (0..3)
        .filter(|a| v.usable[*a])
        .filter_map(|a| v.tips[a].map(|t| (a, seg_dist(ptr, c, t + (t - c).normalized() * 12.0))))
        .filter(|(_, d)| *d < TOL)
        .min_by(|x, y| x.1.total_cmp(&y.1))
        .map(|(a, _)| Lock::Axis(a));
    arm.or_else(|| v.planes.iter().position(|q| q.as_ref().is_some_and(|q| point_in_quad(ptr, q))).map(Lock::Plane))
}

/// The gizmo. `hot` is the handle under the pointer or being dragged; while
/// `busy` (a drag is on) the other handles fade so the one in use stands out.
/// `stretch` lengthens the scale arms in use by the live factor.
#[allow(clippy::too_many_arguments)]
fn draw_gizmo(
    p: &egui::Painter,
    g: &GizmoSpec,
    pivot: Vec3,
    c: egui::Pos2,
    len: f32,
    hot: Option<Lock>,
    busy: bool,
    stretch: f32,
    eye: Vec3,
    to_screen: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    let v = gizmo_view(g, pivot, c, len, eye, to_screen);
    let is_hot = |l: Lock| hot == Some(l);
    // An arm is in use when it is the hot axis, or one of a hot plane's two, or
    // (Free) every arm of a uniform scale.
    let in_use = |a: usize| match hot {
        Some(Lock::Axis(k)) => k == a,
        Some(Lock::Plane(s)) => s != a,
        Some(Lock::Free) => g.mode == GizmoMode::Scale,
        None => false,
    };
    let fade = |col: egui::Color32, on: bool| if busy && !on { col.gamma_multiply(0.25) } else { col };
    let axis_col = |a: usize| {
        let base = if is_hot(Lock::Axis(a)) {
            GIZMO_HOT
        } else if v.usable[a] || g.mode == GizmoMode::Rotate {
            AXIS_COL[a]
        } else {
            AXIS_COL[a].gamma_multiply(0.25)
        };
        fade(base, in_use(a))
    };
    if g.mode == GizmoMode::Rotate {
        // Back halves first and dim, then the front halves on top. While
        // turning, only the ring in use is drawn.
        for front in [false, true] {
            for (a, ring) in v.rings.iter().enumerate() {
                if busy && !is_hot(Lock::Axis(a)) {
                    continue;
                }
                let col = axis_col(a);
                let (w, col) = match (front, is_hot(Lock::Axis(a))) {
                    (true, true) => (4.0, col),
                    (true, false) => (2.5, col),
                    (false, _) => (1.5, col.gamma_multiply(0.3)),
                };
                let pts: Vec<egui::Pos2> = ring.iter().map(|q| q.0).collect();
                for (k, seg) in pts.windows(2).enumerate() {
                    if ring[k].1 == front && ring[k + 1].1 == front {
                        p.line_segment([seg[0], seg[1]], egui::Stroke::new(w, col));
                    }
                }
            }
        }
        p.circle_stroke(c, 4.0, egui::Stroke::new(1.5, egui::Color32::from_white_alpha(170)));
        return;
    }
    // Plane squares under the arms.
    for (skip, q) in v.planes.iter().enumerate() {
        let Some(q) = q else { continue };
        let hotp = is_hot(Lock::Plane(skip));
        let col = fade(if hotp { GIZMO_HOT } else { AXIS_COL[skip] }, hotp);
        p.add(egui::Shape::convex_polygon(q.to_vec(), col.gamma_multiply(if hotp { 0.55 } else { 0.25 }), egui::Stroke::new(1.2, col)));
    }
    for a in 0..3 {
        let Some(mut tip) = v.tips[a] else { continue };
        if g.mode == GizmoMode::Scale && in_use(a) {
            tip = c + (tip - c) * stretch.clamp(0.05, 6.0);
        }
        let col = axis_col(a);
        let w = if is_hot(Lock::Axis(a)) { 4.0 } else { 2.5 };
        p.line_segment([c, tip], egui::Stroke::new(w, col));
        let d = (tip - c).normalized();
        let n = egui::vec2(-d.y, d.x);
        if g.mode == GizmoMode::Move {
            p.add(egui::Shape::convex_polygon(vec![tip + d * 15.0, tip + n * 7.0, tip - n * 7.0], col, egui::Stroke::NONE));
        } else {
            p.rect_filled(egui::Rect::from_center_size(tip, egui::vec2(11.0, 11.0)), 2.0, col);
        }
        if v.usable[a] && !busy {
            p.text(tip + d * 24.0, egui::Align2::CENTER_CENTER, ["X", "Y", "Z"][a], egui::FontId::proportional(12.0), col);
        }
    }
    // The centre: free move, or uniform scale (a cube).
    let hotc = is_hot(Lock::Free);
    let cc = fade(if hotc { GIZMO_HOT } else { egui::Color32::from_white_alpha(200) }, hotc);
    if g.mode == GizmoMode::Scale {
        let s = if hotc && busy { 13.0 * stretch.clamp(0.3, 3.0) } else { 13.0 };
        p.rect_filled(egui::Rect::from_center_size(c, egui::vec2(s, s)), 2.0, cc);
    } else {
        p.circle_stroke(c, 7.0, egui::Stroke::new(if hotc { 3.0 } else { 2.0 }, cc));
    }
}

/// Where on the line `p + s * dir` (unit `dir`) the ray comes nearest; None
/// when they run (nearly) parallel.
fn line_ray_param(p: Vec3, dir: Vec3, (o, d): (Vec3, Vec3)) -> Option<f32> {
    let (b, c) = (dir.dot(d), d.dot(d));
    let w = p - o;
    let den = c - b * b;
    (den.abs() > 1e-4 * c).then(|| (b * d.dot(w) - c * dir.dot(w)) / den)
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
            vfx: p["vfx"].as_bool().unwrap_or(false),
        };
        let f = crate::scene::f32s_pub(m)?;
        out.push((key, f.chunks_exact(16).map(Mat4::from_cols_slice).collect()));
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum TileIcon {
    Folder,
    Prop,
    Gameplay,
    Model,
}

/// A small folder, for the Project tree.
fn folder_glyph(p: &egui::Painter, r: egui::Rect) {
    let c = egui::Color32::from_rgb(226, 176, 86);
    p.rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(6.0, 3.0)), 1.0, egui::Color32::from_rgb(196, 150, 70));
    p.rect_filled(egui::Rect::from_min_max(r.min + egui::vec2(0.0, 2.0), r.max), 2.0, c);
}

/// One Project tile: an icon over the name, Unity's grid view.
/// What the Project panel's folder rows / tiles asked for this frame.
#[derive(Default)]
struct FolderActs {
    move_to: Option<(String, String)>,
    new_in: Option<String>,
    rename: Option<String>,
    delete: Option<String>,
}

impl FolderActs {
    fn take_into(self, m: &mut Option<(String, String)>, n: &mut Option<String>, r: &mut Option<String>, d: &mut Option<String>) {
        *m = self.move_to.or(m.take());
        *n = self.new_in.or(n.take());
        *r = self.rename.or(r.take());
        *d = self.delete.or(d.take());
    }
}

/// A folder row or tile: drop a model (or a folder) on it to move it there;
/// right-click for New folder / Rename / Delete (folders in Assets only).
fn folder_actions(r: &egui::Response, dir: &str, act: &mut FolderActs) {
    use crate::project as P;
    if P::art_dir(dir).is_none() {
        return;
    }
    // A folder of Assets can be dragged onto another.
    if dir != P::ASSETS {
        r.dnd_set_drag_payload(format!("folder:{dir}"));
    }
    if let Some(e) = r.dnd_release_payload::<String>() {
        if e.starts_with(crate::models::PREFIX) || e.starts_with("folder:") {
            act.move_to = Some(((*e).clone(), dir.to_owned()));
        }
    }
    if r.dnd_hover_payload::<String>().is_some_and(|e| e.starts_with(crate::models::PREFIX) || e.starts_with("folder:")) {
        r.ctx.debug_painter().rect_stroke(r.rect, 4.0, egui::Stroke::new(2.0, r.ctx.global_style().visuals.selection.bg_fill), egui::StrokeKind::Outside);
    }
    r.context_menu(|ui| {
        if ui.button("New folder").clicked() {
            act.new_in = Some(dir.to_owned());
            ui.close();
        }
        if dir != P::ASSETS {
            if ui.button("Rename").clicked() {
                act.rename = Some(dir.to_owned());
                ui.close();
            }
            if ui.button("Delete (empty folder)").clicked() {
                act.delete = Some(dir.to_owned());
                ui.close();
            }
        }
    });
}

fn project_tile(ui: &mut egui::Ui, name: &str, icon: TileIcon, on: bool, tip: &str, size: f32, thumb: Option<egui::TextureId>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size - 6.0 + 18.0), egui::Sense::click_and_drag());
    let vis = ui.style().interact_selectable(&resp, on);
    let p = ui.painter();
    if on || resp.hovered() {
        p.rect_filled(rect, 6.0, vis.weak_bg_fill);
    }
    let s = (size / 92.0).max(0.5);
    let ir = egui::Rect::from_center_size(rect.center_top() + egui::vec2(0.0, 30.0 * s), egui::vec2(46.0, 40.0) * s);
    let c = |r: u8, g: u8, b: u8| egui::Color32::from_rgb(r, g, b);
    // `TextureId::default()` marks "nothing to draw" (it is egui's font atlas): show the icon.
    if let Some(t) = thumb.filter(|t| *t != egui::TextureId::default()) {
        // A real preview of the model.
        let sq = egui::Rect::from_center_size(rect.center_top() + egui::vec2(0.0, (size - 24.0) * 0.5 + 3.0), egui::Vec2::splat(size - 24.0));
        p.image(t, sq, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    } else {
    match icon {
        TileIcon::Folder => {
            let tab = egui::Rect::from_min_size(ir.min, egui::vec2(20.0, 8.0) * s);
            p.rect_filled(tab, 2.0, c(196, 150, 70));
            p.rect_filled(egui::Rect::from_min_max(ir.min + egui::vec2(0.0, 6.0 * s), ir.max), 4.0, c(226, 176, 86));
        }
        TileIcon::Prop | TileIcon::Gameplay | TileIcon::Model => {
            // A little cube: top, left and right faces.
            let (top, side, front) = match icon {
                TileIcon::Model => (c(110, 200, 190), c(70, 150, 140), c(90, 175, 165)),
                TileIcon::Gameplay => (c(220, 140, 120), c(160, 90, 80), c(190, 115, 100)),
                _ => (c(200, 196, 188), c(130, 126, 120), c(165, 160, 152)),
            };
            let (cx, cy, s) = (ir.center().x, ir.center().y, 17.0 * s);
            let pt = |x: f32, y: f32| egui::pos2(cx + x * s, cy + y * s);
            let poly = |pts: Vec<egui::Pos2>, col| egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE);
            p.add(poly(vec![pt(0.0, -1.0), pt(1.0, -0.5), pt(0.0, 0.0), pt(-1.0, -0.5)], top));
            p.add(poly(vec![pt(-1.0, -0.5), pt(0.0, 0.0), pt(0.0, 1.1), pt(-1.0, 0.6)], side));
            p.add(poly(vec![pt(0.0, 0.0), pt(1.0, -0.5), pt(1.0, 0.6), pt(0.0, 1.1)], front));
        }
    }
    }
    let label = egui::Rect::from_min_max(egui::pos2(rect.left() + 3.0, rect.bottom() - 28.0), rect.right_bottom() - egui::vec2(3.0, 2.0));
    let text = name.replace('_', " ");
    let galley = ui.painter().layout(text, egui::FontId::proportional(11.5), vis.text_color(), label.width());
    let clip = ui.painter().with_clip_rect(label);
    clip.galley(egui::pos2(label.center().x - galley.size().x / 2.0, label.top()), galley, vis.text_color());
    resp.on_hover_text(tip)
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

#[cfg(test)]
mod gizmo_tests {
    use super::*;

    #[test]
    fn axis_drag_follows_the_ray() {
        // A ray straight down at x = 3 meets the X axis (through the origin) at 3.
        let s = line_ray_param(Vec3::ZERO, Vec3::X, (Vec3::new(3.0, 10.0, 2.0), -Vec3::Y)).unwrap();
        assert!((s - 3.0).abs() < 1e-4);
        // Looking straight along the axis: no answer, the caller falls back.
        assert!(line_ray_param(Vec3::ZERO, Vec3::X, (Vec3::new(-5.0, 0.0, 0.0), Vec3::X)).is_none());
    }

    #[test]
    fn collinear_axes_give_no_plane_and_one_arm() {
        // Y and Z land on the same screen line (top-down view): no YZ square
        // (a sliver that swallowed axis clicks), and only the longer arm grabs.
        let to_screen = |p: Vec3| Some(egui::pos2(100.0 + p.x * 10.0, 100.0 - p.z * 10.0 - p.y * 8.0));
        let g = GizmoSpec { mode: GizmoMode::Move, basis: Quat::IDENTITY, axes: [true; 3], planes: true };
        let v = gizmo_view(&g, Vec3::ZERO, egui::pos2(100.0, 100.0), 9.0, Vec3::new(0.0, 50.0, -5.0), &to_screen);
        assert!(v.planes[0].is_none());
        assert!(v.planes[1].is_some());
        assert_eq!(v.usable, [true, false, true]);
    }
}
