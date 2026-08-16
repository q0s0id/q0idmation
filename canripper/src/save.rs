use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use q0s_format::v2;

use crate::document::{Document, DocumentData, FormatKind};

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

pub fn can_save(document: &Document) -> bool {
    matches!(
        document.data,
        DocumentData::QProject(_) | DocumentData::Q1Legacy(_) | DocumentData::Q0Legacy(_)
    )
}

pub fn can_export_q1s(document: &Document) -> bool {
    matches!(
        document.data,
        DocumentData::QProject(_) | DocumentData::Q1Legacy(_)
    )
}

pub fn save_document(document: &Document, path: &Path) -> Result<Vec<u8>, String> {
    let bytes = serialize_for_native_format(document)?;
    write_bytes_atomic(path, &bytes)
        .map_err(|error| format!("save {}: {error}", path.display()))?;
    Ok(bytes)
}

pub fn export_q1s(document: &Document, path: &Path) -> Result<Vec<u8>, String> {
    let bytes = match &document.data {
        DocumentData::QProject(project) => serialize_q1s_v2(project)?,
        DocumentData::Q1Legacy(project) => {
            let bytes = q0s_format::write_q1s(project)
                .map_err(|error| format!("serialize legacy q1s: {error}"))?;
            let parsed = q0s_format::parse_q1s(&bytes)
                .map_err(|error| format!("legacy q1s parse-back: {error}"))?;
            if &parsed != project {
                return Err("legacy q1s parse-back changed the project".to_string());
            }
            bytes
        }
        _ => {
            return Err(
                "this document cannot be exported to q1s without a lossy conversion".to_string(),
            )
        }
    };
    write_bytes_atomic(path, &bytes)
        .map_err(|error| format!("export q1s {}: {error}", path.display()))?;
    Ok(bytes)
}

fn serialize_for_native_format(document: &Document) -> Result<Vec<u8>, String> {
    match (&document.data, document.format) {
        (DocumentData::QProject(project), FormatKind::Q1s) => serialize_q1s_v2(project),
        (DocumentData::QProject(project), FormatKind::Q0s) => serialize_q0s_v2(project),
        (DocumentData::Q1Legacy(project), FormatKind::Q1s) => {
            let bytes = q0s_format::write_q1s(project)
                .map_err(|error| format!("serialize legacy q1s: {error}"))?;
            let parsed = q0s_format::parse_q1s(&bytes)
                .map_err(|error| format!("legacy q1s parse-back: {error}"))?;
            if &parsed != project {
                return Err("legacy q1s parse-back changed the project".to_string());
            }
            Ok(bytes)
        }
        (DocumentData::Q0Legacy(movie), FormatKind::Q0s) => {
            let bytes = q0s_format::write_q0s(movie)
                .map_err(|error| format!("serialize legacy q0s: {error}"))?;
            let parsed = q0s_format::parse_q0s(&bytes)
                .map_err(|error| format!("legacy q0s parse-back: {error}"))?;
            if &parsed != movie {
                return Err("legacy q0s parse-back changed the movie".to_string());
            }
            Ok(bytes)
        }
        _ => Err("document format is read-only in canripper".to_string()),
    }
}

fn serialize_q1s_v2(project: &v2::ProjectV2) -> Result<Vec<u8>, String> {
    let bytes = v2::write(project).map_err(|error| format!("serialize q1s: {error}"))?;
    let parsed = v2::parse(&bytes).map_err(|error| format!("q1s parse-back: {error}"))?;
    if !v2::wire_equivalent(&parsed, project) {
        return Err("q1s parse-back changed the canonical project".to_string());
    }
    Ok(bytes)
}

fn serialize_q0s_v2(project: &v2::ProjectV2) -> Result<Vec<u8>, String> {
    let bytes =
        q0s_format::write_q0s_v2(project).map_err(|error| format!("serialize q0s: {error}"))?;
    let parsed =
        q0s_format::parse_q0s_v2(&bytes).map_err(|error| format!("q0s parse-back: {error}"))?;
    if !v2::wire_equivalent(&parsed, project) {
        return Err("q0s parse-back changed the canonical project".to_string());
    }
    Ok(bytes)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
    fs::create_dir_all(parent)?;

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
    use crate::document::Document;
    use q0s_format::v2::{self, Asset};
    use std::collections::HashMap;

    fn project() -> v2::ProjectV2 {
        v2::ProjectV2 {
            meta: v2::ProjectMeta {
                name: "save test".into(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(v2::VectorAsset {
                asset_id: 1,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            })],
            asset_names: HashMap::new(),
            asset_appearances: HashMap::new(),
            layer_metadata: HashMap::new(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![v2::Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![v2::Layer {
                    layer_id: 1,
                    name: "Layer".into(),
                    explicit_keyframes: vec![0],
                    placements: Vec::new(),
                }],
            }],
        }
    }

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("canripper-save-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn q0s_save_and_q1s_export_both_roundtrip_without_staging_leaks() {
        let dir = TestDir::new();
        let q0s = dir.0.join("movie.q0s");
        let q1s = dir.0.join("movie.q1s");
        let source = q0s_format::write_q0s_v2(&project()).unwrap();
        let document = Document::from_bytes(q0s.clone(), source).unwrap();
        save_document(&document, &q0s).unwrap();
        export_q1s(&document, &q1s).unwrap();
        let saved = q0s_format::parse_q0s_v2(&fs::read(&q0s).unwrap()).unwrap();
        let exported = v2::parse(&fs::read(&q1s).unwrap()).unwrap();
        let DocumentData::QProject(original) = &document.data else {
            panic!()
        };
        assert!(v2::wire_equivalent(&saved, original));
        assert!(v2::wire_equivalent(&exported, original));
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
    }
}
