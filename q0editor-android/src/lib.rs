//! Android entry point for q0editor.
//!
//! `MainActivity` loads this library; the `android-activity` glue then calls
//! [`android_main`] on its own native thread.

#![cfg(target_os = "android")]

mod keyboard;

use std::path::Path;

use q0editor::launch::{Launch, PREFERRED_MULTISAMPLING};
use winit::platform::android::activity::input::Axis;
use winit::platform::android::activity::AndroidApp;
use winit::platform::android::EventLoopBuilderExtAndroid;

#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("q0editor")
            .with_max_level(log::LevelFilter::Debug),
    );
    // No shell starts this process, so this is the only place to ask for one.
    std::env::set_var("RUST_BACKTRACE", "1");

    match app.internal_data_path() {
        Some(files) => adopt_app_storage(&files),
        None => log::error!("no app data directory; settings and recovery are unavailable"),
    }
    // GameActivity reports only X and Y unless asked, and the brush wants pressure.
    app.enable_motion_axis(Axis::Pressure);

    let multisampling = choose_multisampling(PREFERRED_MULTISAMPLING, egl_offers_multisampling);
    log::info!("framebuffer multisampling: {multisampling}x");

    let launch = Launch {
        multisampling,
        soft_keyboard: Some(Box::new(keyboard::ActivityKeyboard::new(app.clone()))),
        event_loop_builder: Some(Box::new(move |builder| {
            builder.with_android_app(app);
        })),
        ..Launch::default()
    };
    if let Err(error) = q0editor::launch::run(launch) {
        log::error!("q0editor stopped: {error}");
    }
}

/// Points the environment at the app sandbox.
///
/// `dirs` and `std::env::temp_dir` resolve through these variables. Android
/// sets none of them usefully, which would leave the editor with nowhere to
/// keep settings, recovery sessions or temporary media.
fn adopt_app_storage(files: &Path) {
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
fn choose_multisampling(preferred: u16, is_offered: impl Fn(u16) -> bool) -> u16 {
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
fn egl_offers_multisampling(samples: u16) -> bool {
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
