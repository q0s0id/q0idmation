//! Editable theme — the user can recolour every accent / panel through the
//! Settings dialog and changes round-trip through `settings.json`. All UI
//! widgets read from `Theme` indirectly via the egui `Visuals` we install
//! in `apply()`, so changes feed through the whole app live.

use egui::{Color32, Rounding, Stroke};
use q0theme::{BuiltinTheme, BUILTIN_THEMES};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ColorRgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
    pub fn to_color32(self) -> Color32 {
        Color32::from_rgb(self.r, self.g, self.b)
    }
    pub fn from_color32(c: Color32) -> Self {
        Self {
            r: c.r(),
            g: c.g(),
            b: c.b(),
        }
    }
}

impl From<q0theme::Rgb> for ColorRgb {
    fn from(value: q0theme::Rgb) -> Self {
        Self::new(value.r, value.g, value.b)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Theme {
    /// The hero accent — buttons, hyperlinks, sliders, the playhead.
    pub accent: ColorRgb,
    /// Side panels (left recent files, right file info).
    pub panel: ColorRgb,
    /// Top window chrome (menu bar, title bar background).
    pub window: ColorRgb,
    /// Sunken background — text fields, code, inputs.
    pub deep_bg: ColorRgb,
    /// Primary text colour.
    pub text: ColorRgb,
    /// Dimmed text — labels, hints, status footnotes.
    pub text_dim: ColorRgb,
    /// Stroke around windows / popups — usually 1-2 shades darker than panel.
    pub stroke_dark: ColorRgb,
    /// Player viewport background — what shows behind the .q0s scene.
    #[serde(default = "default_viewport_bg")]
    pub viewport_bg: ColorRgb,
}

fn default_viewport_bg() -> ColorRgb {
    ColorRgb::new(0x18, 0x18, 0x18)
}

impl Default for Theme {
    fn default() -> Self {
        // Deliberately the same palette family as q0editor for a unified look.
        Self::from_builtin(q0theme::default_theme())
    }
}

impl Theme {
    pub const PRESETS: &'static [BuiltinTheme] = BUILTIN_THEMES;

    pub fn from_builtin(preset: &BuiltinTheme) -> Self {
        Self {
            accent: preset.accent.into(),
            panel: preset.panel.into(),
            window: preset.window.into(),
            deep_bg: preset.deep_bg.into(),
            text: preset.text.into(),
            text_dim: preset.text_dim.into(),
            stroke_dark: preset.stroke_dark.into(),
            viewport_bg: preset.viewport_bg.into(),
        }
    }

    /// Push the theme into egui's `Visuals` so every widget — buttons,
    /// frames, scrollbars, popups — picks up the new colours next frame.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        let dark = self.text.r as i32 + self.text.g as i32 + self.text.b as i32 > 384;
        let mut visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let accent = self.accent.to_color32();
        let panel = self.panel.to_color32();
        let window = self.window.to_color32();
        let deep = self.deep_bg.to_color32();
        let text = self.text.to_color32();
        let stroke_dark = self.stroke_dark.to_color32();

        visuals.panel_fill = panel;
        visuals.window_fill = window;
        visuals.window_stroke = Stroke::new(1.0_f32, stroke_dark);
        visuals.menu_rounding = Rounding::same(2.0);
        visuals.window_rounding = Rounding::same(3.0);
        visuals.extreme_bg_color = deep;
        visuals.faint_bg_color = mid_blend(panel, window);
        visuals.code_bg_color = deep;
        visuals.hyperlink_color = accent;
        visuals.selection.bg_fill = accent;
        visuals.selection.stroke = Stroke::new(1.0_f32, text);

        for w in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
        ] {
            w.bg_fill = panel;
            w.weak_bg_fill = panel;
            w.bg_stroke = Stroke::new(1.0_f32, stroke_dark);
            w.fg_stroke = Stroke::new(1.0_f32, text);
            w.rounding = Rounding::same(2.0);
        }
        // Hover / pressed get progressively brighter accents.
        visuals.widgets.hovered.bg_fill = mid_blend(panel, accent);
        visuals.widgets.hovered.weak_bg_fill = mid_blend(panel, accent);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, text);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, accent);
        visuals.widgets.hovered.rounding = Rounding::same(2.0);

        visuals.widgets.active.bg_fill = accent;
        visuals.widgets.active.weak_bg_fill = accent;
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, text);
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, accent);
        visuals.widgets.active.rounding = Rounding::same(2.0);

        visuals.widgets.open = visuals.widgets.active;

        style.visuals = visuals;
        ctx.set_style(style);
    }
}

// ---------------- .q7s persistence ----------------
//
// `.q7s` uses a small JSON envelope so themes persist, remain readable,
// and can be validated before they are applied.

const Q7S_MAGIC: &str = "q7s";
const Q7S_KIND: &str = "theme";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Q7sFile {
    /// Sentinel — lets us reject random JSON files dropped in the picker.
    magic: String,
    /// Bumped whenever the schema changes incompatibly. Forward-compat
    /// loaders pin to a known list and refuse unknown values.
    version: u8,
    /// Discriminator for the future "themes can carry more than colours"
    /// idea (per-tool defaults, font sizes, etc.). Today: always "theme".
    kind: String,
    /// User-visible label — preserved on round-trip.
    name: String,
    theme: Theme,
}

impl Theme {
    /// Save this theme to a `.q7s` file (JSON inside). `name` is stored
    /// in the file so saved themes round-trip with a label, not "Untitled".
    pub fn write_q7s(&self, path: &std::path::Path, name: &str) -> std::io::Result<()> {
        let file = Q7sFile {
            magic: Q7S_MAGIC.to_string(),
            version: 1,
            kind: Q7S_KIND.to_string(),
            name: name.to_string(),
            theme: self.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        super::atomic_file::write(path, &bytes)
    }

    /// Load a theme from a `.q7s` file. Returns `(name, theme)` so
    /// callers can show the saved label in the UI.
    pub fn read_q7s(path: &std::path::Path) -> std::io::Result<(String, Theme)> {
        let bytes = std::fs::read(path)?;
        let parsed: Q7sFile = serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if parsed.magic != Q7S_MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "not a q7s file",
            ));
        }
        if parsed.kind != Q7S_KIND {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "q7s kind is not 'theme'",
            ));
        }
        if parsed.version != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported q7s version {}", parsed.version),
            ));
        }
        Ok((parsed.name, parsed.theme))
    }
}

fn mid_blend(a: Color32, b: Color32) -> Color32 {
    Color32::from_rgb(
        ((a.r() as u16 + b.r() as u16) / 2) as u8,
        ((a.g() as u16 + b.g() as u16) / 2) as u8,
        ((a.b() as u16 + b.b() as u16) / 2) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT_TEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    #[test]
    fn builtins_follow_the_shared_theme_catalogue() {
        assert_eq!(Theme::PRESETS, q0theme::BUILTIN_THEMES);
        for preset in Theme::PRESETS {
            let theme = Theme::from_builtin(preset);
            assert_eq!(theme.accent, preset.accent.into(), "{} accent", preset.name);
            assert_eq!(theme.panel, preset.panel.into(), "{} panel", preset.name);
            assert_eq!(theme.window, preset.window.into(), "{} window", preset.name);
            assert_eq!(theme.deep_bg, preset.deep_bg.into(), "{} deep", preset.name);
            assert_eq!(theme.text, preset.text.into(), "{} text", preset.name);
            assert_eq!(
                theme.text_dim,
                preset.text_dim.into(),
                "{} dim",
                preset.name
            );
            assert_eq!(
                theme.stroke_dark,
                preset.stroke_dark.into(),
                "{} stroke",
                preset.name
            );
            assert_eq!(
                theme.viewport_bg,
                preset.viewport_bg.into(),
                "{} viewport",
                preset.name
            );
        }
    }

    #[test]
    fn imports_editor_theme_without_player_only_viewport_colour() {
        let json = r#"{
            "magic": "q7s",
            "version": 1,
            "kind": "theme",
            "name": "Editor theme",
            "theme": {
                "accent": {"r": 200, "g": 16, "b": 46},
                "panel": {"r": 60, "g": 60, "b": 60},
                "window": {"r": 47, "g": 47, "b": 47},
                "deep_bg": {"r": 34, "g": 34, "b": 34},
                "text": {"r": 230, "g": 230, "b": 230},
                "text_dim": {"r": 159, "g": 159, "b": 159},
                "stroke_dark": {"r": 32, "g": 32, "b": 32},
                "canvas_bg": {"r": 85, "g": 85, "b": 85}
            }
        }"#;
        let directory = std::env::temp_dir().join(format!(
            "q0player-q7s-{}-{}",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("editor-theme.q7s");
        std::fs::write(&path, json).expect("write editor theme");

        let (name, theme) = Theme::read_q7s(&path).expect("load editor theme in player");

        assert_eq!(name, "Editor theme");
        assert_eq!(theme.accent, ColorRgb::new(200, 16, 46));
        assert_eq!(theme.viewport_bg, default_viewport_bg());
        std::fs::remove_dir_all(directory).expect("remove test directory");
    }
}
