//! Shared built-in UI palettes for q0editor and q0player.
//! App-specific colours are derived by each frontend from this common base.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinTheme {
    pub id: &'static str,
    pub name: &'static str,
    pub accent: Rgb,
    pub panel: Rgb,
    pub window: Rgb,
    pub deep_bg: Rgb,
    pub text: Rgb,
    pub text_dim: Rgb,
    pub stroke_dark: Rgb,
    /// q0player-only background behind transparent movie pixels.
    pub viewport_bg: Rgb,
}

pub const Q0S_SIGNATURE_ID: &str = "q0s-signature";

pub const BUILTIN_THEMES: &[BuiltinTheme] = &[
    BuiltinTheme {
        id: "dark",
        name: "Dark (default)",
        accent: Rgb::new(0xC8, 0x10, 0x2E),
        panel: Rgb::new(0x3C, 0x3C, 0x3C),
        window: Rgb::new(0x2F, 0x2F, 0x2F),
        deep_bg: Rgb::new(0x22, 0x22, 0x22),
        text: Rgb::new(0xE6, 0xE6, 0xE6),
        text_dim: Rgb::new(0x9F, 0x9F, 0x9F),
        stroke_dark: Rgb::new(0x20, 0x20, 0x20),
        viewport_bg: Rgb::new(0x18, 0x18, 0x18),
    },
    BuiltinTheme {
        id: Q0S_SIGNATURE_ID,
        name: "Q0S Signature",
        accent: Rgb::new(0xC8, 0x10, 0x2E),
        panel: Rgb::new(0x14, 0x14, 0x14),
        window: Rgb::new(0x0E, 0x0E, 0x0E),
        deep_bg: Rgb::new(0x05, 0x05, 0x05),
        text: Rgb::new(0xF4, 0xF4, 0xF4),
        text_dim: Rgb::new(0xA6, 0x8A, 0x90),
        stroke_dark: Rgb::new(0x00, 0x00, 0x00),
        viewport_bg: Rgb::new(0x09, 0x09, 0x09),
    },
    BuiltinTheme {
        id: "light",
        name: "Light",
        accent: Rgb::new(0x0F, 0x84, 0xCE),
        panel: Rgb::new(0xEC, 0xEC, 0xEC),
        window: Rgb::new(0xF6, 0xF6, 0xF6),
        deep_bg: Rgb::new(0xFF, 0xFF, 0xFF),
        text: Rgb::new(0x18, 0x18, 0x18),
        text_dim: Rgb::new(0x60, 0x60, 0x60),
        stroke_dark: Rgb::new(0xC0, 0xC0, 0xC0),
        viewport_bg: Rgb::new(0xDA, 0xDA, 0xDA),
    },
    BuiltinTheme {
        id: "solarized-dark",
        name: "Solarized Dark",
        accent: Rgb::new(0x26, 0x8B, 0xD2),
        panel: Rgb::new(0x07, 0x36, 0x42),
        window: Rgb::new(0x00, 0x2B, 0x36),
        deep_bg: Rgb::new(0x00, 0x21, 0x2B),
        text: Rgb::new(0x93, 0xA1, 0xA1),
        text_dim: Rgb::new(0x58, 0x6E, 0x75),
        stroke_dark: Rgb::new(0x00, 0x1B, 0x22),
        viewport_bg: Rgb::new(0xFD, 0xF6, 0xE3),
    },
    BuiltinTheme {
        id: "solarized-light",
        name: "Solarized Light",
        accent: Rgb::new(0x26, 0x8B, 0xD2),
        panel: Rgb::new(0xEE, 0xE8, 0xD5),
        window: Rgb::new(0xFD, 0xF6, 0xE3),
        deep_bg: Rgb::new(0xFF, 0xFD, 0xF0),
        text: Rgb::new(0x65, 0x7B, 0x83),
        text_dim: Rgb::new(0x93, 0xA1, 0xA1),
        stroke_dark: Rgb::new(0xC8, 0xC0, 0xAA),
        viewport_bg: Rgb::new(0xFD, 0xF6, 0xE3),
    },
    BuiltinTheme {
        id: "dracula",
        name: "Dracula",
        accent: Rgb::new(0xBD, 0x93, 0xF9),
        panel: Rgb::new(0x33, 0x35, 0x42),
        window: Rgb::new(0x28, 0x2A, 0x36),
        deep_bg: Rgb::new(0x1E, 0x1F, 0x29),
        text: Rgb::new(0xF8, 0xF8, 0xF2),
        text_dim: Rgb::new(0x6B, 0x6F, 0x82),
        stroke_dark: Rgb::new(0x16, 0x17, 0x20),
        viewport_bg: Rgb::new(0x14, 0x15, 0x1B),
    },
    BuiltinTheme {
        id: "nord",
        name: "Nord",
        accent: Rgb::new(0x88, 0xC0, 0xD0),
        panel: Rgb::new(0x3B, 0x42, 0x52),
        window: Rgb::new(0x2E, 0x34, 0x40),
        deep_bg: Rgb::new(0x29, 0x2E, 0x39),
        text: Rgb::new(0xEC, 0xEF, 0xF4),
        text_dim: Rgb::new(0x81, 0x8C, 0x9F),
        stroke_dark: Rgb::new(0x1E, 0x22, 0x2A),
        viewport_bg: Rgb::new(0x1B, 0x1E, 0x25),
    },
    BuiltinTheme {
        id: "gruvbox",
        name: "Gruvbox",
        accent: Rgb::new(0xFA, 0xBD, 0x2F),
        panel: Rgb::new(0x3C, 0x38, 0x36),
        window: Rgb::new(0x28, 0x28, 0x28),
        deep_bg: Rgb::new(0x1D, 0x20, 0x21),
        text: Rgb::new(0xEB, 0xDB, 0xB2),
        text_dim: Rgb::new(0xA8, 0x99, 0x84),
        stroke_dark: Rgb::new(0x16, 0x18, 0x18),
        viewport_bg: Rgb::new(0x14, 0x16, 0x16),
    },
    BuiltinTheme {
        id: "mocha",
        name: "Mocha",
        accent: Rgb::new(0xE0, 0x8E, 0x45),
        panel: Rgb::new(0x3A, 0x2D, 0x25),
        window: Rgb::new(0x2A, 0x20, 0x1A),
        deep_bg: Rgb::new(0x1F, 0x18, 0x12),
        text: Rgb::new(0xF0, 0xE2, 0xCC),
        text_dim: Rgb::new(0xA0, 0x8E, 0x76),
        stroke_dark: Rgb::new(0x18, 0x10, 0x0C),
        viewport_bg: Rgb::new(0x14, 0x0E, 0x0A),
    },
    BuiltinTheme {
        id: "forest",
        name: "Forest",
        accent: Rgb::new(0x9E, 0xC9, 0x55),
        panel: Rgb::new(0x29, 0x36, 0x2D),
        window: Rgb::new(0x1A, 0x24, 0x1F),
        deep_bg: Rgb::new(0x12, 0x1A, 0x16),
        text: Rgb::new(0xDE, 0xE7, 0xC9),
        text_dim: Rgb::new(0x82, 0x97, 0x83),
        stroke_dark: Rgb::new(0x0C, 0x12, 0x0F),
        viewport_bg: Rgb::new(0x08, 0x0D, 0x0A),
    },
    BuiltinTheme {
        id: "sunset",
        name: "Sunset",
        accent: Rgb::new(0xFF, 0x6B, 0x6B),
        panel: Rgb::new(0x3D, 0x2B, 0x3A),
        window: Rgb::new(0x2A, 0x1B, 0x2A),
        deep_bg: Rgb::new(0x1F, 0x12, 0x1F),
        text: Rgb::new(0xFF, 0xE3, 0xCC),
        text_dim: Rgb::new(0xC2, 0x90, 0x9D),
        stroke_dark: Rgb::new(0x18, 0x0D, 0x18),
        viewport_bg: Rgb::new(0x12, 0x09, 0x12),
    },
    BuiltinTheme {
        id: "cobalt",
        name: "Cobalt",
        accent: Rgb::new(0xFF, 0x9D, 0x00),
        panel: Rgb::new(0x12, 0x35, 0x4F),
        window: Rgb::new(0x09, 0x24, 0x3D),
        deep_bg: Rgb::new(0x06, 0x1B, 0x2C),
        text: Rgb::new(0xFF, 0xFF, 0xFF),
        text_dim: Rgb::new(0x7F, 0xA0, 0xC2),
        stroke_dark: Rgb::new(0x03, 0x12, 0x1F),
        viewport_bg: Rgb::new(0x00, 0x0E, 0x18),
    },
    BuiltinTheme {
        id: "monokai",
        name: "Monokai",
        accent: Rgb::new(0xA6, 0xE2, 0x2E),
        panel: Rgb::new(0x3E, 0x3D, 0x32),
        window: Rgb::new(0x27, 0x28, 0x22),
        deep_bg: Rgb::new(0x1E, 0x1F, 0x1C),
        text: Rgb::new(0xF8, 0xF8, 0xF2),
        text_dim: Rgb::new(0x75, 0x71, 0x5E),
        stroke_dark: Rgb::new(0x14, 0x14, 0x12),
        viewport_bg: Rgb::new(0x12, 0x12, 0x10),
    },
    BuiltinTheme {
        id: "high-contrast",
        name: "High contrast",
        accent: Rgb::new(0xFF, 0xCC, 0x00),
        panel: Rgb::new(0x1A, 0x1A, 0x1A),
        window: Rgb::new(0x0E, 0x0E, 0x0E),
        deep_bg: Rgb::new(0x05, 0x05, 0x05),
        text: Rgb::new(0xFF, 0xFF, 0xFF),
        text_dim: Rgb::new(0xC0, 0xC0, 0xC0),
        stroke_dark: Rgb::new(0x00, 0x00, 0x00),
        viewport_bg: Rgb::new(0x00, 0x00, 0x00),
    },
];

pub fn default_theme() -> &'static BuiltinTheme {
    &BUILTIN_THEMES[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn builtin_theme_ids_and_names_are_unique() {
        let ids: BTreeSet<_> = BUILTIN_THEMES.iter().map(|theme| theme.id).collect();
        let names: BTreeSet<_> = BUILTIN_THEMES.iter().map(|theme| theme.name).collect();
        assert_eq!(ids.len(), BUILTIN_THEMES.len());
        assert_eq!(names.len(), BUILTIN_THEMES.len());
    }

    #[test]
    fn default_theme_is_first() {
        assert_eq!(default_theme(), &BUILTIN_THEMES[0]);
        assert_eq!(default_theme().name, "Dark (default)");
    }
}
