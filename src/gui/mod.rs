//! GUI module for Pulsar.

pub mod app;
pub mod live_recorder;
pub mod rig_simulator;
pub mod timeline;

use app::FunGenApp;
use eframe::egui::vec2;

/// Launch the interactive Pulsar GUI application
pub fn run_gui() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("⚡ Pulsar — Funscript Generator")
            .with_inner_size(vec2(1360.0, 860.0))
            .with_min_inner_size(vec2(860.0, 540.0)),
        ..Default::default()
    };

    eframe::run_native(
        "⚡ Pulsar — Funscript Generator",
        native_options,
        Box::new(|_cc| Ok(Box::new(FunGenApp::default()))),
    )
}
