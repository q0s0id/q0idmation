use std::path::{Path, PathBuf};

use super::workspace::unused_path;

/// Where documents live on a device without a user-visible file system.
fn workspace_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join("projects"))
}

/// Stand-in for `rfd::FileDialog`, which has no Android backend.
///
/// It takes the same builder calls so call sites stay platform-neutral.
/// Saving needs no dialog here: the file goes into the workspace under the
/// suggested name. Picking an existing file comes back as a cancelled dialog.
#[derive(Default)]
pub(crate) struct FileDialog {
    file_name: Option<String>,
}

impl FileDialog {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn add_filter(
        self,
        _name: impl Into<String>,
        _extensions: &[impl ToString],
    ) -> Self {
        self
    }

    pub(crate) fn set_directory<P: AsRef<Path>>(self, _path: P) -> Self {
        self
    }

    pub(crate) fn set_file_name(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = Some(file_name.into());
        self
    }

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
        Some(unused_path(&workspace, file_name))
    }
}
