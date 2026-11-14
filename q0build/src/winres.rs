//! The icon and file details inside a Windows exe: what Explorer, the
//! taskbar and Task Manager show. The runtime's own (q0player's) are taken
//! out and the app's put in. This rewrites the file, so it runs before the
//! bundle is appended.

use crate::icons::{AppIcon, ICO_SIZES};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;

/// What goes into the exe's file details.
pub struct FileInfo<'a> {
    pub app: &'a q0qir::AppInfo,
    pub file_name: &'a str,
}

/// The VS_VERSIONINFO resource for `info`.
pub fn version_resource(info: &FileInfo) -> Vec<u8> {
    let numbers: Vec<u16> = info
        .app
        .version
        .split('.')
        .map(|part| part.parse::<u16>().unwrap_or(0))
        .chain(std::iter::repeat(0))
        .take(4)
        .collect();
    let ms = (u32::from(numbers[0]) << 16) | u32::from(numbers[1]);
    let ls = (u32::from(numbers[2]) << 16) | u32::from(numbers[3]);
    let mut fixed = Vec::with_capacity(52);
    for value in [
        0xFEEF_04BD_u32, // signature
        0x0001_0000,     // structure version
        ms,
        ls,
        ms,
        ls,
        0x3F,        // flags mask
        0,           // flags
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0,
        0,
        0,
    ] {
        fixed.extend_from_slice(&value.to_le_bytes());
    }
    let app = info.app;
    let description = if app.description.trim().is_empty() {
        app.name.as_str()
    } else {
        app.description.as_str()
    };
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", app.author.as_str()),
        ("FileDescription", description),
        ("FileVersion", app.version.as_str()),
        ("InternalName", app.id.as_str()),
        ("LegalCopyright", app.copyright.as_str()),
        ("OriginalFilename", info.file_name),
        ("ProductName", app.name.as_str()),
        ("ProductVersion", app.version.as_str()),
        ("Comments", "Built with Q0Build (q0idmation QIR)"),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(key, value)| {
        let text = utf16z(value);
        block(key, &bytes_of(&text), text.len() as u16, true, &[])
    })
    .collect();
    let table = block("040904B0", &[], 0, true, &strings);
    let string_info = block("StringFileInfo", &[], 0, true, &[table]);
    let translation = block("Translation", &[0x09, 0x04, 0xB0, 0x04], 4, false, &[]);
    let var_info = block("VarFileInfo", &[], 0, true, &[translation]);
    block(
        "VS_VERSION_INFO",
        &fixed,
        52,
        false,
        &[string_info, var_info],
    )
}

/// One version-resource block: header, key, value, children, each child
/// starting on four bytes.
fn block(key: &str, value: &[u8], value_length: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0, 0];
    out.extend_from_slice(&value_length.to_le_bytes());
    out.extend_from_slice(&u16::from(text).to_le_bytes());
    out.extend_from_slice(&bytes_of(&utf16z(key)));
    pad4(&mut out);
    out.extend_from_slice(value);
    for child in children {
        pad4(&mut out);
        out.extend_from_slice(child);
    }
    let length = out.len() as u16;
    out[..2].copy_from_slice(&length.to_le_bytes());
    out
}

fn pad4(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

fn utf16z(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn bytes_of(words: &[u16]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// RT_ICON images (PNG, ids 1..) and the RT_GROUP_ICON that lists them.
pub fn icon_resources(icon: &AppIcon) -> (Vec<(u16, Vec<u8>)>, Vec<u8>) {
    let images: Vec<(u16, Vec<u8>)> = ICO_SIZES
        .iter()
        .enumerate()
        .map(|(index, &side)| (index as u16 + 1, icon.png(side)))
        .collect();
    let mut group = Vec::new();
    group.extend_from_slice(&0_u16.to_le_bytes());
    group.extend_from_slice(&1_u16.to_le_bytes());
    group.extend_from_slice(&(images.len() as u16).to_le_bytes());
    for ((id, png), side) in images.iter().zip(ICO_SIZES) {
        let side = if side >= 256 { 0 } else { side as u8 };
        group.extend_from_slice(&[side, side, 0, 0]);
        group.extend_from_slice(&1_u16.to_le_bytes());
        group.extend_from_slice(&32_u16.to_le_bytes());
        group.extend_from_slice(&(png.len() as u32).to_le_bytes());
        group.extend_from_slice(&id.to_le_bytes());
    }
    (images, group)
}

/// Replaces the exe's icon and file details in place.
#[cfg(windows)]
pub fn apply(exe: &std::path::Path, icon: &AppIcon, info: &FileInfo) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{FreeLibrary, BOOL, HMODULE};
    use windows_sys::Win32::System::LibraryLoader::{
        BeginUpdateResourceW, EndUpdateResourceW, EnumResourceLanguagesW, EnumResourceNamesW,
        LoadLibraryExW, UpdateResourceW, LOAD_LIBRARY_AS_DATAFILE,
    };

    #[derive(Clone)]
    enum Name {
        Id(u16),
        Text(Vec<u16>),
    }
    impl Name {
        fn ptr(&self) -> *const u16 {
            match self {
                Name::Id(id) => *id as usize as *const u16,
                Name::Text(text) => text.as_ptr(),
            }
        }
    }
    fn int_resource(id: u16) -> *const u16 {
        id as usize as *const u16
    }
    unsafe fn name_of(raw: *const u16) -> Name {
        if (raw as usize) >> 16 == 0 {
            Name::Id(raw as usize as u16)
        } else {
            let mut length = 0;
            while *raw.add(length) != 0 {
                length += 1;
            }
            let mut text = std::slice::from_raw_parts(raw, length + 1).to_vec();
            text[length] = 0;
            Name::Text(text)
        }
    }
    unsafe extern "system" fn on_name(
        _: HMODULE,
        _: *const u16,
        name: *const u16,
        state: isize,
    ) -> BOOL {
        let names = &mut *(state as *mut Vec<Name>);
        names.push(name_of(name));
        1
    }
    unsafe extern "system" fn on_language(
        _: HMODULE,
        _: *const u16,
        _: *const u16,
        language: u16,
        state: isize,
    ) -> BOOL {
        let languages = &mut *(state as *mut Vec<u16>);
        languages.push(language);
        1
    }

    let wide_path = utf16z(&exe.to_string_lossy());
    // What the runtime carries now, with each language it carries it in.
    let mut existing: Vec<(u16, Name, u16)> = Vec::new();
    unsafe {
        let module = LoadLibraryExW(
            wide_path.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_AS_DATAFILE,
        );
        if module.is_null() {
            return Err(format!("could not read the resources of {}", exe.display()));
        }
        for kind in [RT_ICON, RT_GROUP_ICON, RT_VERSION] {
            let mut names: Vec<Name> = Vec::new();
            EnumResourceNamesW(
                module,
                int_resource(kind),
                Some(on_name),
                &mut names as *mut Vec<Name> as isize,
            );
            for name in names {
                let mut languages: Vec<u16> = Vec::new();
                EnumResourceLanguagesW(
                    module,
                    int_resource(kind),
                    name.ptr(),
                    Some(on_language),
                    &mut languages as *mut Vec<u16> as isize,
                );
                for language in languages {
                    existing.push((kind, name.clone(), language));
                }
            }
        }
        FreeLibrary(module);
    }
    let language = existing
        .iter()
        .find(|(kind, _, _)| *kind == RT_GROUP_ICON)
        .map_or(0x0409, |(_, _, language)| *language);

    let (images, group) = icon_resources(icon);
    let version = version_resource(info);
    unsafe {
        let update = BeginUpdateResourceW(wide_path.as_ptr(), 0);
        if update.is_null() {
            return Err(format!(
                "could not open {} to change its icon",
                exe.display()
            ));
        }
        let mut ok = true;
        for (kind, name, old_language) in &existing {
            ok &= UpdateResourceW(
                update,
                int_resource(*kind),
                name.ptr(),
                *old_language,
                std::ptr::null(),
                0,
            ) != 0;
        }
        for (id, png) in &images {
            ok &= UpdateResourceW(
                update,
                int_resource(RT_ICON),
                int_resource(*id),
                language,
                png.as_ptr().cast(),
                png.len() as u32,
            ) != 0;
        }
        ok &= UpdateResourceW(
            update,
            int_resource(RT_GROUP_ICON),
            int_resource(1),
            language,
            group.as_ptr().cast(),
            group.len() as u32,
        ) != 0;
        ok &= UpdateResourceW(
            update,
            int_resource(RT_VERSION),
            int_resource(1),
            language,
            version.as_ptr().cast(),
            version.len() as u32,
        ) != 0;
        if EndUpdateResourceW(update, i32::from(!ok)) == 0 || !ok {
            return Err(format!(
                "Windows refused to change the icon and file details of {}",
                exe.display()
            ));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn apply(_exe: &std::path::Path, _icon: &AppIcon, _info: &FileInfo) -> Result<(), String> {
    Err(
        "exe icons and file details can only be written on Windows; the exe keeps q0player's"
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_resource_is_well_formed() {
        let app = q0qir::AppInfo {
            name: "Bullet Box".into(),
            version: "1.2.3".into(),
            author: "q0s".into(),
            ..q0qir::AppInfo::default()
        };
        let resource = version_resource(&FileInfo {
            app: &app,
            file_name: "Bullet-Box.exe",
        });
        assert_eq!(
            u16::from_le_bytes([resource[0], resource[1]]) as usize,
            resource.len()
        );
        assert_eq!(u16::from_le_bytes([resource[2], resource[3]]), 52);
        let key: Vec<u16> = resource[6..36]
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(String::from_utf16_lossy(&key), "VS_VERSION_INFO");
        // VS_FIXEDFILEINFO starts after the key and its padding, at 40.
        assert_eq!(&resource[40..44], &0xFEEF_04BD_u32.to_le_bytes());
        assert_eq!(&resource[48..52], &((1_u32 << 16) | 2).to_le_bytes());
        let text = String::from_utf16_lossy(
            &resource
                .chunks(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        );
        assert!(text.contains("ProductName") && text.contains("Bullet Box"));
    }

    #[test]
    fn the_icon_group_points_at_each_image() {
        let icon = crate::icons::placeholder("x", [0, 0, 0]);
        let (images, group) = icon_resources(&icon);
        assert_eq!(images.len(), ICO_SIZES.len());
        assert_eq!(group.len(), 6 + 14 * ICO_SIZES.len());
        let last = 6 + 14 * (ICO_SIZES.len() - 1);
        assert_eq!(group[last], 0, "256 is written as 0");
        assert_eq!(
            u16::from_le_bytes([group[last + 12], group[last + 13]]),
            ICO_SIZES.len() as u16
        );
    }
}
