//! Installed outline fonts, including per-user fonts and collection faces.
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

#[derive(Debug, Clone)]
pub(crate) struct InstalledFont {
    pub name: String,
    pub path: PathBuf,
    pub index: u32,
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
        if let Some(home) = dirs::home_dir() {
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
                fonts.push(InstalledFont {
                    name,
                    path: path.clone(),
                    index,
                });
            }
        }
    }
    fonts.sort_by_cached_key(|f| f.name.to_lowercase());
    fonts.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    fonts
}

pub(crate) fn installed() -> Option<&'static [InstalledFont]> {
    static FONTS: OnceLock<Arc<OnceLock<Vec<InstalledFont>>>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let fonts = Arc::new(OnceLock::new());
            let result = Arc::clone(&fonts);
            std::thread::spawn(move || {
                let _ = result.set(scan());
            });
            fonts
        })
        .get()
        .map(Vec::as_slice)
}

pub(crate) fn find(name: &str) -> Option<&'static InstalledFont> {
    installed()?
        .iter()
        .find(|font| font.name.eq_ignore_ascii_case(name))
}
