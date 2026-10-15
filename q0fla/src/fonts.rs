//! Installed outline fonts, including per-user fonts and collection faces.
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::OnceLock,
};

#[derive(Debug, Clone)]
pub struct InstalledFont {
    pub name: String,
    pub path: PathBuf,
    pub index: u32,
    aliases: Vec<String>,
}

fn font_files() -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        let system =
            PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
                .join("Fonts");
        let user = std::env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("Microsoft/Windows/Fonts"));
        dirs.push(system.clone());
        dirs.extend(user.clone());
        // Fonts installed by reference can live outside the standard directories.
        for hive in [
            winreg::enums::HKEY_LOCAL_MACHINE,
            winreg::enums::HKEY_CURRENT_USER,
        ] {
            if let Ok(key) = winreg::RegKey::predef(hive)
                .open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Fonts")
            {
                for (name, _) in key.enum_values().flatten() {
                    if let Ok(value) = key.get_value::<String, _>(&name) {
                        let path = PathBuf::from(value);
                        if path.is_absolute() {
                            files.insert(path);
                        } else {
                            files.insert(system.join(&path));
                            if let Some(user) = &user {
                                files.insert(user.join(path));
                            }
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        dirs.extend(
            [
                "/usr/share/fonts",
                "/usr/local/share/fonts",
                "/System/Library/Fonts",
                "/Library/Fonts",
            ]
            .map(PathBuf::from),
        );
        // Android's own families, plus what the device maker adds. Fonts
        // there are system-wide; apps cannot install their own.
        #[cfg(target_os = "android")]
        dirs.extend(["/system/fonts", "/product/fonts", "/system_ext/fonts"].map(PathBuf::from));
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            dirs.extend([
                home.join(".fonts"),
                home.join(".local/share/fonts"),
                home.join("Library/Fonts"),
            ]);
        }
    }
    while let Some(dir) = dirs.pop() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    dirs.push(entry.path());
                } else if kind.is_file() {
                    files.insert(entry.path());
                }
            }
        }
    }
    files.retain(|path| {
        path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc" | "otc"
            )
        })
    });
    files
}

fn scan() -> Vec<InstalledFont> {
    let mut fonts = Vec::new();
    for path in font_files() {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        for index in 0..ttf_parser::fonts_in_collection(&bytes).unwrap_or(1) {
            let Ok(face) = ttf_parser::Face::parse(&bytes, index) else {
                continue;
            };
            let names: Vec<_> = face
                .names()
                .into_iter()
                .filter(|n| n.name_id == ttf_parser::name_id::FULL_NAME)
                .collect();
            let name = names
                .iter()
                .find(|n| n.language_id == 0x409)
                .and_then(|n| n.to_string())
                .or_else(|| names.iter().find_map(|n| n.to_string()));
            if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
                // Animate stores PostScript face names (e.g. MicrosoftTaiLe),
                // while Windows pickers display full names with spaces.
                // Keep all localized/full/PostScript aliases of this face.
                let mut aliases: Vec<String> = face.names().into_iter()
                    .filter(|n| matches!(n.name_id, ttf_parser::name_id::FULL_NAME | ttf_parser::name_id::POST_SCRIPT_NAME))
                    .filter_map(|n| n.to_string())
                    .map(|n| normalized_name(&n))
                    .collect();
                let regular = face.names().into_iter()
                    .filter(|n| matches!(n.name_id, ttf_parser::name_id::SUBFAMILY | ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY))
                    .filter_map(|n| n.to_string())
                    .any(|n| matches!(n.to_lowercase().as_str(), "regular" | "normal" | "roman" | "book"));
                if regular {
                    aliases.extend(face.names().into_iter()
                        .filter(|n| matches!(n.name_id, ttf_parser::name_id::FAMILY | ttf_parser::name_id::TYPOGRAPHIC_FAMILY))
                        .filter_map(|n| n.to_string())
                        .map(|n| normalized_name(&n)));
                }
                aliases.sort();
                aliases.dedup();
                fonts.push(InstalledFont {
                    name,
                    path: path.clone(),
                    index,
                    aliases,
                });
            }
        }
    }
    fonts.sort_by_cached_key(|f| f.name.to_lowercase());
    fonts.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    fonts
}

pub fn installed() -> &'static [InstalledFont] {
    static FONTS: OnceLock<Vec<InstalledFont>> = OnceLock::new();
    FONTS.get_or_init(scan)
}

fn normalized_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(char::to_lowercase).collect()
}

pub fn find(name: &str) -> Option<&'static InstalledFont> {
    let fonts = installed();
    fonts.iter().find(|font| font.name.eq_ignore_ascii_case(name.trim()))
        .or_else(|| {
            let key = normalized_name(name);
            fonts.iter().find(|font| font.aliases.contains(&key))
        })
}

pub fn load(name: &str) -> Option<(Vec<u8>, u32)> {
    let font = find(name)?;
    std::fs::read(&font.path).ok().map(|bytes| (bytes, font.index))
}

pub fn default_name() -> Option<&'static str> {
    ["Segoe UI", "Arial", "Tahoma", "DejaVu Sans", "Liberation Sans", "Roboto"].iter()
        .find_map(|name| find(name)).or_else(|| installed().first())
        .map(|font| font.name.as_str())
}
