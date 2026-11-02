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

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

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
    add_script_fallbacks(&mut defs);
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

/// System fonts covering scripts egui's own fonts lack, file and face
/// index: Chinese, Japanese and Korean names in imported FLA files, and
/// symbols. Windows first, then Android's system fonts.
const SCRIPT_FALLBACKS: [(&str, u32); 7] = [
    ("msyh.ttc", 0),
    ("YuGothR.ttc", 0),
    ("malgun.ttf", 0),
    ("seguisym.ttf", 0),
    ("/system/fonts/NotoSansCJK-Regular.ttc", 0),
    ("/system/fonts/NotoSansSymbols-Regular-Subsetted.ttf", 0),
    ("/system/fonts/DroidSansFallback.ttf", 0),
];

/// egui 0.27 clones owned font bytes both when it keeps the definitions and
/// when it parses them. The large CJK collections must instead have one stable
/// backing store. This bounded cache lives as long as egui's borrowed slices.
static SCRIPT_FALLBACK_DATA: [OnceLock<Option<FallbackFontBytes>>; SCRIPT_FALLBACKS.len()] =
    [const { OnceLock::new() }; SCRIPT_FALLBACKS.len()];

enum FallbackFontBytes {
    #[cfg(windows)]
    Mapped {
        bytes: memmap2::Mmap,
        // Keep the share-mode guard alive as long as the borrowed mapping.
        _file: std::fs::File,
    },
    Owned(Box<[u8]>),
}

impl FallbackFontBytes {
    fn load(path: &Path) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            // FILE_SHARE_READ permits other readers but prevents writes and
            // deletion for the entire lifetime of the mapping and its handle.
            if let Ok(file) = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(path)
            {
                // SAFETY: the retained file handle denies modification and
                // truncation. The mapping is read-only and is never dropped
                // while egui can borrow it from SCRIPT_FALLBACK_DATA.
                if let Ok(bytes) = unsafe { memmap2::Mmap::map(&file) } {
                    return Ok(Self::Mapped { bytes, _file: file });
                }
            }
        }
        // Platforms without mappings, or a file that cannot be mapped, still
        // read once; cloning FontData never duplicates this shared backing.
        std::fs::read(path).map(|bytes| Self::Owned(bytes.into_boxed_slice()))
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            #[cfg(windows)]
            Self::Mapped { bytes, .. } => bytes,
            Self::Owned(bytes) => bytes,
        }
    }
}

fn script_fallback_bytes(slot: usize) -> Option<&'static [u8]> {
    SCRIPT_FALLBACK_DATA[slot]
        .get_or_init(|| {
            let path = Path::new(SCRIPT_FALLBACKS[slot].0);
            if path.is_absolute() {
                FallbackFontBytes::load(path).ok()
            } else {
                font_dirs()
                    .iter()
                    .find_map(|dir| FallbackFontBytes::load(&dir.join(path)).ok())
            }
        })
        .as_ref()
        .map(FallbackFontBytes::as_slice)
}

/// Puts the installed fonts of [`SCRIPT_FALLBACKS`] at the end of every
/// family, so characters the UI fonts lack still show instead of boxes.
pub fn add_script_fallbacks(defs: &mut FontDefinitions) {
    for (slot, (file, index)) in SCRIPT_FALLBACKS.into_iter().enumerate() {
        let Some(bytes) = script_fallback_bytes(slot) else {
            continue;
        };
        let key = format!("q0_fallback_{file}");
        let mut data = FontData::from_static(bytes);
        data.index = index;
        defs.font_data.insert(key.clone(), data);
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            defs.families.entry(family).or_default().push(key.clone());
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_fallbacks_share_bytes_across_contexts_and_definition_clones() {
        let mut first = FontDefinitions::default();
        add_script_fallbacks(&mut first);
        let copy = first.clone();
        let mut second = FontDefinitions::default();
        add_script_fallbacks(&mut second);
        for (key, data) in &first.font_data {
            if !key.starts_with("q0_fallback_") {
                continue;
            }
            assert!(matches!(data.font, std::borrow::Cow::Borrowed(_)));
            assert!(std::ptr::eq(
                data.font.as_ref(),
                copy.font_data[key].font.as_ref()
            ));
            assert!(std::ptr::eq(
                data.font.as_ref(),
                second.font_data[key].font.as_ref()
            ));
        }
    }

    #[test]
    fn shared_script_fallbacks_keep_their_glyph_coverage() {
        let ctx = Context::default();
        install(&ctx, "Consolas");
        let _ = ctx.run(Default::default(), |ctx| {
            ctx.fonts(|fonts| {
                for (slot, (_, index)) in SCRIPT_FALLBACKS.into_iter().enumerate() {
                    let Some(bytes) = script_fallback_bytes(slot) else {
                        continue;
                    };
                    let face = ttf_parser::Face::parse(bytes, index).expect("system font");
                    for ch in "Привет中文日本語한국어♠".chars() {
                        if face.glyph_index(ch).is_some() {
                            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                                assert!(fonts.has_glyph(&egui::FontId::new(13.0, family), ch));
                            }
                        }
                    }
                }
            });
        });
    }
}
