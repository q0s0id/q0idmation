//! Opens an FLA in a throwaway editor window and saves what the editor
//! draws, without touching the user's projects or settings.
//! cargo run -p q0editor --example capture_fla -- OUT.png FILE.fla [frame] [width] [height]

use eframe::App;
use q0editor::{
    app::{Action, NewProjectSpec},
    EditorApp,
};
use std::path::PathBuf;

struct Capture {
    editor: EditorApp,
    output: PathBuf,
    started: std::time::Instant,
    requested: bool,
}

impl App for Capture {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        ctx.input_mut(|input| input.pointer = Default::default());
        self.editor.update(ctx, frame);
        if !self.requested && self.started.elapsed() >= std::time::Duration::from_millis(2500) {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        let screenshot = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
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
            self.editor.state.dirty = false;
            self.editor.close_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        assert!(
            self.started.elapsed() < std::time::Duration::from_secs(30),
            "renderer did not return a screenshot"
        );
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let output = PathBuf::from(&args[1]);
    let source = PathBuf::from(&args[2]);
    let frame = args.get(3).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
    let width = args
        .get(4)
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(1600.0);
    let height = args
        .get(5)
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(1000.0);
    let imported = q0fla::import_path(&source).expect("import the fla");
    let entry = imported.project.meta.entry_q0rg_id;
    let mut editor = EditorApp::default();
    editor.queue(Action::CreateProject(NewProjectSpec {
        name: "fla capture".into(),
        ..NewProjectSpec::default()
    }));
    editor.flush_pending_actions();
    editor.state.project = imported.project;
    editor.session.current_q0rg_id = entry;
    editor.session.current_frame = frame;
    editor.state.mark_dirty();
    editor.queue(Action::ZoomReset);
    eframe::run_native(
        "q0editor fla capture",
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
                requested: false,
            })
        }),
    )
}
