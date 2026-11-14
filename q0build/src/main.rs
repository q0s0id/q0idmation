//! Q0Build: q0s movies packed into apps of their own (QIR).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod apk;
mod app;
mod appimage;
mod builder;
mod icons;
mod inspect;
mod profile;
mod runtimes;
mod settings;
mod squashfs;
mod style;
mod winres;

include!(concat!(env!("OUT_DIR"), "/window_icon.rs"));

/// `q0build --build app.q0build`: builds without a window, for scripts and
/// CI. Prints the log and exits non-zero when nothing was built.
fn build_headless(profile: &std::path::Path) -> std::process::ExitCode {
    let profile = match profile::Profile::load(profile) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let events = builder::spawn(profile);
    for event in events {
        match event {
            builder::Event::Log(level, text) => {
                let mark = match level {
                    builder::Level::Info => " ",
                    builder::Level::Good => "+",
                    builder::Level::Warn => "!",
                    builder::Level::Bad => "x",
                };
                println!("{mark} {text}");
            }
            builder::Event::Progress(..) => {}
            builder::Event::Finished(result) => {
                return match result {
                    Ok(_) => std::process::ExitCode::SUCCESS,
                    Err(_) => std::process::ExitCode::FAILURE,
                };
            }
        }
    }
    std::process::ExitCode::FAILURE
}

fn main() -> std::process::ExitCode {
    let arguments: Vec<std::path::PathBuf> = std::env::args_os()
        .skip(1)
        .map(std::path::PathBuf::from)
        .collect();
    if let [flag, profile] = arguments.as_slice() {
        if flag.as_os_str() == "--build" {
            #[cfg(windows)]
            attach_console();
            return build_headless(profile);
        }
    }
    match run_window(arguments.into_iter().next()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// A release build has no console of its own; a headless build prints to
/// the one it was started from.
#[cfg(windows)]
fn attach_console() {
    extern "system" {
        fn AttachConsole(process: u32) -> i32;
    }
    unsafe {
        AttachConsole(u32::MAX);
    }
}

fn run_window(open: Option<std::path::PathBuf>) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Q0Build")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([1040.0, 640.0])
            .with_icon(q0_window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Q0Build",
        options,
        Box::new(move |cc| Box::new(app::Q0Build::new(&cc.egui_ctx, open))),
    )
}
