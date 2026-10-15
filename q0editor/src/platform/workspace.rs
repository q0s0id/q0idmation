//! Naming for documents kept in an app-owned folder instead of wherever a
//! save dialog would have pointed.

use std::path::{Path, PathBuf};

/// `file_name` inside `dir`, numbered if that name is taken. With no dialog to
/// confirm an overwrite, an existing document is never replaced.
pub(crate) fn unused_path(dir: &Path, file_name: &str) -> PathBuf {
    // Only the final component counts, so a suggestion can never leave `dir`.
    let name = Path::new(file_name)
        .file_name()
        .map_or(Path::new("untitled"), Path::new);
    let stem = name
        .file_stem()
        .unwrap_or(name.as_os_str())
        .to_string_lossy();
    let extension = name
        .extension()
        .map(|extension| extension.to_string_lossy());

    let mut candidate = dir.join(name);
    let mut number = 2_u32;
    while candidate.exists() {
        candidate = dir.join(match &extension {
            Some(extension) => format!("{stem} {number}.{extension}"),
            None => format!("{stem} {number}"),
        });
        number += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("q0editor-workspace-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn a_free_name_is_used_as_suggested() {
        let dir = scratch_dir("free");
        assert_eq!(unused_path(&dir, "walk.q1s"), dir.join("walk.q1s"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_taken_name_is_numbered_instead_of_replaced() {
        let dir = scratch_dir("taken");
        std::fs::write(dir.join("walk.q1s"), b"first").expect("write first");
        assert_eq!(unused_path(&dir, "walk.q1s"), dir.join("walk 2.q1s"));
        std::fs::write(dir.join("walk 2.q1s"), b"second").expect("write second");
        assert_eq!(unused_path(&dir, "walk.q1s"), dir.join("walk 3.q1s"));

        std::fs::write(dir.join("notes"), b"plain").expect("write plain");
        assert_eq!(unused_path(&dir, "notes"), dir.join("notes 2"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_suggestion_cannot_leave_the_folder() {
        let dir = scratch_dir("contained");
        assert_eq!(
            unused_path(&dir, "../elsewhere/walk.q1s"),
            dir.join("walk.q1s")
        );
        assert_eq!(unused_path(&dir, ".."), dir.join("untitled"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
