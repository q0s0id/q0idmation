use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use q0s_format::v2::{parse as parse_v2, write as write_v2, ProjectV2, Q1S_V2_MAGIC};
use q0s_format::{migrate_v1_to_v2, parse_q1s, Q1S_MAGIC};

pub const Q1S_EXTENSION: &str = "q1s";
/// Refuse unexpectedly large projects before allocating a matching buffer.
/// Normal beta projects are far smaller; 256 MiB still leaves ample room for
/// embedded raster assets while preventing accidental multi-gigabyte loads.
pub const MAX_PROJECT_BYTES: u64 = 256 * 1024 * 1024;

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum FileError {
    Io(std::io::Error),
    Format(q0s_format::Error),
    UnknownMagic([u8; 4]),
    Empty,
    TooLarge { size: u64, max: u64 },
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::Io(e) => write!(f, "io error: {e}"),
            FileError::Format(e) => write!(f, "format error: {e}"),
            FileError::UnknownMagic(m) => write!(f, "unknown file magic: {m:?}"),
            FileError::Empty => write!(f, "file is empty"),
            FileError::TooLarge { size, max } => {
                write!(f, "file is too large ({size} bytes; limit is {max})")
            }
        }
    }
}

impl From<std::io::Error> for FileError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<q0s_format::Error> for FileError {
    fn from(value: q0s_format::Error) -> Self {
        Self::Format(value)
    }
}

pub fn load_project(path: &Path) -> Result<ProjectV2, FileError> {
    let size = fs::metadata(path)?.len();
    if size > MAX_PROJECT_BYTES {
        return Err(FileError::TooLarge {
            size,
            max: MAX_PROJECT_BYTES,
        });
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > MAX_PROJECT_BYTES {
        return Err(FileError::TooLarge {
            size: bytes.len() as u64,
            max: MAX_PROJECT_BYTES,
        });
    }
    if bytes.len() < 6 {
        return Err(FileError::Empty);
    }
    let mut magic = [0_u8; 4];
    magic.copy_from_slice(&bytes[..4]);
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);

    if magic == Q1S_MAGIC && version == 1 {
        let v1 = parse_q1s(&bytes)?;
        Ok(migrate_v1_to_v2(v1)?)
    } else if magic == Q1S_V2_MAGIC {
        Ok(parse_v2(&bytes)?)
    } else {
        Err(FileError::UnknownMagic(magic))
    }
}

pub fn save_project(path: &Path, project: &ProjectV2) -> Result<(), FileError> {
    let bytes = write_v2(project)?;
    // Do not put bytes on disk unless the writer's result can be parsed back
    // as a complete project. The staged file is also read back byte-for-byte
    // by `write_bytes_atomic` before it may replace the current project.
    parse_v2(&bytes)?;
    write_bytes_atomic(path, &bytes)?;
    Ok(())
}

/// Write a complete file beside its destination, then replace the destination.
///
/// Keeping the staging file in the same directory makes the final rename stay
/// on one filesystem. On Windows, `ReplaceFileW` provides replace-existing
/// semantics that `std::fs::rename` does not.
pub(crate) fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_bytes_atomic_with(path, bytes, replace_staged_file)
}

fn write_bytes_atomic_with<F>(path: &Path, bytes: &[u8], replace: F) -> io::Result<()>
where
    F: FnOnce(&Path, &Path) -> io::Result<()>,
{
    let (staging_path, mut staging_file) = create_staging_file(path)?;

    let stage_result = (|| {
        staging_file.write_all(bytes)?;
        staging_file.flush()?;
        staging_file.sync_all()?;

        if let Ok(metadata) = fs::metadata(path) {
            fs::set_permissions(&staging_path, metadata.permissions())?;
        }

        Ok(())
    })();
    drop(staging_file);

    if let Err(error) = stage_result {
        let _ = fs::remove_file(&staging_path);
        return Err(error);
    }

    let staged_bytes = match fs::read(&staging_path) {
        Ok(staged_bytes) => staged_bytes,
        Err(error) => {
            let _ = fs::remove_file(&staging_path);
            return Err(error);
        }
    };
    if staged_bytes != bytes {
        let _ = fs::remove_file(&staging_path);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "staged file did not match the complete serialized output",
        ));
    }

    if let Err(error) = replace(&staging_path, path) {
        let _ = fs::remove_file(&staging_path);
        return Err(error);
    }

    Ok(())
}

pub(crate) fn create_staging_file(path: &Path) -> io::Result<(PathBuf, File)> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "save path must include a file name",
        )
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    for _ in 0..128 {
        let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let mut staging_name = OsString::from(".");
        staging_name.push(file_name);
        staging_name.push(format!(".{}.{}.tmp", std::process::id(), id));
        let staging_path = parent.join(staging_name);

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging_path)
        {
            Ok(file) => return Ok((staging_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique staging file",
    ))
}

#[cfg(not(windows))]
pub(crate) fn replace_staged_file(staging_path: &Path, path: &Path) -> io::Result<()> {
    fs::rename(staging_path, path)
}

#[cfg(windows)]
pub(crate) fn replace_staged_file(staging_path: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            replace_flags: u32,
            exclude: *mut core::ffi::c_void,
            reserved: *mut core::ffi::c_void,
        ) -> i32;
    }

    match fs::symlink_metadata(path) {
        Ok(_) => {
            let replaced: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let replacement: Vec<u16> = staging_path
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect();
            let result = unsafe {
                ReplaceFileW(
                    replaced.as_ptr(),
                    replacement.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if result == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::rename(staging_path, path),
        Err(error) => Err(error),
    }
}

pub fn pick_open_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("q0s project (.q1s)", &[Q1S_EXTENSION])
        .add_filter("any", &["*"])
        .pick_file()
}

pub fn pick_bitmap_import_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("bitmap image", &["png", "jpg", "jpeg", "webp"])
        .pick_file()
}

pub fn pick_media_import_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(
            "q0editor media",
            &["png", "jpg", "jpeg", "webp", "mp4", "q0v"],
        )
        .add_filter("video", &["mp4", "q0v"])
        .add_filter("bitmap image", &["png", "jpg", "jpeg", "webp"])
        .pick_file()
}

pub fn is_supported_video_import_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("mp4") || extension.eq_ignore_ascii_case("q0v")
        })
}

pub fn pick_save_path(suggested: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .add_filter("q0s project (.q1s)", &[Q1S_EXTENSION])
        .set_file_name("untitled.q1s");
    if let Some(p) = suggested {
        if let Some(parent) = p.parent() {
            dialog = dialog.set_directory(parent);
        }
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            dialog = dialog.set_file_name(name);
        }
    }
    dialog.save_file()
}

/// Save dialog for the player-format export (`.q0s`). Defaults the file
/// name to whatever the project's currently saved as, with the extension
/// swapped — keeps "open editor on `foo.q1s` → export to `foo.q0s`" a one
/// click flow.
pub fn pick_export_q0s_path(suggested: Option<&Path>) -> Option<PathBuf> {
    let default_name = suggested
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(|s| format!("{s}.q0s"))
        .unwrap_or_else(|| "movie.q0s".to_string());
    let mut dialog = rfd::FileDialog::new()
        .add_filter("q0s movie (.q0s)", &["q0s"])
        .set_file_name(&default_name);
    if let Some(parent) = suggested.and_then(|p| p.parent()) {
        dialog = dialog.set_directory(parent);
    }
    dialog.save_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::default_project;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("q0editor-{label}-{}-{id}", std::process::id()));
            fs::create_dir(&path).expect("create isolated test directory");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn save_project_replaces_an_existing_file_without_staging_leaks() {
        let dir = TestDir::new("atomic-save-success");
        let path = dir.0.join("project.q1s");
        fs::write(&path, b"old incomplete project").expect("seed existing file");

        let mut project = default_project();
        project.meta.name = "atomic replacement".to_string();
        project
            .assets
            .push(q0s_format::v2::Asset::Vector(q0s_format::v2::VectorAsset {
                asset_id: 77,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            }));
        project
            .asset_names
            .insert(77, "saved vector / герой".to_string());
        save_project(&path, &project).expect("replace existing project");

        assert_eq!(load_project(&path).expect("load replacement"), project);
        assert_eq!(
            fs::read_dir(&dir.0).expect("read test directory").count(),
            1,
            "the destination should be the only file left"
        );
    }

    #[test]
    fn replace_failure_preserves_destination_and_cleans_staging_file() {
        let dir = TestDir::new("atomic-save-failure");
        let path = dir.0.join("project.q1s");
        fs::write(&path, b"original project").expect("seed existing file");

        let error = write_bytes_atomic_with(&path, b"replacement project", |staging, target| {
            assert_eq!(
                fs::read(staging).expect("read staged file"),
                b"replacement project"
            );
            assert_eq!(
                fs::read(target).expect("read original file"),
                b"original project"
            );
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected replace failure",
            ))
        })
        .expect_err("replacement should fail");

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            fs::read(&path).expect("read preserved file"),
            b"original project"
        );
        assert_eq!(
            fs::read_dir(&dir.0).expect("read test directory").count(),
            1,
            "failed staging file should be removed"
        );
    }

    #[test]
    fn oversized_project_is_rejected_before_reading_it() {
        let dir = TestDir::new("oversized-load");
        let path = dir.0.join("huge.q1s");
        let file = File::create(&path).expect("create sparse project");
        file.set_len(MAX_PROJECT_BYTES + 1)
            .expect("grow sparse project");

        assert!(matches!(
            load_project(&path),
            Err(FileError::TooLarge { .. })
        ));
    }
}
