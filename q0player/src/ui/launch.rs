//! Shared start-up for every q0player entry point.
//!
//! The desktop binary and the Android activity glue each settle the parts of a
//! launch that depend on the platform, then hand the rest to [`run`].

use std::path::PathBuf;

use super::app::{PlayerApp, PreviewOptions};

include!(concat!(env!("OUT_DIR"), "/window_icon.rs"));

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
}

impl Default for Launch {
    fn default() -> Self {
        Self {
            initial_file: None,
            preview: PreviewOptions::default(),
            multisampling: PREFERRED_MULTISAMPLING,
            event_loop_builder: None,
        }
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
    } = launch;
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
