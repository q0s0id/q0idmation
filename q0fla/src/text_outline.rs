use q0s_format::v2::{Anchor, Path as VectorPath, Vec2};
use ttf_parser::{Face, OutlineBuilder};

struct GlyphPaths {
    paths: Vec<VectorPath>,
    current: Vec<Anchor>,
    x: f32,
    baseline: f32,
    scale: f32,
}

impl GlyphPaths {
    fn point(&self, x: f32, y: f32) -> Vec2 {
        Vec2::new(self.x + x * self.scale, self.baseline - y * self.scale)
    }

    fn flush(&mut self) {
        if self.current.len() >= 2 {
            self.paths.push(VectorPath {
                anchors: std::mem::take(&mut self.current),
                closed: true,
            });
        } else {
            self.current.clear();
        }
    }
}

impl OutlineBuilder for GlyphPaths {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush();
        self.current.push(Anchor {
            point: self.point(x, y),
            in_handle: None,
            out_handle: None,
        });
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.current.push(Anchor {
            point: self.point(x, y),
            in_handle: None,
            out_handle: None,
        });
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let control = self.point(cx, cy);
        let end = self.point(x, y);
        if let Some(last) = self.current.last_mut() {
            last.out_handle = Some(Vec2::new(
                last.point.x + (control.x - last.point.x) * (2.0 / 3.0),
                last.point.y + (control.y - last.point.y) * (2.0 / 3.0),
            ));
        }
        self.current.push(Anchor {
            point: end,
            in_handle: Some(Vec2::new(
                end.x + (control.x - end.x) * (2.0 / 3.0),
                end.y + (control.y - end.y) * (2.0 / 3.0),
            )),
            out_handle: None,
        });
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let first = self.point(x1, y1);
        let second = self.point(x2, y2);
        let end = self.point(x, y);
        if let Some(last) = self.current.last_mut() {
            last.out_handle = Some(first);
        }
        self.current.push(Anchor {
            point: end,
            in_handle: Some(second),
            out_handle: None,
        });
    }

    fn close(&mut self) {
        if self.current.len() >= 3 {
            let first = self.current[0].point;
            let last = self.current.last().expect("contour endpoint");
            if (first.x - last.point.x).abs() < 0.001 && (first.y - last.point.y).abs() < 0.001 {
                let closing = self.current.pop().expect("contour endpoint");
                self.current[0].in_handle = closing.in_handle;
            }
        }
        self.flush();
    }
}

/// Font bytes by face name, loaded once per run: an FLA can hold thousands
/// of texts in a handful of fonts.
fn cached_font(name: &str) -> Option<(std::sync::Arc<Vec<u8>>, u32)> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    type Cache = Mutex<HashMap<String, Option<(Arc<Vec<u8>>, u32)>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = name.trim().to_lowercase();
    if let Some(hit) = cache.lock().ok()?.get(&key) {
        return hit.clone();
    }
    let loaded = crate::fonts::load(name).map(|(bytes, index)| (Arc::new(bytes), index));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, loaded.clone());
    }
    loaded
}

/// Faces tried, in order, for characters the text's own font lacks.
const GLYPH_FALLBACKS: [&str; 22] = [
    "Segoe UI",
    "Arial",
    "Segoe UI Symbol",
    "Segoe UI Emoji",
    "Yu Gothic",
    "Meiryo",
    "MS Gothic",
    "Microsoft YaHei",
    "SimSun",
    "Malgun Gothic",
    "Microsoft JhengHei",
    "Nirmala UI",
    "Leelawadee UI",
    "Ebrima",
    "Gadugi",
    "Arial Unicode MS",
    "Noto Sans",
    "Noto Sans CJK JP",
    "Noto Sans Symbols",
    "DejaVu Sans",
    "Droid Sans Fallback",
    "Roboto",
];

pub(crate) fn outline_installed_font(
    face_name: &str,
    text: &str,
    size: f32,
) -> Result<Vec<VectorPath>, String> {
    let (bytes, index) =
        cached_font(face_name).ok_or_else(|| format!("Font '{face_name}' is not installed"))?;
    let face =
        Face::parse(&bytes, index).map_err(|_| format!("Font '{face_name}' is unreadable"))?;
    let fallbacks = std::cell::OnceCell::<Vec<(std::sync::Arc<Vec<u8>>, u32)>>::new();
    let mut builder = GlyphPaths {
        paths: Vec::new(),
        current: Vec::new(),
        x: 0.0,
        baseline: size,
        scale: size / f32::from(face.units_per_em()),
    };
    let mut previous_glyph = None;
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        // Flash ends paragraphs with a carriage return; "\r\n" is one break.
        if character == '\r' || character == '\n' {
            if character == '\r' && characters.peek() == Some(&'\n') {
                characters.next();
            }
            builder.x = 0.0;
            builder.baseline += size * 1.25;
            previous_glyph = None;
            continue;
        }
        if character.is_control() {
            continue;
        }
        if let Some(glyph) = face.glyph_index(character) {
            builder.scale = size / f32::from(face.units_per_em());
            if let (Some(previous), Some(kern)) = (previous_glyph, face.tables().kern) {
                for subtable in kern.subtables {
                    if subtable.horizontal
                        && !subtable.variable
                        && !subtable.has_cross_stream
                        && !subtable.has_state_machine
                    {
                        builder.x +=
                            f32::from(subtable.glyphs_kerning(previous, glyph).unwrap_or(0))
                                * builder.scale;
                    }
                }
            }
            face.outline_glyph(glyph, &mut builder);
            previous_glyph = Some(glyph);
            builder.x += f32::from(
                face.glyph_hor_advance(glyph)
                    .unwrap_or(face.units_per_em() / 2),
            ) * builder.scale;
            continue;
        }
        previous_glyph = None;
        // A character the font lacks comes from the first face that has it.
        let faces = fallbacks.get_or_init(|| {
            GLYPH_FALLBACKS
                .iter()
                .filter_map(|name| cached_font(name))
                .collect()
        });
        let drawn = faces.iter().find_map(|(bytes, index)| {
            let other = Face::parse(bytes, *index).ok()?;
            let glyph = other.glyph_index(character)?;
            builder.scale = size / f32::from(other.units_per_em());
            other.outline_glyph(glyph, &mut builder);
            Some(
                f32::from(
                    other
                        .glyph_hor_advance(glyph)
                        .unwrap_or(other.units_per_em() / 2),
                ) * builder.scale,
            )
        });
        builder.x += drawn.unwrap_or(size * 0.5);
    }
    builder.flush();
    Ok(builder.paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_font_is_a_clean_error() {
        let error = outline_installed_font("q0fla definitely missing font", "x", 12.0)
            .expect_err("missing font must not silently substitute");
        assert!(error.contains("not installed"));
    }

    #[test]
    fn characters_the_font_lacks_come_from_a_fallback_face() {
        let Some(font) = crate::fonts::default_name() else {
            return;
        };
        let latin = outline_installed_font(font, "ab", 12.0).unwrap();
        // Hiragana and a circled letter, which few Latin fonts carry.
        let wide = outline_installed_font(font, "\u{304b}\u{24d2}", 12.0).unwrap();
        assert!(!latin.is_empty());
        if GLYPH_FALLBACKS[2..]
            .iter()
            .any(|name| crate::fonts::find(name).is_some())
        {
            assert!(!wide.is_empty(), "drawn from a fallback face");
        }
    }

    #[test]
    fn a_carriage_return_starts_a_new_line() {
        let Some(font) = crate::fonts::default_name() else {
            return;
        };
        let lowest = |text: &str| {
            outline_installed_font(font, text, 20.0)
                .unwrap()
                .iter()
                .flat_map(|path| path.anchors.iter())
                .fold(f32::MIN, |low, anchor| low.max(anchor.point.y))
        };
        assert!(lowest("A\rB") > lowest("AB") + 15.0);
        assert!(
            (lowest("A\r\nB") - lowest("A\rB")).abs() < 0.01,
            "CRLF is one break"
        );
    }
}
