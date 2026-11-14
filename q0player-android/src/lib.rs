//! Android entry point for q0player.
//!
//! `MainActivity` loads this library; the `android-activity` glue then calls
//! [`android_main`] on its own native thread. Movies arrive later, through
//! the activity: picked, opened from other apps or sent by q0editor.

#![cfg(target_os = "android")]

use q0player::ui::launch::{Launch, PREFERRED_MULTISAMPLING};
use winit::platform::android::activity::AndroidApp;
use winit::platform::android::EventLoopBuilderExtAndroid;

#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("q0player")
            .with_max_level(log::LevelFilter::Debug),
    );
    // No shell starts this process, so this is the only place to ask for one.
    std::env::set_var("RUST_BACKTRACE", "1");

    match app.internal_data_path() {
        Some(files) => q0android::adopt_app_storage(&files),
        None => log::error!("no app data directory; settings and recent files are unavailable"),
    }

    let multisampling = q0android::choose_multisampling(
        PREFERRED_MULTISAMPLING,
        q0android::egl_offers_multisampling,
    );
    log::info!("framebuffer multisampling: {multisampling}x");

    // A QIR app carries its movie in its assets: Q0Build put it there.
    let qir = app
        .asset_manager()
        .open(c"qir/app.qir")
        .and_then(|mut asset| {
            use std::io::Read;
            let mut bytes = Vec::new();
            asset.read_to_end(&mut bytes).ok()?;
            match q0qir::Bundle::decode(&bytes, true) {
                Ok(bundle) => Some(bundle),
                Err(error) => {
                    log::error!("the app's bundle cannot be read: {error}");
                    None
                }
            }
        });
    if let Some(bundle) = &qir {
        log::info!(
            "QIR app: {} {}",
            bundle.manifest.app.name,
            bundle.manifest.app.version
        );
    }

    let launch = Launch {
        multisampling,
        event_loop_builder: Some(Box::new(move |builder| {
            builder.with_android_app(app);
        })),
        qir,
        ..Launch::default()
    };
    if let Err(error) = q0player::ui::launch::run(launch) {
        log::error!("q0player stopped: {error}");
    }
}
