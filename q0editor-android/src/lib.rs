//! Android entry point for q0editor.
//!
//! `MainActivity` loads this library; the `android-activity` glue then calls
//! [`android_main`] on its own native thread.

#![cfg(target_os = "android")]

mod keyboard;

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
        Some(files) => q0android::adopt_app_storage(&files),
        None => log::error!("no app data directory; settings and recovery are unavailable"),
    }
    // GameActivity reports only X and Y unless asked, and the brush wants pressure.
    app.enable_motion_axis(Axis::Pressure);

    let multisampling = q0android::choose_multisampling(
        PREFERRED_MULTISAMPLING,
        q0android::egl_offers_multisampling,
    );
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
