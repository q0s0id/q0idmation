//! MIME types for the extensions the editor's dialogs name. Android's
//! document picker filters by type, not by extension.

/// The types that make the picker show just these extensions, or none, which
/// shows every file, when one of them has no type of its own.
pub(crate) fn mime_types(extensions: &[String]) -> Vec<&'static str> {
    let mut types = Vec::new();
    for extension in extensions {
        let Some(mime_type) = mime_type(extension) else {
            return Vec::new();
        };
        if !types.contains(&mime_type) {
            types.push(mime_type);
        }
    }
    types
}

pub(crate) fn mime_type(extension: &str) -> Option<&'static str> {
    Some(match extension.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "mp4" => "video/mp4",
        "zip" => "application/zip",
        // Audio apps disagree on these, so any audio will do.
        "wav" | "mp3" | "ogg" | "flac" => "audio/*",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extensions(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn known_extensions_narrow_the_picker_once_per_type() {
        assert_eq!(
            mime_types(&extensions(&["png", "jpg", "JPEG", "webp"])),
            ["image/png", "image/jpeg", "image/webp"]
        );
        assert_eq!(mime_types(&extensions(&["wav", "mp3"])), ["audio/*"]);
    }

    #[test]
    fn one_extension_without_a_type_shows_every_file() {
        // .q1s and .fla have no registered type, so filtering would hide them.
        assert!(mime_types(&extensions(&["q1s", "fla"])).is_empty());
        assert!(mime_types(&extensions(&["png", "q0v"])).is_empty());
        assert!(mime_types(&[]).is_empty());
    }
}
