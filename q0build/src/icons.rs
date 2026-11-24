//! The app icon: any PNG made square and sized for each place it shows.

use image::{imageops::FilterType, DynamicImage, ImageEncoder, RgbaImage};

/// Sizes written into .ico files and exe icon groups.
pub const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

pub struct AppIcon {
    /// Square, 256 × 256.
    pub image: RgbaImage,
    /// Things worth knowing about the source picture.
    pub notes: Vec<String>,
}

impl AppIcon {
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        let source = image::load_from_memory(bytes)
            .map_err(|error| format!("the icon is not a readable image: {error}"))?;
        let (width, height) = (source.width(), source.height());
        let mut notes = Vec::new();
        if width != height {
            notes.push(format!(
                "{width} × {height} is not square: it is centred on a clear square"
            ));
        }
        if width.max(height) < 256 {
            notes.push(format!(
                "{}px is smaller than 256: large icons will look soft",
                width.max(height)
            ));
        }
        let side = width.max(height);
        let mut square = RgbaImage::new(side, side);
        image::imageops::overlay(
            &mut square,
            &source.to_rgba8(),
            i64::from((side - width) / 2),
            i64::from((side - height) / 2),
        );
        let image = image::imageops::resize(&square, 256, 256, FilterType::Lanczos3);
        Ok(Self { image, notes })
    }

    pub fn sized(&self, side: u32) -> RgbaImage {
        if side == 256 {
            self.image.clone()
        } else {
            image::imageops::resize(&self.image, side, side, FilterType::Lanczos3)
        }
    }

    pub fn png(&self, side: u32) -> Vec<u8> {
        encode_png(&self.sized(side))
    }

    /// A Windows .ico with every size, each stored as PNG.
    pub fn ico(&self) -> Vec<u8> {
        let images: Vec<(u32, Vec<u8>)> = ICO_SIZES
            .iter()
            .map(|&side| (side, self.png(side)))
            .collect();
        let mut out = Vec::new();
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&1_u16.to_le_bytes());
        out.extend_from_slice(&(images.len() as u16).to_le_bytes());
        let mut offset = 6 + 16 * images.len() as u32;
        for (side, png) in &images {
            out.push(if *side >= 256 { 0 } else { *side as u8 });
            out.push(if *side >= 256 { 0 } else { *side as u8 });
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&1_u16.to_le_bytes());
            out.extend_from_slice(&32_u16.to_le_bytes());
            out.extend_from_slice(&(png.len() as u32).to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            offset += png.len() as u32;
        }
        for (_, png) in images {
            out.extend_from_slice(&png);
        }
        out
    }

    /// Pixels for an egui texture.
    pub fn color_image(&self, side: u32) -> egui::ColorImage {
        let sized = self.sized(side);
        egui::ColorImage::from_rgba_unmultiplied([side as usize, side as usize], sized.as_raw())
    }
}

pub fn encode_png(image: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ColorType::Rgba8,
        )
        .expect("PNG encodes to memory");
    out
}

/// A plain icon when the profile has none: the app's initial on the
/// splash colour, so no app ships with q0player's face by accident.
pub fn placeholder(name: &str, background: [u8; 3]) -> AppIcon {
    let mut image = RgbaImage::from_pixel(
        256,
        256,
        image::Rgba([background[0], background[1], background[2], 255]),
    );
    // A lighter rounded plate in the middle.
    for y in 0..256_u32 {
        for x in 0..256_u32 {
            let (dx, dy) = (
                (x as f32 - 127.5).abs() - 70.0,
                (y as f32 - 127.5).abs() - 70.0,
            );
            let outside = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - 30.0;
            if outside < 0.0 {
                let pixel = image.get_pixel_mut(x, y);
                for channel in &mut pixel.0[..3] {
                    *channel = channel.saturating_add(46);
                }
            }
        }
    }
    let _ = name;
    AppIcon {
        image: DynamicImage::ImageRgba8(image).to_rgba8(),
        notes: vec!["no icon chosen: a plain one is used".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ico_lists_every_size_and_its_images() {
        let icon = placeholder("x", [20, 40, 80]);
        let ico = icon.ico();
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        assert_eq!(
            u16::from_le_bytes([ico[4], ico[5]]) as usize,
            ICO_SIZES.len()
        );
        let last = 6 + 16 * (ICO_SIZES.len() - 1);
        let size = u32::from_le_bytes(ico[last + 8..last + 12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(ico[last + 12..last + 16].try_into().unwrap()) as usize;
        assert_eq!(offset + size, ico.len());
        assert_eq!(&ico[offset + 1..offset + 4], b"PNG");
    }

    #[test]
    fn a_wide_picture_is_centred_on_a_square() {
        let wide = RgbaImage::from_pixel(64, 32, image::Rgba([255, 0, 0, 255]));
        let icon = AppIcon::load(&encode_png(&wide)).unwrap();
        assert_eq!(icon.image.dimensions(), (256, 256));
        assert_eq!(icon.image.get_pixel(128, 2).0[3], 0, "clear above");
        assert_eq!(
            icon.image.get_pixel(128, 128).0[0],
            255,
            "red in the middle"
        );
        assert_eq!(icon.notes.len(), 2);
    }
}
