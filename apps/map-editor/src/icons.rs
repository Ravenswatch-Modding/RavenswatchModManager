//! Small icons, drawn as glyphs of a bundled icon font.
//!
//! The bundled text fonts carry no icon glyphs, so the editor ships Phosphor
//! Icons (MIT, assets/PHOSPHOR-LICENSE-MIT.txt), cut down to the glyphs used:
//! the same on every machine, scaled with the UI.

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Response, Sense, Stroke, Ui};

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Eye(bool),
    Lock(bool),
    Move,
    Rotate,
    Scale,
    Magnet,
    Grid,
    Sun,
    Search,
    /// Right-drag look / fly.
    Camera,
    /// Middle-drag pan.
    Hand,
    /// Alt + drag orbit.
    Orbit,
    /// Terrain brush.
    Brush,
    /// Dropping an asset into the Scene.
    Pin,
    Save,
    Check,
    Import,
    Trash,
    Copy,
    /// Drop to the ground.
    Down,
    /// Reset / undo.
    Undo,
    Folder,
    Play,
    /// The ruler (measure tool).
    Ruler,
    /// Snap onto surfaces: a box resting on a slab.
    Surface,
    /// Snap to other objects: two boxes and an alignment line.
    Align,
    /// Local axes (the gizmo follows the object).
    Local,
}

/// A button with an icon before its text, framed like egui's own buttons.
pub fn text_button(ui: &mut Ui, icon: Icon, text: &str) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let pad = ui.spacing().button_padding;
    let icon_w = galley.size().y.max(12.0);
    let size = vec2(pad.x * 2.0 + icon_w + 5.0 + galley.size().x, (galley.size().y + pad.y * 2.0).max(ui.spacing().interact_size.y));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact(&resp);
        ui.painter().rect(rect, v.corner_radius, v.weak_bg_fill, v.bg_stroke, egui::StrokeKind::Inside);
        let ir = Rect::from_min_size(pos2(rect.left() + pad.x, rect.center().y - icon_w * 0.5), vec2(icon_w, icon_w));
        paint(ui.painter(), ir.shrink(1.0), icon, v.fg_stroke.color);
        ui.painter().galley(pos2(ir.right() + 5.0, rect.center().y - galley.size().y * 0.5), galley, v.fg_stroke.color);
    }
    resp
}

/// A small round badge with `icon`: beside the cursor (a tool hint), or in
/// its place (`replace`, the pointer hidden: moving the camera).
pub fn cursor_badge(p: &egui::Painter, at: Pos2, icon: Icon, fill: Color32, replace: bool) {
    if replace {
        // A pointer of its own: the icon in white with a dark outline, like a cursor.
        let r = Rect::from_center_size(at, vec2(20.0, 20.0));
        for d in [vec2(-1.5, 0.0), vec2(1.5, 0.0), vec2(0.0, -1.5), vec2(0.0, 1.5), vec2(-1.1, -1.1), vec2(1.1, 1.1), vec2(-1.1, 1.1), vec2(1.1, -1.1)] {
            paint(p, r.translate(d), icon, Color32::from_black_alpha(220));
        }
        paint(p, r, icon, Color32::WHITE);
        return;
    }
    let c = at + vec2(20.0, 20.0);
    p.circle_filled(c, 12.0, Color32::from_black_alpha(180));
    p.circle_stroke(c, 12.0, Stroke::new(1.5, fill));
    paint(p, Rect::from_center_size(c, vec2(14.0, 14.0)), icon, Color32::WHITE);
}

/// Every icon, large and small (`--icon-sheet`, for checking how they read).
pub fn sheet(ui: &mut Ui) {
    // The camera pointers, on dark and light ground.
    ui.horizontal(|ui| {
        for bg in [40u8, 200] {
            for i in [Icon::Eye(true), Icon::Hand, Icon::Orbit, Icon::Search] {
                let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                ui.painter().rect_filled(r, 4.0, Color32::from_gray(bg));
                cursor_badge(ui.painter(), r.center(), i, Color32::RED, true);
            }
        }
    });
    for size in [48.0f32, 16.0] {
        ui.horizontal_wrapped(|ui| {
            for i in ALL {
                let (r, _) = ui.allocate_exact_size(vec2(size + 8.0, size + 8.0), Sense::hover());
                ui.painter().rect_filled(r, 4.0, Color32::from_gray(40));
                paint(ui.painter(), r.shrink(4.0), i, Color32::WHITE);
            }
        });
    }
}

/// Every icon (the sheet and the paint test).
pub const ALL: [Icon; 29] = [
    Icon::Eye(true), Icon::Eye(false), Icon::Lock(true), Icon::Lock(false), Icon::Move, Icon::Rotate, Icon::Scale,
    Icon::Magnet, Icon::Grid, Icon::Sun, Icon::Search, Icon::Camera, Icon::Hand, Icon::Orbit, Icon::Brush,
    Icon::Pin, Icon::Save, Icon::Check, Icon::Import, Icon::Trash, Icon::Copy, Icon::Down, Icon::Undo,
    Icon::Folder, Icon::Play, Icon::Ruler, Icon::Surface, Icon::Align, Icon::Local,
];

/// A clickable icon. `on` draws it highlighted (a toggled tool).
pub fn button(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let size = vec2(20.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let v = ui.style().interact_selectable(&resp, on);
    if on || resp.hovered() {
        ui.painter().rect_filled(rect.expand(1.0), 4.0, v.weak_bg_fill);
    }
    paint(ui.painter(), rect.shrink(3.0), icon, v.fg_stroke.color);
    resp.on_hover_text(tip)
}

/// Just the icon, no button (inline in a hint).
pub fn paint(p: &egui::Painter, r: Rect, icon: Icon, col: Color32) {
    let size = r.width().min(r.height());
    p.text(r.center(), egui::Align2::CENTER_CENTER, glyph(icon), egui::FontId::proportional(size), col);
}

/// The Phosphor glyph for `icon` (assets/phosphor-subset.ttf holds only these;
/// add a codepoint there with pyftsubset when an icon is added).
fn glyph(icon: Icon) -> &'static str {
    match icon {
        Icon::Eye(true) => "\u{E220}",
        Icon::Eye(false) => "\u{E224}",
        Icon::Lock(true) => "\u{E308}",
        Icon::Lock(false) => "\u{E30A}",
        Icon::Move => "\u{E0A4}",
        Icon::Rotate => "\u{E036}",
        Icon::Scale => "\u{E0A6}",
        Icon::Magnet => "\u{E680}",
        Icon::Grid => "\u{E296}",
        Icon::Sun => "\u{E472}",
        Icon::Search => "\u{E30C}",
        Icon::Camera => "\u{E4DA}",
        Icon::Hand => "\u{E298}",
        Icon::Orbit => "\u{E652}",
        Icon::Brush => "\u{E6F0}",
        Icon::Pin => "\u{E316}",
        Icon::Save => "\u{E248}",
        Icon::Check => "\u{E182}",
        Icon::Import => "\u{E20C}",
        Icon::Trash => "\u{E4A6}",
        Icon::Copy => "\u{E1CA}",
        Icon::Down => "\u{E05C}",
        Icon::Undo => "\u{E038}",
        Icon::Folder => "\u{E24A}",
        Icon::Play => "\u{E3D0}",
        Icon::Ruler => "\u{E6B8}",
        Icon::Surface => "\u{E506}",
        Icon::Align => "\u{EB10}",
        Icon::Local => "\u{EE62}",
    }
}

/// Add the icon font behind the text fonts (its glyphs sit in the Private Use
/// Area, so no letter changes). Call once at start.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("phosphor".into(), std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/phosphor-subset.ttf"))));
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(fam).or_default().push("phosphor".into());
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Click the eye in a Hierarchy-like row: a collapsing header inside a scroll area.
    fn clicks_in_header() -> (bool, bool) {
        let ctx = egui::Context::default();
        let mut at = None;
        let mut got = (false, false);
        let mut frame = |ctx: &egui::Context, events: Vec<egui::Event>, got: &mut (bool, bool), at: &mut Option<Pos2>| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let id = ui.make_persistent_id("row");
                    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                        .show_header(ui, |ui| {
                            ui.horizontal(|ui| {
                                let eye = button(ui, Icon::Eye(true), false, "eye");
                                let lock = button(ui, Icon::Lock(false), false, "lock");
                                *at = Some(eye.rect.center());
                                got.0 |= eye.clicked();
                                got.1 |= lock.clicked();
                            });
                        })
                        .body(|ui| ui.label("x"));
                });
            });
            out.textures_delta.clear();
        };
        frame(&ctx, vec![], &mut got, &mut at);
        let p = at.unwrap();
        let press = |down| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: Default::default() };
        frame(&ctx, vec![egui::Event::PointerMoved(p)], &mut got, &mut at);
        frame(&ctx, vec![press(true)], &mut got, &mut at);
        frame(&ctx, vec![press(false)], &mut got, &mut at);
        got
    }

    #[test]
    fn every_icon_paints() {
        let ctx = egui::Context::default();
        install(&ctx);
        let all = ALL;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            for i in all {
                paint(ui.painter(), Rect::from_min_size(pos2(10.0, 10.0), vec2(14.0, 14.0)), i, Color32::WHITE);
                cursor_badge(ui.painter(), pos2(50.0, 50.0), i, Color32::RED, false);
            }
            assert!(text_button(ui, Icon::Save, "Save").rect.width() > 40.0);
        });
        out.textures_delta.clear();
        assert!(!out.shapes.is_empty());
    }

    #[test]
    fn hierarchy_row_icons_take_clicks() {
        assert_eq!(clicks_in_header(), (true, false));
    }
}
