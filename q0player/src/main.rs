//! q0player binary entry point. Recognizes help/version flags, otherwise
//! opens the first CLI argument as a movie so `q0player foo.q0s` and the
//! registered file association both work transparently.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ffi::OsString;
use std::path::PathBuf;

use q0player::ui::PlayerApp;

include!(concat!(env!("OUT_DIR"), "/window_icon.rs"));

#[derive(Debug, PartialEq, Eq)]
enum StartupAction {
    Launch(Option<PathBuf>),
    Help,
    Version,
}

fn startup_action(argument: Option<OsString>) -> StartupAction {
    match argument.as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("-h" | "--help") => StartupAction::Help,
        Some("-V" | "--version") => StartupAction::Version,
        _ => StartupAction::Launch(argument.map(PathBuf::from)),
    }
}

fn print_help() {
    println!(
        "q0player {}\n\nUsage: q0player [MOVIE.q0s]\n\nOptions:\n  -h, --help       Show this help\n  -V, --version    Show version",
        env!("CARGO_PKG_VERSION")
    );
}

fn native_options() -> eframe::NativeOptions {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([640.0, 420.0])
        .with_title("q0player")
        .with_icon(q0_window_icon());
    eframe::NativeOptions {
        viewport,
        // Match q0editor: lyon emits one fill mesh and the native framebuffer
        // resolves triangle-edge coverage. This gives smooth vector edges
        // without painting a second translucent outline or creating dark halos.
        multisampling: 4,
        ..Default::default()
    }
}

fn main() -> Result<(), eframe::Error> {
    let initial_file = match startup_action(std::env::args_os().nth(1)) {
        StartupAction::Launch(path) => path,
        StartupAction::Help => {
            print_help();
            return Ok(());
        }
        StartupAction::Version => {
            println!("q0player {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
    };

    eframe::run_native(
        "q0player",
        native_options(),
        Box::new(move |cc| Box::new(PlayerApp::new(initial_file, &cc.egui_ctx))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_and_version_flags_are_not_treated_as_paths() {
        assert_eq!(
            startup_action(Some(OsString::from("--help"))),
            StartupAction::Help
        );
        assert_eq!(
            startup_action(Some(OsString::from("-V"))),
            StartupAction::Version
        );
    }

    #[test]
    fn native_window_uses_the_q0player_icon() {
        let options = native_options();
        let icon = options.viewport.icon.expect("q0player viewport icon");
        assert_eq!((icon.width, icon.height), (48, 48));
        assert_eq!(icon.rgba.len(), 48 * 48 * 4);
        assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }

    #[test]
    fn native_window_matches_q0editor_four_sample_msaa() {
        assert_eq!(native_options().multisampling, 4);
    }

    #[test]
    fn movie_argument_is_forwarded_to_the_app() {
        assert_eq!(
            startup_action(Some(OsString::from("movie.q0s"))),
            StartupAction::Launch(Some(PathBuf::from("movie.q0s")))
        );
        assert_eq!(startup_action(None), StartupAction::Launch(None));
    }
}
