use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use q0s_format::v2::{
    parse as parse_v2, wire_equivalent, write as write_v2, ProjectV2, Q1S_V2_MAGIC,
};
use q0s_format::{migrate_v1_to_v2, parse_q1s, Q1S_MAGIC};

pub const Q1S_EXTENSION: &str = "q1s";
pub const Q0LANG_EXTENSIONS: &[&str] = &["q0l", "q0lang"];
pub const MAX_Q0LANG_BYTES: u64 = 8 * 1024 * 1024;
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
        let mut project = migrate_v1_to_v2(v1)?;
        q0s_format::v2::assign_missing_instance_ids(&mut project)?;
        Ok(project)
    } else if magic == Q1S_V2_MAGIC {
        let mut project = parse_v2(&bytes)?;
        if version < q0s_format::v2::Q1S_VERSION_RIGGING {
            q0s_format::v2::assign_missing_instance_ids(&mut project)?;
        }
        if version == q0s_format::v2::Q1S_VERSION_AUDIO_CLIP_FX {
            crate::audio::migrate_legacy_audio_placements(&mut project);
            q0s_format::v2::validate(&project)?;
        }
        Ok(project)
    } else {
        Err(FileError::UnknownMagic(magic))
    }
}

pub fn save_project(path: &Path, project: &ProjectV2) -> Result<(), FileError> {
    let bytes = write_v2(project)?;
    // Do not put bytes on disk unless the writer's result can be parsed back
    // as a complete project. The staged file is also read back byte-for-byte
    // by `write_bytes_atomic` before it may replace the current project.
    let parsed = parse_v2(&bytes)?;
    if !wire_equivalent(&parsed, project) {
        return Err(FileError::Format(q0s_format::Error::Validation(
            "q1s parse-back changed the canonical project",
        )));
    }
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

pub fn is_q0lang_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            Q0LANG_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

pub fn load_q0lang_document(path: &Path) -> io::Result<String> {
    let size = fs::metadata(path)?.len();
    if size > MAX_Q0LANG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("q0lang source is too large ({size} bytes; limit is {MAX_Q0LANG_BYTES})"),
        ));
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > MAX_Q0LANG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "q0lang source exceeded the size limit while reading",
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("q0lang source must be utf-8: {error}"),
        )
    })
}

pub fn save_q0lang_document(path: &Path, text: &str) -> io::Result<()> {
    if text.len() as u64 > MAX_Q0LANG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("q0lang source is too large (limit is {MAX_Q0LANG_BYTES} bytes)"),
        ));
    }
    write_bytes_atomic(path, text.as_bytes())
}

pub fn pick_open_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("q0editor documents", &["q1s", "q0l", "q0lang"])
        .add_filter("q0s project (.q1s)", &[Q1S_EXTENSION])
        .add_filter("q0lang source (.q0l, .q0lang)", Q0LANG_EXTENSIONS)
        .add_filter("any", &["*"])
        .pick_file()
}

pub fn pick_q0lang_save_path(suggested: Option<&Path>) -> Option<PathBuf> {
    let default_name = suggested
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("script.q0l");
    let mut dialog = rfd::FileDialog::new()
        .add_filter("q0lang source (.q0l, .q0lang)", Q0LANG_EXTENSIONS)
        .set_file_name(default_name);
    if let Some(parent) = suggested.and_then(Path::parent) {
        dialog = dialog.set_directory(parent);
    }
    dialog.save_file()
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
            &[
                "png", "jpg", "jpeg", "webp", "wav", "mp3", "ogg", "flac", "mp4", "q0v",
            ],
        )
        .add_filter("video", &["mp4", "q0v"])
        .add_filter("audio", &["wav", "mp3", "ogg", "flac"])
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
    fn save_project_accepts_noncanonical_order_but_verifies_all_content() {
        let dir = TestDir::new("canonical-save");
        let path = dir.0.join("project.q1s");
        let mut project = default_project();
        project
            .assets
            .push(q0s_format::v2::Asset::Vector(q0s_format::v2::VectorAsset {
                asset_id: 2,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            }));
        project
            .assets
            .push(q0s_format::v2::Asset::Vector(q0s_format::v2::VectorAsset {
                asset_id: 1,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            }));
        project.q0rgs[0].frame_count = 5;
        project.q0rgs[0].layers[0].placements = vec![
            q0s_format::v2::Placement {
                instance_id: 0,
                frame: 4,
                target: q0s_format::v2::Target::Asset(1),
                transform: q0s_format::v2::Transform2D::IDENTITY,
                tween: q0s_format::v2::Tween::None,
                fx: Default::default(),
            },
            q0s_format::v2::Placement {
                instance_id: 0,
                frame: 0,
                target: q0s_format::v2::Target::Asset(2),
                transform: q0s_format::v2::Transform2D::IDENTITY,
                tween: q0s_format::v2::Tween::None,
                fx: Default::default(),
            },
        ];
        project.q0rgs[0].layers[0].explicit_keyframes = vec![3, 2];

        save_project(&path, &project).expect("save noncanonical project");
        let loaded = load_project(&path).expect("load canonical save");
        assert!(wire_equivalent(&loaded, &project));
        assert_eq!(
            loaded
                .assets
                .iter()
                .map(q0s_format::v2::Asset::id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(loaded.q0rgs[0].layers[0].explicit_keyframes, vec![2, 3]);
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

    #[test]
    fn q0l_and_q0lang_are_the_same_utf8_source_format() {
        let dir = TestDir::new("q0lang-source");
        let q0l = dir
            .0
            .join("\u{441}\u{43a}\u{440}\u{438}\u{43f}\u{442} one.q0l");
        let q0lang = dir
            .0
            .join("\u{441}\u{43a}\u{440}\u{438}\u{43f}\u{442} two.q0lang");
        let source =
            "import q0.math\nname = \"\u{433}\u{435}\u{440}\u{43e}\u{439}\"\nx = sin(pi / 2)\n";

        save_q0lang_document(&q0l, source).expect("save .q0l");
        save_q0lang_document(&q0lang, source).expect("save .q0lang");

        assert!(is_q0lang_path(&q0l));
        assert!(is_q0lang_path(&q0lang));
        assert_eq!(load_q0lang_document(&q0l).expect("load .q0l"), source);
        assert_eq!(load_q0lang_document(&q0lang).expect("load .q0lang"), source);
    }

    #[test]
    fn q0lang_save_atomically_replaces_existing_source() {
        let dir = TestDir::new("q0lang-atomic");
        let path = dir.0.join("logic.q0l");
        fs::write(&path, "old").expect("seed q0lang source");
        save_q0lang_document(&path, "new\nsource\n").expect("replace q0lang source");
        assert_eq!(
            fs::read_to_string(&path).expect("read replacement"),
            "new\nsource\n"
        );
        assert_eq!(fs::read_dir(&dir.0).expect("read source dir").count(), 1);
    }

    #[test]
    fn protected_save_roundtrips_rig_binding_and_stable_identity() {
        let dir = TestDir::new("rig-protected-save");
        let path = dir.0.join("rigged.q1s");
        let mut project = default_project();
        project
            .assets
            .push(q0s_format::v2::Asset::Vector(q0s_format::v2::VectorAsset {
                asset_id: 77,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            }));
        project.q0rgs[0].layers[0]
            .placements
            .push(q0s_format::v2::Placement {
                instance_id: 123,
                frame: 0,
                target: q0s_format::v2::Target::Asset(77),
                transform: q0s_format::v2::Transform2D::IDENTITY,
                tween: q0s_format::v2::Tween::None,
                fx: Default::default(),
            });
        crate::rigging::ensure_rig(&mut project, 1).expect("rig");
        let bone = crate::rigging::add_bone(
            &mut project,
            1,
            None,
            q0s_format::v2::Vec2::new(0.0, 0.0),
            q0s_format::v2::Vec2::new(20.0, 0.0),
            0,
        )
        .expect("bone");
        let selection = crate::state::Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        crate::rigging::bind_selected_placement_to_node(&mut project, &selection, bone, 0)
            .expect("bind");
        crate::rigging::set_node_rotation(&mut project, 1, bone, 6, 0.4, true);
        {
            use q0s_format::v2::{
                Asset, RigControl, RigControlKind, RigMirrorPair, RigPoseBlendMode, RigPoseDriver,
                RigPosePreset, RigPoseValue, RigPropertyRef, RigVariantChoice, RigVariantSet,
                Target,
            };
            let rig = project
                .assets
                .iter_mut()
                .find_map(|asset| match asset {
                    Asset::Rig(rig) if rig.owner_q0rg_id == 1 => Some(rig),
                    _ => None,
                })
                .expect("rig metadata");
            let source = rig
                .controls
                .iter()
                .map(|control| control.control_id)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            let partner = source.saturating_add(1);
            rig.controls.extend([
                RigControl {
                    control_id: source,
                    name: "protected save pose source".into(),
                    kind: RigControlKind::Slider,
                    target_node: None,
                    rest_x: 0.0,
                    rest_y: 0.0,
                    rest_value: 0.0,
                    min_value: 0.0,
                    max_value: 1.0,
                    public_in_simple: true,
                },
                RigControl {
                    control_id: partner,
                    name: "protected save pose target".into(),
                    kind: RigControlKind::Slider,
                    target_node: None,
                    rest_x: 0.0,
                    rest_y: 0.0,
                    rest_value: 0.0,
                    min_value: 0.0,
                    max_value: 1.0,
                    public_in_simple: true,
                },
            ]);
            rig.poses.push(RigPosePreset {
                pose_id: 1,
                name: "protected save pose".into(),
                values: vec![RigPoseValue {
                    property: RigPropertyRef::ControlValue(partner),
                    value: 1.0,
                }],
            });
            rig.pose_drivers.push(RigPoseDriver {
                driver_id: 1,
                source_control: source,
                pose_id: 1,
                source_min: 0.0,
                source_max: 1.0,
                weight_min: 0.0,
                weight_max: 1.0,
                mode: RigPoseBlendMode::Override,
            });
            rig.mirror_pairs.push(RigMirrorPair {
                left: RigPropertyRef::ControlValue(source),
                right: RigPropertyRef::ControlValue(partner),
                multiplier: 1.0,
                offset: 0.0,
            });
            rig.variants.push(RigVariantSet {
                variant_id: 1,
                name: "protected save variant".into(),
                instance_id: 123,
                source_control: source,
                choices: vec![RigVariantChoice {
                    name: "default".into(),
                    target: Target::Asset(77),
                }],
            });
        }

        save_project(&path, &project).expect("protected save rigged project");
        let loaded = load_project(&path).expect("load protected rigged project");
        assert!(wire_equivalent(&loaded, &project));
        assert_eq!(loaded.q0rgs[0].layers[0].placements[0].instance_id, 123);
        let rig = q0s_format::rig::rig_for_q0rg(&loaded, 1).expect("loaded rig");
        assert_eq!(
            rig.nodes[0].binding.expect("loaded binding").instance_id,
            123
        );
        assert!(rig.channels.iter().any(|channel| {
            channel.property == q0s_format::v2::RigPropertyRef::NodeRotation(bone)
        }));
        assert_eq!(rig.pose_drivers.len(), 1);
        assert_eq!(rig.mirror_pairs.len(), 1);
        assert_eq!(rig.variants.len(), 1);
        assert_eq!(rig.variants[0].instance_id, 123);
        assert_eq!(
            rig.variants[0].choices[0].target,
            q0s_format::v2::Target::Asset(77)
        );
        assert_eq!(
            fs::read_dir(&dir.0).expect("read save dir").count(),
            1,
            "protected save must not leak a staging file"
        );
    }
}
