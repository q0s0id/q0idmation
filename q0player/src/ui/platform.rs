//! The seam between the player and the platform it runs on.
//!
//! On the desktop, file dialogs answer straight away. On Android the system
//! document picker answers in a later frame, and other apps can hand movies
//! over at any time, so both arrive through [`take_events`].

use std::path::PathBuf;

use super::app::PreviewOptions;

/// Running on a touch screen without a desktop window around it.
pub(crate) const TOUCH: bool = cfg!(target_os = "android");

/// Something the platform asks of the player between frames.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) enum PlayerEvent {
    /// A movie to play, with the preview settings q0editor asked for.
    Open {
        path: PathBuf,
        preview: Option<PreviewOptions>,
    },
    /// A picked .q7s theme.
    Theme(PathBuf),
    /// A message for the status line.
    Status(String),
    /// The system Back button.
    Back,
}

#[cfg(not(target_os = "android"))]
mod imp {
    use std::path::{Path, PathBuf};

    use super::PlayerEvent;

    pub(crate) fn pick_movie() -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter("q0player media", &["q0s", "q0v"])
            .pick_file()
    }

    pub(crate) fn pick_theme() -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter("q7s theme", &["q7s"])
            .pick_file()
    }

    pub(crate) fn theme_save_path() -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter("q7s theme", &["q7s"])
            .set_file_name("my-theme.q7s")
            .save_file()
    }

    pub(crate) fn file_written(_path: &Path) {}

    pub(crate) fn take_events(_ctx: &egui::Context) -> Vec<PlayerEvent> {
        Vec::new()
    }

    pub(crate) fn leave() {}

    pub(crate) fn keep_screen_on(_on: bool) {}

    pub(crate) fn immersive(_on: bool) {}
}

#[cfg(target_os = "android")]
mod imp {
    use std::path::{Path, PathBuf};

    use super::super::app::PreviewOptions;
    use super::PlayerEvent;

    const PICK_MOVIE: i32 = 1;
    const PICK_THEME: i32 = 2;

    fn pick(tag: i32) {
        // .q0s, .q0v and .q7s have no MIME types, so the picker shows everything.
        if let Err(error) = q0android::pick_document(tag, &[]) {
            log::error!("could not show the document picker: {error}");
        }
    }

    pub(crate) fn pick_movie() -> Option<PathBuf> {
        pick(PICK_MOVIE);
        None
    }

    pub(crate) fn pick_theme() -> Option<PathBuf> {
        pick(PICK_THEME);
        None
    }

    /// Writes into the app's own files and asks where the theme should live
    /// outside them; the activity copies it there once it is written.
    pub(crate) fn theme_save_path() -> Option<PathBuf> {
        let folder = dirs::home_dir()?.join("themes");
        std::fs::create_dir_all(&folder).ok()?;
        let path = folder.join("my-theme.q7s");
        if let Err(error) = q0android::create_document(&path, "application/octet-stream") {
            log::error!("could not ask where to keep the theme: {error}");
        }
        Some(path)
    }

    pub(crate) fn file_written(path: &Path) {
        if let Err(error) = q0android::document_written(path) {
            log::error!("could not pass {} on: {error}", path.display());
        }
    }

    pub(crate) fn take_events(ctx: &egui::Context) -> Vec<PlayerEvent> {
        q0android::set_waker(ctx);
        q0android::take_events()
            .into_iter()
            .filter_map(|event| match event {
                q0android::Event::Opened { tag, path } => Some(match tag {
                    PICK_THEME => PlayerEvent::Theme(path),
                    q0android::PREVIEW_SCENE | q0android::PREVIEW_ALL_SCENES => PlayerEvent::Open {
                        path,
                        preview: Some(PreviewOptions {
                            enabled: true,
                            all_scenes: tag == q0android::PREVIEW_ALL_SCENES,
                            delete_after_load: false,
                        }),
                    },
                    _ => PlayerEvent::Open {
                        path,
                        preview: None,
                    },
                }),
                q0android::Event::Failed { message, .. } => {
                    (!message.is_empty()).then_some(PlayerEvent::Status(message))
                }
                q0android::Event::Status(message) => Some(PlayerEvent::Status(message)),
                q0android::Event::Back => Some(PlayerEvent::Back),
            })
            .collect()
    }

    pub(crate) fn leave() {
        q0android::leave();
    }

    pub(crate) fn keep_screen_on(on: bool) {
        q0android::keep_screen_on(on);
    }

    pub(crate) fn immersive(on: bool) {
        q0android::immersive(on);
    }
}

pub(crate) use imp::{
    file_written, immersive, keep_screen_on, leave, pick_movie, pick_theme, take_events,
    theme_save_path,
};
