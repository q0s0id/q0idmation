//! Linux AppImage: the AppImage type-2 runtime, then a squashfs image of an
//! AppDir holding the Linux q0player with the app's bundle appended.

use crate::squashfs::Image;

/// The program name inside the AppImage: `com.q0s.bulletbox` runs as
/// `bulletbox`.
pub fn program_name(manifest: &q0qir::Manifest) -> String {
    manifest
        .app
        .id
        .rsplit('.')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("app")
        .to_string()
}

pub fn desktop_entry(manifest: &q0qir::Manifest, category: &str) -> String {
    let app = &manifest.app;
    let program = program_name(manifest);
    let mut entry = format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={program}\nIcon={}\nCategories={};\nTerminal=false\nX-AppImage-Version={}\n",
        one_line(&app.name),
        app.id,
        one_line(category),
        one_line(&app.version)
    );
    if !app.description.trim().is_empty() {
        entry.push_str(&format!("Comment={}\n", one_line(&app.description)));
    }
    entry
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

/// The AppImage's bytes. `player` is the Linux q0player with the bundle
/// already appended; `runtime` the type-2 AppImage runtime.
pub fn build(
    player: &[u8],
    manifest: &q0qir::Manifest,
    icon_png: &[u8],
    category: &str,
    runtime: &[u8],
) -> Result<Vec<u8>, String> {
    if runtime.get(..4) != Some(b"\x7fELF".as_slice())
        || runtime.get(8..11) != Some(b"AI\x02".as_slice())
    {
        return Err("the AppImage runtime is not a type-2 AppImage runtime".into());
    }
    if player.get(..4) != Some(b"\x7fELF".as_slice()) {
        return Err("the Linux runtime is not a Linux program (no ELF header)".into());
    }
    let program = program_name(manifest);
    let id = &manifest.app.id;
    let app_run = format!(
        "#!/bin/sh\nHERE=\"$(dirname \"$(readlink -f \"$0\")\")\"\nexec \"$HERE/usr/bin/{program}\" \"$@\"\n"
    );
    let desktop = desktop_entry(manifest, category);
    let mut image = Image::default();
    image.add("AppRun", app_run.into_bytes(), 0o755);
    image.add(&format!("usr/bin/{program}"), player.to_vec(), 0o755);
    image.add(
        &format!("{id}.desktop"),
        desktop.clone().into_bytes(),
        0o644,
    );
    image.add(
        &format!("usr/share/applications/{id}.desktop"),
        desktop.into_bytes(),
        0o644,
    );
    image.add(&format!("{id}.png"), icon_png.to_vec(), 0o644);
    image.add(".DirIcon", icon_png.to_vec(), 0o644);
    image.add(
        &format!("usr/share/icons/hicolor/256x256/apps/{id}.png"),
        icon_png.to_vec(),
        0o644,
    );
    // A fixed time keeps the same inputs giving the same AppImage.
    let squashfs = image.write(1_700_000_000);

    let mut out = runtime.to_vec();
    out.extend_from_slice(&squashfs);
    Ok(out)
}

/// The bundle inside an AppImage made here: the image is uncompressed, so
/// the player and its bundle sit in it byte for byte.
pub fn find_bundle(appimage: &[u8]) -> Result<q0qir::Bundle, q0qir::Error> {
    let mut search_end = appimage.len();
    while let Some(at) = appimage[..search_end]
        .windows(q0qir::MAGIC.len())
        .rposition(|window| window == q0qir::MAGIC)
    {
        if let Ok(bundle) = q0qir::Bundle::decode(
            &appimage[..(at + q0qir::TRAILER_LEN).min(appimage.len())],
            true,
        ) {
            return Ok(bundle);
        }
        search_end = at;
    }
    Err(q0qir::Error::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_runtime() -> Vec<u8> {
        let mut runtime = b"\x7fELF\x02\x01\x01\x00AI\x02".to_vec();
        runtime.resize(4096, 0);
        runtime
    }

    #[test]
    fn the_image_holds_the_app_dir_after_the_runtime() {
        let mut manifest = q0qir::Manifest::default();
        manifest.app.name = "Bullet Box".into();
        manifest.app.id = "com.q0s.bulletbox".into();
        let mut player = b"\x7fELF linux q0player".to_vec();
        player.extend(
            q0qir::Bundle {
                manifest: manifest.clone(),
                icon_png: None,
                movie: vec![7; 1000],
            }
            .encode(),
        );
        let runtime = fake_runtime();
        let appimage = build(&player, &manifest, b"\x89PNG icon", "Game", &runtime).unwrap();
        assert_eq!(&appimage[..runtime.len()], &runtime[..]);
        assert_eq!(&appimage[runtime.len()..runtime.len() + 4], b"hsqs");
        let bundle = find_bundle(&appimage).expect("bundle inside the image");
        assert_eq!(bundle.movie, vec![7; 1000]);
    }

    #[test]
    fn the_desktop_entry_names_the_program_and_icon() {
        let mut manifest = q0qir::Manifest::default();
        manifest.app.id = "ru.shadowmol.grunt".into();
        manifest.app.name = "Grunt\nRevenge".into();
        let entry = desktop_entry(&manifest, "Game");
        assert!(entry.contains("Exec=grunt\n"));
        assert!(entry.contains("Icon=ru.shadowmol.grunt\n"));
        assert!(entry.contains("Name=Grunt Revenge\n"));
    }

    #[test]
    fn a_wrong_runtime_is_refused() {
        let manifest = q0qir::Manifest::default();
        assert!(build(b"\x7fELF", &manifest, b"", "Game", b"MZ windows program").is_err());
    }
}
