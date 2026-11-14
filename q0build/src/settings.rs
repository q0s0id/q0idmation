//! What Q0Build remembers between runs: the theme, the text size and the
//! profiles opened last.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const RECENT: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub skin: String,
    /// Text and controls, 1.0 as designed.
    pub zoom: f32,
    pub recent: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            skin: "black-glass".into(),
            zoom: 1.0,
            recent: Vec::new(),
        }
    }
}

impl Settings {
    fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|folder| folder.join("q0build").join("settings.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, bytes);
        }
    }

    pub fn zoom(&self) -> f32 {
        if self.zoom.is_finite() {
            self.zoom.clamp(0.75, 1.6)
        } else {
            1.0
        }
    }

    /// Puts `path` first among the recent profiles and keeps the list short.
    pub fn remember(&mut self, path: &Path) {
        self.recent.retain(|existing| existing != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(RECENT);
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_profiles_stay_unique_and_short() {
        let mut settings = Settings {
            recent: (0..RECENT)
                .map(|i| PathBuf::from(format!("{i}.q0build")))
                .collect(),
            ..Settings::default()
        };
        let first = PathBuf::from("3.q0build");
        settings.recent.retain(|existing| existing != &first);
        settings.recent.insert(0, first.clone());
        assert_eq!(settings.recent[0], first);
        assert_eq!(
            settings
                .recent
                .iter()
                .filter(|path| **path == first)
                .count(),
            1
        );
        assert_eq!(settings.recent.len(), RECENT);
    }

    #[test]
    fn a_strange_zoom_reads_as_normal() {
        let settings = Settings {
            zoom: f32::NAN,
            ..Settings::default()
        };
        assert_eq!(settings.zoom(), 1.0);
    }
}
