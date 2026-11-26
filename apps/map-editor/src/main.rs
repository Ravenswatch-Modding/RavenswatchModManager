//! RSMM Map Editor: a native map creator and editor for Ravenswatch mods.
//!
//! The game data side (decoding chapters, proving an edit cooks, writing the
//! mod) is the `rsmm` CLI, run as `rsmm editor --stdio`; this binary is the
//! viewport and the tools. See README.md.

mod app;
mod assets;
mod backend;
mod camera;
mod edit;
mod icons;
mod outliner;
mod brush;
mod recipe;
mod session;
mod render;
mod scene;

use eframe::egui;
use eframe::egui_wgpu;
use eframe::wgpu;

fn main() -> eframe::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut rsmm = None;
    let mut opts = app::Options::default();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rsmm" => rsmm = args.next(),
            "--chapter" => opts.chapter = args.next(),
            "--snapshot" => opts.snapshot = args.next().map(Into::into),
            "--self-check" => opts.self_check = true,
            "--tile-check" => opts.tile_check = true,
            "--open-tile" => opts.open_tile = args.next(),
            "--ui-snapshot" => opts.ui_snapshot = args.next().map(Into::into),
            "--view" => {
                let v: Vec<f32> = args.next().unwrap_or_default().split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if v.len() == 3 {
                    opts.view = Some([v[0], v[1], v[2]]);
                }
            }
            "-h" | "--help" => {
                println!("rsmm-map-editor [--rsmm <path to rsmm>] [--chapter <key>] [--snapshot <out.png>]\n\nRuns `rsmm editor --stdio` for game data.");
                return Ok(());
            }
            _ => {}
        }
    }
    let cmd = backend::find_rsmm(rsmm);
    let backend = match backend::Backend::start(&cmd) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("rsmm-map-editor: {e:#}\n(tried: {})", cmd.join(" "));
            std::process::exit(1);
        }
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RSMM Map Editor")
            .with_inner_size([1680.0, 1000.0])
            .with_min_inner_size([900.0, 560.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: {
            // A laptop's discrete GPU, not the integrated one.
            let mut cfg = egui_wgpu::WgpuConfiguration::default();
            if let egui_wgpu::WgpuSetup::CreateNew(c) = &mut cfg.wgpu_setup {
                c.power_preference = wgpu::PowerPreference::HighPerformance;
            }
            cfg
        },
        ..Default::default()
    };
    eframe::run_native("RSMM Map Editor", options, Box::new(move |cc| Ok(Box::new(app::MapEditor::new(cc, backend, opts)))))
}
