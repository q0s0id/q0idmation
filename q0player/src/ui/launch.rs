//! Shared start-up for every q0player entry point.
//!
//! The desktop binary and the Android activity glue each settle the parts of a
//! launch that depend on the platform, then hand the rest to [`run`].

use std::path::PathBuf;

use super::app::{PlayerApp, PreviewOptions};

include!(concat!(env!("OUT_DIR"), "/window_icon.rs"));

pub(crate) fn default_window_icon() -> egui::IconData {
    q0_window_icon()
}

/// Framebuffer samples per pixel, matching q0editor: lyon emits one fill
/// mesh and the framebuffer resolves triangle-edge coverage, which smooths
/// vector edges without a second translucent outline or dark halos.
pub const PREFERRED_MULTISAMPLING: u16 = 4;

pub struct Launch {
    /// Movie to open as soon as the player is up.
    pub initial_file: Option<PathBuf>,
    pub preview: PreviewOptions,
    /// Framebuffer samples per pixel. Start-up fails when the platform offers
    /// no matching surface configuration, so an entry point that cannot count
    /// on [`PREFERRED_MULTISAMPLING`] has to probe before launching.
    pub multisampling: u16,
    pub event_loop_builder: Option<eframe::EventLoopBuilderHook>,
    /// A QIR app: the movie, its manifest and icon, which decide the window.
    pub qir: Option<q0qir::Bundle>,
    /// What a QIR app reads as `app.args`.
    pub args: Vec<String>,
}

impl Default for Launch {
    fn default() -> Self {
        Self {
            initial_file: None,
            preview: PreviewOptions::default(),
            multisampling: PREFERRED_MULTISAMPLING,
            event_loop_builder: None,
            qir: None,
            args: Vec::new(),
        }
    }
}

/// The window a QIR app's manifest asks for. `most_samples` caps the
/// anti-aliasing where the platform was probed for what it offers.
pub fn qir_options(
    manifest: &q0qir::Manifest,
    icon_png: Option<&[u8]>,
    most_samples: u16,
) -> eframe::NativeOptions {
    let window = &manifest.window;
    let icon = icon_png
        .and_then(|png| eframe::icon_data::from_png_bytes(png).ok())
        .unwrap_or_else(q0_window_icon);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([window.width as f32, window.height as f32])
        .with_min_inner_size([
            window.min_width.max(64) as f32,
            window.min_height.max(64) as f32,
        ])
        .with_title(manifest.title())
        .with_resizable(window.resizable)
        .with_decorations(window.decorations)
        .with_app_id(manifest.app.id.clone())
        .with_icon(icon);
    viewport = match window.start {
        q0qir::StartMode::Windowed => viewport,
        q0qir::StartMode::Maximized => viewport.with_maximized(true),
        q0qir::StartMode::Fullscreen => viewport.with_fullscreen(true),
    };
    if window.always_on_top {
        viewport = viewport.with_always_on_top();
    }
    let samples = manifest.render.msaa.min(most_samples);
    eframe::NativeOptions {
        viewport,
        multisampling: if samples <= 1 { 0 } else { samples },
        vsync: manifest.render.vsync,
        centered: window.centered,
        persist_window: false,
        ..Default::default()
    }
}

pub fn native_options(multisampling: u16) -> eframe::NativeOptions {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([640.0, 420.0])
        .with_title("q0player")
        .with_icon(q0_window_icon());
    eframe::NativeOptions {
        viewport,
        multisampling,
        ..Default::default()
    }
}

pub fn run(launch: Launch) -> eframe::Result<()> {
    let Launch {
        initial_file,
        preview,
        multisampling,
        event_loop_builder,
        qir,
        args,
    } = launch;
    if let Some(bundle) = qir {
        // Desktops offer 8 samples; a phone says what it offers.
        let most = if super::platform::TOUCH {
            multisampling
        } else {
            8
        };
        let mut options = qir_options(&bundle.manifest, bundle.icon_png.as_deref(), most);
        options.event_loop_builder = event_loop_builder;
        let app_id = bundle.manifest.app.id.clone();
        let mut session = super::qir::QirSession::new(bundle, args);
        if !session.claim_single_instance() {
            session.log("already running; this copy closes");
            return Ok(());
        }
        return eframe::run_native(
            &app_id,
            options,
            Box::new(move |cc| Box::new(PlayerApp::new(None, &cc.egui_ctx).with_qir(session))),
        );
    }
    let mut options = native_options(multisampling);
    options.event_loop_builder = event_loop_builder;
    eframe::run_native(
        "q0player",
        options,
        Box::new(move |cc| {
            Box::new(PlayerApp::new(initial_file, &cc.egui_ctx).with_preview(preview))
        }),
    )
}
