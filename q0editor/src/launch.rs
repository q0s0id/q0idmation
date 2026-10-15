//! Shared start-up for every q0editor entry point.
//!
//! The desktop binary and the Android activity glue each settle the parts of a
//! launch that depend on the platform, then hand the rest to [`run`].

use std::path::PathBuf;

use crate::app::Action;
use crate::platform::SoftKeyboard;
use crate::single_instance::ExternalOpenInbox;
use crate::EditorApp;

/// Framebuffer samples per pixel the stage renderer is tuned for.
pub const PREFERRED_MULTISAMPLING: u16 = 4;

pub struct Launch {
    /// Document to open as soon as the editor is up.
    pub initial_path: Option<PathBuf>,
    /// Overrides the saved vertical-sync preference when set.
    pub vertical_sync: Option<bool>,
    pub external_open_inbox: Option<ExternalOpenInbox>,
    pub window_icon: Option<egui::IconData>,
    /// Framebuffer samples per pixel. Start-up fails when the platform offers
    /// no matching surface configuration, so an entry point that cannot count
    /// on [`PREFERRED_MULTISAMPLING`] has to probe before launching.
    pub multisampling: u16,
    pub event_loop_builder: Option<eframe::EventLoopBuilderHook>,
    /// Where typed text comes from when the platform has no hardware keys.
    pub soft_keyboard: Option<Box<dyn SoftKeyboard>>,
}

impl Default for Launch {
    fn default() -> Self {
        Self {
            initial_path: None,
            vertical_sync: None,
            external_open_inbox: None,
            window_icon: None,
            multisampling: PREFERRED_MULTISAMPLING,
            event_loop_builder: None,
            soft_keyboard: None,
        }
    }
}

pub fn native_options(
    window_icon: Option<egui::IconData>,
    multisampling: u16,
) -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size(crate::ui::MIN_SCREEN_SIZE)
        .with_title("q0editor");
    if let Some(icon) = window_icon {
        viewport = viewport.with_icon(icon);
    }
    eframe::NativeOptions {
        viewport,
        // Lyon gives us a correct non-zero fill mesh for concave raw graphics.
        // Let the framebuffer resolve its actual triangle coverage instead of
        // painting a second translucent outline over the fill.
        multisampling,
        vsync: crate::settings::Settings::load().vertical_sync,
        ..Default::default()
    }
}

pub fn run(launch: Launch) -> eframe::Result<()> {
    let Launch {
        initial_path,
        vertical_sync,
        external_open_inbox,
        window_icon,
        multisampling,
        event_loop_builder,
        soft_keyboard,
    } = launch;

    let mut options = native_options(window_icon, multisampling);
    if let Some(vertical_sync) = vertical_sync {
        options.vsync = vertical_sync;
    }
    options.event_loop_builder = event_loop_builder;
    let vertical_sync = options.vsync;
    eframe::run_native(
        "q0editor",
        options,
        Box::new(move |cc| {
            crate::theme::install(&cc.egui_ctx);
            let mut app = EditorApp::default();
            app.perf.set_presentation_sync(vertical_sync);
            if let Some(inbox) = external_open_inbox {
                app.set_external_open_inbox(inbox);
            }
            if let Some(keyboard) = soft_keyboard {
                app.set_soft_keyboard(keyboard);
            }
            if let Some(path) = initial_path {
                app.queue(Action::OpenProjectFromPath(path));
            }
            Box::new(app)
        }),
    )
}
