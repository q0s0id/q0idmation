#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod theme;

use app::TermApp;

fn main() -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([760.0, 520.0])
        .with_min_inner_size([420.0, 280.0])
        .with_title("q0term");

    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "q0term",
        opts,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Box::new(TermApp::new())
        }),
    )
}
