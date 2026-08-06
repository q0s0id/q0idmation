use std::path::Path;

use image::GenericImageView;
use q0s_format::v2::BitmapAsset;

const MAX_ENCODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DECODED_PIXELS: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum BitmapImportError {
    Io(std::io::Error),
    Decode(image::ImageError),
    Empty,
    EncodedTooLarge(u64),
    DimensionsTooLarge(u32, u32),
    PixelBudgetExceeded(u64),
}

impl std::fmt::Display for BitmapImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Decode(error) => write!(f, "image decode error: {error}"),
            Self::Empty => write!(f, "image has zero width or height"),
            Self::EncodedTooLarge(size) => write!(
                f,
                "image file is too large ({size} bytes; limit is {MAX_ENCODED_BYTES})"
            ),
            Self::DimensionsTooLarge(width, height) => write!(
                f,
                "image dimensions {width}x{height} exceed the bitmap format limit"
            ),
            Self::PixelBudgetExceeded(pixels) => write!(
                f,
                "decoded image is too large ({pixels} pixels; limit is {MAX_DECODED_PIXELS})"
            ),
        }
    }
}

impl From<std::io::Error> for BitmapImportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<image::ImageError> for BitmapImportError {
    fn from(value: image::ImageError) -> Self {
        Self::Decode(value)
    }
}

pub fn is_supported_bitmap_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("png")
                || extension.eq_ignore_ascii_case("jpg")
                || extension.eq_ignore_ascii_case("jpeg")
                || extension.eq_ignore_ascii_case("webp")
        })
}

pub fn decode_bitmap_path(path: &Path, asset_id: u16) -> Result<BitmapAsset, BitmapImportError> {
    let encoded_size = std::fs::metadata(path)?.len();
    if encoded_size > MAX_ENCODED_BYTES {
        return Err(BitmapImportError::EncodedTooLarge(encoded_size));
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() as u64 > MAX_ENCODED_BYTES {
        return Err(BitmapImportError::EncodedTooLarge(bytes.len() as u64));
    }
    decode_bitmap_bytes(&bytes, asset_id)
}

pub fn decode_bitmap_bytes(bytes: &[u8], asset_id: u16) -> Result<BitmapAsset, BitmapImportError> {
    let image = image::load_from_memory(bytes)?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Err(BitmapImportError::Empty);
    }
    if width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return Err(BitmapImportError::DimensionsTooLarge(width, height));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_DECODED_PIXELS {
        return Err(BitmapImportError::PixelBudgetExceeded(pixels));
    }
    let rgba = image.into_rgba8().into_raw();
    Ok(BitmapAsset {
        asset_id,
        width: width as u16,
        height: height as u16,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, RgbaImage};

    #[test]
    fn decodes_png_to_embedded_rgba_bitmap() {
        let source =
            RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 128]).expect("test image");
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(source.as_raw(), 2, 1, image::ColorType::Rgba8)
            .expect("encode png");

        let bitmap = decode_bitmap_bytes(&png, 42).expect("decode png");
        assert_eq!(bitmap.asset_id, 42);
        assert_eq!((bitmap.width, bitmap.height), (2, 1));
        assert_eq!(bitmap.rgba, source.into_raw());
    }

    #[test]
    fn recognizes_supported_bitmap_extensions_case_insensitively() {
        for name in ["a.png", "b.JPG", "c.jpeg", "d.WeBp"] {
            assert!(is_supported_bitmap_path(Path::new(name)), "{name}");
        }
        assert!(!is_supported_bitmap_path(Path::new("movie.q1s")));
    }
}
