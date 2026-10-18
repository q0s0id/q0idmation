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

pub(crate) fn outline_installed_font(
    face_name: &str,
    text: &str,
    size: f32,
) -> Result<Vec<VectorPath>, String> {
    let (bytes, index) = crate::fonts::load(face_name)
        .ok_or_else(|| format!("Font '{face_name}' is not installed"))?;
    let face =
        Face::parse(&bytes, index).map_err(|_| format!("Font '{face_name}' is unreadable"))?;
    let scale = size / f32::from(face.units_per_em());
    let mut builder = GlyphPaths {
        paths: Vec::new(),
        current: Vec::new(),
        x: 0.0,
        baseline: size,
        scale,
    };
    let mut previous_glyph = None;
    for character in text.chars() {
        if character == '\r' {
            continue;
        }
        if character == '\n' {
            builder.x = 0.0;
            builder.baseline += size * 1.25;
            previous_glyph = None;
            continue;
        }
        if let Some(glyph) = face.glyph_index(character) {
            if let (Some(previous), Some(kern)) = (previous_glyph, face.tables().kern) {
                for subtable in kern.subtables {
                    if subtable.horizontal
                        && !subtable.variable
                        && !subtable.has_cross_stream
                        && !subtable.has_state_machine
                    {
                        builder.x +=
                            f32::from(subtable.glyphs_kerning(previous, glyph).unwrap_or(0))
                                * scale;
                    }
                }
            }
            face.outline_glyph(glyph, &mut builder);
            previous_glyph = Some(glyph);
            builder.x += f32::from(
                face.glyph_hor_advance(glyph)
                    .unwrap_or(face.units_per_em() / 2),
            ) * scale;
        } else {
            previous_glyph = None;
            builder.x += size * 0.5;
        }
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
}
