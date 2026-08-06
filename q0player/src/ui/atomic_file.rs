//! Small, dependency-free atomic file writer shared by player preferences
//! and exported themes.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

/// Write a complete file beside its destination, flush it, then atomically
/// replace the destination. A same-directory staging file keeps the final
/// operation on one filesystem.
pub(crate) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
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

    if let Err(error) = replace_staged_file(&staging_path, path) {
        let _ = fs::remove_file(&staging_path);
        return Err(error);
    }
    Ok(())
}

fn create_staging_file(path: &Path) -> io::Result<(PathBuf, File)> {
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
fn replace_staged_file(staging_path: &Path, path: &Path) -> io::Result<()> {
    fs::rename(staging_path, path)
}

#[cfg(windows)]
fn replace_staged_file(staging_path: &Path, path: &Path) -> io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_file_without_leaving_staging_files() {
        let directory = std::env::temp_dir().join(format!(
            "q0player-atomic-{}-{}",
            std::process::id(),
            NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("settings.json");
        fs::write(&path, b"old").expect("write original");

        write(&path, b"complete replacement").expect("replace atomically");

        assert_eq!(
            fs::read(&path).expect("read replacement"),
            b"complete replacement"
        );
        let entries = fs::read_dir(&directory)
            .expect("read test directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect test directory");
        assert_eq!(entries.len(), 1);
        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
