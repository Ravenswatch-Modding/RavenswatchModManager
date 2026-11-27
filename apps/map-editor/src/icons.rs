//! Small vector icons drawn with the painter.
//!
//! The bundled fonts carry no icon glyphs (an eye, a lock, a magnet … come out
//! as empty boxes), so the editor draws its own: they look the same on every
//! machine and scale with the UI.

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};

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

/// A small round badge with `icon`, drawn beside the cursor (what the mouse is doing).
pub fn cursor_badge(p: &egui::Painter, at: Pos2, icon: Icon, fill: Color32) {
    let c = at + vec2(20.0, 20.0);
    p.circle_filled(c, 11.0, Color32::from_black_alpha(170));
    p.circle_stroke(c, 11.0, Stroke::new(1.5, fill));
    paint(p, Rect::from_center_size(c, vec2(13.0, 13.0)), icon, Color32::WHITE);
}

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
    let s = Stroke::new(1.6, col);
    let c = r.center();
    let w = r.width().min(r.height());
    match icon {
        Icon::Eye(open) => {
            let pts: Vec<Pos2> = (0..=24)
                .map(|k| {
                    let t = k as f32 / 24.0 * std::f32::consts::TAU;
                    pos2(c.x + t.cos() * w * 0.5, c.y + t.sin() * w * 0.27)
                })
                .collect();
            if open {
                p.add(Shape::closed_line(pts, s));
                p.circle_filled(c, w * 0.16, col);
            } else {
                // Closed: the lower lid and lashes.
                let lid: Vec<Pos2> = (0..=12)
                    .map(|k| {
                        let t = k as f32 / 12.0 * std::f32::consts::PI;
                        pos2(c.x - t.cos() * w * 0.5, c.y + t.sin() * w * 0.2)
                    })
                    .collect();
                p.add(Shape::line(lid, Stroke::new(1.4, col.gamma_multiply(0.6))));
                for dx in [-0.25f32, 0.0, 0.25] {
                    let a = pos2(c.x + dx * w, c.y + w * 0.18);
                    p.line_segment([a, a + vec2(dx * w * 0.3, w * 0.18)], Stroke::new(1.2, col.gamma_multiply(0.6)));
                }
            }
        }
        Icon::Lock(locked) => {
            let body = Rect::from_center_size(pos2(c.x, c.y + w * 0.15), vec2(w * 0.8, w * 0.55));
            if locked {
                p.rect_filled(body, 2.0, col);
            } else {
                p.rect_stroke(body, 2.0, Stroke::new(1.4, col.gamma_multiply(0.6)), egui::StrokeKind::Inside);
            }
            let shift = if locked { 0.0 } else { w * 0.22 };
            let arc: Vec<Pos2> = (0..=12)
                .map(|k| {
                    let t = std::f32::consts::PI + k as f32 / 12.0 * std::f32::consts::PI;
                    pos2(c.x + shift + t.cos() * w * 0.26, body.top() - w * 0.02 + t.sin() * w * 0.32)
                })
                .collect();
            p.add(Shape::line(arc, Stroke::new(1.6, if locked { col } else { col.gamma_multiply(0.6) })));
        }
        Icon::Move => {
            for d in [vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0)] {
                let tip = c + d * w * 0.5;
                p.line_segment([c, tip], s);
                let n = vec2(-d.y, d.x);
                p.add(Shape::convex_polygon(vec![tip + d * 2.0, tip - d * 4.0 + n * 3.5, tip - d * 4.0 - n * 3.5], col, Stroke::NONE));
            }
        }
        Icon::Rotate => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|k| {
                    let t = -0.6 + k as f32 / 20.0 * 4.6;
                    pos2(c.x + t.cos() * w * 0.42, c.y + t.sin() * w * 0.42)
                })
                .collect();
            let end = *pts.last().unwrap();
            p.add(Shape::line(pts, s));
            p.add(Shape::convex_polygon(vec![end + vec2(4.0, 1.0), end + vec2(-3.0, -3.0), end + vec2(-2.0, 4.0)], col, Stroke::NONE));
        }
        Icon::Scale => {
            let a = pos2(r.left() + 1.0, r.bottom() - 1.0);
            let b = pos2(r.right() - 1.0, r.top() + 1.0);
            p.rect_stroke(Rect::from_min_max(pos2(a.x, c.y), pos2(c.x, a.y)), 1.0, s, egui::StrokeKind::Inside);
            p.line_segment([c, b], s);
            p.add(Shape::convex_polygon(vec![b, b + vec2(-6.0, 0.0), b + vec2(0.0, 6.0)], col, Stroke::NONE));
        }
        Icon::Magnet => {
            let pts: Vec<Pos2> = (0..=12)
                .map(|k| {
                    let t = k as f32 / 12.0 * std::f32::consts::PI;
                    pos2(c.x - t.cos() * w * 0.36, c.y + w * 0.05 + t.sin() * w * 0.36)
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(3.0, col)));
            for x in [c.x - w * 0.36, c.x + w * 0.36] {
                p.line_segment([pos2(x, c.y + w * 0.05), pos2(x, r.top() + 1.0)], Stroke::new(3.0, col));
                p.line_segment([pos2(x - 1.5, r.top() + 3.0), pos2(x + 1.5, r.top() + 3.0)], Stroke::new(2.0, Color32::from_rgb(235, 72, 72)));
            }
        }
        Icon::Grid => {
            for k in 0..=3 {
                let f = k as f32 / 3.0;
                p.line_segment([pos2(r.left() + f * r.width(), r.top()), pos2(r.left() + f * r.width(), r.bottom())], Stroke::new(1.0, col));
                p.line_segment([pos2(r.left(), r.top() + f * r.height()), pos2(r.right(), r.top() + f * r.height())], Stroke::new(1.0, col));
            }
        }
        Icon::Sun => {
            p.circle_filled(c, w * 0.22, col);
            for k in 0..8 {
                let t = k as f32 / 8.0 * std::f32::consts::TAU;
                let d = vec2(t.cos(), t.sin());
                p.line_segment([c + d * w * 0.34, c + d * w * 0.5], Stroke::new(1.4, col));
            }
        }
        Icon::Search => {
            p.circle_stroke(c - vec2(w * 0.1, w * 0.1), w * 0.3, s);
            p.line_segment([c + vec2(w * 0.12, w * 0.12), c + vec2(w * 0.45, w * 0.45)], Stroke::new(2.0, col));
        }
        Icon::Camera => {
            let body = Rect::from_center_size(c + vec2(0.0, w * 0.06), vec2(w * 0.9, w * 0.6));
            p.rect_stroke(body, 2.0, s, egui::StrokeKind::Inside);
            p.rect_filled(Rect::from_center_size(pos2(c.x - w * 0.18, body.top() - w * 0.06), vec2(w * 0.28, w * 0.14)), 1.0, col);
            p.circle_stroke(body.center(), w * 0.17, s);
        }
        Icon::Hand => {
            // Four fingers and a palm.
            let palm = Rect::from_min_max(pos2(c.x - w * 0.32, c.y), pos2(c.x + w * 0.3, c.y + w * 0.42));
            p.rect_filled(palm, w * 0.12, col);
            for (i, h) in [0.34f32, 0.44, 0.42, 0.32].iter().enumerate() {
                let x = c.x - w * 0.26 + i as f32 * w * 0.17;
                p.line_segment([pos2(x, c.y + w * 0.05), pos2(x, c.y - h * w)], Stroke::new(w * 0.12, col));
            }
            p.line_segment([pos2(c.x + w * 0.28, c.y + w * 0.2), pos2(c.x + w * 0.48, c.y - w * 0.02)], Stroke::new(w * 0.12, col));
        }
        Icon::Orbit => {
            let pts: Vec<Pos2> = (0..=24)
                .map(|k| {
                    let t = 0.3 + k as f32 / 24.0 * 5.2;
                    pos2(c.x + t.cos() * w * 0.48, c.y + t.sin() * w * 0.22)
                })
                .collect();
            let end = *pts.last().unwrap();
            p.add(Shape::line(pts, s));
            p.add(Shape::convex_polygon(vec![end + vec2(3.5, -1.0), end + vec2(-2.0, -3.5), end + vec2(-1.0, 3.0)], col, Stroke::NONE));
            p.circle_filled(c, w * 0.14, col);
        }
        Icon::Brush => {
            let a = pos2(r.right() - 1.0, r.top() + 1.0);
            let b = pos2(c.x - w * 0.05, c.y + w * 0.05);
            p.line_segment([a, b], Stroke::new(2.0, col));
            p.add(Shape::convex_polygon(vec![b + vec2(-1.5, -2.5), b + vec2(2.5, 1.5), pos2(r.left() + 1.0, r.bottom() - 1.0)], col, Stroke::NONE));
        }
        Icon::Pin => {
            let head = pos2(c.x, c.y - w * 0.12);
            p.circle_filled(head, w * 0.28, col);
            p.add(Shape::convex_polygon(vec![head + vec2(-w * 0.22, w * 0.12), head + vec2(w * 0.22, w * 0.12), pos2(c.x, r.bottom())], col, Stroke::NONE));
            p.circle_filled(head, w * 0.1, Color32::from_black_alpha(160));
        }
        Icon::Save => {
            let b = r.shrink(1.0);
            p.rect_stroke(b, 2.0, s, egui::StrokeKind::Inside);
            p.rect_filled(Rect::from_min_size(b.min + vec2(w * 0.22, 1.0), vec2(w * 0.45, w * 0.28)), 1.0, col);
            p.rect_stroke(Rect::from_min_max(pos2(b.left() + w * 0.2, c.y + w * 0.08), pos2(b.right() - w * 0.2, b.bottom())), 1.0, Stroke::new(1.2, col), egui::StrokeKind::Inside);
        }
        Icon::Check => {
            p.add(Shape::line(vec![pos2(r.left() + 1.0, c.y), pos2(c.x - w * 0.1, r.bottom() - w * 0.15), pos2(r.right() - 1.0, r.top() + w * 0.15)], Stroke::new(2.0, col)));
        }
        Icon::Import => {
            p.line_segment([pos2(c.x, r.top()), pos2(c.x, c.y + w * 0.12)], s);
            p.add(Shape::convex_polygon(vec![pos2(c.x, c.y + w * 0.25), pos2(c.x - w * 0.22, c.y), pos2(c.x + w * 0.22, c.y)], col, Stroke::NONE));
            p.add(Shape::line(vec![pos2(r.left(), c.y + w * 0.15), pos2(r.left(), r.bottom()), pos2(r.right(), r.bottom()), pos2(r.right(), c.y + w * 0.15)], s));
        }
        Icon::Trash => {
            let b = Rect::from_min_max(pos2(r.left() + w * 0.18, r.top() + w * 0.25), pos2(r.right() - w * 0.18, r.bottom()));
            p.rect_stroke(b, 2.0, s, egui::StrokeKind::Inside);
            p.line_segment([pos2(r.left(), r.top() + w * 0.18), pos2(r.right(), r.top() + w * 0.18)], s);
            p.line_segment([pos2(c.x - w * 0.12, r.top()), pos2(c.x + w * 0.12, r.top())], s);
            for dx in [-0.12f32, 0.12] {
                p.line_segment([pos2(c.x + dx * w, b.top() + 3.0), pos2(c.x + dx * w, b.bottom() - 3.0)], Stroke::new(1.0, col));
            }
        }
        Icon::Copy => {
            let back = Rect::from_min_size(r.min, vec2(w * 0.65, w * 0.65));
            p.rect_stroke(back, 2.0, Stroke::new(1.2, col.gamma_multiply(0.6)), egui::StrokeKind::Inside);
            p.rect_stroke(back.translate(vec2(w * 0.32, w * 0.32)), 2.0, s, egui::StrokeKind::Inside);
        }
        Icon::Down => {
            p.line_segment([pos2(c.x, r.top()), pos2(c.x, c.y + w * 0.15)], s);
            p.add(Shape::convex_polygon(vec![pos2(c.x, c.y + w * 0.3), pos2(c.x - w * 0.2, c.y + w * 0.02), pos2(c.x + w * 0.2, c.y + w * 0.02)], col, Stroke::NONE));
            p.line_segment([pos2(r.left(), r.bottom()), pos2(r.right(), r.bottom())], Stroke::new(2.0, col));
        }
        Icon::Undo => {
            let pts: Vec<Pos2> = (0..=16)
                .map(|k| {
                    let t = 3.6 - k as f32 / 16.0 * 4.4;
                    pos2(c.x + t.cos() * w * 0.4, c.y + t.sin() * w * 0.4)
                })
                .collect();
            let start = pts[0];
            p.add(Shape::line(pts, s));
            p.add(Shape::convex_polygon(vec![start + vec2(-3.5, 0.5), start + vec2(2.5, -3.0), start + vec2(2.0, 3.5)], col, Stroke::NONE));
        }
        Icon::Folder => {
            p.rect_filled(Rect::from_min_size(pos2(r.left(), r.top() + w * 0.12), vec2(w * 0.45, w * 0.2)), 1.5, col.gamma_multiply(0.8));
            p.rect_filled(Rect::from_min_max(pos2(r.left(), r.top() + w * 0.25), r.max), 2.0, col);
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![pos2(r.left() + w * 0.2, r.top()), pos2(r.right(), c.y), pos2(r.left() + w * 0.2, r.bottom())], col, Stroke::NONE));
        }
    }
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
        let all = [
            Icon::Eye(true), Icon::Eye(false), Icon::Lock(true), Icon::Lock(false), Icon::Move, Icon::Rotate, Icon::Scale,
            Icon::Magnet, Icon::Grid, Icon::Sun, Icon::Search, Icon::Camera, Icon::Hand, Icon::Orbit, Icon::Brush,
            Icon::Pin, Icon::Save, Icon::Check, Icon::Import, Icon::Trash, Icon::Copy, Icon::Down, Icon::Undo,
            Icon::Folder, Icon::Play,
        ];
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            for i in all {
                paint(ui.painter(), Rect::from_min_size(pos2(10.0, 10.0), vec2(14.0, 14.0)), i, Color32::WHITE);
                cursor_badge(ui.painter(), pos2(50.0, 50.0), i, Color32::RED);
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
