//! q0player binary entry point. Recognizes help/version flags, otherwise
//! opens the first CLI argument as a movie so `q0player foo.q0s` and the
//! registered file association both work transparently.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ffi::OsString;
use std::path::PathBuf;

use q0player::ui::app::PreviewOptions;
use q0player::ui::launch::{self, Launch};

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
        "q0player {}\n\nUsage: q0player [OPTIONS] [MOVIE.q0s]\n\nOptions:\n  -h, --help       Show this help\n  -V, --version    Show version\n  --preview       Play the scene without player panels (Esc closes)\n  --all-scenes    Play document scenes in order in preview mode\n  --delete-after-load  Remove an editor-owned temporary preview after loading\n  --qir FILE      Run a QIR bundle as its app would (Q0Build test runs)",
        env!("CARGO_PKG_VERSION")
    );
}

/// The app this program carries, when Q0Build made it a QIR app; or the
/// bundle `--qir file` names, for Q0Build's test runs.
fn qir_bundle(arguments: &[OsString]) -> Option<Result<q0qir::Bundle, String>> {
    let path = match arguments {
        [flag, path, ..] if flag == "--qir" => PathBuf::from(path),
        _ => {
            let exe = std::env::current_exe().ok()?;
            // Only the tail is read: a plain q0player has no bundle there.
            let mut file = std::fs::File::open(&exe).ok()?;
            use std::io::{Read, Seek, SeekFrom};
            file.seek(SeekFrom::End(-(q0qir::TRAILER_LEN as i64)))
                .ok()?;
            let mut tail = [0_u8; q0qir::TRAILER_LEN];
            file.read_exact(&mut tail).ok()?;
            q0qir::Trailer::read(&tail).ok()?;
            exe
        }
    };
    let bundle = match q0qir::Bundle::read_from_file(&path, false) {
        Ok(bundle) if bundle.manifest.diagnostics.verify_on_start => {
            q0qir::Bundle::read_from_file(&path, true)
        }
        other => other,
    };
    Some(bundle.map_err(|error| error.to_string()))
}

fn main() -> Result<(), eframe::Error> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if let Some(bundle) = qir_bundle(&arguments) {
        let bundle = match bundle {
            Ok(bundle) => bundle,
            Err(error) => {
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Error)
                    .set_title("This app cannot start")
                    .set_description(format!(
                        "{error}\n\nDownload it again or ask its author for a new copy."
                    ))
                    .show();
                return Ok(());
            }
        };
        let skip = usize::from(arguments.first().is_some_and(|flag| flag == "--qir")) * 2;
        let args = arguments
            .iter()
            .skip(skip)
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        return launch::run(Launch {
            qir: Some(bundle),
            args,
            ..Launch::default()
        });
    }
    let mut preview = PreviewOptions::default();
    let mut file = None;
    for argument in std::env::args_os().skip(1) {
        match argument.to_str() {
            Some("--preview") => preview.enabled = true,
            Some("--all-scenes") => preview.all_scenes = true,
            Some("--delete-after-load") => preview.delete_after_load = true,
            Some("--help" | "-h" | "--version" | "-V") => {
                file = Some(argument);
                break;
            }
            Some(flag) if flag.starts_with('-') => {
                eprintln!("unknown option: {flag}");
                return Ok(());
            }
            _ if file.is_none() => file = Some(argument),
            _ => {
                eprintln!("only one movie path can be opened");
                return Ok(());
            }
        }
    }
    let initial_file = match startup_action(file) {
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

    launch::run(Launch {
        initial_file,
        preview,
        ..Launch::default()
    })
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
        let options = launch::native_options(launch::PREFERRED_MULTISAMPLING);
        let icon = options.viewport.icon.expect("q0player viewport icon");
        assert_eq!((icon.width, icon.height), (48, 48));
        assert_eq!(icon.rgba.len(), 48 * 48 * 4);
        assert!(icon
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] != 0));
    }

    #[test]
    fn native_window_matches_q0editor_four_sample_msaa() {
        assert_eq!(launch::PREFERRED_MULTISAMPLING, 4);
        assert_eq!(launch::native_options(4).multisampling, 4);
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
