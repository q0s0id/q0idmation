//! Shared system font catalogue, discovered off the UI thread.
pub(crate) use q0fla::fonts::InstalledFont;
use std::sync::{Arc, OnceLock};

pub(crate) fn installed() -> Option<&'static [InstalledFont]> {
    static READY: OnceLock<Arc<OnceLock<()>>> = OnceLock::new();
    READY
        .get_or_init(|| {
            let ready = Arc::new(OnceLock::new());
            let worker = ready.clone();
            std::thread::spawn(move || {
                q0fla::fonts::installed();
                let _ = worker.set(());
            });
            ready
        })
        .get()?;
    Some(q0fla::fonts::installed())
}

pub(crate) fn find(name: &str) -> Option<&'static InstalledFont> {
    installed()?;
    q0fla::fonts::find(name)
}
