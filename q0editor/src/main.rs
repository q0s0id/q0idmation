#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use q0editor::{app::Action, EditorApp};

include!(concat!(env!("OUT_DIR"), "/window_icon.rs"));

fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("q0editor")
            .with_icon(q0_window_icon()),
        // Lyon gives us a correct non-zero fill mesh for concave raw graphics.
        // Let the framebuffer resolve its actual triangle coverage instead of
        // painting a second translucent outline over the fill.
        multisampling: 4,
        ..Default::default()
    }
}

fn main() -> eframe::Result<()> {
    let initial_path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let external_open_inbox =
        match q0editor::single_instance::claim_or_forward(initial_path.clone()) {
            q0editor::single_instance::LaunchDisposition::Primary(primary) => Some(primary.start()),
            q0editor::single_instance::LaunchDisposition::Forwarded => return Ok(()),
            q0editor::single_instance::LaunchDisposition::Secondary => None,
        };

    eframe::run_native(
        "q0editor",
        native_options(),
        Box::new(move |cc| {
            q0editor::theme::install(&cc.egui_ctx);
            let mut app = EditorApp::default();
            if let Some(inbox) = external_open_inbox {
                app.set_external_open_inbox(inbox);
            }
            if let Some(path) = initial_path {
                app.queue(Action::OpenProjectFromPath(path));
            }
            Box::new(app)
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_window_uses_the_q0editor_icon() {
        let options = native_options();
        let icon = options.viewport.icon.expect("q0editor viewport icon");
        assert_eq!((icon.width, icon.height), (48, 48));
        assert_eq!(icon.rgba.len(), 48 * 48 * 4);
        assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }
}
