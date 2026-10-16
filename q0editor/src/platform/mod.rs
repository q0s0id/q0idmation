//! The seam between the editor and the platform it runs on.

use std::path::{Path, PathBuf};

use crate::app::Action;

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "android", test))]
mod mime;
mod text_input;
#[cfg(any(target_os = "android", test))]
mod workspace;

#[cfg(target_os = "android")]
pub(crate) use android::FileDialog;
#[cfg(not(target_os = "android"))]
pub(crate) use rfd::FileDialog;
pub use text_input::{ImeAction, ImeTranslator, SoftKeyboard};

/// Something the platform asks of the editor between frames.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) enum PlatformEvent {
    /// A picked or received file, already turned into what to do with it.
    Run(Action),
    /// A message for the status line.
    Status(String),
    /// The system Back button.
    Back,
}

/// Lets the person pick a file with `dialog`, then turns it into an action.
///
/// Desktop dialogs answer straight away, and the action comes back here.
/// Android's document picker answers a few frames later, so there the action
/// arrives through [`take_events`] instead.
pub(crate) fn pick_file_then(
    dialog: FileDialog,
    then: impl FnOnce(PathBuf) -> Action + Send + 'static,
) -> Option<Action> {
    #[cfg(target_os = "android")]
    {
        dialog.pick_then(Box::new(then));
        None
    }
    #[cfg(not(target_os = "android"))]
    {
        dialog.pick_file().map(then)
    }
}

/// What the platform asked for since the last frame.
pub(crate) fn take_events(ctx: &egui::Context) -> Vec<PlatformEvent> {
    #[cfg(target_os = "android")]
    {
        android::take_events(ctx)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = ctx;
        Vec::new()
    }
}

/// Reports that a document the person chose a place for was written in full.
/// On Android the copy where they chose to keep it is brought up to date.
pub(crate) fn file_written(path: &Path) {
    #[cfg(target_os = "android")]
    android::file_written(path);
    #[cfg(not(target_os = "android"))]
    let _ = path;
}

/// Reports a finished export. On Android, where the file was written inside
/// the app, it is copied out to where the person chooses.
pub(crate) fn export_finished(path: &Path) {
    #[cfg(target_os = "android")]
    android::export_finished(path);
    #[cfg(not(target_os = "android"))]
    let _ = path;
}

/// Sends the app to the background, for Back with nothing left to close.
/// Desktop windows have no Back button.
pub(crate) fn leave() {
    #[cfg(target_os = "android")]
    android::leave();
}

/// Where fast preview snapshots go when q0player is another app.
#[cfg(target_os = "android")]
pub(crate) fn preview_dir() -> PathBuf {
    android::preview_dir()
}

/// Plays a snapshot from [`preview_dir`] in the q0player app.
#[cfg(target_os = "android")]
pub(crate) fn open_in_player(path: &Path, all_scenes: bool) -> Result<(), String> {
    android::open_in_player(path, all_scenes)
}
