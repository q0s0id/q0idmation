//! System font loader for the q0lang editor.
//!
//! egui uses its own font registry rather than resolving family names through
//! the operating system. This module:
//!   1. Maintains a hardcoded list of well-known Windows fonts and their
//!      typical filenames in `C:\Windows\Fonts\`.
//!   2. Reads the file from disk and registers it with egui under a
//!      stable family name (`q0lang`), so the script editor can pick it
//!      up via `FontId::new(size, FontFamily::Name("q0lang".into()))`.
//!   3. Keeps egui's built-in monospace fonts as fallbacks, so missing
//!      glyphs do not turn the editor text into boxes.
//!
//! q0s currently targets Windows. The loader still fails gracefully so
//! non-Windows builds continue to compile.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// Stable family name registered in egui.  Pick this in any `FontId` and
/// the script editor will render with the user's chosen font.
pub const Q0LANG_FAMILY: &str = "q0lang";

/// Display name → typical Windows font filename (any of these), tried in
/// order. Filenames vary by OS install (regular vs bold vs variant), so
/// each entry is a best-effort hit list.
pub fn known_fonts() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("Impact", &["impact.ttf"]),
        ("Arial", &["arial.ttf", "ariblk.ttf"]),
        ("Arial Black", &["ariblk.ttf"]),
        ("Consolas", &["consola.ttf"]),
        ("Courier New", &["cour.ttf", "couri.ttf"]),
        ("Tahoma", &["tahoma.ttf"]),
        ("Verdana", &["verdana.ttf"]),
        ("Segoe UI", &["segoeui.ttf"]),
        ("Segoe UI Mono", &["segoeuib.ttf"]),
        ("Times New Roman", &["times.ttf"]),
        ("Comic Sans MS", &["comic.ttf"]),
        ("Trebuchet MS", &["trebuc.ttf"]),
        ("Georgia", &["georgia.ttf"]),
        ("Lucida Console", &["lucon.ttf"]),
        ("Cascadia Mono", &["CascadiaMono.ttf"]),
        ("Cascadia Code", &["CascadiaCode.ttf"]),
    ]
}

/// Search paths Windows uses for fonts. Per-user fonts (no admin install)
/// land under `%LOCALAPPDATA%\Microsoft\Windows\Fonts\`.
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(windir) = std::env::var("WINDIR") {
        dirs.push(PathBuf::from(windir).join("Fonts"));
    } else {
        dirs.push(PathBuf::from(r"C:\Windows\Fonts"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(
            PathBuf::from(local)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    dirs
}

/// Try every typical filename of `display_name` against every search dir
/// and return the first hit's bytes.
pub fn load_font_bytes(display_name: &str) -> Option<Vec<u8>> {
    let entries = known_fonts();
    let candidates = entries
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(display_name))
        .map(|(_, files)| *files)
        .unwrap_or(&[]);
    let dirs = font_dirs();
    for filename in candidates {
        for dir in &dirs {
            let path = dir.join(filename);
            if let Ok(bytes) = std::fs::read(&path) {
                return Some(bytes);
            }
        }
    }
    None
}

/// Install a font by display name into the egui context under
/// `Q0LANG_FAMILY`. Replaces any previously installed q0lang font.
/// Returns `true` if the font was found and installed; `false` falls
/// back to the proportional default and the caller should surface that
/// in the status bar.
pub fn install(ctx: &Context, display_name: &str) -> bool {
    let mut defs = FontDefinitions::default();
    let mut installed_real_font = false;

    if let Some(bytes) = load_font_bytes(display_name) {
        defs.font_data
            .insert("q0lang_user".to_string(), FontData::from_owned(bytes));
        let mut chain = vec!["q0lang_user".to_string()];
        if let Some(fallbacks) = defs.families.get(&FontFamily::Monospace) {
            for font in fallbacks {
                if !chain.contains(font) {
                    chain.push(font.clone());
                }
            }
        }
        defs.families
            .insert(FontFamily::Name(Arc::from(Q0LANG_FAMILY)), chain);
        installed_real_font = true;
    } else {
        // Fallback: alias q0lang family to whatever the monospace
        // family is using, so FontId::new(size, FontFamily::Name("q0lang"))
        // still resolves to *something* renderable.
        let monospace_chain = defs
            .families
            .get(&FontFamily::Monospace)
            .cloned()
            .unwrap_or_default();
        defs.families
            .insert(FontFamily::Name(Arc::from(Q0LANG_FAMILY)), monospace_chain);
    }
    ctx.set_fonts(defs);
    installed_real_font
}

/// What's actually available on this machine — used to populate the font
/// picker so we don't offer fonts that won't render. Always includes the
/// currently selected name even if not detected, so the picker shows it.
pub fn available_fonts(currently_selected: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (display_name, files) in known_fonts() {
        let dirs = font_dirs();
        let found = files
            .iter()
            .any(|f| dirs.iter().any(|d| d.join(f).exists()));
        if found {
            out.push((*display_name).to_string());
        }
    }
    if !out
        .iter()
        .any(|s| s.eq_ignore_ascii_case(currently_selected))
    {
        out.push(currently_selected.to_string());
    }
    out.sort_by_key(|a| a.to_lowercase());
    out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    out
}
