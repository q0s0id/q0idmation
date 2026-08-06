//! Persistent editor settings: theme, drawing defaults, and script editor options.
//! One JSON blob in `<config>/q0editor/settings.json`. Failures are
//! non-fatal: we fall back to `Default::default()` rather than crash on
//! a malformed file.
//!
//! The `Theme` half mirrors q0player's so the two apps look like
//! siblings; `.q7s` files saved from either app load in the other.

use std::path::{Path, PathBuf};

use egui::{Color32, Rounding, Stroke};
use q0theme::{BuiltinTheme, BUILTIN_THEMES, Q0S_SIGNATURE_ID};
use serde::{Deserialize, Serialize};

use crate::brush::{BrushNib, BrushSettings};
use crate::easing::EasingPreset;
use crate::l10n::Language;
use q0s_format::geom::CapShape;
use q0s_format::v2::Rgba;

const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
const MAX_THEME_BYTES: u64 = 2 * 1024 * 1024;

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

fn blend(a: ColorRgb, b: ColorRgb, t: f32) -> ColorRgb {
    let mix = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    ColorRgb::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b))
}

fn scale(c: ColorRgb, k: f32) -> ColorRgb {
    let m = |x: u8| (x as f32 * k).round().clamp(0.0, 255.0) as u8;
    ColorRgb::new(m(c.r), m(c.g), m(c.b))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Theme {
    // ----- Base widget palette -----
    pub accent: ColorRgb,
    pub panel: ColorRgb,
    pub window: ColorRgb,
    pub deep_bg: ColorRgb,
    pub text: ColorRgb,
    pub text_dim: ColorRgb,
    pub stroke_dark: ColorRgb,

    // ----- Stage area outside the paper (and the paper border / shadow) -----
    // The stage paper itself is always white — it's the user's drawing
    // canvas, not chrome, so we don't expose it as a theme colour.
    /// Colour of the canvas behind the stage paper — what shows in the
    /// margins around the stage when the window is bigger than the
    /// content. Default matches Flash CS3 dark grey.
    #[serde(default = "default_canvas_bg")]
    pub canvas_bg: ColorRgb,
    #[serde(default = "default_stage_border")]
    pub stage_border: ColorRgb,
    /// Overlay used outside the current symbol bounds while editing inside a q0rg.
    /// Stored in .q7s so custom themes control the edit-in-place atmosphere.
    #[serde(default = "default_symbol_edit_mask")]
    pub symbol_edit_mask: ColorRgb,
    /// Light half of the adaptive Library preview background.
    #[serde(default = "default_library_preview_light")]
    pub library_preview_light: ColorRgb,
    /// Dark half of the adaptive Library preview background.
    #[serde(default = "default_library_preview_dark")]
    pub library_preview_dark: ColorRgb,

    // ----- Timeline -----
    /// Per-row separators / very faint vertical grid below the header.
    #[serde(default = "default_timeline_grid")]
    pub timeline_grid: ColorRgb,
    /// Brighter every-5th-frame vertical grid line.
    #[serde(default = "default_timeline_grid_5")]
    pub timeline_grid_5: ColorRgb,
    /// Header strip at the top of the timeline (frame numbers row).
    #[serde(default = "default_timeline_header")]
    pub timeline_header: ColorRgb,
    /// Timeline playhead line. Built-in themes derive it from their accent.
    #[serde(default = "default_playhead")]
    pub playhead: ColorRgb,
    /// Black dot drawn on a keyframe cell.
    #[serde(default = "default_keyframe")]
    pub keyframe: ColorRgb,
    /// Static-span fill (Tween::None hold cells).
    #[serde(default = "default_extension")]
    pub extension: ColorRgb,
    /// Same as `extension` but for the *current* layer — slightly
    /// distinct so the user sees which row the next op lands on.
    #[serde(default = "default_extension_active")]
    pub extension_active: ColorRgb,
    /// Linear-tween fill behind the cells.
    #[serde(default = "default_tween_fill")]
    pub tween_fill: ColorRgb,
    /// Tween direction arrow drawn over the tween fill.
    #[serde(default = "default_tween_arrow")]
    pub tween_arrow: ColorRgb,
    /// Empty-but-inside-frame_count cell colour (mid grey).
    #[serde(default = "default_empty_inside")]
    pub empty_inside: ColorRgb,
    #[serde(default = "default_empty_inside_active")]
    pub empty_inside_active: ColorRgb,
    /// Beyond-frame_count cells (very dark).
    #[serde(default = "default_empty_beyond")]
    pub empty_beyond: ColorRgb,
    /// Every 5th beyond-frame_count cell — slightly brighter.
    #[serde(default = "default_empty_beyond_5")]
    pub empty_beyond_5: ColorRgb,

    // ----- Onion-skin tints -----
    /// Past-frame ghost tint (cool / blue side by default).
    #[serde(default = "default_onion_past")]
    pub onion_past: ColorRgb,
    /// Future-frame ghost tint (warm / orange side by default).
    #[serde(default = "default_onion_future")]
    pub onion_future: ColorRgb,

    // ----- q0lang editor syntax palette -----
    // The script editor renders source with a real tokenizer; each token
    // class picks its colour from these fields so the entire q0lang
    // editor recolours when the user picks a new theme preset (or saves
    // a custom one to .q7s). All defaults are tuned to read against the
    // CS3-dark base; the `with_timeline_from_base` recompute also rebuilds
    // them whenever a preset is applied.
    #[serde(default = "default_syntax_keyword")]
    pub syntax_keyword: ColorRgb,
    #[serde(default = "default_syntax_builtin")]
    pub syntax_builtin: ColorRgb,
    #[serde(default = "default_syntax_string")]
    pub syntax_string: ColorRgb,
    #[serde(default = "default_syntax_number")]
    pub syntax_number: ColorRgb,
    #[serde(default = "default_syntax_comment")]
    pub syntax_comment: ColorRgb,
    #[serde(default = "default_syntax_operator")]
    pub syntax_operator: ColorRgb,
    #[serde(default = "default_syntax_identifier")]
    pub syntax_identifier: ColorRgb,
    #[serde(default = "default_syntax_signal")]
    pub syntax_signal: ColorRgb,
    /// Gutter (line number column) background.
    #[serde(default = "default_syntax_gutter_bg")]
    pub syntax_gutter_bg: ColorRgb,
    /// Gutter line-number text colour.
    #[serde(default = "default_syntax_gutter_fg")]
    pub syntax_gutter_fg: ColorRgb,
    /// Highlight tint for the line the cursor is on.
    #[serde(default = "default_syntax_current_line")]
    pub syntax_current_line: ColorRgb,
}

// Default-fns for the new fields. Backwards-compat hook: `#[serde(default)]`
// reaches for these when older `settings.json` files are loaded that
// pre-date the timeline+canvas fields.
fn default_canvas_bg() -> ColorRgb {
    ColorRgb::new(0x29, 0x29, 0x29)
}
fn default_stage_border() -> ColorRgb {
    ColorRgb::new(0x80, 0x80, 0x80)
}
fn default_symbol_edit_mask() -> ColorRgb {
    ColorRgb::new(0x18, 0x18, 0x18)
}
fn default_library_preview_light() -> ColorRgb {
    ColorRgb::new(0xE8, 0xE8, 0xE8)
}
fn default_library_preview_dark() -> ColorRgb {
    ColorRgb::new(0x24, 0x24, 0x24)
}
fn default_timeline_grid() -> ColorRgb {
    ColorRgb::new(0x22, 0x22, 0x22)
}
fn default_timeline_grid_5() -> ColorRgb {
    ColorRgb::new(0x3A, 0x3A, 0x3A)
}
fn default_timeline_header() -> ColorRgb {
    ColorRgb::new(0x2A, 0x2A, 0x2A)
}
fn default_playhead() -> ColorRgb {
    q0theme::default_theme().accent.into()
}
fn default_keyframe() -> ColorRgb {
    ColorRgb::new(0x10, 0x10, 0x10)
}
fn default_extension() -> ColorRgb {
    ColorRgb::new(0xCF, 0xCF, 0xCF)
}
fn default_extension_active() -> ColorRgb {
    ColorRgb::new(0xE0, 0xE6, 0xEC)
}
fn default_tween_fill() -> ColorRgb {
    ColorRgb::new(0xC8, 0xB8, 0xE6)
}
fn default_tween_arrow() -> ColorRgb {
    ColorRgb::new(0x3A, 0x1A, 0x55)
}
fn default_empty_inside() -> ColorRgb {
    ColorRgb::new(0x40, 0x40, 0x40)
}
fn default_empty_inside_active() -> ColorRgb {
    ColorRgb::new(0x4A, 0x4A, 0x4A)
}
fn default_empty_beyond() -> ColorRgb {
    ColorRgb::new(0x1F, 0x1F, 0x1F)
}
fn default_empty_beyond_5() -> ColorRgb {
    ColorRgb::new(0x2A, 0x2A, 0x2A)
}
fn default_onion_past() -> ColorRgb {
    ColorRgb::new(0x70, 0x90, 0xC8)
}
fn default_onion_future() -> ColorRgb {
    ColorRgb::new(0xC8, 0x90, 0x70)
}
fn default_syntax_keyword() -> ColorRgb {
    ColorRgb::new(0x6F, 0xB1, 0xFF)
}
fn default_syntax_builtin() -> ColorRgb {
    ColorRgb::new(0x9C, 0xDC, 0xFE)
}
fn default_syntax_string() -> ColorRgb {
    ColorRgb::new(0xCE, 0x91, 0x78)
}
fn default_syntax_number() -> ColorRgb {
    ColorRgb::new(0xB5, 0xCE, 0xA8)
}
fn default_syntax_comment() -> ColorRgb {
    ColorRgb::new(0x6A, 0x99, 0x55)
}
fn default_syntax_operator() -> ColorRgb {
    ColorRgb::new(0xD4, 0xD4, 0xD4)
}
fn default_syntax_identifier() -> ColorRgb {
    ColorRgb::new(0xDC, 0xDC, 0xDC)
}
fn default_syntax_signal() -> ColorRgb {
    ColorRgb::new(0xE6, 0x6E, 0x6E)
}
fn default_syntax_gutter_bg() -> ColorRgb {
    ColorRgb::new(0x1A, 0x1A, 0x1A)
}
fn default_syntax_gutter_fg() -> ColorRgb {
    ColorRgb::new(0x70, 0x70, 0x70)
}
fn default_syntax_current_line() -> ColorRgb {
    ColorRgb::new(0x2A, 0x2A, 0x2A)
}

impl Default for Theme {
    fn default() -> Self {
        // The editor's existing CS3-flavoured palette as the baseline.
        Self {
            accent: q0theme::default_theme().accent.into(),
            panel: q0theme::default_theme().panel.into(),
            window: q0theme::default_theme().window.into(),
            deep_bg: q0theme::default_theme().deep_bg.into(),
            text: q0theme::default_theme().text.into(),
            text_dim: q0theme::default_theme().text_dim.into(),
            stroke_dark: q0theme::default_theme().stroke_dark.into(),
            canvas_bg: default_canvas_bg(),
            stage_border: default_stage_border(),
            symbol_edit_mask: default_symbol_edit_mask(),
            library_preview_light: default_library_preview_light(),
            library_preview_dark: default_library_preview_dark(),
            timeline_grid: default_timeline_grid(),
            timeline_grid_5: default_timeline_grid_5(),
            timeline_header: default_timeline_header(),
            playhead: default_playhead(),
            keyframe: default_keyframe(),
            extension: default_extension(),
            extension_active: default_extension_active(),
            tween_fill: default_tween_fill(),
            tween_arrow: default_tween_arrow(),
            empty_inside: default_empty_inside(),
            empty_inside_active: default_empty_inside_active(),
            empty_beyond: default_empty_beyond(),
            empty_beyond_5: default_empty_beyond_5(),
            onion_past: default_onion_past(),
            onion_future: default_onion_future(),
            syntax_keyword: default_syntax_keyword(),
            syntax_builtin: default_syntax_builtin(),
            syntax_string: default_syntax_string(),
            syntax_number: default_syntax_number(),
            syntax_comment: default_syntax_comment(),
            syntax_operator: default_syntax_operator(),
            syntax_identifier: default_syntax_identifier(),
            syntax_signal: default_syntax_signal(),
            syntax_gutter_bg: default_syntax_gutter_bg(),
            syntax_gutter_fg: default_syntax_gutter_fg(),
            syntax_current_line: default_syntax_current_line(),
        }
    }
}

impl Theme {
    /// Recompute every "chrome" colour (canvas backdrop, timeline cells,
    /// span fills, etc.) from the current base palette. Each preset
    /// chains this after setting accent/panel/window/text so the timeline
    /// always blends into its theme — no light-blue tween fills bleeding
    /// through Solarized Dark, no CS3-grey strip sitting in the middle
    /// of a Light layout.
    ///
    /// Heuristic: pick canvas/header/grid as scaled tones of `window`,
    /// span/empty cells as blends between `panel` and `text`/`accent`,
    /// keyframe = `text` (always legible), playhead = theme accent.
    /// User-saved overrides via the Settings color-pickers persist on
    /// top — derivation only runs when a preset is picked.
    pub fn with_timeline_from_base(mut self) -> Self {
        let panel = self.panel;
        let window = self.window;
        let text = self.text;
        let stroke_dark = self.stroke_dark;
        let accent = self.accent;
        let dark = (text.r as u16 + text.g as u16 + text.b as u16) > 384;

        // Backdrop / paper border —
        // dark themes get a slightly darkened window; light themes a
        // mid grey so the white paper doesn't fade into the canvas.
        self.canvas_bg = if dark {
            scale(window, 0.88)
        } else {
            ColorRgb::new(0xC4, 0xC4, 0xC4)
        };
        self.stage_border = blend(self.canvas_bg, text, 0.3);
        self.symbol_edit_mask = if dark {
            blend(self.deep_bg, self.accent, 0.08)
        } else {
            blend(self.panel, self.text_dim, 0.18)
        };
        self.library_preview_light = if dark {
            blend(self.panel, self.text, 0.82)
        } else {
            blend(self.panel, self.window, 0.65)
        };
        self.library_preview_dark = if dark {
            blend(self.deep_bg, self.stroke_dark, 0.45)
        } else {
            blend(self.text, self.stroke_dark, 0.35)
        };

        // Header / grid — header reads as a "panel light" stripe over
        // the timeline base; grid is just stroke-dark hairlines.
        self.timeline_header = blend(window, panel, 0.5);
        self.timeline_grid = stroke_dark;
        self.timeline_grid_5 = blend(stroke_dark, panel, 0.6);

        // Span fills — extension is `text * panel` mid (legible on
        // both); active tints toward `accent`. Tween picks up a hint
        // of accent so it stands apart from static spans without
        // losing the theme.
        self.extension = blend(panel, text, 0.55);
        self.extension_active = blend(self.extension, accent, 0.30);
        self.tween_fill = blend(panel, accent, 0.45);
        self.tween_arrow = if dark {
            blend(text, accent, 0.5)
        } else {
            blend(text, accent, 0.7)
        };

        // Keyframe dot — text colour reads as the strongest contrast
        // dot against any cell.
        self.keyframe = text;

        // Empty cells — inside is panel-with-a-touch-of-text so it
        // shows as "blank canvas, drawable". Active layer goes a step
        // brighter to flag the row drawing will land in.
        self.empty_inside = blend(panel, text, 0.10);
        self.empty_inside_active = blend(self.empty_inside, accent, 0.18);
        // Beyond cells — clearly darker than inside so the q0rg edge
        // is unmistakable; every-5th cell takes a small lift.
        self.empty_beyond = scale(window, 0.75);
        self.empty_beyond_5 = blend(self.empty_beyond, panel, 0.4);

        // Timeline focus follows the theme's own identity instead of
        // leaking the dark/q0s-signature red into every preset.
        self.playhead = accent;

        // ----- q0lang syntax palette -----
        // Pick syntax colours by mixing the theme's own accent with
        // text/panel so each preset's editor looks native, not a CS3
        // gradient bolted onto a Solarized chrome. Logic:
        //   keyword  = accent shifted toward text (saturated focal)
        //   builtin  = accent itself (the user's brand colour)
        //   string   = warm complement of accent
        //   number   = blend of text and accent (cool/neutral)
        //   comment  = text_dim slightly tinted toward panel (recedes)
        //   operator = text (full contrast for punctuation)
        //   identifier = text (default reading colour)
        //   signal   = warm red ("Q0Signal UP!", attention markers)
        let warm_complement = ColorRgb::new(
            255u8.saturating_sub(accent.b.saturating_sub(64)),
            255u8.saturating_sub(accent.g.saturating_sub(64)),
            255u8.saturating_sub(accent.r.saturating_sub(64)),
        );
        self.syntax_keyword = blend(accent, text, 0.35);
        self.syntax_builtin = accent;
        self.syntax_string = blend(warm_complement, text, 0.25);
        self.syntax_number = blend(text, accent, 0.45);
        self.syntax_comment = blend(self.text_dim, panel, 0.15);
        self.syntax_operator = text;
        self.syntax_identifier = text;
        self.syntax_signal = ColorRgb::new(0xE6, 0x6E, 0x6E);
        self.syntax_gutter_bg = if dark {
            scale(window, 0.75)
        } else {
            scale(panel, 0.92)
        };
        self.syntax_gutter_fg = self.text_dim;
        self.syntax_current_line = if dark {
            blend(window, accent, 0.10)
        } else {
            blend(panel, accent, 0.12)
        };

        self
    }
}

impl Theme {
    /// All built-in presets, displayed left-to-right in the Settings
    /// dialog. The first entry is the editor's classic CS3 dark.
    pub const PRESETS: &'static [BuiltinTheme] = BUILTIN_THEMES;

    pub fn from_builtin(preset: &BuiltinTheme) -> Self {
        if preset.id == q0theme::default_theme().id {
            return Theme::default();
        }
        let mut theme = Theme {
            accent: preset.accent.into(),
            panel: preset.panel.into(),
            window: preset.window.into(),
            deep_bg: preset.deep_bg.into(),
            text: preset.text.into(),
            text_dim: preset.text_dim.into(),
            stroke_dark: preset.stroke_dark.into(),
            ..Theme::default()
        }
        .with_timeline_from_base();

        if preset.id == Q0S_SIGNATURE_ID {
            theme.canvas_bg = ColorRgb::new(0x09, 0x09, 0x09);
            theme.stage_border = ColorRgb::new(0xC8, 0x10, 0x2E);
            theme.symbol_edit_mask = ColorRgb::new(0x12, 0x02, 0x06);
            theme.library_preview_light = ColorRgb::new(0xF4, 0xE8, 0xEB);
            theme.library_preview_dark = ColorRgb::new(0x17, 0x03, 0x08);
            theme.timeline_header = ColorRgb::new(0x12, 0x04, 0x07);
            theme.timeline_grid = ColorRgb::new(0x2A, 0x0B, 0x10);
            theme.timeline_grid_5 = ColorRgb::new(0x50, 0x0D, 0x18);
            theme.playhead = theme.accent;
            theme.keyframe = ColorRgb::new(0xFF, 0xF0, 0xF3);
            theme.extension = ColorRgb::new(0x4A, 0x1A, 0x22);
            theme.extension_active = ColorRgb::new(0x82, 0x16, 0x2A);
            theme.tween_fill = ColorRgb::new(0x3A, 0x09, 0x13);
            theme.tween_arrow = ColorRgb::new(0xFF, 0x6B, 0x7F);
            theme.empty_inside = ColorRgb::new(0x1C, 0x1C, 0x1C);
            theme.empty_inside_active = ColorRgb::new(0x30, 0x0A, 0x12);
            theme.empty_beyond = ColorRgb::new(0x08, 0x08, 0x08);
            theme.empty_beyond_5 = ColorRgb::new(0x18, 0x05, 0x08);
            theme.syntax_keyword = ColorRgb::new(0xFF, 0x33, 0x55);
            theme.syntax_builtin = ColorRgb::new(0xFF, 0x6B, 0x7F);
            theme.syntax_string = ColorRgb::new(0xFF, 0xC0, 0x66);
            theme.syntax_number = ColorRgb::new(0xFF, 0x8A, 0x9D);
            theme.syntax_comment = ColorRgb::new(0x77, 0x55, 0x5B);
            theme.syntax_operator = ColorRgb::new(0xFF, 0xF0, 0xF3);
            theme.syntax_identifier = ColorRgb::new(0xF4, 0xF4, 0xF4);
            theme.syntax_signal = ColorRgb::new(0xFF, 0x33, 0x55);
            theme.syntax_gutter_bg = ColorRgb::new(0x08, 0x08, 0x08);
            theme.syntax_gutter_fg = ColorRgb::new(0x99, 0x55, 0x61);
            theme.syntax_current_line = ColorRgb::new(0x20, 0x05, 0x0B);
        }
        theme
    }

    /// Push the theme into egui's `Visuals` so every widget — buttons,
    /// frames, scrollbars, popups — picks up the new colours next frame.
    /// Timeline-specific colours stay hardcoded in `theme.rs` constants;
    /// only the global widget chrome flexes here.
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

fn mid_blend(a: Color32, b: Color32) -> Color32 {
    Color32::from_rgb(
        ((a.r() as u16 + b.r() as u16) / 2) as u8,
        ((a.g() as u16 + b.g() as u16) / 2) as u8,
        ((a.b() as u16 + b.b() as u16) / 2) as u8,
    )
}

// ---------------- .q7s persistence ----------------

const Q7S_MAGIC: &str = "q7s";
const Q7S_KIND: &str = "theme";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Q7sFile {
    magic: String,
    version: u8,
    kind: String,
    name: String,
    theme: Theme,
}

impl Theme {
    pub fn write_q7s(&self, path: &Path, name: &str) -> std::io::Result<()> {
        let file = Q7sFile {
            magic: Q7S_MAGIC.to_string(),
            version: 1,
            kind: Q7S_KIND.to_string(),
            name: name.to_string(),
            theme: self.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::file_io::write_bytes_atomic(path, &bytes)
    }

    pub fn read_q7s(path: &Path) -> std::io::Result<(String, Theme)> {
        let bytes = read_limited(path, MAX_THEME_BYTES, "theme")?;
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

// ---------------- Editor settings ----------------

/// Brush cap shape stored as a serializable copy. We keep a separate
/// type rather than serde-ing `q0s_format::geom::CapShape` directly so
/// q0s-format stays serde-free for downstream callers that don't want
/// the dep.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BrushCap {
    Round,
    Butt,
}

impl From<BrushCap> for CapShape {
    fn from(b: BrushCap) -> Self {
        match b {
            BrushCap::Round => CapShape::Round,
            BrushCap::Butt => CapShape::Butt,
        }
    }
}
impl From<CapShape> for BrushCap {
    fn from(c: CapShape) -> Self {
        match c {
            CapShape::Round => BrushCap::Round,
            CapShape::Butt => BrushCap::Butt,
        }
    }
}

/// Wrap an f32 in a serde-friendly newtype that hashes / Eqs reasonably so
/// `Settings` can stay `PartialEq`. Floats inside settings are bounded
/// (font size 8..=72) so the float-Eq pitfall is fine here.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PersistFontSize(pub f32);

impl PartialEq for PersistFontSize {
    fn eq(&self, other: &Self) -> bool {
        (self.0 - other.0).abs() < 1e-3
    }
}
impl Eq for PersistFontSize {}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColorRgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl From<Rgba> for ColorRgba {
    fn from(color: Rgba) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        }
    }
}

impl From<ColorRgba> for Rgba {
    fn from(color: ColorRgba) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PersistBrushSize(pub f32);

impl PartialEq for PersistBrushSize {
    fn eq(&self, other: &Self) -> bool {
        (self.0 - other.0).abs() < 1e-3
    }
}

impl Eq for PersistBrushSize {}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum PersistBrushNib {
    #[default]
    Circle,
    Square,
    Horizontal,
    Vertical,
    Slash,
    Backslash,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BrushPreferences {
    pub color: ColorRgba,
    pub size: PersistBrushSize,
    pub smoothing: u8,
    pub nib: PersistBrushNib,
    pub scale_with_stage: bool,
    pub sync_with_eraser: bool,
}

impl Default for BrushPreferences {
    fn default() -> Self {
        Self::from_runtime(BrushSettings::default())
    }
}

impl BrushPreferences {
    pub fn from_runtime(settings: BrushSettings) -> Self {
        Self {
            color: settings.color.into(),
            size: PersistBrushSize(settings.size),
            smoothing: settings.smoothing,
            nib: match settings.nib {
                BrushNib::Circle => PersistBrushNib::Circle,
                BrushNib::Square => PersistBrushNib::Square,
                BrushNib::Horizontal => PersistBrushNib::Horizontal,
                BrushNib::Vertical => PersistBrushNib::Vertical,
                BrushNib::Slash => PersistBrushNib::Slash,
                BrushNib::Backslash => PersistBrushNib::Backslash,
            },
            scale_with_stage: settings.scale_with_stage,
            sync_with_eraser: settings.sync_with_eraser,
        }
    }

    pub fn to_runtime(self) -> BrushSettings {
        let size = if self.size.0.is_finite() {
            self.size.0.clamp(0.1, 512.0)
        } else {
            BrushSettings::default().size
        };
        BrushSettings {
            color: self.color.into(),
            size,
            smoothing: self.smoothing.min(100),
            nib: match self.nib {
                PersistBrushNib::Circle => BrushNib::Circle,
                PersistBrushNib::Square => BrushNib::Square,
                PersistBrushNib::Horizontal => BrushNib::Horizontal,
                PersistBrushNib::Vertical => BrushNib::Vertical,
                PersistBrushNib::Slash => BrushNib::Slash,
                PersistBrushNib::Backslash => BrushNib::Backslash,
            },
            scale_with_stage: self.scale_with_stage,
            sync_with_eraser: self.sync_with_eraser,
        }
    }

    pub fn update_from_runtime(&mut self, settings: BrushSettings) {
        *self = Self::from_runtime(settings);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    #[serde(default)]
    pub language: Language,
    pub theme: Theme,
    pub brush_cap: BrushCap,
    #[serde(default)]
    pub brush: BrushPreferences,
    /// User-created easing curves shared by every project.
    #[serde(default)]
    pub easing_presets: Vec<EasingPreset>,
    /// Font name used inside the q0lang script editor. Resolved via
    /// `q0lang::fonts::resolve` at install time; if the system can't find
    /// it, falls back to bundled monospace and warns once in status.
    #[serde(default = "default_q0lang_font_name")]
    pub q0lang_font_name: String,
    #[serde(default = "default_q0lang_font_size")]
    pub q0lang_font_size: PersistFontSize,
    /// Visible-line-numbers toggle, tab width in spaces.
    #[serde(default = "default_q0lang_show_line_numbers")]
    pub q0lang_show_line_numbers: bool,
    #[serde(default = "default_q0lang_tab_width")]
    pub q0lang_tab_width: u8,
    /// Most recently opened projects, newest first.
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
    /// Last directory chosen in q0enc. Shared by all export formats so the
    /// next dialog opens where the user was actually working.
    #[serde(default)]
    pub last_export_directory: Option<PathBuf>,
    /// PNG sequence ergonomics preference.
    #[serde(default = "default_png_sequence_create_subfolder")]
    pub png_sequence_create_subfolder: bool,
}

fn default_q0lang_font_name() -> String {
    "Consolas".to_string()
}
fn default_q0lang_font_size() -> PersistFontSize {
    PersistFontSize(16.0)
}
fn default_q0lang_show_line_numbers() -> bool {
    true
}
fn default_q0lang_tab_width() -> u8 {
    4
}
fn default_png_sequence_create_subfolder() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::English,
            theme: Theme::default(),
            brush_cap: BrushCap::Round,
            brush: BrushPreferences::default(),
            easing_presets: Vec::new(),
            q0lang_font_name: default_q0lang_font_name(),
            q0lang_font_size: default_q0lang_font_size(),
            q0lang_show_line_numbers: default_q0lang_show_line_numbers(),
            q0lang_tab_width: default_q0lang_tab_width(),
            recent_projects: Vec::new(),
            last_export_directory: None,
            png_sequence_create_subfolder: default_png_sequence_create_subfolder(),
        }
    }
}

fn migrate_legacy_timeline_focus_color(theme: &mut Theme) {
    const LEGACY_FIXED_PLAYHEAD: ColorRgb = ColorRgb::new(0xCC, 0x33, 0x33);
    if theme.playhead == LEGACY_FIXED_PLAYHEAD {
        theme.playhead = theme.accent;
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut settings = match Self::path()
            .and_then(|path| read_limited(&path, MAX_SETTINGS_BYTES, "settings").ok())
        {
            Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            None => Self::default(),
        };
        // Migrate the original pre-beta defaults without requiring users to
        // find and delete their settings file before trying a new build.
        if settings.theme.accent == ColorRgb::new(0x0F, 0x84, 0xCE) {
            settings.theme.accent = ColorRgb::new(0xC8, 0x10, 0x2E);
        }
        migrate_legacy_timeline_focus_color(&mut settings.theme);
        if settings.q0lang_font_name == "Impact" {
            settings.q0lang_font_name = default_q0lang_font_name();
        }
        if settings.q0lang_font_name.trim().is_empty() {
            settings.q0lang_font_name = default_q0lang_font_name();
        }
        if !settings.q0lang_font_size.0.is_finite() {
            settings.q0lang_font_size = default_q0lang_font_size();
        }
        settings.q0lang_font_size.0 = settings.q0lang_font_size.0.clamp(8.0, 72.0);
        settings.q0lang_tab_width = settings.q0lang_tab_width.clamp(1, 8);
        crate::easing::sanitize_library(&mut settings.easing_presets);
        settings.prune_recent();
        settings
    }

    pub fn save(&self) {
        let _ = self.try_save();
    }

    pub fn try_save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "settings directory is unavailable",
            ));
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        crate::file_io::write_bytes_atomic(&path, &bytes)
    }

    pub fn path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("q0editor").join("settings.json"))
    }

    pub fn push_recent(&mut self, path: &Path) {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.recent_projects.retain(|item| item != &canonical);
        self.recent_projects.insert(0, canonical);
        self.recent_projects.truncate(10);
    }

    pub fn prune_recent(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.recent_projects
            .retain(|path| path.is_file() && seen.insert(path.clone()));
        self.recent_projects.truncate(10);
    }
}

fn read_limited(path: &Path, max_bytes: u64, label: &str) -> std::io::Result<Vec<u8>> {
    let size = std::fs::metadata(path)?.len();
    if size > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{label} file is too large ({size} bytes; limit is {max_bytes})"),
        ));
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() as u64 > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{label} file grew beyond the {max_bytes}-byte limit while it was being read"),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_fields_use_current_defaults() {
        let settings: Settings = serde_json::from_str("{}").expect("deserialize old settings");
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn symbol_edit_mask_round_trips_and_defaults_for_old_themes() {
        let theme = Theme {
            symbol_edit_mask: ColorRgb::new(7, 8, 9),
            library_preview_light: ColorRgb::new(230, 220, 210),
            library_preview_dark: ColorRgb::new(10, 20, 30),
            ..Theme::default()
        };
        let encoded = serde_json::to_string(&theme).expect("serialize theme");
        let decoded: Theme = serde_json::from_str(&encoded).expect("deserialize theme");
        assert_eq!(decoded.symbol_edit_mask, ColorRgb::new(7, 8, 9));
        assert_eq!(decoded.library_preview_light, ColorRgb::new(230, 220, 210));
        assert_eq!(decoded.library_preview_dark, ColorRgb::new(10, 20, 30));

        let mut value = serde_json::to_value(Theme::default()).expect("theme json");
        let object = value.as_object_mut().expect("theme object");
        object.remove("symbol_edit_mask");
        object.remove("library_preview_light");
        object.remove("library_preview_dark");
        let old: Theme = serde_json::from_value(value).expect("old q7s theme");
        assert_eq!(old.symbol_edit_mask, default_symbol_edit_mask());
        assert_eq!(old.library_preview_light, default_library_preview_light());
        assert_eq!(old.library_preview_dark, default_library_preview_dark());
    }

    #[test]
    fn builtins_follow_the_shared_theme_catalogue() {
        assert_eq!(Theme::PRESETS, q0theme::BUILTIN_THEMES);
        for preset in Theme::PRESETS {
            let theme = Theme::from_builtin(preset);
            assert_eq!(theme.accent, preset.accent.into(), "{} accent", preset.name);
            assert_eq!(theme.playhead, theme.accent, "{} playhead", preset.name);
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
        }
    }

    #[test]
    fn legacy_fixed_red_playhead_migrates_to_the_current_theme_accent() {
        let mut theme = Theme {
            accent: ColorRgb::new(0x26, 0x8B, 0xD2),
            playhead: ColorRgb::new(0xCC, 0x33, 0x33),
            ..Theme::default()
        };

        migrate_legacy_timeline_focus_color(&mut theme);
        assert_eq!(theme.playhead, theme.accent);

        let custom = ColorRgb::new(0x12, 0x34, 0x56);
        theme.playhead = custom;
        migrate_legacy_timeline_focus_color(&mut theme);
        assert_eq!(theme.playhead, custom);
    }

    #[test]
    fn brush_preferences_round_trip_and_bound_untrusted_values() {
        let runtime = BrushSettings {
            color: Rgba {
                r: 12,
                g: 34,
                b: 56,
                a: 78,
            },
            size: 27.5,
            smoothing: 83,
            nib: BrushNib::Backslash,
            scale_with_stage: false,
            sync_with_eraser: false,
        };
        let encoded = serde_json::to_string(&BrushPreferences::from_runtime(runtime))
            .expect("serialize brush settings");
        let decoded: BrushPreferences =
            serde_json::from_str(&encoded).expect("deserialize brush settings");
        assert_eq!(decoded.to_runtime(), runtime);

        for nib in BrushNib::ALL {
            let runtime = BrushSettings { nib, ..runtime };
            let encoded = serde_json::to_string(&BrushPreferences::from_runtime(runtime))
                .expect("serialize every nib");
            let decoded: BrushPreferences =
                serde_json::from_str(&encoded).expect("deserialize every nib");
            assert_eq!(decoded.to_runtime().nib, nib);
        }

        let unsafe_values: BrushPreferences = serde_json::from_str(
            r#"{
                "color":{"r":1,"g":2,"b":3,"a":4},
                "size":null,
                "smoothing":255,
                "nib":"Circle",
                "scale_with_stage":true,
                "sync_with_eraser":true
            }"#,
        )
        .unwrap_or_else(|_| BrushPreferences {
            size: PersistBrushSize(f32::NAN),
            smoothing: 255,
            ..BrushPreferences::default()
        });
        let bounded = unsafe_values.to_runtime();
        assert!(bounded.size.is_finite());
        assert!((0.1..=512.0).contains(&bounded.size));
        assert_eq!(bounded.smoothing, 100);
    }

    #[test]
    fn easing_preset_library_survives_settings_roundtrip() {
        let mut settings = Settings::default();
        settings.easing_presets.push(EasingPreset::from_curve(
            "impact bounce",
            crate::easing::CubicCurve {
                x1: 0.15,
                y1: -0.75,
                x2: 0.82,
                y2: 1.5,
            },
        ));
        let bytes = serde_json::to_vec(&settings).expect("serialize settings");
        let decoded: Settings = serde_json::from_slice(&bytes).expect("deserialize settings");
        assert_eq!(decoded.easing_presets, settings.easing_presets);
    }

    #[test]
    fn recent_projects_are_deduplicated_and_capped() {
        let mut settings = Settings::default();
        for index in 0..12 {
            settings.push_recent(Path::new(&format!("project-{index}.q1s")));
        }
        settings.push_recent(Path::new("project-5.q1s"));

        assert_eq!(settings.recent_projects.len(), 10);
        assert!(settings.recent_projects[0].ends_with("project-5.q1s"));
        assert_eq!(
            settings
                .recent_projects
                .iter()
                .filter(|path| path.ends_with("project-5.q1s"))
                .count(),
            1
        );
    }
}
