//! Persistent settings: theme + viewport size + recent files. One JSON
//! blob lives in the user's config dir (`%APPDATA%\q0player\settings.json`
//! on Windows, `$XDG_CONFIG_HOME/q0player/...` elsewhere). Failures are
//! non-fatal — we fall back to defaults rather than crash on a malformed
//! file.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use egui::{FontId, TextStyle};
use serde::{Deserialize, Serialize};

use super::theme::{ColorRgb, Theme};

const MAX_RECENT_FILES: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    pub viewport_w: u32,
    pub viewport_h: u32,
    pub recent: Vec<PathBuf>,
    pub loop_playback: bool,
    pub show_left_panel: bool,
    pub show_right_panel: bool,
    /// Global UI text size in points. egui's stock 14pt was too small
    /// for q0player's dense panels, so the player owns this as a first-class
    /// setting instead of relying on OS scaling alone.
    #[serde(default = "default_text_size_points")]
    pub text_size_points: u8,
    /// Match the rasteriser's resolution to the actual canvas pixel size
    /// every frame. With this off, vector v2 movies render at the fixed
    /// `viewport_w/h` and egui upscales the texture — that's what makes
    /// them look pixelated when the window is bigger than the stage.
    /// Default ON; flip off to lock resolution (e.g. for screen-record).
    #[serde(default = "default_true")]
    pub auto_resolution: bool,
    /// Supersampling factor for vector rasterisation (1 / 2 / 4).
    /// 2 is the sweet spot — smooth diagonals with ~4× cost. 4 is
    /// overkill for moving images but useful when paused and zoomed.
    #[serde(default = "default_ss")]
    pub supersample: u8,
    /// Paint a solid white rectangle behind the v2 stage rect. Without
    /// this, transparent fills / unfilled vectors blend straight onto
    /// the player's viewport-bg colour — invisible on dark themes.
    /// Default ON because that's the editor's stage paper.
    #[serde(default = "default_true")]
    pub white_stage_bg: bool,
}

fn default_true() -> bool {
    true
}
fn default_ss() -> u8 {
    2
}
fn default_text_size_points() -> u8 {
    16
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            viewport_w: 640,
            viewport_h: 360,
            recent: Vec::new(),
            loop_playback: true,
            show_left_panel: true,
            show_right_panel: true,
            text_size_points: default_text_size_points(),
            auto_resolution: true,
            supersample: 2,
            white_stage_bg: true,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut settings = match Self::path().and_then(|p| std::fs::read(&p).ok()) {
            Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            None => Self::default(),
        };
        if settings.theme.accent == ColorRgb::new(0x0F, 0x84, 0xCE) {
            settings.theme.accent = ColorRgb::new(0xC8, 0x10, 0x2E);
        }
        settings.normalize();
        settings
    }

    pub fn save(&self) {
        let _ = self.try_save();
    }

    /// Persist settings atomically. The UI keeps `save()` as a non-fatal
    /// convenience, while callers that need to report an error can use this.
    pub fn try_save(&self) -> io::Result<()> {
        let path = Self::path().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "configuration directory unavailable",
            )
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        super::atomic_file::write(&path, &bytes)
    }

    pub fn path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("q0player").join("settings.json"))
    }

    /// Push `path` to the front of recent-files, dedup, cap at 10.
    pub fn push_recent(&mut self, path: &Path) {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.recent.retain(|p| p != &canonical);
        self.recent.insert(0, canonical);
        self.normalize_recent();
    }

    /// Drop entries that no longer exist on disk — called on startup so
    /// stale references don't pile up forever.
    pub fn prune_recent(&mut self) {
        self.normalize_recent();
    }

    pub fn apply_text_size(&self, ctx: &egui::Context) {
        let size = self.text_size_points.clamp(11, 24) as f32;
        let mut style = (*ctx.style()).clone();
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::proportional(size + 6.0));
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(size));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(size));
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(size));
        style.text_styles.insert(
            TextStyle::Small,
            FontId::proportional((size - 2.0).max(10.0)),
        );
        ctx.set_style(style);
    }

    fn normalize(&mut self) {
        self.viewport_w = self.viewport_w.clamp(16, 4096);
        self.viewport_h = self.viewport_h.clamp(16, 4096);
        self.text_size_points = self.text_size_points.clamp(11, 24);
        if !matches!(self.supersample, 1 | 2 | 4) {
            self.supersample = default_ss();
        }
        self.normalize_recent();
    }

    fn normalize_recent(&mut self) {
        let mut seen = HashSet::new();
        self.recent = self
            .recent
            .drain(..)
            .filter_map(|path| std::fs::canonicalize(path).ok())
            .filter(|path| seen.insert(path.clone()))
            .take(MAX_RECENT_FILES)
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT_TEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    #[test]
    fn missing_settings_fields_use_defaults() {
        let settings: Settings = serde_json::from_str("{}").expect("deserialize defaults");
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn normalization_bounds_untrusted_numeric_settings() {
        let mut settings = Settings {
            viewport_w: 0,
            viewport_h: u32::MAX,
            text_size_points: u8::MAX,
            supersample: 3,
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(settings.viewport_w, 16);
        assert_eq!(settings.viewport_h, 4096);
        assert_eq!(settings.text_size_points, 24);
        assert_eq!(settings.supersample, 2);
    }

    #[test]
    fn recent_files_are_pruned_deduplicated_and_capped() {
        let directory = std::env::temp_dir().join(format!(
            "q0player-recents-{}-{}",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("create test directory");
        let paths: Vec<PathBuf> = (0..12)
            .map(|index| {
                let path = directory.join(format!("{index}.q0s"));
                std::fs::write(&path, b"q0s").expect("write recent file");
                path
            })
            .collect();
        let mut settings = Settings {
            recent: paths
                .iter()
                .cloned()
                .chain(std::iter::once(paths[0].clone()))
                .chain(std::iter::once(directory.join("missing.q0s")))
                .collect(),
            ..Settings::default()
        };

        settings.prune_recent();

        assert_eq!(settings.recent.len(), MAX_RECENT_FILES);
        assert_eq!(
            settings.recent[0],
            std::fs::canonicalize(&paths[0]).expect("canonical first path")
        );
        std::fs::remove_dir_all(directory).expect("remove test directory");
    }
}
