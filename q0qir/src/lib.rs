//! QIR: a q0s movie packed with q0player into an app of its own, the way a
//! Flash projector carried its SWF.
//!
//! A bundle is the movie, the app's manifest and its icon, followed by a
//! fixed trailer. The trailer comes last so a runtime finds its bundle from
//! the end of its own file: appended to `q0player.exe`, to the player inside
//! an AppImage, or alone as `app.qir` in an APK's assets.
//!
//! ```text
//! [manifest JSON][icon PNG][movie][trailer: 40 bytes]
//! trailer = MAGIC(8) payload_len(u64) manifest_len(u32) icon_len(u32)
//!           movie_len(u64) fnv1a64(payload)(u64), little-endian
//! ```

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Ends every bundle. The last byte is the layout version.
pub const MAGIC: [u8; 8] = *b"Q0QIR\0\0\x01";
pub const TRAILER_LEN: usize = 40;
/// Manifest format this crate writes; readers accept this and older.
pub const FORMAT: u32 = 1;
/// The file name of a bundle inside an APK's assets.
pub const ANDROID_ASSET: &str = "qir/app.qir";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub format: u32,
    pub app: AppInfo,
    pub window: WindowSettings,
    pub render: RenderSettings,
    pub behaviour: Behaviour,
    pub data: DataSettings,
    pub diagnostics: Diagnostics,
    pub splash: Splash,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            format: FORMAT,
            app: AppInfo::default(),
            window: WindowSettings::default(),
            render: RenderSettings::default(),
            behaviour: Behaviour::default(),
            data: DataSettings::default(),
            diagnostics: Diagnostics::default(),
            splash: Splash::default(),
        }
    }
}

/// Who the app is. Shown in the title bar, the Windows file properties,
/// the Linux menu and the Android launcher, and readable from q0lang as
/// `app.name`, `app.version` and so on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppInfo {
    pub name: String,
    /// Reverse-domain id: `com.author.game`. The Android package name and
    /// the default save folder.
    pub id: String,
    /// `major.minor.patch`; Android's version code is made from it.
    pub version: String,
    pub author: String,
    pub description: String,
    pub copyright: String,
    pub website: String,
}

impl Default for AppInfo {
    fn default() -> Self {
        Self {
            name: "My q0 App".into(),
            id: "com.example.myapp".into(),
            version: "1.0.0".into(),
            author: String::new(),
            description: String::new(),
            copyright: String::new(),
            website: String::new(),
        }
    }
}

impl AppInfo {
    /// Android's integer version code: 1.2.3 is 10203.
    pub fn version_code(&self) -> u32 {
        let mut parts = self
            .version
            .split(|c: char| !c.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .map(|part| part.parse::<u32>().unwrap_or(0).min(99));
        let major = parts.next().unwrap_or(1).min(20_000);
        let minor = parts.next().unwrap_or(0);
        let patch = parts.next().unwrap_or(0);
        (major * 10_000 + minor * 100 + patch).max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StartMode {
    #[default]
    Windowed,
    Maximized,
    /// Borderless, covering the screen.
    Fullscreen,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    /// Empty uses the app name.
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub resizable: bool,
    pub start: StartMode,
    /// The system frame: title bar and window buttons.
    pub decorations: bool,
    pub always_on_top: bool,
    pub centered: bool,
    /// What shows around the movie when the window's shape differs from
    /// the stage's. Scripts change it with `q0window.background!`.
    pub background: Backdrop,
}

/// The colour around the movie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Backdrop {
    #[default]
    Black,
    /// The movie's own stage colour, so the stage seems to fill the window.
    Stage,
    Custom([u8; 3]),
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            title: String::new(),
            width: 960,
            height: 540,
            min_width: 320,
            min_height: 180,
            resizable: true,
            start: StartMode::Windowed,
            decorations: true,
            always_on_top: false,
            centered: true,
            background: Backdrop::Black,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    /// Samples per pixel: 1, 2, 4 or 8. Smoother vector edges cost GPU
    /// time; 1 suits pixel art and weak machines.
    pub msaa: u16,
    /// Waits for the display. Off can tear but answers input sooner.
    pub vsync: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            msaa: 4,
            vsync: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Unfocused {
    /// Keeps running behind other windows.
    #[default]
    Keep,
    /// Pauses the movie and its sound until the window is back.
    Pause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EscapeKey {
    /// Esc closes the app.
    #[default]
    Quit,
    /// Esc only leaves fullscreen.
    LeaveFullscreen,
    /// Esc belongs to the movie's scripts.
    Movie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CursorMode {
    #[default]
    Show,
    /// Hidden after two seconds without moving.
    HideWhenIdle,
    /// Hidden over the window; games that draw their own.
    Hide,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Behaviour {
    pub unfocused: Unfocused,
    pub escape: EscapeKey,
    /// F11 toggles fullscreen.
    pub fullscreen_key: bool,
    /// q0player's own F9 (movie only) and F4 (performance log) keys.
    pub player_hotkeys: bool,
    pub cursor: CursorMode,
    /// A second launch quits at once instead of opening another window.
    pub single_instance: bool,
    /// The timeline starts over at its end; games usually keep this on.
    pub loop_playback: bool,
    /// Plays the document's scenes one after another.
    pub all_scenes: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            unfocused: Unfocused::Keep,
            escape: EscapeKey::Quit,
            fullscreen_key: true,
            player_hotkeys: false,
            cursor: CursorMode::Show,
            single_instance: false,
            loop_playback: true,
            all_scenes: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DataSettings {
    /// Folder for `q0app.save!` data and the log; empty uses the app id.
    pub folder: String,
    /// Keeps saves beside the app instead of the user's data folder, for
    /// apps carried on a USB stick.
    pub portable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Diagnostics {
    /// Checks the bundle against its checksum at start and refuses a
    /// damaged one with a message instead of playing garbage.
    pub verify_on_start: bool,
    /// Windows: opens a console showing `q0app.log!` lines and script errors.
    pub console: bool,
    /// Appends `q0app.log!` lines and script errors to `log.txt` in the
    /// app's data folder.
    pub log_file: bool,
    /// Shows script errors in a message box; off keeps them to the log.
    pub error_dialog: bool,
    /// `app.debug` reads true, so scripts can show their own debug views.
    pub debug: bool,
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            verify_on_start: true,
            console: false,
            log_file: true,
            error_dialog: false,
            debug: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Splash {
    pub enabled: bool,
    pub duration_ms: u32,
    pub background: [u8; 3],
    /// The line under the name: a studio, a credit, or nothing.
    pub text: String,
    /// The app's icon above its name.
    pub show_icon: bool,
    /// A click, tap or key press ends it early.
    pub skippable: bool,
}

impl Default for Splash {
    fn default() -> Self {
        Self {
            enabled: false,
            duration_ms: 1600,
            background: [0, 0, 0],
            text: "made with q0".into(),
            show_icon: true,
            skippable: true,
        }
    }
}

impl Manifest {
    pub fn title(&self) -> &str {
        if self.window.title.trim().is_empty() {
            &self.app.name
        } else {
            &self.window.title
        }
    }

    /// The save folder's name.
    pub fn data_folder(&self) -> String {
        let folder = if self.data.folder.trim().is_empty() {
            &self.app.id
        } else {
            &self.data.folder
        };
        file_safe(folder)
    }

    /// What is wrong with the manifest, if anything, as messages a person
    /// can act on.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.app.name.trim().is_empty() {
            problems.push("the app needs a name".into());
        }
        if !is_app_id(&self.app.id) {
            problems.push(format!(
                "\"{}\" is not an app id: use two or more dot-separated parts of letters, digits and _, each starting with a letter (com.author.game)",
                self.app.id
            ));
        }
        if self
            .app
            .version
            .split('.')
            .any(|part| part.parse::<u32>().is_err())
        {
            problems.push(format!(
                "version \"{}\" should be numbers separated by dots, like 1.0.0",
                self.app.version
            ));
        }
        if self.window.width < 64 || self.window.height < 64 {
            problems.push("the window must be at least 64 × 64".into());
        }
        if self.window.min_width > self.window.width || self.window.min_height > self.window.height
        {
            problems.push("the minimum window size is larger than the window".into());
        }
        if !matches!(self.render.msaa, 1 | 2 | 4 | 8) {
            problems.push("anti-aliasing must be 1, 2, 4 or 8 samples".into());
        }
        problems
    }
}

/// Whether `id` can be an Android package name and a folder name.
pub fn is_app_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|part| {
            let mut chars = part.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// `name` with what Windows, macOS and Linux refuse in file names replaced.
pub fn file_safe(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        "app".into()
    } else {
        cleaned
    }
}

/// Everything an app is made of.
#[derive(Debug, Clone, PartialEq)]
pub struct Bundle {
    pub manifest: Manifest,
    /// PNG; the window and taskbar icon.
    pub icon_png: Option<Vec<u8>>,
    /// The `.q0s`/`.q1s` movie as saved.
    pub movie: Vec<u8>,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// No bundle at the end of the file.
    Missing,
    /// A bundle is there but its lengths do not fit the file.
    Truncated,
    /// The bundle's bytes do not match its checksum.
    Damaged,
    Manifest(String),
    /// Written by a newer Q0Build than this runtime understands.
    TooNew(u32),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(error) => write!(f, "{error}"),
            Error::Missing => write!(f, "no QIR bundle in this file"),
            Error::Truncated => write!(f, "the QIR bundle is cut short"),
            Error::Damaged => write!(f, "the QIR bundle is damaged (checksum mismatch)"),
            Error::Manifest(error) => write!(f, "the QIR manifest cannot be read: {error}"),
            Error::TooNew(format) => write!(
                f,
                "this app was built for a newer runtime (manifest format {format}, this runtime reads {FORMAT})"
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io(error)
    }
}

/// FNV-1a, 64-bit: no dependencies and fast enough to check a few hundred
/// megabytes at start. It guards against damage, not tampering.
pub fn checksum(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

impl Bundle {
    /// The bundle as bytes, trailer last.
    pub fn encode(&self) -> Vec<u8> {
        let manifest = serde_json::to_vec_pretty(&self.manifest).expect("manifest serializes");
        let icon = self.icon_png.as_deref().unwrap_or_default();
        let mut out =
            Vec::with_capacity(manifest.len() + icon.len() + self.movie.len() + TRAILER_LEN);
        out.extend_from_slice(&manifest);
        out.extend_from_slice(icon);
        out.extend_from_slice(&self.movie);
        let sum = checksum(&out);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&(out.len() as u64 - MAGIC.len() as u64).to_le_bytes()); // payload length
        out.extend_from_slice(&(manifest.len() as u32).to_le_bytes());
        out.extend_from_slice(&(icon.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.movie.len() as u64).to_le_bytes());
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// The bundle at the end of `bytes`.
    pub fn decode(bytes: &[u8], verify: bool) -> Result<Self, Error> {
        let trailer = Trailer::read(
            bytes
                .get(bytes.len().saturating_sub(TRAILER_LEN)..)
                .ok_or(Error::Missing)?,
        )?;
        let end = bytes.len() - TRAILER_LEN;
        let start = end
            .checked_sub(trailer.payload_len())
            .ok_or(Error::Truncated)?;
        Self::from_payload(&bytes[start..end], &trailer, verify)
    }

    /// The bundle at the end of the file at `path`, reading only the bundle,
    /// not the program in front of it.
    pub fn read_from_file(path: &Path, verify: bool) -> Result<Self, Error> {
        let mut file = File::open(path)?;
        let length = file.metadata()?.len();
        if length < TRAILER_LEN as u64 {
            return Err(Error::Missing);
        }
        file.seek(SeekFrom::Start(length - TRAILER_LEN as u64))?;
        let mut tail = [0_u8; TRAILER_LEN];
        file.read_exact(&mut tail)?;
        let trailer = Trailer::read(&tail)?;
        let payload_len = trailer.payload_len() as u64;
        if payload_len > length - TRAILER_LEN as u64 {
            return Err(Error::Truncated);
        }
        file.seek(SeekFrom::Start(length - TRAILER_LEN as u64 - payload_len))?;
        let mut payload = vec![0_u8; payload_len as usize];
        file.read_exact(&mut payload)?;
        Self::from_payload(&payload, &trailer, verify)
    }

    fn from_payload(payload: &[u8], trailer: &Trailer, verify: bool) -> Result<Self, Error> {
        if verify && checksum(payload) != trailer.checksum {
            return Err(Error::Damaged);
        }
        let manifest_end = trailer.manifest_len as usize;
        let icon_end = manifest_end + trailer.icon_len as usize;
        let manifest: Manifest = serde_json::from_slice(&payload[..manifest_end])
            .map_err(|error| Error::Manifest(error.to_string()))?;
        if manifest.format > FORMAT {
            return Err(Error::TooNew(manifest.format));
        }
        Ok(Self {
            manifest,
            icon_png: (trailer.icon_len > 0).then(|| payload[manifest_end..icon_end].to_vec()),
            movie: payload[icon_end..].to_vec(),
        })
    }
}

/// Where a bundle sits at the end of a file, without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trailer {
    pub manifest_len: u32,
    pub icon_len: u32,
    pub movie_len: u64,
    pub checksum: u64,
}

impl Trailer {
    pub fn read(tail: &[u8]) -> Result<Self, Error> {
        if tail.len() != TRAILER_LEN || tail[..8] != MAGIC {
            return Err(Error::Missing);
        }
        let u64_at = |at: usize| u64::from_le_bytes(tail[at..at + 8].try_into().unwrap());
        let u32_at = |at: usize| u32::from_le_bytes(tail[at..at + 4].try_into().unwrap());
        let trailer = Self {
            manifest_len: u32_at(16),
            icon_len: u32_at(20),
            movie_len: u64_at(24),
            checksum: u64_at(32),
        };
        if u64_at(8) != trailer.payload_len() as u64 {
            return Err(Error::Truncated);
        }
        Ok(trailer)
    }

    pub fn payload_len(&self) -> usize {
        self.manifest_len as usize + self.icon_len as usize + self.movie_len as usize
    }
}

/// How many bytes at the end of `bytes` belong to a bundle, so a packer can
/// replace one instead of stacking a second behind it.
pub fn bundle_len(bytes: &[u8]) -> Option<usize> {
    let tail = bytes.get(bytes.len().checked_sub(TRAILER_LEN)?..)?;
    let trailer = Trailer::read(tail).ok()?;
    let total = trailer.payload_len() + TRAILER_LEN;
    (total <= bytes.len()).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Bundle {
        let mut manifest = Manifest::default();
        manifest.app.name = "Bullet Box".into();
        manifest.app.id = "com.q0s.bulletbox".into();
        Bundle {
            manifest,
            icon_png: Some(vec![0x89, b'P', b'N', b'G', 1, 2, 3]),
            movie: (0..10_000).map(|i| (i * 7) as u8).collect(),
        }
    }

    #[test]
    fn a_bundle_comes_back_from_the_end_of_a_program() {
        let bundle = sample();
        let mut exe = b"MZ pretend program".to_vec();
        let program_len = exe.len();
        exe.extend(bundle.encode());
        assert_eq!(Bundle::decode(&exe, true).unwrap(), bundle);
        assert_eq!(bundle_len(&exe), Some(exe.len() - program_len));

        let path = std::env::temp_dir().join(format!("q0qir-test-{}.bin", std::process::id()));
        std::fs::write(&path, &exe).unwrap();
        let read = Bundle::read_from_file(&path, true);
        std::fs::remove_file(&path).ok();
        assert_eq!(read.unwrap(), bundle);
    }

    #[test]
    fn a_damaged_bundle_is_refused_when_checked() {
        let mut bytes = sample().encode();
        // A byte of the movie: the manifest still reads, the checksum fails.
        let inside_movie = bytes.len() - TRAILER_LEN - 10;
        bytes[inside_movie] ^= 0xff;
        assert!(matches!(Bundle::decode(&bytes, true), Err(Error::Damaged)));
        assert!(Bundle::decode(&bytes, false).is_ok());
    }

    #[test]
    fn a_plain_program_has_no_bundle() {
        assert!(matches!(
            Bundle::decode(b"just a program, nothing appended at all here", true),
            Err(Error::Missing)
        ));
        assert_eq!(bundle_len(b"short"), None);
    }

    #[test]
    fn app_ids_follow_android_package_rules() {
        assert!(is_app_id("com.q0s.bulletbox"));
        assert!(is_app_id("ru.shadowmol.grunt_2"));
        assert!(!is_app_id("bulletbox"));
        assert!(!is_app_id("com.1game"));
        assert!(!is_app_id("com..game"));
        assert!(!is_app_id("com.my-game"));
    }

    #[test]
    fn version_codes_grow_with_versions() {
        let code = |version: &str| {
            AppInfo {
                version: version.into(),
                ..AppInfo::default()
            }
            .version_code()
        };
        assert_eq!(code("1.2.3"), 10203);
        assert!(code("1.10.0") > code("1.9.9"));
        assert_eq!(code("0.0.0"), 1);
    }

    #[test]
    fn old_manifests_fill_in_new_settings() {
        let manifest: Manifest = serde_json::from_str(r#"{"app":{"name":"Old"}}"#).unwrap();
        assert_eq!(manifest.app.name, "Old");
        assert_eq!(manifest.window, WindowSettings::default());
        assert_eq!(manifest.title(), "Old");
    }

    #[test]
    fn file_names_lose_what_systems_refuse() {
        assert_eq!(file_safe("Grunt: Revenge?"), "Grunt_ Revenge_");
        assert_eq!(file_safe("  ...  "), "app");
    }
}
