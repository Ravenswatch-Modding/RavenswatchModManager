//! The editor's look, every colour the author can change (View > Theme…):
//! accent, panels, windows, inputs, buttons, text, borders, the Scene's
//! background, corner roundness and text size. Saved in the prefs.

use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

type Rgb = [u8; 3];

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Theme {
    /// Start from egui's light widgets (shadows, contrast) instead of dark.
    pub light: bool,
    /// Selection, active tool, links, focus.
    pub accent: Rgb,
    /// Side and bottom panels.
    pub panel: Rgb,
    /// Floating windows and menus.
    pub window: Rgb,
    /// Text boxes, sliders, the darkest wells.
    pub input: Rgb,
    /// Buttons at rest.
    pub button: Rgb,
    /// Buttons under the mouse.
    pub hover: Rgb,
    pub text: Rgb,
    /// Hints and counts.
    pub weak_text: Rgb,
    /// Separators and outlines.
    pub border: Rgb,
    /// The Scene behind the map; `None` uses the chapter's own sky.
    pub scene: Option<Rgb>,
    /// Corner radius of buttons and panels, px.
    pub rounding: f32,
    /// Text size multiplier.
    pub text_scale: f32,
}

impl Default for Theme {
    fn default() -> Self {
        PRESETS[0].1
    }
}

/// Starting points in the Theme window; the first is the default (dark, orange).
pub const PRESETS: [(&str, Theme); 4] = [
    (
        "Dark",
        Theme {
            light: false,
            accent: [196, 120, 40],
            panel: [27, 27, 27],
            window: [32, 32, 32],
            input: [10, 10, 10],
            button: [60, 60, 60],
            hover: [70, 70, 70],
            text: [210, 210, 210],
            weak_text: [130, 130, 130],
            border: [60, 60, 60],
            scene: None,
            rounding: 3.0,
            text_scale: 1.0,
        },
    ),
    (
        "Light",
        Theme {
            light: true,
            accent: [214, 120, 30],
            panel: [242, 242, 242],
            window: [248, 248, 248],
            input: [255, 255, 255],
            button: [225, 225, 225],
            hover: [210, 210, 210],
            text: [30, 30, 30],
            weak_text: [110, 110, 110],
            border: [190, 190, 190],
            scene: None,
            rounding: 3.0,
            text_scale: 1.0,
        },
    ),
    (
        "Midnight",
        Theme {
            light: false,
            accent: [90, 140, 240],
            panel: [20, 23, 32],
            window: [26, 30, 42],
            input: [10, 12, 18],
            button: [42, 48, 66],
            hover: [56, 64, 88],
            text: [214, 222, 240],
            weak_text: [125, 135, 160],
            border: [50, 58, 80],
            scene: Some([14, 18, 28]),
            rounding: 5.0,
            text_scale: 1.0,
        },
    ),
    (
        "Forest",
        Theme {
            light: false,
            accent: [110, 180, 90],
            panel: [24, 30, 24],
            window: [30, 37, 30],
            input: [12, 16, 12],
            button: [46, 58, 46],
            hover: [60, 76, 60],
            text: [218, 228, 210],
            weak_text: [130, 150, 125],
            border: [56, 70, 56],
            scene: None,
            rounding: 4.0,
            text_scale: 1.0,
        },
    ),
];

fn c(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// A colour moved `t` of the way toward `to`.
fn mix(a: Rgb, to: Rgb, t: f32) -> Color32 {
    let m = |i: usize| (a[i] as f32 + (to[i] as f32 - a[i] as f32) * t).round() as u8;
    Color32::from_rgb(m(0), m(1), m(2))
}

impl Theme {
    /// Apply to every egui widget, panel and window.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = if self.light { egui::Visuals::light() } else { egui::Visuals::dark() };
        let ac = c(self.accent);
        v.panel_fill = c(self.panel);
        v.window_fill = c(self.window);
        v.extreme_bg_color = c(self.input);
        v.text_edit_bg_color = Some(c(self.input));
        v.faint_bg_color = mix(self.panel, self.button, 0.35);
        v.code_bg_color = c(self.input);
        v.hyperlink_color = ac;
        v.selection.bg_fill = ac;
        v.selection.stroke.color = if self.light { Color32::BLACK } else { Color32::WHITE };
        v.window_stroke.color = c(self.border);
        v.weak_text_color = Some(c(self.weak_text));
        let r = egui::CornerRadius::same(self.rounding.round().clamp(0.0, 16.0) as u8);
        v.window_corner_radius = egui::CornerRadius::same((self.rounding * 2.0).round().clamp(0.0, 24.0) as u8);
        v.menu_corner_radius = r;
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = c(self.panel);
        w.noninteractive.weak_bg_fill = c(self.panel);
        w.noninteractive.bg_stroke.color = c(self.border);
        w.noninteractive.fg_stroke.color = c(self.text);
        w.inactive.bg_fill = c(self.button);
        w.inactive.weak_bg_fill = c(self.button);
        w.inactive.fg_stroke.color = c(self.text);
        w.hovered.bg_fill = c(self.hover);
        w.hovered.weak_bg_fill = c(self.hover);
        w.hovered.bg_stroke.color = ac;
        w.hovered.fg_stroke.color = c(self.text);
        w.active.bg_fill = mix(self.accent, self.button, 0.25);
        w.active.weak_bg_fill = mix(self.accent, self.button, 0.25);
        w.active.fg_stroke.color = c(self.text);
        w.open.bg_fill = c(self.hover);
        w.open.weak_bg_fill = c(self.hover);
        w.open.fg_stroke.color = c(self.text);
        for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            s.corner_radius = r;
        }
        ctx.set_visuals(v);
        let scale = self.text_scale.clamp(0.7, 1.6);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(8.0, 5.0);
            s.spacing.button_padding = egui::vec2(8.0, 3.0);
            // From egui's own sizes each time, so the scale never compounds.
            s.text_styles = egui::Style::default().text_styles;
            for f in s.text_styles.values_mut() {
                f.size *= scale;
            }
        });
    }

    /// The Scene background, linear 0..1, when the theme sets one.
    pub fn scene_linear(&self) -> Option<[f32; 4]> {
        self.scene.map(|s| {
            let l = |v: u8| (v as f32 / 255.0).powf(2.2);
            [l(s[0]), l(s[1]), l(s[2]), 1.0]
        })
    }

    /// The Theme window's body. True when something changed.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let before = *self;
        ui.horizontal_wrapped(|ui| {
            ui.label("Start from");
            for (name, t) in PRESETS {
                if ui.button(name).clicked() {
                    *self = t;
                }
            }
        });
        ui.separator();
        egui::Grid::new("theme_colours").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            let row = |ui: &mut egui::Ui, label: &str, tip: &str, rgb: &mut Rgb| {
                ui.label(label).on_hover_text(tip);
                ui.color_edit_button_srgb(rgb);
                ui.end_row();
            };
            row(ui, "Accent", "Selection, the active tool, links, focus", &mut self.accent);
            row(ui, "Panels", "Hierarchy, Inspector, Project, the menu bar", &mut self.panel);
            row(ui, "Windows", "Floating windows and menus", &mut self.window);
            row(ui, "Inputs", "Text boxes and sliders", &mut self.input);
            row(ui, "Buttons", "Buttons at rest", &mut self.button);
            row(ui, "Hover", "Buttons under the mouse", &mut self.hover);
            row(ui, "Text", "Labels and buttons", &mut self.text);
            row(ui, "Dim text", "Hints and counts", &mut self.weak_text);
            row(ui, "Borders", "Separators and outlines", &mut self.border);
            ui.label("Scene background").on_hover_text("Behind the map; off = the chapter's own sky");
            ui.horizontal(|ui| {
                let mut own = self.scene.is_some();
                if ui.checkbox(&mut own, "").changed() {
                    self.scene = own.then_some(self.scene.unwrap_or([30, 34, 40]));
                }
                match &mut self.scene {
                    Some(s) => {
                        ui.color_edit_button_srgb(s);
                    }
                    None => {
                        ui.label(egui::RichText::new("the chapter's sky").weak());
                    }
                }
            });
            ui.end_row();
            ui.label("Light widgets").on_hover_text("Shadows and contrast tuned for light colours");
            ui.checkbox(&mut self.light, "");
            ui.end_row();
            ui.label("Roundness");
            ui.add(egui::Slider::new(&mut self.rounding, 0.0..=12.0).suffix(" px"));
            ui.end_row();
            ui.label("Text size");
            ui.add(egui::Slider::new(&mut self.text_scale, 0.8..=1.4).fixed_decimals(2).suffix("×"));
            ui.end_row();
        });
        ui.separator();
        let import_open = egui::Id::new("theme_import_open");
        ui.horizontal(|ui| {
            if ui.button("Reset to default").clicked() {
                *self = Theme::default();
            }
            if ui.button("Copy as text").on_hover_text("Share it: paste it into Import on another machine").clicked() {
                ui.ctx().copy_text(serde_json::to_string(self).unwrap_or_default());
            }
            if ui.button("Import…").on_hover_text("Paste a copied theme").clicked() {
                ui.memory_mut(|m| m.data.insert_temp(import_open, true));
            }
        });
        if ui.memory(|m| m.data.get_temp::<bool>(import_open).unwrap_or(false)) {
            let id = egui::Id::new("theme_import_text");
            let mut text: String = ui.memory(|m| m.data.get_temp(id).unwrap_or_default());
            ui.add(egui::TextEdit::multiline(&mut text).desired_rows(2).hint_text("Paste a theme here").desired_width(f32::INFINITY));
            if let Ok(t) = serde_json::from_str::<Theme>(text.trim()) {
                *self = t;
                text.clear();
                ui.memory_mut(|m| m.data.insert_temp(import_open, false));
            }
            ui.memory_mut(|m| m.data.insert_temp(id, text));
        }
        *self != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_round_trips_and_partial_ones_load() {
        let t = PRESETS[2].1;
        let back: Theme = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
        // Fields a saved theme lacks take the default's.
        let partial: Theme = serde_json::from_str(r#"{"accent":[1,2,3]}"#).unwrap();
        assert_eq!(partial.accent, [1, 2, 3]);
        assert_eq!(partial.panel, Theme::default().panel);
    }

    #[test]
    fn applying_sets_the_colours() {
        let ctx = egui::Context::default();
        let t = Theme { panel: [1, 2, 3], accent: [200, 10, 10], ..Theme::default() };
        t.apply(&ctx);
        let v = ctx.global_style().visuals.clone();
        assert_eq!(v.panel_fill, Color32::from_rgb(1, 2, 3));
        assert_eq!(v.selection.bg_fill, Color32::from_rgb(200, 10, 10));
    }
}
