//! A build profile: the movie, the app's manifest, the targets and where
//! the results go. Saved beside the movie as `<movie>.q0build`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub movie: Option<PathBuf>,
    /// PNG for the window, the exe, the Linux menu and the launcher.
    pub icon: Option<PathBuf>,
    /// Re-save the movie packed before embedding it: smaller apps, and old
    /// unpacked files gain nothing from staying unpacked inside one.
    pub repack: bool,
    pub manifest: q0qir::Manifest,
    pub targets: Targets,
    pub windows: WindowsOptions,
    pub linux: LinuxOptions,
    pub android: AndroidOptions,
    pub output: OutputOptions,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            movie: None,
            icon: None,
            repack: true,
            manifest: q0qir::Manifest::default(),
            targets: Targets::default(),
            windows: WindowsOptions::default(),
            linux: LinuxOptions::default(),
            android: AndroidOptions::default(),
            output: OutputOptions::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Targets {
    pub windows_x64: bool,
    pub windows_arm64: bool,
    pub linux_appimage: bool,
    pub android_apk: bool,
}

impl Default for Targets {
    fn default() -> Self {
        Self {
            windows_x64: true,
            windows_arm64: false,
            linux_appimage: false,
            android_apk: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsOptions {
    /// Write the icon and the name, version and author into the exe, so
    /// Explorer, the taskbar and Task Manager show the app, not q0player.
    pub file_info: bool,
}

impl Default for WindowsOptions {
    fn default() -> Self {
        Self { file_info: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LinuxOptions {
    /// The freedesktop menu category the app is listed under.
    pub category: String,
}

impl Default for LinuxOptions {
    fn default() -> Self {
        Self {
            category: "Game".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AndroidOptions {
    /// Leave q0player's "Open with" entries out, so the app is not offered
    /// for every .q0s file on the phone.
    pub drop_open_with: bool,
}

impl Default for AndroidOptions {
    fn default() -> Self {
        Self {
            drop_open_with: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputOptions {
    /// Empty puts builds in a `build` folder beside the movie.
    pub folder: Option<PathBuf>,
    /// `{name}`, `{version}`, `{target}` and `{date}` are filled in.
    pub pattern: String,
    /// A .zip beside each build, ready to upload.
    pub zip: bool,
    /// SHA256SUMS.txt for people who check their downloads.
    pub checksums: bool,
    /// Read every result back and check its bundle before calling it done.
    pub verify: bool,
    pub open_folder: bool,
    /// Keep the build log as build-log.txt with the results.
    pub keep_log: bool,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            folder: None,
            pattern: "{name}-{version}-{target}".into(),
            zip: false,
            checksums: true,
            verify: true,
            open_folder: true,
            keep_log: true,
        }
    }
}

impl Profile {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Where builds go.
    pub fn output_folder(&self) -> Option<PathBuf> {
        self.output.folder.clone().or_else(|| {
            self.movie
                .as_ref()
                .and_then(|movie| movie.parent())
                .map(|folder| folder.join("build"))
        })
    }

    /// The profile file for a movie: `grunt.q1s` keeps `grunt.q0build`.
    pub fn path_for_movie(movie: &Path) -> PathBuf {
        movie.with_extension("q0build")
    }

    /// A file name for one target's build, from the pattern.
    pub fn file_stem(&self, target: &str, date: &str) -> String {
        let app = &self.manifest.app;
        let stem = self
            .output
            .pattern
            .replace("{name}", &app.name)
            .replace("{version}", &app.version)
            .replace("{target}", target)
            .replace("{date}", date);
        q0qir::file_safe(&stem.replace(' ', "-"))
    }
}

/// A reverse-domain app id from what the person typed as the name and the
/// author, for the App page's Suggest button and new profiles.
pub fn default_app_id(name: &str, author: &str) -> String {
    let author = if author.trim().is_empty() {
        "q0"
    } else {
        author
    };
    format!("com.{}.{}", id_part(author), id_part(name))
}

/// One dot-separated part of an app id made from free text. It must come
/// out as lowercase ASCII letters, digits or `_`, starting with a letter:
/// Android refuses anything else as a package name.
fn id_part(text: &str) -> String {
    // TODO(human): text that is not plain ASCII vanishes here, so a Russian
    // name like "Грант 2" becomes just "q2". Decide how such names turn into
    // id letters (the result must still pass q0qir::is_app_id).
    let mut word: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if word.is_empty() || word.starts_with(|c: char| c.is_ascii_digit()) {
        word.insert(0, 'q');
    }
    word
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_ids_are_valid_app_ids() {
        for (name, author) in [
            ("Bullet Box", "q0s"),
            ("Грант 2", "ShAdOwMoL 301"),
            ("", ""),
            ("2nd Strike", "Walibite"),
        ] {
            let id = default_app_id(name, author);
            assert!(q0qir::is_app_id(&id), "{name:?} by {author:?} gave {id:?}");
        }
    }

    #[test]
    fn file_names_follow_the_pattern() {
        let mut profile = Profile::default();
        profile.manifest.app.name = "Bullet Box".into();
        profile.manifest.app.version = "1.2.0".into();
        assert_eq!(
            profile.file_stem("windows-x64", "2026-11-11"),
            "Bullet-Box-1.2.0-windows-x64"
        );
        profile.output.pattern = "{name}_{date}".into();
        assert_eq!(
            profile.file_stem("x", "2026-11-11"),
            "Bullet-Box_2026-11-11"
        );
    }
}
