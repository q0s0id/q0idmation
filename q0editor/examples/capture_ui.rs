//! Capture real egui frames without changing the user's projects or settings.
//! cargo run -p q0editor --example capture_ui -- out/ui home dark 1280 800
//! Screens: home, new, workspace, settings, export, credits. Themes: any built-in id.
//! Optional final argument: milliseconds to settle before capture (default 1000).

use eframe::App;
use q0editor::{
    app::{Action, NewProjectSpec},
    settings::{Settings, Theme},
    EditorApp,
};
use std::path::PathBuf;

struct Capture {
    editor: EditorApp,
    output: PathBuf,
    started: std::time::Instant,
    delay: std::time::Duration,
    requested: bool,
}

impl App for Capture {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Pointer position should not introduce incidental tooltips into captures.
        ctx.input_mut(|input| input.pointer = Default::default());
        self.editor.update(ctx, frame);
        if !self.requested && self.started.elapsed() >= self.delay {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        let screenshot = ctx.input(|input| {
            input.events.iter().find_map(|event| {
                if let egui::Event::Screenshot { image, .. } = event {
                    Some(image.clone())
                } else {
                    None
                }
            })
        });
        if let Some(image) = screenshot {
            let bytes: Vec<u8> = image
                .pixels
                .iter()
                .flat_map(|pixel| pixel.to_array())
                .collect();
            image::save_buffer(
                &self.output,
                &bytes,
                image.width() as u32,
                image.height() as u32,
                image::ColorType::Rgba8,
            )
            .expect("save screenshot");
            println!("Captured {}", self.output.display());
            // The capture owns only a disposable in-memory document.
            self.editor.state.dirty = false;
            self.editor.close_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        assert!(
            self.started.elapsed() < self.delay + std::time::Duration::from_secs(15),
            "renderer did not return a screenshot"
        );
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("out/ui"));
    let screen = args.get(2).map(String::as_str).unwrap_or("home");
    assert!(
        ["home", "new", "workspace", "settings", "export", "credits"].contains(&screen),
        "unknown screen"
    );
    let theme_id = args.get(3).map(String::as_str).unwrap_or("dark");
    let preset = Theme::PRESETS
        .iter()
        .find(|preset| preset.id == theme_id)
        .expect("known theme id");
    let width = args
        .get(4)
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(1280.0);
    let height = args
        .get(5)
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(800.0);
    let delay = std::time::Duration::from_millis(
        args.get(6)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(1000),
    );
    std::fs::create_dir_all(&dir).expect("create capture directory");
    let output = dir.join(format!(
        "{screen}-{theme_id}-{}x{}.png",
        width as u32, height as u32
    ));
    let mut editor = EditorApp::default();
    editor.settings = Settings {
        theme: Theme::from_builtin(preset),
        ..Settings::default()
    };
    if ["workspace", "export", "credits"].contains(&screen) {
        editor.queue(Action::CreateProject(NewProjectSpec {
            name: "Motion study".into(),
            ..NewProjectSpec::default()
        }));
        editor.flush_pending_actions();
    }
    match screen {
        "new" => editor.queue(Action::NewProject),
        "settings" => editor.session.show_settings = true,
        "export" => editor.queue(Action::OpenQ0Enc),
        "credits" => editor.queue(Action::ToggleCredits),
        _ => {}
    }
    eframe::run_native(
        "q0editor visual capture",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([width, height]),
            multisampling: 4,
            ..Default::default()
        },
        Box::new(move |_cc| {
            Box::new(Capture {
                editor,
                output,
                started: std::time::Instant::now(),
                delay,
                requested: false,
            })
        }),
    )
}
