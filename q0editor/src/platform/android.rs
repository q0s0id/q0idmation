use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, PoisonError};

use super::mime::{mime_type, mime_types};
use super::workspace::unused_path;
use super::PlatformEvent;
use crate::app::Action;

/// What to do with a picked file, once the picker answers.
type Then = Box<dyn FnOnce(PathBuf) -> Action + Send>;

/// The pick still waiting for an answer, with its tag. The system picker
/// covers the whole app, so there is never more than one.
static WAITING: Mutex<Option<(i32, Then)>> = Mutex::new(None);
/// Tags start above [`q0android::EXTERNAL`] and the preview tags below it.
static NEXT_TAG: AtomicI32 = AtomicI32::new(1);

/// Where documents live on a device without a user-visible file system.
fn workspace_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join("projects"))
}

/// Stand-in for `rfd::FileDialog`, which has no Android backend.
///
/// It takes the same builder calls so call sites stay platform-neutral.
/// Picking goes through the system document picker, which answers in a later
/// frame: see [`super::pick_file_then`]. Saving writes into the app's own
/// workspace and asks where the file should live outside it, where the
/// activity then keeps a copy up to date.
#[derive(Default)]
pub(crate) struct FileDialog {
    file_name: Option<String>,
    extensions: Vec<String>,
}

impl FileDialog {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn add_filter(
        mut self,
        _name: impl Into<String>,
        extensions: &[impl ToString],
    ) -> Self {
        self.extensions
            .extend(extensions.iter().map(ToString::to_string));
        self
    }

    pub(crate) fn set_directory<P: AsRef<Path>>(self, _path: P) -> Self {
        self
    }

    pub(crate) fn set_file_name(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = Some(file_name.into());
        self
    }

    /// Nothing can be picked synchronously here; [`super::pick_file_then`]
    /// is the way to pick on every platform.
    pub(crate) fn pick_file(self) -> Option<PathBuf> {
        None
    }

    pub(crate) fn pick_folder(self) -> Option<PathBuf> {
        None
    }

    pub(crate) fn save_file(self) -> Option<PathBuf> {
        let workspace = workspace_dir()?;
        std::fs::create_dir_all(&workspace).ok()?;
        let file_name = self.file_name.as_deref().unwrap_or("untitled");
        let path = unused_path(&workspace, file_name);
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        let mime_type = mime_type(extension).unwrap_or("application/octet-stream");
        if let Err(error) = q0android::create_document(&path, mime_type) {
            eprintln!("could not ask where to keep {}: {error}", path.display());
        }
        Some(path)
    }

    pub(super) fn pick_then(self, then: Then) {
        let tag = NEXT_TAG.fetch_add(1, Ordering::Relaxed);
        *WAITING.lock().unwrap_or_else(PoisonError::into_inner) = Some((tag, then));
        if let Err(error) = q0android::pick_document(tag, &mime_types(&self.extensions)) {
            eprintln!("could not show the document picker: {error}");
            WAITING
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
        }
    }
}

pub(super) fn take_events(ctx: &egui::Context) -> Vec<PlatformEvent> {
    q0android::set_waker(ctx);
    let mut events = Vec::new();
    for event in q0android::take_events() {
        match event {
            q0android::Event::Opened { tag, path } => {
                if let Some(then) = answered(tag) {
                    events.push(PlatformEvent::Run(then(path)));
                } else if tag <= q0android::EXTERNAL {
                    events.push(PlatformEvent::Run(Action::OpenProjectFromPath(path)));
                }
            }
            q0android::Event::Failed { tag, message } => {
                answered(tag);
                if !message.is_empty() {
                    events.push(PlatformEvent::Status(message));
                }
            }
            q0android::Event::Status(message) => events.push(PlatformEvent::Status(message)),
            q0android::Event::Back => events.push(PlatformEvent::Back),
        }
    }
    events
}

/// Takes the pick waiting for `tag`'s answer, if any is.
fn answered(tag: i32) -> Option<Then> {
    let mut waiting = WAITING.lock().unwrap_or_else(PoisonError::into_inner);
    if waiting.as_ref().is_some_and(|(waiting, _)| *waiting == tag) {
        waiting.take().map(|(_, then)| then)
    } else {
        None
    }
}

pub(super) fn file_written(path: &Path) {
    if let Err(error) = q0android::document_written(path) {
        eprintln!("could not pass {} on: {error}", path.display());
    }
}

pub(super) fn exports_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join("exports"))
}

pub(super) fn export_finished(path: &Path, folder: bool) {
    let result = if folder {
        q0android::export_folder_finished(path)
    } else {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        let mime_type = mime_type(extension).unwrap_or("application/octet-stream");
        q0android::export_finished(path, mime_type)
    };
    if let Err(error) = result {
        eprintln!("could not pass {} on: {error}", path.display());
    }
}

/// The device's own H.264 encoder, shaped like the Media Foundation one q0enc
/// uses on Windows.
pub(crate) struct Mp4Encoder(q0android::Mp4Encoder);

impl Mp4Encoder {
    pub(crate) fn create(path: &Path, spec: q0video::mp4::Mp4Spec) -> Result<Self, String> {
        spec.validate()?;
        q0android::Mp4Encoder::create(path, spec.width, spec.height, spec.fps, spec.bitrate)
            .map(Self)
    }

    pub(crate) fn write_nv12_frame(&mut self, nv12: &[u8]) -> Result<(), String> {
        self.0.write_nv12_frame(nv12)
    }

    pub(crate) fn finish(self) -> Result<(), String> {
        self.0.finish()
    }
}

pub(super) fn leave() {
    q0android::leave();
}

/// The folder q0player can be handed files from.
pub(super) fn preview_dir() -> PathBuf {
    std::env::temp_dir().join("preview")
}

pub(super) fn open_in_player(path: &Path, all_scenes: bool) -> Result<(), String> {
    let tag = if all_scenes {
        q0android::PREVIEW_ALL_SCENES
    } else {
        q0android::PREVIEW_SCENE
    };
    q0android::open_in_player(path, tag)
}
