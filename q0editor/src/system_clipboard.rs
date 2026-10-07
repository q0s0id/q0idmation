#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardBitmap {
    /// Encoded PNG or BMP bytes, retaining alpha when supplied by the clipboard.
    pub encoded: Vec<u8>,
}

#[cfg(windows)]
pub(crate) fn read_bitmap() -> Result<Option<ClipboardBitmap>, String> {
    use clipboard_win::{formats, get, is_format_avail, register_format, Clipboard};

    let png_formats = ["PNG", "image/png"].map(register_format);
    let _clipboard = Clipboard::new_attempts(10)
        .map_err(|error| format!("clipboard image open failed: {error}"))?;
    let mut last_error = None;

    // Prefer the original PNG to Windows' synthesized, often opaque bitmap.
    for format in png_formats.into_iter().flatten() {
        if !is_format_avail(format.get()) {
            continue;
        }
        let result = read_encoded_format(format.get()).and_then(|encoded| {
            if !encoded.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Err("clipboard PNG has an invalid signature".to_owned());
            }
            validate_bitmap_payload(encoded)
        });
        match result {
            Ok(bitmap) => return Ok(Some(bitmap)),
            Err(error) => last_error = Some(error),
        }
    }

    // DIBV5 carries alpha masks; CF_BITMAP goes through GDI and loses alpha.
    for format in [formats::CF_DIBV5, formats::CF_DIB] {
        if !is_format_avail(format) {
            continue;
        }
        let result = read_encoded_format(format)
            .and_then(dib_to_bmp)
            .and_then(validate_bitmap_payload);
        match result {
            Ok(bitmap) => return Ok(Some(bitmap)),
            Err(error) => last_error = Some(error),
        }
    }

    if is_format_avail(formats::CF_BITMAP) {
        let result = get(formats::Bitmap)
            .map_err(|error| format!("clipboard bitmap read failed: {error}"))
            .and_then(validate_bitmap_payload);
        match result {
            Ok(bitmap) => return Ok(Some(bitmap)),
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

#[cfg(windows)]
const MAX_CLIPBOARD_BYTES: usize = 64 * 1024 * 1024;

#[cfg(windows)]
fn read_encoded_format(format: u32) -> Result<Vec<u8>, String> {
    if clipboard_win::size(format).is_some_and(|size| size.get() > MAX_CLIPBOARD_BYTES) {
        return Err("clipboard image exceeds the 64 MiB limit".to_owned());
    }
    clipboard_win::get(clipboard_win::formats::RawData(format))
        .map_err(|error| format!("clipboard image read failed: {error}"))
}

#[cfg(windows)]
fn validate_bitmap_payload(encoded: Vec<u8>) -> Result<ClipboardBitmap, String> {
    if encoded.is_empty() || encoded.len() > MAX_CLIPBOARD_BYTES {
        return Err("clipboard image is empty or exceeds the 64 MiB limit".to_owned());
    }
    let (width, height) = image::io::Reader::new(std::io::Cursor::new(&encoded))
        .with_guessed_format()
        .map_err(|error| format!("clipboard image format read failed: {error}"))?
        .into_dimensions()
        .map_err(|error| format!("clipboard image header read failed: {error}"))?;
    if width == 0
        || height == 0
        || width > u32::from(u16::MAX)
        || height > u32::from(u16::MAX)
        || u64::from(width) * u64::from(height) > 16 * 1024 * 1024
    {
        return Err("clipboard image exceeds the bitmap dimensions limit".to_owned());
    }
    Ok(ClipboardBitmap { encoded })
}

#[cfg(windows)]
fn dib_to_bmp(mut dib: Vec<u8>) -> Result<Vec<u8>, String> {
    let read_u32 = |offset: usize| -> Result<u32, String> {
        dib.get(offset..offset + 4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .ok_or_else(|| "clipboard DIB header is truncated".to_owned())
    };
    let header_size = read_u32(0)? as usize;
    if !matches!(header_size, 12 | 40 | 52 | 56 | 108 | 124) || dib.len() < header_size {
        return Err("clipboard DIB has an unsupported or truncated header".to_owned());
    }
    let bits_offset = if header_size == 12 { 10 } else { 14 };
    let bit_count = u16::from_le_bytes(dib[bits_offset..bits_offset + 2].try_into().unwrap());
    let compression = if header_size == 12 { 0 } else { read_u32(16)? };
    let colors_used = if header_size == 12 {
        0
    } else {
        read_u32(32)? as usize
    };
    let palette_entries = if colors_used != 0 {
        colors_used
    } else if bit_count <= 8 {
        1usize << bit_count
    } else {
        0
    };
    let palette_entry_size = if header_size == 12 { 3 } else { 4 };
    let external_masks = if header_size == 40 {
        match compression {
            3 => 12,
            6 => 16,
            _ => 0,
        }
    } else {
        0
    };
    let pixel_offset = palette_entries
        .checked_mul(palette_entry_size)
        .and_then(|size| size.checked_add(header_size + external_masks))
        .filter(|offset| *offset < dib.len())
        .ok_or_else(|| "clipboard DIB pixel offset is invalid".to_owned())?;

    // Some browsers declare BI_RGB but explicitly supply an alpha mask in
    // their V4/V5 header. Use that mask instead of treating alpha as padding.
    if header_size >= 56 && bit_count == 32 && compression == 0 && read_u32(52)? == 0xff000000 {
        let default_rgb_masks = read_u32(40)? == 0 && read_u32(44)? == 0 && read_u32(48)? == 0;
        dib[16..20].copy_from_slice(&3u32.to_le_bytes());
        if default_rgb_masks {
            for (offset, mask) in [(40, 0x00ff0000u32), (44, 0x0000ff00), (48, 0x000000ff)] {
                dib[offset..offset + 4].copy_from_slice(&mask.to_le_bytes());
            }
        }
    }

    // Add only the file header: keep the original pixels, palette and alpha
    // masks intact, with no GDI conversion or extra image encoding.
    let file_size = dib.len() + 14;
    if file_size > MAX_CLIPBOARD_BYTES {
        return Err("clipboard image exceeds the 64 MiB limit".to_owned());
    }
    let mut encoded = Vec::with_capacity(file_size);
    encoded.extend_from_slice(b"BM");
    encoded.extend_from_slice(&(file_size as u32).to_le_bytes());
    encoded.extend_from_slice(&[0; 4]);
    encoded.extend_from_slice(&((pixel_offset + 14) as u32).to_le_bytes());
    encoded.extend_from_slice(&dib);
    Ok(encoded)
}

#[cfg(not(windows))]
pub(crate) fn read_bitmap() -> Result<Option<ClipboardBitmap>, String> {
    Ok(None)
}

#[cfg(windows)]
pub(crate) fn ctrl_v_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL};

    const VK_V: i32 = 0x56;
    // GetAsyncKeyState's high bit is set while the physical key is down.
    unsafe {
        (GetAsyncKeyState(i32::from(VK_CONTROL)) as u16 & 0x8000) != 0
            && (GetAsyncKeyState(VK_V) as u16 & 0x8000) != 0
    }
}

#[cfg(not(windows))]
pub(crate) fn ctrl_v_down() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_bitmap_keeps_encoded_payload_verbatim() {
        let bitmap = ClipboardBitmap {
            encoded: vec![b'B', b'M', 1, 2, 3, 4],
        };
        assert_eq!(bitmap.encoded, vec![b'B', b'M', 1, 2, 3, 4]);
    }
}
