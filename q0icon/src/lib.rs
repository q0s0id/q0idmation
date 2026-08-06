#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn decode_ico_rgba(bytes: &[u8], preferred_size: u32) -> Result<IconImage, String> {
    if bytes.len() < 6 {
        return Err("ICO header is truncated".to_owned());
    }
    if read_u16(bytes, 0)? != 0 || read_u16(bytes, 2)? != 1 {
        return Err("file is not a Windows icon".to_owned());
    }
    let count = usize::from(read_u16(bytes, 4)?);
    if count == 0 || bytes.len() < 6 + count * 16 {
        return Err("ICO directory is truncated or empty".to_owned());
    }

    let mut candidates = Vec::new();
    for index in 0..count {
        let base = 6 + index * 16;
        let width = directory_dimension(bytes[base]);
        let height = directory_dimension(bytes[base + 1]);
        let bit_count = read_u16(bytes, base + 6)?;
        let data_len = usize::try_from(read_u32(bytes, base + 8)?)
            .map_err(|_| "ICO image length does not fit this platform")?;
        let data_offset = usize::try_from(read_u32(bytes, base + 12)?)
            .map_err(|_| "ICO image offset does not fit this platform")?;
        let end = data_offset
            .checked_add(data_len)
            .ok_or_else(|| "ICO image range overflows".to_owned())?;
        if end > bytes.len() || width != height || bit_count != 32 {
            continue;
        }
        candidates.push((width, data_offset, data_len));
    }

    candidates.sort_by_key(|(size, _, _)| {
        let exact_penalty = u32::from(*size != preferred_size);
        (
            exact_penalty,
            preferred_size.abs_diff(*size),
            std::cmp::Reverse(*size),
        )
    });
    let (directory_size, data_offset, data_len) = candidates
        .into_iter()
        .next()
        .ok_or_else(|| "ICO has no square 32-bit bitmap image".to_owned())?;

    decode_bitmap_icon(&bytes[data_offset..data_offset + data_len], directory_size)
}

fn decode_bitmap_icon(data: &[u8], directory_size: u32) -> Result<IconImage, String> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("PNG-compressed ICO entries are not supported".to_owned());
    }
    if data.len() < 40 {
        return Err("ICO bitmap header is truncated".to_owned());
    }
    let header_size =
        usize::try_from(read_u32(data, 0)?).map_err(|_| "ICO bitmap header is too large")?;
    if header_size < 40 || header_size > data.len() {
        return Err("ICO bitmap header size is invalid".to_owned());
    }

    let width_signed = read_i32(data, 4)?;
    let doubled_height = read_i32(data, 8)?;
    let planes = read_u16(data, 12)?;
    let bit_count = read_u16(data, 14)?;
    let compression = read_u32(data, 16)?;
    if width_signed <= 0
        || doubled_height == 0
        || planes != 1
        || bit_count != 32
        || compression != 0
    {
        return Err("ICO entry is not an uncompressed 32-bit bitmap".to_owned());
    }

    let width = u32::try_from(width_signed).map_err(|_| "ICO width is invalid")?;
    let height_abs = doubled_height.unsigned_abs();
    if height_abs % 2 != 0 {
        return Err("ICO bitmap height does not include an even mask height".to_owned());
    }
    let height = height_abs / 2;
    if width != directory_size || height != directory_size {
        return Err("ICO directory dimensions do not match bitmap dimensions".to_owned());
    }

    let row_bytes = usize::try_from(width)
        .map_err(|_| "ICO width is too large")?
        .checked_mul(4)
        .ok_or_else(|| "ICO row size overflows".to_owned())?;
    let pixel_bytes = row_bytes
        .checked_mul(usize::try_from(height).map_err(|_| "ICO height is too large")?)
        .ok_or_else(|| "ICO pixel size overflows".to_owned())?;
    let pixel_end = header_size
        .checked_add(pixel_bytes)
        .ok_or_else(|| "ICO pixel range overflows".to_owned())?;
    if pixel_end > data.len() {
        return Err("ICO bitmap pixels are truncated".to_owned());
    }

    let top_down = doubled_height < 0;
    let mut rgba = vec![0; pixel_bytes];
    for output_y in 0..usize::try_from(height).map_err(|_| "ICO height is too large")? {
        let source_y = if top_down {
            output_y
        } else {
            usize::try_from(height).map_err(|_| "ICO height is too large")? - 1 - output_y
        };
        let source_row = header_size + source_y * row_bytes;
        let output_row = output_y * row_bytes;
        for x in 0..usize::try_from(width).map_err(|_| "ICO width is too large")? {
            let source = source_row + x * 4;
            let output = output_row + x * 4;
            rgba[output] = data[source + 2];
            rgba[output + 1] = data[source + 1];
            rgba[output + 2] = data[source];
            rgba[output + 3] = data[source + 3];
        }
    }

    Ok(IconImage {
        width,
        height,
        rgba,
    })
}

fn directory_dimension(value: u8) -> u32 {
    if value == 0 {
        256
    } else {
        u32::from(value)
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "binary value is truncated".to_owned())?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "binary value is truncated".to_owned())?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, String> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "binary value is truncated".to_owned())?;
    Ok(i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_both_generated_application_icons_at_runtime_size() {
        let editor = decode_ico_rgba(include_bytes!("../../installer/assets/q0editor.ico"), 48)
            .expect("editor icon must decode");
        let player = decode_ico_rgba(include_bytes!("../../installer/assets/q0player.ico"), 48)
            .expect("player icon must decode");

        assert_eq!((editor.width, editor.height), (48, 48));
        assert_eq!((player.width, player.height), (48, 48));
        assert_eq!(editor.rgba.len(), 48 * 48 * 4);
        assert_eq!(player.rgba.len(), 48 * 48 * 4);
        assert!(editor.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
        assert!(player.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
        assert_ne!(editor.rgba, player.rgba);
    }

    #[test]
    fn rejects_truncated_icon_data() {
        assert!(decode_ico_rgba(&[0, 0, 1], 48).is_err());
    }
}
