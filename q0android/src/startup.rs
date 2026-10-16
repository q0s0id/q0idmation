//! What both apps set up before their event loop starts.

use std::path::Path;

/// Points the environment at the app sandbox.
///
/// `dirs` and `std::env::temp_dir` resolve through these variables. Android
/// sets none of them usefully, which would leave the apps with nowhere to
/// keep settings, recovery sessions or temporary media.
pub fn adopt_app_storage(files: &Path) {
    std::env::set_var("HOME", files);
    std::env::set_var("XDG_CONFIG_HOME", files.join("config"));
    std::env::set_var("XDG_DATA_HOME", files.join("data"));
    // The cache directory sits beside `files` in every app sandbox.
    let cache = files.parent().unwrap_or(files).join("cache");
    if std::fs::create_dir_all(&cache).is_ok() {
        std::env::set_var("TMPDIR", cache);
    }
}

/// Picks the framebuffer sample count to launch with: the preferred count, or
/// the nearest lower one this device offers. The stage relies on the
/// framebuffer alone to smooth edges, so fewer samples still beat none.
pub fn choose_multisampling(preferred: u16, is_offered: impl Fn(u16) -> bool) -> u16 {
    let mut samples = preferred;
    while samples > 1 && !is_offered(samples) {
        samples /= 2;
    }
    if samples > 1 {
        samples
    } else {
        0
    }
}

/// Whether EGL has a window configuration with `samples` per pixel.
///
/// The template mirrors the one eframe derives from its native options;
/// eframe panics during start-up when that template matches nothing.
pub fn egl_offers_multisampling(samples: u16) -> bool {
    use glutin::config::ConfigTemplateBuilder;
    use glutin::display::{Display, DisplayApiPreference, GlDisplay};
    use raw_window_handle::{AndroidDisplayHandle, RawDisplayHandle};

    let Ok(samples) = u8::try_from(samples) else {
        return false;
    };
    let handle = RawDisplayHandle::Android(AndroidDisplayHandle::empty());
    // SAFETY: the Android display handle carries no pointer to keep alive.
    let Ok(display) = (unsafe { Display::new(handle, DisplayApiPreference::Egl) }) else {
        return false;
    };
    let template = ConfigTemplateBuilder::new()
        .with_depth_size(0)
        .with_stencil_size(0)
        .with_transparency(false)
        .with_multisampling(samples)
        .build();
    // SAFETY: the template references no native window.
    let offered =
        unsafe { display.find_configs(template) }.is_ok_and(|mut configs| configs.next().is_some());
    offered
}
