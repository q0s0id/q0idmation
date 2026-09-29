#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardBitmap {
    /// Complete BMP file bytes reconstructed from the Windows CF_BITMAP handle.
    pub encoded: Vec<u8>,
}

#[cfg(windows)]
pub(crate) fn read_bitmap() -> Result<Option<ClipboardBitmap>, String> {
    use clipboard_win::{formats, get_clipboard, is_format_avail};

    if !is_format_avail(formats::CF_BITMAP) {
        return Ok(None);
    }
    let encoded: Vec<u8> = get_clipboard(formats::Bitmap)
        .map_err(|error| format!("clipboard image read failed: {error}"))?;
    if encoded.is_empty() {
        return Ok(None);
    }
    Ok(Some(ClipboardBitmap { encoded }))
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
