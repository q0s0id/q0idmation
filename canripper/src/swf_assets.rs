//! What the tags of an SWF hold: characters (shapes, sprites, buttons,
//! images, sounds, texts, fonts, binary data), the frames that place them
//! and the ActionScript that drives them. Images are decoded, shapes become
//! vector paths, frames are laid out from their display list, and AS1/2
//! bytecode is listed as P-code.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Read as _;
use std::sync::Arc;

use q0s_format::geom::{CapShape, JoinShape};
use q0s_format::v2::{Anchor, Path, Rgba, Stroke, Vec2};

use crate::preview::VectorLayer;
use crate::swf::{SwfMovie, SwfTag};

/// Most a decoded image may hold, in pixels.
const MAX_IMAGE_PIXELS: usize = 64 * 1024 * 1024;
/// How deep sprites inside sprites are followed.
const MAX_NESTING: usize = 12;
/// Most paths one frame may hold before the rest is left out.
const MAX_FRAME_PATHS: usize = 60_000;

// ---------------------------------------------------------------- reading

/// Reads SWF fields: little-endian bytes, and big-endian bit fields that
/// byte-aligned fields start after.
pub struct Reader<'a> {
    bytes: &'a [u8],
    byte: usize,
    bit: u8,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            byte: 0,
            bit: 0,
        }
    }

    pub fn align(&mut self) {
        if self.bit > 0 {
            self.bit = 0;
            self.byte += 1;
        }
    }

    pub fn position(&self) -> usize {
        self.byte + usize::from(self.bit > 0)
    }

    pub fn rest(&mut self) -> &'a [u8] {
        self.align();
        &self.bytes[self.byte.min(self.bytes.len())..]
    }

    pub fn is_empty(&self) -> bool {
        self.position() >= self.bytes.len()
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        self.align();
        let end = self
            .byte
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| "field runs past the end of its tag".to_string())?;
        let out = &self.bytes[self.byte..end];
        self.byte = end;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Result<i16, String> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A string up to its terminating zero.
    pub fn string(&mut self) -> Result<String, String> {
        self.align();
        let rest = &self.bytes[self.byte.min(self.bytes.len())..];
        let end = rest
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| "string has no end".to_string())?;
        self.byte += end + 1;
        Ok(decode_text(&rest[..end]))
    }

    pub fn ub(&mut self, count: u32) -> Result<u32, String> {
        if count > 32 {
            return Err("bit field is wider than 32 bits".to_string());
        }
        let mut value = 0u32;
        for _ in 0..count {
            let byte = *self
                .bytes
                .get(self.byte)
                .ok_or_else(|| "bit field runs past the end of its tag".to_string())?;
            let bit = (byte >> (7 - self.bit)) & 1;
            value = (value << 1) | u32::from(bit);
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.byte += 1;
            }
        }
        Ok(value)
    }

    pub fn sb(&mut self, count: u32) -> Result<i32, String> {
        if count == 0 {
            return Ok(0);
        }
        let raw = self.ub(count)?;
        let shift = 32 - count;
        Ok(((raw << shift) as i32) >> shift)
    }

    /// A 16.16 fixed-point number of `count` bits.
    pub fn fb(&mut self, count: u32) -> Result<f32, String> {
        Ok(self.sb(count)? as f32 / 65536.0)
    }

    pub fn rect(&mut self) -> Result<Bounds, String> {
        self.align();
        let bits = self.ub(5)?;
        let x_min = self.sb(bits)?;
        let x_max = self.sb(bits)?;
        let y_min = self.sb(bits)?;
        let y_max = self.sb(bits)?;
        self.align();
        Ok(Bounds {
            x_min: x_min as f32 / 20.0,
            y_min: y_min as f32 / 20.0,
            x_max: x_max as f32 / 20.0,
            y_max: y_max as f32 / 20.0,
        })
    }

    pub fn matrix(&mut self) -> Result<Matrix, String> {
        self.align();
        let mut matrix = Matrix::IDENTITY;
        if self.ub(1)? == 1 {
            let bits = self.ub(5)?;
            matrix.a = self.fb(bits)?;
            matrix.d = self.fb(bits)?;
        }
        if self.ub(1)? == 1 {
            let bits = self.ub(5)?;
            matrix.b = self.fb(bits)?;
            matrix.c = self.fb(bits)?;
        }
        let bits = self.ub(5)?;
        matrix.tx = self.sb(bits)? as f32 / 20.0;
        matrix.ty = self.sb(bits)? as f32 / 20.0;
        self.align();
        Ok(matrix)
    }

    pub fn color_transform(&mut self, alpha: bool) -> Result<ColorTransform, String> {
        self.align();
        let has_add = self.ub(1)? == 1;
        let has_mult = self.ub(1)? == 1;
        let bits = self.ub(4)?;
        let channels = if alpha { 4 } else { 3 };
        let mut transform = ColorTransform::IDENTITY;
        if has_mult {
            for channel in 0..channels {
                transform.mult[channel] = self.sb(bits)? as f32 / 256.0;
            }
        }
        if has_add {
            for channel in 0..channels {
                transform.add[channel] = self.sb(bits)? as f32;
            }
        }
        self.align();
        Ok(transform)
    }

    pub fn rgb(&mut self) -> Result<Rgba, String> {
        let b = self.take(3)?;
        Ok(Rgba {
            r: b[0],
            g: b[1],
            b: b[2],
            a: 255,
        })
    }

    pub fn rgba(&mut self) -> Result<Rgba, String> {
        let b = self.take(4)?;
        Ok(Rgba {
            r: b[0],
            g: b[1],
            b: b[2],
            a: b[3],
        })
    }
}

/// SWF strings are UTF-8 from version 6 on and a local codepage before;
/// the bytes that are not UTF-8 are kept as Latin-1.
fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|byte| char::from(*byte)).collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Matrix {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    pub fn apply(&self, point: Vec2) -> Vec2 {
        Vec2::new(
            self.a * point.x + self.c * point.y + self.tx,
            self.b * point.x + self.d * point.y + self.ty,
        )
    }

    /// `self` after `inner`: what places something placed inside `inner`.
    pub fn then(&self, inner: &Matrix) -> Matrix {
        Matrix {
            a: self.a * inner.a + self.c * inner.b,
            b: self.b * inner.a + self.d * inner.b,
            c: self.a * inner.c + self.c * inner.d,
            d: self.b * inner.c + self.d * inner.d,
            tx: self.a * inner.tx + self.c * inner.ty + self.tx,
            ty: self.b * inner.tx + self.d * inner.ty + self.ty,
        }
    }

    fn scale(&self) -> f32 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTransform {
    pub mult: [f32; 4],
    pub add: [f32; 4],
}

impl ColorTransform {
    pub const IDENTITY: Self = Self {
        mult: [1.0; 4],
        add: [0.0; 4],
    };

    pub fn apply(&self, color: Rgba) -> Rgba {
        let channel = |value: u8, i: usize| {
            (f32::from(value) * self.mult[i] + self.add[i])
                .round()
                .clamp(0.0, 255.0) as u8
        };
        Rgba {
            r: channel(color.r, 0),
            g: channel(color.g, 1),
            b: channel(color.b, 2),
            a: channel(color.a, 3),
        }
    }

    pub fn then(&self, inner: &ColorTransform) -> ColorTransform {
        let mut out = ColorTransform::IDENTITY;
        for i in 0..4 {
            out.mult[i] = self.mult[i] * inner.mult[i];
            out.add[i] = inner.add[i] * self.mult[i] + self.add[i];
        }
        out
    }
}

// ------------------------------------------------------------ the library

/// What a tag is, the way JPEXS sorts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Shape,
    MorphShape,
    Sprite,
    Button,
    Image,
    Text,
    Font,
    Sound,
    Video,
    BinaryData,
    Script,
    Other,
    Control,
}

impl Category {
    pub fn of(code: u16) -> Self {
        match code {
            2 | 22 | 32 | 83 => Self::Shape,
            46 | 84 => Self::MorphShape,
            39 => Self::Sprite,
            7 | 34 => Self::Button,
            6 | 20 | 21 | 35 | 36 | 90 => Self::Image,
            11 | 33 | 37 => Self::Text,
            10 | 48 | 75 | 91 => Self::Font,
            14 => Self::Sound,
            60 => Self::Video,
            87 => Self::BinaryData,
            12 | 59 | 72 | 82 => Self::Script,
            0 | 1 | 4 | 5 | 15 | 18 | 19 | 26 | 28 | 43 | 45 | 61 | 70 => Self::Control,
            _ => Self::Other,
        }
    }

    pub fn folder(self) -> &'static str {
        match self {
            Self::Shape => "shapes",
            Self::MorphShape => "morphshapes",
            Self::Sprite => "sprites",
            Self::Button => "buttons",
            Self::Image => "images",
            Self::Text => "texts",
            Self::Font => "fonts",
            Self::Sound => "sounds",
            Self::Video => "videos",
            Self::BinaryData => "binaryData",
            Self::Script => "scripts",
            Self::Other => "others",
            Self::Control => "timeline",
        }
    }
}

/// Whether a tag defines a character, whose id is its first two bytes.
pub fn defines_character(code: u16) -> bool {
    !matches!(
        Category::of(code),
        Category::Script | Category::Other | Category::Control
    )
}

pub fn character_id(tag: &SwfTag) -> Option<u16> {
    (defines_character(tag.code) && tag.payload.len() >= 2)
        .then(|| u16::from_le_bytes([tag.payload[0], tag.payload[1]]))
}

/// Where a movie's characters are, what they are exported as, and its JPEG
/// tables; worked out once per movie.
pub struct Index {
    characters: HashMap<u16, usize>,
    names: HashMap<u16, String>,
    jpeg_tables: Option<usize>,
}

impl Index {
    fn build(movie: &SwfMovie) -> Self {
        let mut characters = HashMap::new();
        let mut names = HashMap::new();
        let mut jpeg_tables = None;
        for tag in &movie.tags {
            match tag.code {
                8 if !tag.payload.is_empty() => jpeg_tables = Some(tag.index),
                56 | 76 => {
                    for (id, name) in exported_names(tag) {
                        names.insert(id, name);
                    }
                }
                _ => {}
            }
            if let Some(id) = character_id(tag) {
                characters.entry(id).or_insert(tag.index);
            }
        }
        Self {
            characters,
            names,
            jpeg_tables,
        }
    }
}

/// A movie's characters, drawn and decoded on request, with what the movie
/// keeps between previews.
pub struct Library<'a> {
    pub movie: &'a SwfMovie,
    index: &'a Index,
}

impl<'a> Library<'a> {
    pub fn new(movie: &'a SwfMovie) -> Self {
        Self {
            movie,
            index: movie.cache.index.get_or_init(|| Index::build(movie)),
        }
    }

    /// What a character is exported as.
    pub fn name(&self, id: u16) -> Option<&'a str> {
        self.index.names.get(&id).map(String::as_str)
    }

    fn jpeg_tables(&self) -> Option<&'a [u8]> {
        self.index
            .jpeg_tables
            .and_then(|index| self.tag(index))
            .map(|tag| tag.payload.as_slice())
    }

    pub fn tag(&self, index: usize) -> Option<&'a SwfTag> {
        self.movie.tags.get(index).filter(|tag| tag.index == index)
    }

    pub fn character(&self, id: u16) -> Option<&'a SwfTag> {
        self.index
            .characters
            .get(&id)
            .and_then(|index| self.tag(*index))
    }

    fn shape(&self, tag: &SwfTag) -> Result<Arc<Vec<VectorLayer>>, String> {
        let id = character_id(tag).unwrap_or(u16::MAX);
        let cache = &self.movie.cache.shapes;
        if let Some(shape) = cache
            .lock()
            .ok()
            .and_then(|shapes| shapes.get(&id).cloned())
        {
            return Ok(shape);
        }
        let shape = Arc::new(parse_shape(tag)?.layers);
        if let Ok(mut shapes) = cache.lock() {
            shapes.insert(id, Arc::clone(&shape));
        }
        Ok(shape)
    }

    /// A character's vector layers as placed by `matrix` and `color`, at
    /// `frame` of its own timeline, added to `out`.
    fn draw_character(
        &self,
        id: u16,
        frame: u32,
        matrix: &Matrix,
        color: &ColorTransform,
        depth: usize,
        out: &mut Vec<VectorLayer>,
    ) -> Result<(), String> {
        if depth > MAX_NESTING || paths_in(out) > MAX_FRAME_PATHS {
            return Ok(());
        }
        let Some(tag) = self.character(id) else {
            return Ok(());
        };
        match Category::of(tag.code) {
            Category::Shape => {
                for layer in self.shape(tag)?.iter() {
                    out.push(transform_layer(layer, matrix, color));
                }
            }
            Category::Sprite => {
                let (frames, tags) = sprite_tags(tag)?;
                let frame = if frames == 0 {
                    0
                } else {
                    frame % u32::from(frames)
                };
                let list = display_list(&tags, frame);
                self.draw_list(&list, frame, matrix, color, depth + 1, out)?;
            }
            Category::Button => {
                for placed in button_up_state(tag)? {
                    let matrix = matrix.then(&placed.matrix);
                    let color = color.then(&placed.color);
                    self.draw_character(placed.id, 0, &matrix, &color, depth + 1, out)?;
                }
            }
            Category::Image => {
                // A bitmap placed on its own: its outline, as Flash shows a
                // missing bitmap fill.
                if let Ok((width, height, _)) = self.decode_image(tag) {
                    let corners = [
                        (0.0, 0.0),
                        (width as f32, 0.0),
                        (width as f32, height as f32),
                        (0.0, height as f32),
                    ];
                    let path = Path {
                        anchors: corners
                            .iter()
                            .map(|(x, y)| plain_anchor(Vec2::new(*x, *y)))
                            .collect(),
                        closed: true,
                    };
                    out.push(transform_layer(
                        &VectorLayer {
                            paths: vec![path],
                            fill: Some(Rgba {
                                r: 128,
                                g: 128,
                                b: 128,
                                a: 90,
                            }),
                            stroke: None,
                        },
                        matrix,
                        color,
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn draw_list(
        &self,
        list: &BTreeMap<u16, Placed>,
        frame: u32,
        matrix: &Matrix,
        color: &ColorTransform,
        depth: usize,
        out: &mut Vec<VectorLayer>,
    ) -> Result<(), String> {
        for placed in list.values() {
            // Masks shape what is under them; drawn, they would cover it.
            if placed.clip_depth > 0 {
                continue;
            }
            let matrix = matrix.then(&placed.matrix);
            let color = color.then(&placed.color);
            let local = frame.saturating_sub(placed.placed_at);
            self.draw_character(placed.id, local, &matrix, &color, depth, out)?;
        }
        Ok(())
    }

    /// The main timeline at `frame`. Moving forward goes on from where the
    /// last frame was laid out, so playing does not replay the movie.
    pub fn draw_frame(&self, frame: u32) -> Result<Vec<VectorLayer>, String> {
        let list = {
            let mut cached = self
                .movie
                .cache
                .timeline
                .lock()
                .map_err(|_| "timeline cache is poisoned")?;
            let timeline = match cached.take() {
                Some(timeline) if timeline.frame <= frame => timeline,
                _ => Timeline::default(),
            };
            let timeline = timeline.advanced_to(&self.movie.tags, frame);
            let list = timeline.list.clone();
            *cached = Some(timeline);
            list
        };
        let mut out = Vec::new();
        self.draw_list(
            &list,
            frame,
            &Matrix::IDENTITY,
            &ColorTransform::IDENTITY,
            0,
            &mut out,
        )?;
        Ok(out)
    }

    /// A character drawn at the origin, at `frame` of its own timeline.
    pub fn draw(&self, tag: &SwfTag, frame: u32) -> Result<Vec<VectorLayer>, String> {
        let mut out = Vec::new();
        if Category::of(tag.code) == Category::Shape {
            return Ok(self.shape(tag)?.as_ref().clone());
        }
        let id = character_id(tag).ok_or("this tag is not a character")?;
        self.draw_character(
            id,
            frame,
            &Matrix::IDENTITY,
            &ColorTransform::IDENTITY,
            0,
            &mut out,
        )?;
        Ok(out)
    }

    pub fn decode_image(&self, tag: &SwfTag) -> Result<(u32, u32, Vec<u8>), String> {
        decode_image(tag, self.jpeg_tables())
    }
}

fn paths_in(layers: &[VectorLayer]) -> usize {
    layers.iter().map(|layer| layer.paths.len()).sum()
}

fn transform_layer(layer: &VectorLayer, matrix: &Matrix, color: &ColorTransform) -> VectorLayer {
    let point = |p: Vec2| matrix.apply(p);
    VectorLayer {
        paths: layer
            .paths
            .iter()
            .map(|path| Path {
                anchors: path
                    .anchors
                    .iter()
                    .map(|anchor| Anchor {
                        point: point(anchor.point),
                        in_handle: anchor.in_handle.map(point),
                        out_handle: anchor.out_handle.map(point),
                    })
                    .collect(),
                closed: path.closed,
            })
            .collect(),
        fill: layer.fill.map(|fill| color.apply(fill)),
        stroke: layer.stroke.map(|stroke| Stroke {
            color: color.apply(stroke.color),
            width: stroke.width * matrix.scale(),
            ..stroke
        }),
    }
}

/// Where everything is drawn, in pixels.
pub fn layer_bounds(layers: &[VectorLayer]) -> Option<Bounds> {
    let mut bounds: Option<Bounds> = None;
    for layer in layers {
        let pad = layer.stroke.map_or(0.0, |stroke| stroke.width * 0.5);
        for path in &layer.paths {
            for anchor in &path.anchors {
                for p in [Some(anchor.point), anchor.in_handle, anchor.out_handle]
                    .into_iter()
                    .flatten()
                {
                    if !p.x.is_finite() || !p.y.is_finite() {
                        continue;
                    }
                    let b = bounds.get_or_insert(Bounds {
                        x_min: p.x - pad,
                        y_min: p.y - pad,
                        x_max: p.x + pad,
                        y_max: p.y + pad,
                    });
                    b.x_min = b.x_min.min(p.x - pad);
                    b.y_min = b.y_min.min(p.y - pad);
                    b.x_max = b.x_max.max(p.x + pad);
                    b.y_max = b.y_max.max(p.y + pad);
                }
            }
        }
    }
    bounds
}

// ------------------------------------------------------------ timelines

#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub id: u16,
    pub matrix: Matrix,
    pub color: ColorTransform,
    pub clip_depth: u16,
    /// The frame the character was put in, from which its own timeline runs.
    pub placed_at: u32,
}

/// The frames of a sprite and the tags of its timeline.
pub fn sprite_tags(tag: &SwfTag) -> Result<(u16, Vec<SwfTag>), String> {
    let mut reader = Reader::new(&tag.payload);
    reader.u16()?;
    let frames = reader.u16()?;
    let start = reader.position();
    let mut tags = Vec::new();
    let mut offset = start;
    let bytes = &tag.payload;
    while offset + 2 <= bytes.len() {
        let record = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        offset += 2;
        let code = record >> 6;
        let mut length = usize::from(record & 0x3f);
        if length == 0x3f {
            if offset + 4 > bytes.len() {
                break;
            }
            length = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
        }
        let Some(end) = offset.checked_add(length).filter(|end| *end <= bytes.len()) else {
            break;
        };
        tags.push(SwfTag {
            index: tags.len(),
            code,
            name: crate::swf::tag_name(code),
            payload_offset: tag.payload_offset + offset,
            payload: bytes[offset..end].to_vec(),
        });
        offset = end;
        if code == 0 {
            break;
        }
    }
    Ok((frames, tags))
}

/// What is on stage at `frame` (counted from zero) of a timeline.
pub fn display_list(tags: &[SwfTag], frame: u32) -> BTreeMap<u16, Placed> {
    Timeline::default().advanced_to(tags, frame).list
}

/// A timeline laid out up to a frame, ready to go on from there.
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    pub list: BTreeMap<u16, Placed>,
    pub frame: u32,
    /// The first tag not yet run.
    next: usize,
}

impl Timeline {
    pub fn advanced_to(mut self, tags: &[SwfTag], frame: u32) -> Self {
        while let Some(tag) = tags.get(self.next) {
            if tag.code == 1 {
                if self.frame == frame {
                    break;
                }
                self.frame += 1;
                self.next += 1;
                continue;
            }
            self.next += 1;
            self.run(tag);
        }
        self
    }

    fn run(&mut self, tag: &SwfTag) {
        let current = self.frame;
        let list = &mut self.list;
        match tag.code {
            4 => {
                let mut r = Reader::new(&tag.payload);
                let (Ok(id), Ok(depth), Ok(matrix)) = (r.u16(), r.u16(), r.matrix()) else {
                    return;
                };
                let color = if r.is_empty() {
                    ColorTransform::IDENTITY
                } else {
                    r.color_transform(false).unwrap_or(ColorTransform::IDENTITY)
                };
                list.insert(
                    depth,
                    Placed {
                        id,
                        matrix,
                        color,
                        clip_depth: 0,
                        placed_at: current,
                    },
                );
            }
            26 | 70 => {
                let _ = place_object(tag, current, list);
            }
            5 => {
                let mut r = Reader::new(&tag.payload);
                if let (Ok(_), Ok(depth)) = (r.u16(), r.u16()) {
                    list.remove(&depth);
                }
            }
            28 => {
                if let Ok(depth) = Reader::new(&tag.payload).u16() {
                    list.remove(&depth);
                }
            }
            _ => {}
        }
    }
}

fn place_object(tag: &SwfTag, frame: u32, list: &mut BTreeMap<u16, Placed>) -> Result<(), String> {
    let mut r = Reader::new(&tag.payload);
    let flags = r.u8()?;
    let flags2 = if tag.code == 70 { r.u8()? } else { 0 };
    let depth = r.u16()?;
    let moving = flags & 0x01 != 0;
    let has_character = flags & 0x02 != 0;
    if tag.code == 70 && (flags2 & 0x08 != 0 || (flags2 & 0x10 != 0 && has_character)) {
        r.string()?;
    }
    let id = if has_character { Some(r.u16()?) } else { None };
    let matrix = if flags & 0x04 != 0 {
        Some(r.matrix()?)
    } else {
        None
    };
    let color = if flags & 0x08 != 0 {
        Some(r.color_transform(true)?)
    } else {
        None
    };
    if flags & 0x10 != 0 {
        r.u16()?;
    }
    if flags & 0x20 != 0 {
        r.string()?;
    }
    let clip_depth = if flags & 0x40 != 0 {
        Some(r.u16()?)
    } else {
        None
    };
    match (list.get_mut(&depth), id) {
        (Some(existing), id) if moving => {
            if let Some(id) = id {
                existing.id = id;
                existing.placed_at = frame;
            }
            if let Some(matrix) = matrix {
                existing.matrix = matrix;
            }
            if let Some(color) = color {
                existing.color = color;
            }
            if let Some(clip_depth) = clip_depth {
                existing.clip_depth = clip_depth;
            }
        }
        (_, Some(id)) => {
            list.insert(
                depth,
                Placed {
                    id,
                    matrix: matrix.unwrap_or(Matrix::IDENTITY),
                    color: color.unwrap_or(ColorTransform::IDENTITY),
                    clip_depth: clip_depth.unwrap_or(0),
                    placed_at: frame,
                },
            );
        }
        _ => {}
    }
    Ok(())
}

/// What a button shows before the pointer reaches it.
fn button_up_state(tag: &SwfTag) -> Result<Vec<Placed>, String> {
    let mut r = Reader::new(&tag.payload);
    r.u16()?;
    if tag.code == 34 {
        r.u8()?;
        r.u16()?;
    }
    let mut placed = Vec::new();
    loop {
        let flags = r.u8()?;
        if flags == 0 {
            break;
        }
        let id = r.u16()?;
        r.u16()?;
        let matrix = r.matrix()?;
        let color = if tag.code == 34 {
            r.color_transform(true)?
        } else {
            ColorTransform::IDENTITY
        };
        if tag.code == 34 && flags & 0x10 != 0 {
            // Filters would need parsing to step past; what is placed so far
            // is what can be shown.
            if flags & 0x01 != 0 {
                placed.push(Placed {
                    id,
                    matrix,
                    color,
                    clip_depth: 0,
                    placed_at: 0,
                });
            }
            break;
        }
        if tag.code == 34 && flags & 0x20 != 0 {
            r.u8()?;
        }
        if flags & 0x01 != 0 {
            placed.push(Placed {
                id,
                matrix,
                color,
                clip_depth: 0,
                placed_at: 0,
            });
        }
    }
    Ok(placed)
}

/// Frame numbers (from zero) and the tags each frame holds, with labels.
pub struct FrameInfo {
    pub tags: Vec<usize>,
    pub label: Option<String>,
}

pub fn frames(tags: &[SwfTag]) -> Vec<FrameInfo> {
    let mut frames = Vec::new();
    let mut current = FrameInfo {
        tags: Vec::new(),
        label: None,
    };
    for tag in tags {
        match tag.code {
            1 => {
                current.tags.push(tag.index);
                frames.push(std::mem::replace(
                    &mut current,
                    FrameInfo {
                        tags: Vec::new(),
                        label: None,
                    },
                ));
            }
            0 => {}
            43 => {
                current.label = Reader::new(&tag.payload).string().ok();
                current.tags.push(tag.index);
            }
            _ if Category::of(tag.code) == Category::Control
                || Category::of(tag.code) == Category::Script =>
            {
                current.tags.push(tag.index);
            }
            _ => {}
        }
    }
    if !current.tags.is_empty() {
        frames.push(current);
    }
    frames
}

// ---------------------------------------------------------------- shapes

#[derive(Debug, Clone, PartialEq)]
enum Fill {
    Solid(Rgba),
    Gradient(Vec<Rgba>),
    Bitmap,
}

impl Fill {
    /// One colour standing for the fill: gradients by their middle,
    /// bitmaps a translucent grey.
    fn color(&self) -> Rgba {
        match self {
            Self::Solid(color) => *color,
            Self::Gradient(stops) if !stops.is_empty() => {
                let n = stops.len() as u32;
                let sum = |f: fn(&Rgba) -> u8| {
                    (stops.iter().map(|c| u32::from(f(c))).sum::<u32>() / n) as u8
                };
                Rgba {
                    r: sum(|c| c.r),
                    g: sum(|c| c.g),
                    b: sum(|c| c.b),
                    a: sum(|c| c.a),
                }
            }
            Self::Gradient(_) => Rgba {
                r: 128,
                g: 128,
                b: 128,
                a: 255,
            },
            Self::Bitmap => Rgba {
                r: 150,
                g: 150,
                b: 150,
                a: 160,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Line {
    width: f32,
    color: Rgba,
    cap: CapShape,
    join: JoinShape,
}

#[derive(Debug, Clone, Copy)]
struct Edge {
    group: usize,
    from: (i32, i32),
    control: Option<(i32, i32)>,
    to: (i32, i32),
    fill0: u32,
    fill1: u32,
    line: u32,
}

pub struct Shape {
    pub id: u16,
    pub bounds: Bounds,
    pub layers: Vec<VectorLayer>,
    pub fills: usize,
    pub lines: usize,
    pub edges: usize,
}

pub fn parse_shape(tag: &SwfTag) -> Result<Shape, String> {
    let version = match tag.code {
        2 => 1,
        22 => 2,
        32 => 3,
        83 => 4,
        _ => return Err(format!("{} is not a shape", tag.name)),
    };
    let mut r = Reader::new(&tag.payload);
    let id = r.u16()?;
    let bounds = r.rect()?;
    if version == 4 {
        r.rect()?;
        r.u8()?;
    }
    let mut groups = vec![(fill_styles(&mut r, version)?, line_styles(&mut r, version)?)];
    let mut fill_bits = r.ub(4)?;
    let mut line_bits = r.ub(4)?;
    let mut edges = Vec::new();
    let mut at = (0i32, 0i32);
    let (mut fill0, mut fill1, mut line) = (0u32, 0u32, 0u32);
    loop {
        if r.ub(1)? == 0 {
            let flags = r.ub(5)?;
            if flags == 0 {
                break;
            }
            if flags & 0x01 != 0 {
                let bits = r.ub(5)?;
                at = (r.sb(bits)?, r.sb(bits)?);
            }
            let new0 = if flags & 0x02 != 0 {
                Some(r.ub(fill_bits)?)
            } else {
                None
            };
            let new1 = if flags & 0x04 != 0 {
                Some(r.ub(fill_bits)?)
            } else {
                None
            };
            let new_line = if flags & 0x08 != 0 {
                Some(r.ub(line_bits)?)
            } else {
                None
            };
            if flags & 0x10 != 0 {
                r.align();
                groups.push((fill_styles(&mut r, version)?, line_styles(&mut r, version)?));
                fill_bits = r.ub(4)?;
                line_bits = r.ub(4)?;
                fill0 = 0;
                fill1 = 0;
                line = 0;
            }
            fill0 = new0.unwrap_or(fill0);
            fill1 = new1.unwrap_or(fill1);
            line = new_line.unwrap_or(line);
        } else if r.ub(1)? == 1 {
            let bits = r.ub(4)? + 2;
            let (dx, dy) = if r.ub(1)? == 1 {
                (r.sb(bits)?, r.sb(bits)?)
            } else if r.ub(1)? == 1 {
                (0, r.sb(bits)?)
            } else {
                (r.sb(bits)?, 0)
            };
            let to = (at.0 + dx, at.1 + dy);
            edges.push(Edge {
                group: groups.len() - 1,
                from: at,
                control: None,
                to,
                fill0,
                fill1,
                line,
            });
            at = to;
        } else {
            let bits = r.ub(4)? + 2;
            let control = (at.0 + r.sb(bits)?, at.1 + r.sb(bits)?);
            let to = (control.0 + r.sb(bits)?, control.1 + r.sb(bits)?);
            edges.push(Edge {
                group: groups.len() - 1,
                from: at,
                control: Some(control),
                to,
                fill0,
                fill1,
                line,
            });
            at = to;
        }
    }

    let mut layers = Vec::new();
    for (group, (fills, lines)) in groups.iter().enumerate() {
        let mine: Vec<&Edge> = edges.iter().filter(|edge| edge.group == group).collect();
        for (index, fill) in fills.iter().enumerate() {
            let style = index as u32 + 1;
            let directed: Vec<Segment> = mine
                .iter()
                .filter_map(|edge| {
                    if edge.fill0 == style && edge.fill1 == style {
                        // Inside the fill on both sides: not an outline.
                        None
                    } else if edge.fill1 == style {
                        Some(Segment::forward(edge))
                    } else if edge.fill0 == style {
                        Some(Segment::backward(edge))
                    } else {
                        None
                    }
                })
                .collect();
            let paths = chain(directed, true);
            if !paths.is_empty() {
                layers.push(VectorLayer {
                    paths,
                    fill: Some(fill.color()),
                    stroke: None,
                });
            }
        }
        for (index, style) in lines.iter().enumerate() {
            let number = index as u32 + 1;
            let segments: Vec<Segment> = mine
                .iter()
                .filter(|edge| edge.line == number)
                .map(|edge| Segment::forward(edge))
                .collect();
            let paths = chain(segments, false);
            if !paths.is_empty() {
                layers.push(VectorLayer {
                    paths,
                    fill: None,
                    stroke: Some(Stroke {
                        color: style.color,
                        width: style.width.max(1.0),
                        cap: style.cap,
                        join: style.join,
                    }),
                });
            }
        }
    }
    Ok(Shape {
        id,
        bounds,
        fills: groups.iter().map(|(fills, _)| fills.len()).sum(),
        lines: groups.iter().map(|(_, lines)| lines.len()).sum(),
        edges: edges.len(),
        layers,
    })
}

fn fill_styles(r: &mut Reader, version: u8) -> Result<Vec<Fill>, String> {
    let mut count = usize::from(r.u8()?);
    if count == 0xff && version >= 2 {
        count = usize::from(r.u16()?);
    }
    (0..count).map(|_| fill_style(r, version)).collect()
}

fn fill_style(r: &mut Reader, version: u8) -> Result<Fill, String> {
    let kind = r.u8()?;
    match kind {
        0x00 => Ok(Fill::Solid(if version >= 3 { r.rgba()? } else { r.rgb()? })),
        0x10 | 0x12 | 0x13 => {
            r.matrix()?;
            let header = r.u8()?;
            let stops = usize::from(header & 0x0f);
            let mut colors = Vec::with_capacity(stops);
            for _ in 0..stops {
                r.u8()?;
                colors.push(if version >= 3 { r.rgba()? } else { r.rgb()? });
            }
            if kind == 0x13 {
                r.i16()?;
            }
            Ok(Fill::Gradient(colors))
        }
        0x40..=0x43 => {
            r.u16()?;
            r.matrix()?;
            Ok(Fill::Bitmap)
        }
        other => Err(format!("unknown fill style type 0x{other:02x}")),
    }
}

fn line_styles(r: &mut Reader, version: u8) -> Result<Vec<Line>, String> {
    let mut count = usize::from(r.u8()?);
    if count == 0xff && version >= 2 {
        count = usize::from(r.u16()?);
    }
    let mut lines = Vec::with_capacity(count);
    for _ in 0..count {
        let width = f32::from(r.u16()?) / 20.0;
        if version >= 4 {
            let first = r.u8()?;
            let second = r.u8()?;
            let cap = match first >> 6 {
                1 => CapShape::Butt,
                2 => CapShape::Square,
                _ => CapShape::Round,
            };
            let join_code = (first >> 4) & 3;
            let join = match join_code {
                1 => JoinShape::Bevel,
                2 => JoinShape::Miter,
                _ => JoinShape::Round,
            };
            let has_fill = first & 0x08 != 0;
            let _ = second;
            if join_code == 2 {
                r.u16()?;
            }
            let color = if has_fill {
                fill_style(r, version)?.color()
            } else {
                r.rgba()?
            };
            lines.push(Line {
                width,
                color,
                cap,
                join,
            });
        } else {
            let color = if version >= 3 { r.rgba()? } else { r.rgb()? };
            lines.push(Line {
                width,
                color,
                cap: CapShape::Round,
                join: JoinShape::Round,
            });
        }
    }
    Ok(lines)
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    from: (i32, i32),
    control: Option<(i32, i32)>,
    to: (i32, i32),
}

impl Segment {
    fn forward(edge: &Edge) -> Self {
        Self {
            from: edge.from,
            control: edge.control,
            to: edge.to,
        }
    }

    fn backward(edge: &Edge) -> Self {
        Self {
            from: edge.to,
            control: edge.control,
            to: edge.from,
        }
    }
}

fn pixels((x, y): (i32, i32)) -> Vec2 {
    Vec2::new(x as f32 / 20.0, y as f32 / 20.0)
}

fn plain_anchor(point: Vec2) -> Anchor {
    Anchor {
        point,
        in_handle: None,
        out_handle: None,
    }
}

/// Joins segments end to start into paths. Fills want every path closed;
/// lines take them as they come.
fn chain(segments: Vec<Segment>, closed: bool) -> Vec<Path> {
    let mut starts: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        starts.entry(segment.from).or_default().push(index);
    }
    let mut used = vec![false; segments.len()];
    let mut paths = Vec::new();
    for first in 0..segments.len() {
        if used[first] {
            continue;
        }
        let origin = segments[first].from;
        let mut anchors = vec![plain_anchor(pixels(origin))];
        let mut current = first;
        loop {
            used[current] = true;
            let segment = segments[current];
            let end = pixels(segment.to);
            if let Some(control) = segment.control {
                let control = pixels(control);
                let start = anchors.last().map(|a| a.point).unwrap_or(end);
                if let Some(last) = anchors.last_mut() {
                    last.out_handle = Some(Vec2::new(
                        start.x + (control.x - start.x) * (2.0 / 3.0),
                        start.y + (control.y - start.y) * (2.0 / 3.0),
                    ));
                }
                anchors.push(Anchor {
                    point: end,
                    in_handle: Some(Vec2::new(
                        end.x + (control.x - end.x) * (2.0 / 3.0),
                        end.y + (control.y - end.y) * (2.0 / 3.0),
                    )),
                    out_handle: None,
                });
            } else {
                anchors.push(plain_anchor(end));
            }
            if segment.to == origin {
                break;
            }
            let next = starts
                .get(&segment.to)
                .and_then(|candidates| candidates.iter().copied().find(|i| !used[*i]));
            match next {
                Some(next) => current = next,
                None => break,
            }
        }
        if closed {
            // The closing point repeats the first; its handle moves over.
            if anchors.len() > 2
                && anchors.last().map(|a| (a.point.x, a.point.y))
                    == anchors.first().map(|a| (a.point.x, a.point.y))
            {
                let last = anchors.pop().expect("more than two anchors");
                anchors[0].in_handle = last.in_handle;
            }
            if anchors.len() >= 3 {
                paths.push(Path {
                    anchors,
                    closed: true,
                });
            }
        } else if anchors.len() >= 2 {
            paths.push(Path {
                anchors,
                closed: false,
            });
        }
    }
    paths
}

// ---------------------------------------------------------------- images

/// Width, height and straight RGBA of an image tag.
pub fn decode_image(
    tag: &SwfTag,
    jpeg_tables: Option<&[u8]>,
) -> Result<(u32, u32, Vec<u8>), String> {
    let p = &tag.payload;
    match tag.code {
        6 => {
            let data = p.get(2..).ok_or("DefineBits is empty")?;
            let joined = match jpeg_tables {
                Some(tables) if tables.len() > 4 => {
                    let mut joined = tables[..tables.len().saturating_sub(2)].to_vec();
                    joined.extend_from_slice(data.strip_prefix(&[0xff, 0xd8][..]).unwrap_or(data));
                    joined
                }
                _ => data.to_vec(),
            };
            decode_encoded(&joined)
        }
        21 => decode_encoded(p.get(2..).ok_or("DefineBitsJPEG2 is empty")?),
        35 | 90 => {
            let mut r = Reader::new(p);
            r.u16()?;
            let alpha_at = r.u32()? as usize;
            if tag.code == 90 {
                r.u16()?;
            }
            let start = r.position();
            let image = p
                .get(start..start.saturating_add(alpha_at))
                .ok_or("image data runs past its tag")?;
            let (width, height, mut rgba) = decode_encoded(image)?;
            let is_jpeg = image.starts_with(&[0xff, 0xd8]) || image.starts_with(&[0xff, 0xd9]);
            if is_jpeg {
                if let Some(alpha) = p.get(start + alpha_at..).filter(|a| !a.is_empty()) {
                    let alpha = inflate(alpha, (width * height) as usize)?;
                    for (pixel, a) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(alpha) {
                        pixel[3] = a;
                    }
                }
            }
            Ok((width, height, rgba))
        }
        20 | 36 => decode_lossless(tag),
        _ => Err(format!("{} is not an image", tag.name)),
    }
}

/// JPEG, PNG or GIF bytes. SWF JPEGs may carry a stray end-and-start
/// marker pair, which decoders refuse; it is cut out.
fn decode_encoded(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let cleaned;
    let bytes = if bytes.starts_with(&[0xff, 0xd8]) || bytes.starts_with(&[0xff, 0xd9]) {
        cleaned = strip_jpeg_markers(bytes);
        cleaned.as_slice()
    } else {
        bytes
    };
    let image = image::load_from_memory(bytes).map_err(|error| format!("decode image: {error}"))?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok((width, height, rgba.into_raw()))
}

fn strip_jpeg_markers(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    // A leading end marker before the real start.
    if bytes.starts_with(&[0xff, 0xd9, 0xff, 0xd8]) {
        i = 4;
        out.extend_from_slice(&[0xff, 0xd8]);
    }
    while i < bytes.len() {
        if i + 4 <= bytes.len() && bytes[i..i + 4] == [0xff, 0xd9, 0xff, 0xd8] {
            i += 4;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn inflate(bytes: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    if expected > MAX_IMAGE_PIXELS * 4 {
        return Err("image is too large".to_string());
    }
    let mut out = Vec::with_capacity(expected);
    flate2::read::ZlibDecoder::new(bytes)
        .take(expected as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|error| format!("inflate image data: {error}"))?;
    Ok(out)
}

fn decode_lossless(tag: &SwfTag) -> Result<(u32, u32, Vec<u8>), String> {
    let with_alpha = tag.code == 36;
    let mut r = Reader::new(&tag.payload);
    r.u16()?;
    let format = r.u8()?;
    let width = usize::from(r.u16()?);
    let height = usize::from(r.u16()?);
    if width * height > MAX_IMAGE_PIXELS {
        return Err("image is too large".to_string());
    }
    let palette_size = if format == 3 {
        usize::from(r.u8()?) + 1
    } else {
        0
    };
    let entry = if with_alpha { 4 } else { 3 };
    let row = |bytes_per_pixel: usize| (width * bytes_per_pixel).div_ceil(4) * 4;
    let expected = match format {
        3 => palette_size * entry + row(1) * height,
        4 => row(2) * height,
        5 => width * height * 4,
        other => return Err(format!("unknown lossless format {other}")),
    };
    let data = inflate(r.rest(), expected)?;
    if data.len() < expected {
        return Err("lossless image data is short".to_string());
    }
    let mut rgba = vec![0u8; width * height * 4];
    match format {
        3 => {
            let palette = &data[..palette_size * entry];
            let pixels = &data[palette_size * entry..];
            for y in 0..height {
                for x in 0..width {
                    let index = usize::from(pixels[y * row(1) + x]);
                    let out = &mut rgba[(y * width + x) * 4..][..4];
                    if index < palette_size {
                        let color = &palette[index * entry..][..entry];
                        out[..3].copy_from_slice(&color[..3]);
                        out[3] = if with_alpha { color[3] } else { 255 };
                    }
                }
            }
        }
        4 => {
            for y in 0..height {
                for x in 0..width {
                    let at = y * row(2) + x * 2;
                    let value = u16::from_be_bytes([data[at], data[at + 1]]);
                    let five = |v: u16| ((v & 0x1f) as u32 * 255 / 31) as u8;
                    let out = &mut rgba[(y * width + x) * 4..][..4];
                    out.copy_from_slice(&[five(value >> 10), five(value >> 5), five(value), 255]);
                }
            }
        }
        _ => {
            for (out, pixel) in rgba
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(data.as_chunks::<4>().0.iter())
            {
                let alpha = if with_alpha { pixel[0] } else { 255 };
                // Lossless2 colour is premultiplied by its alpha.
                let straight = |c: u8| {
                    if with_alpha && alpha > 0 && alpha < 255 {
                        ((u32::from(c) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255)
                            as u8
                    } else {
                        c
                    }
                };
                out.copy_from_slice(&[
                    straight(pixel[1]),
                    straight(pixel[2]),
                    straight(pixel[3]),
                    alpha,
                ]);
            }
        }
    }
    Ok((width as u32, height as u32, rgba))
}

// ---------------------------------------------------------------- sounds

pub struct Sound {
    pub id: u16,
    pub format: u8,
    pub rate: u32,
    pub sixteen_bit: bool,
    pub stereo: bool,
    pub samples: u32,
    pub data_at: usize,
}

impl Sound {
    pub fn format_name(&self) -> &'static str {
        match self.format {
            0 => "uncompressed (native endian)",
            1 => "ADPCM",
            2 => "MP3",
            3 => "uncompressed (little endian)",
            4..=6 => "Nellymoser",
            11 => "Speex",
            _ => "unknown",
        }
    }

    /// The file this sound saves as: MP3 as it is, PCM as WAV.
    pub fn file(&self, tag: &SwfTag) -> (&'static str, Vec<u8>) {
        let data = tag.payload.get(self.data_at..).unwrap_or(&[]);
        match self.format {
            2 => ("mp3", data.get(2..).unwrap_or(&[]).to_vec()),
            0 | 3 => (
                "wav",
                wav(
                    data,
                    self.rate,
                    if self.stereo { 2 } else { 1 },
                    self.sixteen_bit,
                ),
            ),
            _ => ("snd", data.to_vec()),
        }
    }
}

pub fn parse_sound(tag: &SwfTag) -> Result<Sound, String> {
    let mut r = Reader::new(&tag.payload);
    let id = r.u16()?;
    let format = r.ub(4)? as u8;
    let rate = [5512, 11025, 22050, 44100][r.ub(2)? as usize];
    let sixteen_bit = r.ub(1)? == 1;
    let stereo = r.ub(1)? == 1;
    let samples = r.u32()?;
    Ok(Sound {
        id,
        format,
        rate,
        sixteen_bit,
        stereo,
        samples,
        data_at: r.position(),
    })
}

pub fn wav(pcm: &[u8], rate: u32, channels: u16, sixteen_bit: bool) -> Vec<u8> {
    let bits: u16 = if sixteen_bit { 16 } else { 8 };
    let block = channels * bits / 8;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

// ----------------------------------------------------- texts and fonts

pub struct EditText {
    pub id: u16,
    pub variable: String,
    pub text: Option<String>,
    pub html: bool,
    pub font_height: Option<f32>,
    pub color: Option<Rgba>,
}

pub fn parse_edit_text(tag: &SwfTag) -> Result<EditText, String> {
    let mut r = Reader::new(&tag.payload);
    let id = r.u16()?;
    r.rect()?;
    let flags = r.u8()?;
    let flags2 = r.u8()?;
    let has_font = flags & 0x01 != 0;
    if has_font {
        r.u16()?;
    }
    if flags2 & 0x80 != 0 {
        r.string()?;
    }
    let font_height = if has_font {
        Some(f32::from(r.u16()?) / 20.0)
    } else {
        None
    };
    let color = if flags & 0x04 != 0 {
        Some(r.rgba()?)
    } else {
        None
    };
    if flags & 0x02 != 0 {
        r.u16()?;
    }
    if flags2 & 0x20 != 0 {
        r.take(9)?;
    }
    let variable = r.string()?;
    let text = if flags & 0x80 != 0 {
        Some(r.string()?)
    } else {
        None
    };
    Ok(EditText {
        id,
        variable,
        text,
        html: flags2 & 0x02 != 0,
        font_height,
        color,
    })
}

/// A font's name and how many glyphs it has, where its tag says.
pub fn font_name(tag: &SwfTag) -> Option<(String, u16)> {
    let mut r = Reader::new(&tag.payload);
    r.u16().ok()?;
    match tag.code {
        48 | 75 => {
            r.u8().ok()?;
            r.u8().ok()?;
            let length = usize::from(r.u8().ok()?);
            let name = decode_text(r.take(length).ok()?)
                .trim_end_matches('\0')
                .to_string();
            Some((name, r.u16().ok()?))
        }
        91 => {
            r.u8().ok()?;
            Some((r.string().ok()?, 0))
        }
        10 => {
            let first = r.u16().ok()?;
            Some((String::new(), first / 2))
        }
        _ => None,
    }
}

/// The (id, name) pairs ExportAssets and SymbolClass give.
pub fn exported_names(tag: &SwfTag) -> Vec<(u16, String)> {
    let mut r = Reader::new(&tag.payload);
    let Ok(count) = r.u16() else {
        return Vec::new();
    };
    let mut names = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let (Ok(id), Ok(name)) = (r.u16(), r.string()) else {
            break;
        };
        names.push((id, name));
    }
    names
}

// ---------------------------------------------------------------- scripts

fn action_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0x00 => "End",
        0x04 => "NextFrame",
        0x05 => "PrevFrame",
        0x06 => "Play",
        0x07 => "Stop",
        0x08 => "ToggleQuality",
        0x09 => "StopSounds",
        0x0a => "Add",
        0x0b => "Subtract",
        0x0c => "Multiply",
        0x0d => "Divide",
        0x0e => "Equals",
        0x0f => "Less",
        0x10 => "And",
        0x11 => "Or",
        0x12 => "Not",
        0x13 => "StringEquals",
        0x14 => "StringLength",
        0x15 => "StringExtract",
        0x17 => "Pop",
        0x18 => "ToInteger",
        0x1c => "GetVariable",
        0x1d => "SetVariable",
        0x20 => "SetTarget2",
        0x21 => "StringAdd",
        0x22 => "GetProperty",
        0x23 => "SetProperty",
        0x24 => "CloneSprite",
        0x25 => "RemoveSprite",
        0x26 => "Trace",
        0x27 => "StartDrag",
        0x28 => "EndDrag",
        0x29 => "StringLess",
        0x2a => "Throw",
        0x2b => "CastOp",
        0x2c => "ImplementsOp",
        0x30 => "RandomNumber",
        0x31 => "MBStringLength",
        0x32 => "CharToAscii",
        0x33 => "AsciiToChar",
        0x34 => "GetTime",
        0x35 => "MBStringExtract",
        0x36 => "MBCharToAscii",
        0x37 => "MBAsciiToChar",
        0x3a => "Delete",
        0x3b => "Delete2",
        0x3c => "DefineLocal",
        0x3d => "CallFunction",
        0x3e => "Return",
        0x3f => "Modulo",
        0x40 => "NewObject",
        0x41 => "DefineLocal2",
        0x42 => "InitArray",
        0x43 => "InitObject",
        0x44 => "TypeOf",
        0x45 => "TargetPath",
        0x46 => "Enumerate",
        0x47 => "Add2",
        0x48 => "Less2",
        0x49 => "Equals2",
        0x4a => "ToNumber",
        0x4b => "ToString",
        0x4c => "PushDuplicate",
        0x4d => "StackSwap",
        0x4e => "GetMember",
        0x4f => "SetMember",
        0x50 => "Increment",
        0x51 => "Decrement",
        0x52 => "CallMethod",
        0x53 => "NewMethod",
        0x54 => "InstanceOf",
        0x55 => "Enumerate2",
        0x60 => "BitAnd",
        0x61 => "BitOr",
        0x62 => "BitXor",
        0x63 => "BitLShift",
        0x64 => "BitRShift",
        0x65 => "BitURShift",
        0x66 => "StrictEquals",
        0x67 => "Greater",
        0x68 => "StringGreater",
        0x69 => "Extends",
        0x81 => "GotoFrame",
        0x83 => "GetURL",
        0x87 => "StoreRegister",
        0x88 => "ConstantPool",
        0x8a => "WaitForFrame",
        0x8b => "SetTarget",
        0x8c => "GotoLabel",
        0x8d => "WaitForFrame2",
        0x8e => "DefineFunction2",
        0x8f => "Try",
        0x94 => "With",
        0x96 => "Push",
        0x99 => "Jump",
        0x9a => "GetURL2",
        0x9b => "DefineFunction",
        0x9d => "If",
        0x9e => "Call",
        0x9f => "GotoFrame2",
        _ => return None,
    })
}

fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// AS1/2 bytecode as P-code, one action a line, the way JPEXS lists it.
pub fn disassemble(code: &[u8]) -> String {
    let mut out = String::new();
    let mut pool: Vec<String> = Vec::new();
    let mut at = 0usize;
    let mut lines = 0usize;
    while at < code.len() {
        if lines > 200_000 {
            out.push_str("; … (listing stops here)\n");
            break;
        }
        lines += 1;
        let start = at;
        let op = code[at];
        at += 1;
        let payload: &[u8] = if op >= 0x80 {
            let Some(len) = code.get(at..at + 2) else {
                let _ = writeln!(out, "loc{start:04x}: ; truncated action 0x{op:02x}");
                break;
            };
            let len = usize::from(u16::from_le_bytes([len[0], len[1]]));
            at += 2;
            let end = (at + len).min(code.len());
            let payload = &code[at..end];
            at = end;
            payload
        } else {
            &[]
        };
        let name = action_name(op).map_or_else(|| format!("Unknown0x{op:02X}"), str::to_string);
        let mut r = Reader::new(payload);
        let args = match op {
            0x88 => {
                pool.clear();
                if let Ok(count) = r.u16() {
                    for _ in 0..count {
                        let Ok(text) = r.string() else { break };
                        pool.push(text);
                    }
                }
                pool.iter()
                    .map(|s| quoted(s))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
            0x96 => {
                let mut items = Vec::new();
                while !r.is_empty() {
                    let Ok(kind) = r.u8() else { break };
                    let item = match kind {
                        0 => r.string().map(|s| quoted(&s)),
                        1 => r.u32().map(|v| format!("{}", f32::from_bits(v))),
                        2 => Ok("null".to_string()),
                        3 => Ok("undefined".to_string()),
                        4 => r.u8().map(|v| format!("register{v}")),
                        5 => r.u8().map(|v| (v != 0).to_string()),
                        6 => r.take(8).map(|b| {
                            // Doubles are stored with their halves swapped.
                            let mut bytes = [0u8; 8];
                            bytes[..4].copy_from_slice(&b[4..8]);
                            bytes[4..].copy_from_slice(&b[..4]);
                            format!("{}", f64::from_le_bytes(bytes))
                        }),
                        7 => r.u32().map(|v| format!("{}", v as i32)),
                        8 => r.u8().map(|v| {
                            pool.get(usize::from(v))
                                .map_or(format!("constant{v}"), |s| quoted(s))
                        }),
                        9 => r.u16().map(|v| {
                            pool.get(usize::from(v))
                                .map_or(format!("constant{v}"), |s| quoted(s))
                        }),
                        other => Err(format!("unknown push type {other}")),
                    };
                    match item {
                        Ok(item) => items.push(item),
                        Err(_) => break,
                    }
                }
                items.join(", ")
            }
            0x81 => r.u16().map(|v| v.to_string()).unwrap_or_default(),
            0x83 => {
                let url = r.string().unwrap_or_default();
                let target = r.string().unwrap_or_default();
                format!("{}, {}", quoted(&url), quoted(&target))
            }
            0x87 | 0x8d => r.u8().map(|v| v.to_string()).unwrap_or_default(),
            0x8a => r.u16().map(|v| v.to_string()).unwrap_or_default(),
            0x8b | 0x8c => r.string().map(|s| quoted(&s)).unwrap_or_default(),
            0x99 | 0x9d => r
                .i16()
                .map(|v| format!("loc{:04x}", (at as i64 + i64::from(v)).max(0)))
                .unwrap_or_default(),
            0x9a => r.u8().map(|v| v.to_string()).unwrap_or_default(),
            0x9b => {
                let name = r.string().unwrap_or_default();
                let count = r.u16().unwrap_or(0);
                let params: Vec<String> = (0..count).filter_map(|_| r.string().ok()).collect();
                let size = r.u16().unwrap_or(0);
                format!(
                    "{}, {} ({}) {{ {size} bytes",
                    quoted(&name),
                    count,
                    params.join(", ")
                )
            }
            0x8e => {
                let name = r.string().unwrap_or_default();
                let count = r.u16().unwrap_or(0);
                let registers = r.u8().unwrap_or(0);
                r.u16().ok();
                let params: Vec<String> = (0..count)
                    .filter_map(|_| {
                        let register = r.u8().ok()?;
                        let param = r.string().ok()?;
                        Some(format!("{register}:{param}"))
                    })
                    .collect();
                let size = r.u16().unwrap_or(0);
                format!(
                    "{}, {} params, {registers} registers ({}) {{ {size} bytes",
                    quoted(&name),
                    count,
                    params.join(", ")
                )
            }
            0x94 => r.u16().map(|v| format!("{{ {v} bytes")).unwrap_or_default(),
            0x9f => r
                .u8()
                .map(|v| format!("play: {}", v & 1 != 0))
                .unwrap_or_default(),
            _ => String::new(),
        };
        if args.is_empty() {
            let _ = writeln!(out, "loc{start:04x}: {name}");
        } else {
            let _ = writeln!(out, "loc{start:04x}: {name} {args}");
        }
        if op == 0x00 {
            break;
        }
    }
    out
}

/// Where a script tag's bytecode starts.
pub fn action_code(tag: &SwfTag) -> Option<&[u8]> {
    match tag.code {
        12 => Some(&tag.payload),
        59 => tag.payload.get(2..),
        _ => None,
    }
}

/// An AS3 block's name and the strings in its constant pool.
pub fn abc_strings(tag: &SwfTag) -> Result<(String, Vec<String>), String> {
    let mut r = Reader::new(&tag.payload);
    r.u32()?;
    let name = r.string()?;
    let abc = r.rest();
    let mut at = 4usize;
    let u30 = |at: &mut usize| -> Result<u32, String> {
        let mut value = 0u32;
        for shift in (0..35).step_by(7) {
            let byte = *abc.get(*at).ok_or("abc is truncated")?;
            *at += 1;
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
        }
        Ok(value)
    };
    let ints = u30(&mut at)?;
    for _ in 1..ints {
        u30(&mut at)?;
    }
    let uints = u30(&mut at)?;
    for _ in 1..uints {
        u30(&mut at)?;
    }
    let doubles = u30(&mut at)?;
    at += 8 * doubles.saturating_sub(1) as usize;
    let count = u30(&mut at)?;
    let mut strings = Vec::new();
    for _ in 1..count.min(200_000) {
        let length = u30(&mut at)? as usize;
        let bytes = abc
            .get(at..at + length)
            .ok_or("abc string runs past its end")?;
        at += length;
        strings.push(decode_text(bytes));
    }
    Ok((name, strings))
}

/// A short line about what a tag holds, for its tree label.
pub fn describe(tag: &SwfTag) -> Option<String> {
    match tag.code {
        9 => Reader::new(&tag.payload)
            .rgb()
            .ok()
            .map(|c| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)),
        43 => Reader::new(&tag.payload)
            .string()
            .ok()
            .map(|label| quoted(&label)),
        10 | 48 | 75 | 91 => font_name(tag).map(|(name, glyphs)| {
            if glyphs > 0 {
                format!("{name} ({glyphs} glyphs)")
            } else {
                name
            }
        }),
        14 => parse_sound(tag)
            .ok()
            .map(|s| format!("{} {} Hz", s.format_name(), s.rate)),
        37 => parse_edit_text(tag).ok().map(|text| {
            let shown = text.text.unwrap_or_default();
            let short: String = shown.chars().take(32).collect();
            if text.variable.is_empty() {
                quoted(&short)
            } else {
                format!("{} {}", text.variable, quoted(&short))
            }
        }),
        39 => {
            Reader::new(&tag.payload)
                .take(4)
                .ok()
                .map(|b| match u16::from_le_bytes([b[2], b[3]]) {
                    1 => "1 frame".to_string(),
                    frames => format!("{frames} frames"),
                })
        }
        20 | 36 => {
            let mut r = Reader::new(&tag.payload);
            r.take(3).ok()?;
            Some(format!("{}×{}", r.u16().ok()?, r.u16().ok()?))
        }
        82 => {
            let mut r = Reader::new(&tag.payload);
            r.u32().ok()?;
            r.string().ok().filter(|name| !name.is_empty())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(code: u16, payload: Vec<u8>) -> SwfTag {
        SwfTag {
            index: 0,
            code,
            name: crate::swf::tag_name(code),
            payload_offset: 0,
            payload,
        }
    }

    /// Bits written most significant first, as SWF bit fields are.
    #[derive(Default)]
    struct Bits {
        bytes: Vec<u8>,
        used: u8,
    }

    impl Bits {
        fn put(&mut self, value: i64, count: u32) {
            for i in (0..count).rev() {
                if self.used == 0 {
                    self.bytes.push(0);
                }
                let bit = ((value >> i) & 1) as u8;
                let last = self.bytes.last_mut().unwrap();
                *last |= bit << (7 - self.used);
                self.used = (self.used + 1) % 8;
            }
        }
    }

    /// A 10×10 px red square: DefineShape with one solid fill.
    fn square_shape() -> SwfTag {
        let mut payload = vec![7, 0];
        let mut bits = Bits::default();
        // RECT 0..200 twips, 9 bits a field.
        bits.put(9, 5);
        for v in [0, 200, 0, 200] {
            bits.put(v, 9);
        }
        payload.extend(bits.bytes);
        payload.extend([1, 0x00, 255, 0, 0]); // one solid red fill
        payload.push(0); // no lines
        let mut bits = Bits::default();
        bits.put(1, 4); // fill bits
        bits.put(0, 4); // line bits
                        // Style change: move to 0,0 and fill1 = 1.
        bits.put(0, 1);
        bits.put(0b00101, 5);
        bits.put(1, 5);
        bits.put(0, 1);
        bits.put(0, 1);
        bits.put(1, 1);
        // Four straight edges round the square, clockwise.
        for (dx, dy) in [(200, 0), (0, 200), (-200, 0), (0, -200)] {
            bits.put(1, 1);
            bits.put(1, 1);
            bits.put(7, 4); // 9 bits
            bits.put(1, 1);
            bits.put(dx, 9);
            bits.put(dy, 9);
        }
        bits.put(0, 1);
        bits.put(0, 5);
        payload.extend(bits.bytes);
        tag(2, payload)
    }

    #[test]
    fn a_shape_becomes_a_closed_filled_path() {
        let shape = parse_shape(&square_shape()).unwrap();
        assert_eq!(shape.id, 7);
        assert_eq!(shape.bounds.x_max, 10.0);
        assert_eq!(shape.layers.len(), 1);
        let layer = &shape.layers[0];
        assert_eq!(layer.fill.map(|c| (c.r, c.g, c.b)), Some((255, 0, 0)));
        assert_eq!(layer.paths.len(), 1);
        assert!(layer.paths[0].closed);
        assert_eq!(layer.paths[0].anchors.len(), 4);
        let bounds = layer_bounds(&shape.layers).unwrap();
        assert_eq!((bounds.x_max, bounds.y_max), (10.0, 10.0));
    }

    #[test]
    fn placed_shapes_move_with_their_matrix() {
        let shape = square_shape();
        let mut place = vec![0x02 | 0x04, 1, 0, 7, 0];
        let mut bits = Bits::default();
        bits.put(0, 1); // no scale
        bits.put(0, 1); // no rotation
        bits.put(11, 5);
        bits.put(400, 11); // 20 px right
        bits.put(0, 11);
        place.extend(bits.bytes);
        let place = SwfTag {
            index: 1,
            ..tag(26, place)
        };
        let show = SwfTag {
            index: 2,
            ..tag(1, Vec::new())
        };
        let end = SwfTag {
            index: 3,
            ..tag(0, Vec::new())
        };
        let movie = SwfMovie {
            cache: Default::default(),
            compression: crate::swf::SwfCompression::None,
            version: 8,
            declared_file_length: 0,
            source_length: 0,
            width_px: 100.0,
            height_px: 100.0,
            frame_rate: 24.0,
            frame_count: 1,
            tags: vec![shape, place, show, end],
            normalized_fws: Vec::new(),
        };
        let library = Library::new(&movie);
        let layers = library.draw_frame(0).unwrap();
        let bounds = layer_bounds(&layers).unwrap();
        assert_eq!((bounds.x_min, bounds.x_max), (20.0, 30.0));
    }

    #[test]
    fn lossless_images_decode_their_palette() {
        use std::io::Write as _;
        // 2×1 colour-mapped: palette red, blue; pixels 0, 1; rows padded to 4.
        let mut raw = vec![255, 0, 0, 0, 0, 255];
        raw.extend([0, 1, 0, 0]);
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&raw).unwrap();
        let mut payload = vec![1, 0, 3, 2, 0, 1, 0, 1];
        payload.extend(z.finish().unwrap());
        let (w, h, rgba) = decode_image(&tag(20, payload), None).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn pcode_lists_actions_with_their_constants() {
        // ConstantPool "stop" ; Push constant 0 ; Stop ; End
        let code = vec![
            0x88, 7, 0, 1, 0, b's', b't', b'o', b'p', 0, //
            0x96, 2, 0, 8, 0, //
            0x07, 0x00,
        ];
        let listing = disassemble(&code);
        assert!(listing.contains("ConstantPool \"stop\""), "{listing}");
        assert!(listing.contains("Push \"stop\""), "{listing}");
        assert!(listing.contains("Stop"), "{listing}");
        assert!(listing.trim_end().ends_with("End"), "{listing}");
    }

    #[test]
    fn jpeg_marker_pairs_are_cut_out() {
        assert_eq!(
            strip_jpeg_markers(&[0xff, 0xd8, 1, 0xff, 0xd9, 0xff, 0xd8, 2, 0xff, 0xd9]),
            vec![0xff, 0xd8, 1, 2, 0xff, 0xd9]
        );
        assert_eq!(
            strip_jpeg_markers(&[0xff, 0xd9, 0xff, 0xd8, 3]),
            vec![0xff, 0xd8, 3]
        );
    }
}
