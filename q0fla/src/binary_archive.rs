//! Structured MFC object archives used by binary FLA scene/library streams.
//! Wire-format reference: https://github.com/eddiemoore/fla-decoder
//! See THIRD_PARTY_LICENSES.md. Object indices are assigned for EVERY object,
//! including instances of an existing class, not only class declarations.

use super::{parse_shape_tail, read_color, read_matrix, valid_affine, ByteReader};
use crate::xfl_content::{RawShape, RawText};
use q0s_format::transform::Affine;
use q0s_format::v2::{Q0rgPlaybackMode, Rgba, Vec2};

#[derive(Clone, Default)]
pub(super) struct LayerInfo {
    pub name: String,
    pub kind: u8,
    pub locked: bool,
    pub hidden: bool,
    pub parent: Option<usize>,
}

pub(super) enum Kind {
    Page,
    Layer(LayerInfo),
    Frame {
        span: u16,
    },
    Shape,
    Symbol {
        reference: u32,
        first_frame: u16,
        mode: Q0rgPlaybackMode,
        opacity: f32,
    },
    Bitmap {
        reference: u32,
    },
    Text(RawText),
}

pub(super) struct Node {
    pub kind: Kind,
    pub children: Vec<usize>,
    pub affine: Affine,
    pub shape: Option<RawShape>,
}

pub(super) struct Archive {
    pub nodes: Vec<Node>,
    pub root: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
enum Entry {
    Class(String),
    Object(usize),
    Null,
}

struct Decoder<'a> {
    reader: ByteReader<'a>,
    table: Vec<Entry>,
    nodes: Vec<Node>,
    warnings: Vec<String>,
    context: String,
    gradient_extra: usize,
}

pub(super) fn parse(bytes: &[u8]) -> Result<Archive, String> {
    // Gradient layout is a file revision choice. Validate the whole archive,
    // rather than accepting an accidentally plausible prefix of a fill table.
    parse_revision(bytes, 5)
        .or_else(|modern_error| parse_revision(bytes, 0).map_err(|_| modern_error))
}

fn parse_revision(bytes: &[u8], gradient_extra: usize) -> Result<Archive, String> {
    let mut decoder = Decoder {
        reader: ByteReader::at(bytes, 0),
        table: vec![Entry::Null],
        nodes: Vec::new(),
        warnings: Vec::new(),
        context: "archive header".into(),
        gradient_extra,
    };
    let result = (|| {
        if decoder.reader.u8()? != 1 {
            return None;
        }
        let root = decoder.object(0)??;
        if !matches!(decoder.nodes[root].kind, Kind::Page) {
            return None;
        }
        // Do not accept a truncated walk or turn trailing keyframes into artwork
        // on the first frame. Unsupported revisions use explicit recovery instead.
        if decoder.reader.position != bytes.len() {
            return None;
        }
        Some(root)
    })();
    let Some(root) = result else {
        return Err(format!(
            "unsupported {} at byte {} of {}",
            decoder.context,
            decoder.reader.position,
            bytes.len()
        ));
    };
    Ok(Archive {
        nodes: decoder.nodes,
        root,
        warnings: decoder.warnings,
    })
}

impl Decoder<'_> {
    fn object(&mut self, depth: usize) -> Option<Option<usize>> {
        if depth > 256 || self.nodes.len() >= 100_000 {
            return None;
        }
        let tag = self.reader.u16()?;
        if tag == 0 {
            return Some(None);
        }
        let class = if tag == 0xffff {
            let _schema = self.reader.u16()?;
            let length = usize::from(self.reader.u16()?);
            if length > 64 {
                return None;
            }
            let class = std::str::from_utf8(self.reader.take(length)?)
                .ok()?
                .to_owned();
            self.table.push(Entry::Class(class.clone()));
            class
        } else {
            let (index, class_reference) = if tag == 0x7fff {
                let large = self.reader.u32()?;
                ((large & 0x7fff_ffff) as usize, large & 0x8000_0000 != 0)
            } else {
                (usize::from(tag & 0x7fff), tag & 0x8000 != 0)
            };
            match self.table.get(index)? {
                Entry::Class(name) if class_reference => name.clone(),
                Entry::Object(id) if !class_reference => return Some(Some(*id)),
                _ => return None,
            }
        };
        let id = self.nodes.len();
        self.nodes.push(Node {
            kind: Kind::Shape,
            children: Vec::new(),
            affine: Affine::IDENTITY,
            shape: None,
        });
        // Register before descending: layer parent references can point at an
        // object whose enclosing frame/page has not finished deserializing yet.
        self.table.push(Entry::Object(id));
        let schema = self.reader.u8()?;
        let _flags = self.reader.u8()?;
        self.context = format!("{class} children");
        if schema > 8 {
            return None;
        }
        let mut children = Vec::new();
        while let Some(child) = self.object(depth + 1)? {
            if child == id {
                return None;
            }
            children.push(child);
        }
        if schema > 0 {
            self.reader.skip(8)?;
        }
        if schema > 2 {
            self.reader.u8()?;
        }
        if schema > 3 {
            self.reader.u8()?;
        }
        self.context = format!("{class} payload");
        let mut affine = Affine::IDENTITY;
        let mut shape = None;
        let kind = match class.as_str() {
            "CPicShape" | "CPicFrame" => {
                let mut value = parse_shape_tail(&mut self.reader, self.gradient_extra)?;
                affine = value.affine;
                value.affine = Affine::IDENTITY;
                if !value.edges.is_empty() {
                    shape = Some(value);
                }
                if class == "CPicFrame" {
                    Kind::Frame {
                        span: self.frame_tail()?,
                    }
                } else {
                    Kind::Shape
                }
            }
            "CPicPage" => {
                let version = self.reader.u8()?;
                if version != 4 {
                    self.reader.u16()?;
                }
                if version >= 5 {
                    self.reader.u16()?;
                }
                if version >= 7 {
                    self.reader.u32()?;
                }
                if version >= 3 {
                    let count = self.reader.u32()? as usize;
                    if count > 100_000 {
                        return None;
                    }
                    self.reader.skip(count.checked_mul(8)?)?;
                }
                Kind::Page
            }
            "CPicLayer" => Kind::Layer(self.layer_tail(depth)?),
            "CPicSymbol" | "CPicShapeObj" | "CPicSprite" | "CPicButton" => {
                let version = self.reader.u8()?;
                // The library-reference/color-transform layout below is the
                // Flash 8/MX2004-compatible revision. Unknown revisions must
                // use explicit recovery, never a guessed reference to Symbol 1.
                if version != 19 {
                    return None;
                }
                affine = read_matrix(&mut self.reader)?;
                let first_frame = self.reader.u16()?;
                let loop_code = if version > 1 { self.reader.u16()? } else { 0 };
                let mut opacity = 1.0;
                let reference;
                if version == 19 {
                    self.reader.u8()?; // color-transform marker
                    let alpha_multiplier = self.reader.u16()?;
                    let alpha_offset = self.reader.s16()?;
                    opacity = (f32::from(alpha_multiplier) / 256.0).clamp(0.0, 1.0);
                    let mut advanced_color = alpha_offset != 0;
                    for _ in 0..3 {
                        advanced_color |= self.reader.u16()? != 256;
                        advanced_color |= self.reader.s16()? != 0;
                    }
                    self.reader.u16()?; // color effect type
                    self.reader.u16()?; // effect percent
                    self.reader.u32()?; // effect color
                    if advanced_color {
                        self.warnings.push(format!(
                            "{class}: advanced color transform is not imported yet"
                        ));
                    }
                    self.cstring()?;
                    reference = self.reader.u32()?;
                    self.reader.skip(3)?;
                    if self.reader.u8()? != 0 {
                        self.skip_extension(&class)?;
                    } else {
                        let blend = self.reader.u8()?;
                        if blend > 1 {
                            self.warnings
                                .push(format!("{class}: blend mode {blend} is not imported yet"));
                        }
                        self.reader.skip(2)?;
                    }
                } else {
                    self.reader.skip(9)?;
                    self.cstring()?;
                    reference = self.reader.u32()?;
                    if version >= 11 {
                        self.skip_extension(&class)?;
                    }
                }
                if matches!(class.as_str(), "CPicSprite" | "CPicButton") {
                    self.skip_extension(&class)?;
                }
                Kind::Symbol {
                    reference,
                    first_frame,
                    mode: if class == "CPicSprite" {
                        Q0rgPlaybackMode::Loop
                    } else {
                        match loop_code {
                            1 => Q0rgPlaybackMode::PlayOnce,
                            2 => Q0rgPlaybackMode::SingleFrame,
                            _ => Q0rgPlaybackMode::Loop,
                        }
                    },
                    opacity,
                }
            }
            "CPicBitmap" => {
                let version = self.reader.u8()?;
                affine = read_matrix(&mut self.reader)?;
                let reference = u32::from(self.reader.u16()?);
                if version >= 2 && self.reader.u8()? != 0 {
                    self.skip_extension(&class)?;
                }
                Kind::Bitmap { reference }
            }
            "CPicText" => {
                let text = self.text()?;
                affine = text.affine;
                self.skip_extension(&class)?;
                Kind::Text(RawText {
                    affine: Affine::IDENTITY,
                    ..text
                })
            }
            _ => return None,
        };
        if !valid_affine(affine) {
            return None;
        }
        self.nodes[id] = Node {
            kind,
            children,
            affine,
            shape,
        };
        Some(Some(id))
    }

    fn cstring(&mut self) -> Option<String> {
        let first = self.reader.u8()?;
        let mut unicode = false;
        let count = if first < 255 {
            usize::from(first)
        } else {
            let extended = self.reader.u16()?;
            match extended {
                0xfffe => {
                    unicode = true;
                    let first = self.reader.u8()?;
                    if first < 255 {
                        usize::from(first)
                    } else {
                        let wide = self.reader.u16()?;
                        if wide == 0xffff {
                            self.reader.u32()? as usize
                        } else {
                            usize::from(wide)
                        }
                    }
                }
                0xffff => self.reader.u32()? as usize,
                count => usize::from(count),
            }
        };
        self.string(count, unicode)
    }

    fn string(&mut self, count: usize, unicode: bool) -> Option<String> {
        if count > 1_000_000 {
            return None;
        }
        let bytes = self
            .reader
            .take(count.checked_mul(if unicode { 2 } else { 1 })?)?;
        Some(if unicode {
            let units = bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>();
            String::from_utf16_lossy(&units)
        } else {
            bytes.iter().map(|byte| char::from(*byte)).collect()
        })
    }

    fn timeline_data(&mut self) -> Option<()> {
        let version = self.reader.u32()?;
        let format = self.reader.u32()?;
        if version >= 1 {
            self.reader.u32()?;
            let count = self.reader.u32()? as usize;
            if count > 100_000 {
                return None;
            }
            self.reader.skip(count.checked_mul(4)?)?;
        }
        match format {
            0 => {
                self.reader.u32()?;
                let count = self.reader.u32()? as usize;
                if count > 100_000 {
                    return None;
                }
                self.reader.skip(count.checked_mul(4)?)?;
            }
            1 if version >= 4 => {
                self.cstring()?;
            }
            1 => {}
            2 => {
                self.cstring()?;
            }
            _ => return None,
        }
        Some(())
    }

    fn frame_tail(&mut self) -> Option<u16> {
        let version = self.reader.u8()?;
        self.context = format!("CPicFrame schema {version} tail");
        let span = self.reader.u16()?;
        if span == 0 {
            return None;
        }
        self.reader.skip(if version > 2 { 2 } else { 1 })?;
        if version > 1 {
            self.reader.skip(2)?;
        }
        if version > 4 {
            self.reader.skip(2)?;
        }
        if version > 5 {
            let count = usize::from(self.reader.u16()?);
            self.reader.skip(count.checked_mul(8)?)?;
        }
        if version > 6 {
            self.reader.skip(11)?;
        }
        if version > 7 {
            self.reader.skip(2)?;
        }
        if version > 8 {
            if version >= 23 {
                self.cstring()?;
            }
            if version >= 19 {
                self.timeline_data()?;
            } else {
                return None;
            }
        }
        if version > 10 {
            self.reader.skip(8)?;
        }
        if version > 11 {
            self.reader.skip(4)?;
        }
        if version > 12 && self.reader.u16()? != 0 {
            return None;
        }
        if version > 13 {
            self.reader.skip(4)?;
        }
        if version > 14 && self.reader.u16()? != 0 {
            return None;
        }
        if version > 15 && version >= 23 {
            self.cstring()?;
        }
        if version > 19 {
            self.reader.skip(4)?;
        }
        if version > 20 {
            self.reader.skip(4)?;
        }
        if version >= 22 {
            self.reader.skip(4)?;
        }
        if version >= 24 {
            self.reader.skip(8)?;
        }
        if version > 32 {
            return None;
        }
        Some(span)
    }

    fn layer_tail(&mut self, depth: usize) -> Option<LayerInfo> {
        let version = self.reader.u8()?;
        if version > 30 {
            return None;
        }
        let name = self.cstring()?;
        let mut info = LayerInfo {
            name,
            ..Default::default()
        };
        if version <= 3 {
            let state = self.reader.u8()?;
            info.hidden = state == 0;
            info.locked = state == 1;
        }
        if version >= 4 {
            self.reader.u8()?; // selected layer, not layer type
            info.hidden = self.reader.u8()? != 0;
            info.locked = self.reader.u8()? != 0;
        }
        if version >= 5 {
            self.reader.skip(4)?;
        }
        if version >= 6 {
            self.reader.skip(8)?;
        }
        if version >= 8 {
            self.reader.skip(4)?;
        }
        info.kind = self.reader.u8()?;
        info.parent = self.object(depth + 1)?;
        if (7..9).contains(&version) && self.reader.u16()? != 0 {
            return None;
        }
        if (2..6).contains(&version) {
            self.reader.u8()?;
        }
        if (3..9).contains(&version) {
            self.reader.u8()?;
        }
        if version >= 9 {
            self.reader.u8()?;
        }
        if version >= 10 {
            self.reader.u8()?;
        }
        Some(info)
    }

    fn text_run(&mut self) -> Option<(String, Rgba)> {
        let version = self.reader.u8()?;
        if !(2..=20).contains(&version) {
            return None;
        }
        self.reader.u16()?;
        let length = usize::from(self.reader.u8()?);
        let font = self.string(length, version >= 10)?;
        let color = read_color(self.reader.u32()?);
        self.reader.skip(2)?;
        self.reader.skip(if version >= 3 { 4 } else { 1 })?;
        self.reader.skip(11)?;
        if version >= 5 {
            let count = usize::from(self.reader.u8()?);
            self.string(count, version >= 10)?;
        }
        if version >= 6 {
            self.reader.skip(3)?;
        }
        if version >= 8 {
            self.reader.u8()?;
        }
        if version >= 7 {
            let count = usize::from(self.reader.u8()?);
            self.string(count, version >= 10)?;
        }
        Some((font, color))
    }

    fn text(&mut self) -> Option<RawText> {
        let version = self.reader.u8()?;
        let affine = read_matrix(&mut self.reader)?;
        let left = self.reader.s32()?;
        self.reader.s32()?;
        let top = self.reader.s32()?;
        self.reader.s32()?;
        self.reader.u8()?;
        if version >= 3 {
            self.reader.u8()?;
        }
        let flags = if version >= 5 {
            self.reader.u32()?
        } else if version >= 4 {
            u32::from(self.reader.u16()?)
        } else {
            0
        };
        let size = if version >= 4 {
            f32::from(self.reader.u16()?) / 20.0
        } else {
            12.0
        };
        if version >= 10 {
            self.cstring()?;
            if flags & 0x2000 != 0 {
                self.cstring()?;
            }
        }
        if flags & 0x4000 != 0 {
            self.text_run()?;
        }
        let count = usize::from(self.reader.u16()?);
        let (font, color) = if count > 0 {
            self.text_run()?
        } else {
            (
                "Arial".to_string(),
                Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
            )
        };
        let text = self.string(count, version >= 10)?;
        Some(RawText {
            affine,
            offset: Vec2::new(left as f32 / 20.0, top as f32 / 20.0),
            text,
            font,
            size: size.max(0.05),
            color,
        })
    }

    /// Skip opaque extension records (filters/script UI data) only after
    /// the structured payload was consumed. Candidate boundaries must belong to
    /// a registered class and have a plausible object header, or the containing
    /// object's exact null + INT_MIN origin terminator. Never scan shape bytes.
    fn skip_extension(&mut self, class: &str) -> Option<()> {
        let start = self.reader.position;
        let bytes = self.reader.bytes;
        let end = start
            .saturating_add(65_536)
            .min(bytes.len().saturating_sub(16));
        for offset in start..=end {
            let mut probe = ByteReader::at(bytes, offset);
            let tag = probe.u16()?;
            if tag == 0
                && bytes.get(offset..offset + 10) == Some(&[0, 0, 0, 0, 0, 0x80, 0, 0, 0, 0x80])
            {
                self.reader.position = offset;
                return Some(());
            }
            if tag == 0xffff {
                probe.u16()?;
                let count = usize::from(probe.u16()?);
                if count > 32 {
                    continue;
                }
                let Some(name) = probe.take(count) else {
                    continue;
                };
                if !matches!(
                    name,
                    b"CPicShape"
                        | b"CPicSymbol"
                        | b"CPicFrame"
                        | b"CPicLayer"
                        | b"CPicText"
                        | b"CPicSprite"
                        | b"CPicButton"
                        | b"CPicBitmap"
                ) {
                    continue;
                }
            } else if tag & 0x8000 != 0 {
                if !matches!(
                    self.table.get(usize::from(tag & 0x7fff)),
                    Some(Entry::Class(_))
                ) {
                    continue;
                }
            } else {
                continue;
            }
            if !matches!(probe.u8(), Some(3 | 4)) || probe.u8()? > 7 {
                continue;
            }
            let child = probe.u16()?;
            if child != 0
                && child != 0xffff
                && !matches!(
                    self.table.get(usize::from(child & 0x7fff)),
                    Some(Entry::Class(_))
                )
            {
                continue;
            }
            self.reader.position = offset;
            if class != "CPicText" {
                self.warnings
                    .push(format!("{class}: extension data was skipped"));
            }
            return Some(());
        }
        None
    }
}
