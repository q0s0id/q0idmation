//! Flash2Q0: rebuilds a SWF movie, the file Flash Player runs, as a q0s
//! project.
//!
//! Shapes, bitmaps, movie clips, buttons, text and sounds become library
//! items; every timeline becomes a q0rg with one layer per depth and a
//! keyframe wherever an object on it changes. Frame code is translated where
//! q0lang already says the same thing (stop, play, go to a frame or label,
//! open a link, log) and kept as `# AS:` comments otherwise, so nothing is
//! lost for the full ActionScript translation that comes later.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use q0s_format::geom::{CapShape, JoinShape};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Asset, AudioClip, BitmapAsset, BlendMode, BlurFx, DropShadowFx, FrameLabelKey, FrameScript,
    GlowFx, GradientFill, GradientKind, GradientStop, InstanceKey, Layer, LayerKey, LayerKind,
    LayerMetadata, LibraryItemReference, Placement, PlacementFx, ProjectMeta, ProjectV2, Q0rg,
    Q0rgPlayback, Q0vAsset, Rgba, Stroke, Target, TintFx, Transform2D, Tween, Vec2,
};
use swf::avm1::types::{Action, Value as AsValue};
use swf::{
    AudioCompression, ButtonState, CharacterId, ColorTransform, FillStyle, LineCapStyle,
    LineJoinStyle, LineStyle, Matrix, PlaceObject, PlaceObjectAction, ShapeRecord, ShapeStyles,
    SoundEvent, SoundFormat, SwfStr, Tag, TextAlign,
};

use crate::xfl_content::{
    affine_to_transform, shape_targets, RawEdge, RawFillStyle, RawShape, Segment,
};
use crate::{ImportError, ImportReport, ImportedProject};

/// The main timeline's q0rg.
const MAIN: u16 = 1;
/// Flash's gradient square is 32768 twips wide; q0s gradients span -1..1.
const GRADIENT_HALF_PX: f32 = 16_384.0 / 20.0;

pub fn import_swf(path: &Path) -> Result<ImportedProject, ImportError> {
    crate::xfl_content::forget_drawings();
    let bytes = std::fs::read(path)?;
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("movie")
        .to_string();
    let mut imported = import_swf_bytes(&bytes, &name)?;
    crate::masks::normalize(&mut imported)?;
    Ok(imported)
}

pub fn import_swf_bytes(bytes: &[u8], name: &str) -> Result<ImportedProject, ImportError> {
    let buffer = swf::decompress_swf(bytes)
        .map_err(|error| ImportError::Invalid(format!("not a readable SWF: {error}")))?;
    let movie = swf::parse_swf(&buffer)
        .map_err(|error| ImportError::Invalid(format!("SWF tags could not be read: {error}")))?;
    let header = &movie.header;
    let stage = header.stage_size();
    let width = ((stage.x_max.get() - stage.x_min.get()) as f32 / 20.0).round();
    let height = ((stage.y_max.get() - stage.y_min.get()) as f32 / 20.0).round();
    let fps = header.frame_rate().to_f32().round().clamp(1.0, 240.0) as u16;
    let background = header.background_color().map_or(
        Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        },
        |color| rgba(&color),
    );

    let mut convert = Converter {
        version: header.version(),
        project: ProjectV2 {
            meta: ProjectMeta {
                name: name.to_string(),
                fps,
                stage_width: width.clamp(1.0, f32::from(u16::MAX)) as u16,
                stage_height: height.clamp(1.0, f32::from(u16::MAX)) as u16,
                background_color: Rgba {
                    a: 255,
                    ..background
                },
                entry_q0rg_id: MAIN,
            },
            assets: Vec::new(),
            asset_names: HashMap::new(),
            asset_appearances: Default::default(),
            layer_metadata: HashMap::new(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: Vec::new(),
        },
        next_asset: 1,
        next_q0rg: MAIN + 1,
        next_instance: 1,
        characters: HashMap::new(),
        movie_clips: HashSet::new(),
        bitmaps: HashMap::new(),
        fonts: HashMap::new(),
        sounds: HashMap::new(),
        exports: HashMap::new(),
        jpeg_tables: None,
        folders: Vec::new(),
        notes: BTreeMap::new(),
        layers: 0,
        keyframes: 0,
        scripts: ScriptCounts::default(),
    };
    if header.is_action_script_3() {
        convert.note("this movie runs ActionScript 3, which Flash2Q0 does not translate yet; its art, timelines and sounds are converted");
    }
    convert.collect_exports(&movie.tags);
    convert.define_all(&movie.tags);
    let main = convert.timeline(MAIN, "Main timeline", &movie.tags, header.num_frames());
    convert.project.q0rgs.insert(0, main);
    convert.finish()
}

#[derive(Default)]
struct ScriptCounts {
    /// Frame scripts that became q0lang entirely.
    translated: usize,
    /// Frame scripts with ActionScript kept as comments.
    partly: usize,
    /// `#initclip` blocks (AS2 classes and the like) and their bytecode size.
    class_blocks: usize,
    class_bytes: usize,
    /// Button and clip event handlers (`on (release)`, `onClipEvent`).
    handlers: usize,
    /// Text fields bound to a variable.
    dynamic_text: usize,
}

struct FontGlyphs {
    /// Units per em: 1024, or 20480 for DefineFont3.
    em: f32,
    glyphs: Vec<Vec<ShapeRecord>>,
    codes: Vec<u16>,
    advances: Vec<f32>,
    ascent: f32,
    descent: f32,
    leading: f32,
}

#[derive(Clone, Copy)]
struct ImportedSound {
    asset_id: u16,
    samples: u64,
    rate: u32,
}

/// What stands at one depth of a timeline.
#[derive(Clone, PartialEq)]
struct Shown {
    character: CharacterId,
    matrix: Affine,
    fx: PlacementFx,
    name: Option<String>,
    clip_depth: Option<u16>,
    instance: u32,
}

struct Converter {
    version: u8,
    project: ProjectV2,
    next_asset: u16,
    next_q0rg: u16,
    next_instance: u32,
    /// Placeable library item for every SWF character.
    characters: HashMap<CharacterId, Target>,
    /// q0rgs that came from movie clips: they keep their own clocks.
    movie_clips: HashSet<u16>,
    bitmaps: HashMap<CharacterId, u16>,
    fonts: HashMap<CharacterId, FontGlyphs>,
    sounds: HashMap<CharacterId, ImportedSound>,
    /// Linkage names from ExportAssets.
    exports: HashMap<CharacterId, String>,
    jpeg_tables: Option<Vec<u8>>,
    /// Library items and the folder each goes in.
    folders: Vec<(LibraryItemReference, &'static str)>,
    /// Warnings by text, with how often each came up.
    notes: BTreeMap<String, usize>,
    layers: usize,
    keyframes: usize,
    scripts: ScriptCounts,
}

fn rgba(color: &swf::Color) -> Rgba {
    Rgba {
        r: color.r,
        g: color.g,
        b: color.b,
        a: color.a,
    }
}

/// A SWF matrix in pixels: its translation is in twips.
fn matrix_px(matrix: &Matrix) -> Affine {
    Affine {
        a11: matrix.a.to_f32(),
        a12: matrix.c.to_f32(),
        a21: matrix.b.to_f32(),
        a22: matrix.d.to_f32(),
        tx: matrix.tx.get() as f32 / 20.0,
        ty: matrix.ty.get() as f32 / 20.0,
    }
}

fn twips(x: i32, y: i32) -> Vec2 {
    Vec2::new(x as f32 / 20.0, y as f32 / 20.0)
}

fn nonzero(style: u16) -> Option<u16> {
    (style != 0).then_some(style)
}

fn gradient(gradient: &swf::Gradient, kind: GradientKind) -> RawFillStyle {
    let mut transform = matrix_px(&gradient.matrix);
    transform.a11 *= GRADIENT_HALF_PX;
    transform.a12 *= GRADIENT_HALF_PX;
    transform.a21 *= GRADIENT_HALF_PX;
    transform.a22 *= GRADIENT_HALF_PX;
    RawFillStyle::Gradient(GradientFill {
        kind,
        stops: gradient
            .records
            .iter()
            .map(|record| GradientStop {
                position: f32::from(record.ratio) / 255.0,
                color: rgba(&record.color),
            })
            .collect(),
        transform,
    })
}

fn fill_style(fill: &FillStyle) -> RawFillStyle {
    match fill {
        FillStyle::Color(color) => RawFillStyle::Solid(rgba(color)),
        FillStyle::LinearGradient(g) => gradient(g, GradientKind::Linear),
        FillStyle::RadialGradient(g) => gradient(g, GradientKind::Radial),
        FillStyle::FocalGradient { gradient: g, .. } => gradient(g, GradientKind::Radial),
        FillStyle::Bitmap {
            id,
            matrix,
            is_repeating,
            ..
        } => {
            // Bitmap fill matrices map texels to twips at 20 per pixel.
            let mut transform = matrix_px(matrix);
            transform.a11 /= 20.0;
            transform.a12 /= 20.0;
            transform.a21 /= 20.0;
            transform.a22 /= 20.0;
            RawFillStyle::Bitmap {
                reference: u32::from(*id),
                name: None,
                transform,
                tiled: *is_repeating,
            }
        }
    }
}

fn line_style(line: &LineStyle) -> Stroke {
    let color = match line.fill_style() {
        FillStyle::Color(color) => rgba(color),
        FillStyle::LinearGradient(g)
        | FillStyle::RadialGradient(g)
        | FillStyle::FocalGradient { gradient: g, .. } => g.records.first().map_or(
            Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            |record| rgba(&record.color),
        ),
        FillStyle::Bitmap { .. } => Rgba {
            r: 128,
            g: 128,
            b: 128,
            a: 255,
        },
    };
    let width = line.width().get() as f32 / 20.0;
    Stroke {
        color,
        // Width 0 is Flash's hairline: one pixel at any zoom.
        width: if width <= 0.0 { 1.0 } else { width },
        cap: match line.start_cap() {
            LineCapStyle::Round => CapShape::Round,
            LineCapStyle::None => CapShape::Butt,
            LineCapStyle::Square => CapShape::Square,
        },
        join: match line.join_style() {
            LineJoinStyle::Round => JoinShape::Round,
            LineJoinStyle::Bevel => JoinShape::Bevel,
            LineJoinStyle::Miter(_) => JoinShape::Miter,
        },
    }
}

fn empty_shape() -> RawShape {
    RawShape {
        affine: Affine::IDENTITY,
        fills: HashMap::new(),
        strokes: HashMap::new(),
        edges: Vec::new(),
        widths: HashMap::new(),
    }
}

/// A DefineShape's records as the edges q0fla turns into paths. Styles that
/// a record introduces get indices after the earlier ones.
fn raw_shape(styles: &ShapeStyles, records: &[ShapeRecord]) -> RawShape {
    let mut shape = empty_shape();
    let mut next_fill = 1_u16;
    let mut next_line = 1_u16;
    let mut register = |styles: &ShapeStyles, shape: &mut RawShape| -> (u16, u16) {
        let bases = (next_fill - 1, next_line - 1);
        for fill in &styles.fill_styles {
            shape.fills.insert(next_fill, fill_style(fill));
            next_fill = next_fill.saturating_add(1);
        }
        for line in &styles.line_styles {
            shape.strokes.insert(next_line, line_style(line));
            next_line = next_line.saturating_add(1);
        }
        bases
    };
    let (mut fill_base, mut line_base) = register(styles, &mut shape);
    let (mut x, mut y) = (0_i32, 0_i32);
    let (mut fill0, mut fill1, mut line) = (0_u16, 0_u16, 0_u16);
    let mut segments: Vec<Segment> = Vec::new();
    let flush = |segments: &mut Vec<Segment>, shape: &mut RawShape, f0: u16, f1: u16, l: u16| {
        if !segments.is_empty() {
            shape.edges.push(RawEdge {
                fill0: nonzero(f0),
                fill1: nonzero(f1),
                stroke: nonzero(l),
                segments: std::mem::take(segments),
            });
        }
    };
    for record in records {
        match record {
            ShapeRecord::StyleChange(change) => {
                flush(&mut segments, &mut shape, fill0, fill1, line);
                if let Some(styles) = &change.new_styles {
                    (fill_base, line_base) = register(styles, &mut shape);
                    (fill0, fill1, line) = (0, 0, 0);
                }
                let pick = |value: u32, base: u16| {
                    if value == 0 {
                        0
                    } else {
                        base.saturating_add(value.min(u32::from(u16::MAX)) as u16)
                    }
                };
                if let Some(value) = change.fill_style_0 {
                    fill0 = pick(value, fill_base);
                }
                if let Some(value) = change.fill_style_1 {
                    fill1 = pick(value, fill_base);
                }
                if let Some(value) = change.line_style {
                    line = pick(value, line_base);
                }
                if let Some(point) = change.move_to {
                    (x, y) = (point.x.get(), point.y.get());
                }
            }
            ShapeRecord::StraightEdge { delta } => {
                let start = twips(x, y);
                x += delta.dx.get();
                y += delta.dy.get();
                segments.push(Segment {
                    start,
                    end: twips(x, y),
                    control: None,
                });
            }
            ShapeRecord::CurvedEdge {
                control_delta,
                anchor_delta,
            } => {
                let start = twips(x, y);
                let (cx, cy) = (x + control_delta.dx.get(), y + control_delta.dy.get());
                (x, y) = (cx + anchor_delta.dx.get(), cy + anchor_delta.dy.get());
                segments.push(Segment {
                    start,
                    end: twips(x, y),
                    control: Some(twips(cx, cy)),
                });
            }
        }
    }
    flush(&mut segments, &mut shape, fill0, fill1, line);
    shape
}

/// A glyph's outline, placed by `affine` (glyph units to pixels), filled
/// with style `fill`.
fn glyph_edges(records: &[ShapeRecord], affine: &Affine, fill: u16, edges: &mut Vec<RawEdge>) {
    let place = |x: i32, y: i32| affine.apply(Vec2::new(x as f32, y as f32));
    let (mut x, mut y) = (0_i32, 0_i32);
    let (mut fill0, mut fill1) = (0_u32, 0_u32);
    let mut segments = Vec::new();
    let mut flush = |segments: &mut Vec<Segment>, f0: u32, f1: u32| {
        if !segments.is_empty() {
            edges.push(RawEdge {
                fill0: (f0 != 0).then_some(fill),
                fill1: (f1 != 0).then_some(fill),
                stroke: None,
                segments: std::mem::take(segments),
            });
        }
    };
    for record in records {
        match record {
            ShapeRecord::StyleChange(change) => {
                flush(&mut segments, fill0, fill1);
                if let Some(value) = change.fill_style_0 {
                    fill0 = value;
                }
                if let Some(value) = change.fill_style_1 {
                    fill1 = value;
                }
                if let Some(point) = change.move_to {
                    (x, y) = (point.x.get(), point.y.get());
                }
            }
            ShapeRecord::StraightEdge { delta } => {
                let start = place(x, y);
                x += delta.dx.get();
                y += delta.dy.get();
                segments.push(Segment {
                    start,
                    end: place(x, y),
                    control: None,
                });
            }
            ShapeRecord::CurvedEdge {
                control_delta,
                anchor_delta,
            } => {
                let start = place(x, y);
                let (cx, cy) = (x + control_delta.dx.get(), y + control_delta.dy.get());
                (x, y) = (cx + anchor_delta.dx.get(), cy + anchor_delta.dy.get());
                segments.push(Segment {
                    start,
                    end: place(x, y),
                    control: Some(place(cx, cy)),
                });
            }
        }
    }
    flush(&mut segments, fill0, fill1);
}

/// Flash colour transforms as q0s effects: alpha becomes opacity, colour
/// offsets and multipliers a tint.
fn color_fx(transform: &ColorTransform) -> PlacementFx {
    let alpha = transform.a_multiply.to_f32();
    let alpha_add = f32::from(transform.a_add) / 255.0;
    let opacity = (alpha + alpha_add).clamp(0.0, 1.0);
    let multiply = [
        transform.r_multiply.to_f32(),
        transform.g_multiply.to_f32(),
        transform.b_multiply.to_f32(),
    ];
    let add = [transform.r_add, transform.g_add, transform.b_add];
    let average = (multiply[0] + multiply[1] + multiply[2]) / 3.0;
    let amount = (1.0 - average).clamp(0.0, 1.0);
    let tint = (amount > 0.004 || add.iter().any(|value| *value != 0)).then(|| {
        let channel = |add: i16| -> u8 {
            if amount > 0.004 {
                (f32::from(add) / amount).clamp(0.0, 255.0) as u8
            } else {
                add.clamp(0, 255) as u8
            }
        };
        TintFx {
            color: Rgba {
                r: channel(add[0]),
                g: channel(add[1]),
                b: channel(add[2]),
                a: 255,
            },
            amount: if amount > 0.004 {
                amount
            } else {
                // A pure offset brightens: approximate it as a light tint.
                (f32::from(add[0].max(add[1]).max(add[2]).max(0)) / 255.0).clamp(0.0, 1.0)
            },
        }
    });
    PlacementFx {
        opacity,
        tint,
        ..PlacementFx::default()
    }
}

fn blend_mode(mode: swf::BlendMode) -> Option<BlendMode> {
    Some(match mode {
        swf::BlendMode::Normal | swf::BlendMode::Layer => BlendMode::Normal,
        swf::BlendMode::Multiply => BlendMode::Multiply,
        swf::BlendMode::Screen | swf::BlendMode::Lighten => BlendMode::Screen,
        swf::BlendMode::Add => BlendMode::Add,
        swf::BlendMode::Overlay | swf::BlendMode::HardLight => BlendMode::Overlay,
        _ => return None,
    })
}

fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|error| format!("zlib: {error}"))?;
    Ok(out)
}

/// Strips the extra end and start markers old Flash wrote before JPEGs.
fn clean_jpeg(data: &[u8]) -> &[u8] {
    let mut data = data;
    while data.starts_with(&[0xFF, 0xD9, 0xFF, 0xD8]) {
        data = &data[4..];
    }
    data
}

fn decode_jpeg(data: &[u8]) -> Result<(u16, u16, Vec<u8>), String> {
    let data = clean_jpeg(data);
    if data.starts_with(b"\x89PNG") || data.starts_with(b"GIF8") {
        return Err("PNG and GIF images inside SWF JPEG tags are not decoded yet".into());
    }
    crate::xfl_content::decode_jpeg(data).map_err(|error| error.to_string())
}

/// DefineBitsLossless(2) pixels as straight RGBA.
fn decode_lossless(bits: &swf::DefineBitsLossless<'_>) -> Result<(u16, u16, Vec<u8>), String> {
    let data = inflate(&bits.data).unwrap_or_else(|_| bits.data.to_vec());
    let (width, height) = (usize::from(bits.width), usize::from(bits.height));
    let alpha = bits.version >= 2;
    let mut rgba = Vec::with_capacity(width * height * 4);
    let short = || "bitmap data is shorter than its size".to_string();
    match bits.format {
        swf::BitmapFormat::ColorMap8 { num_colors } => {
            let entry = if alpha { 4 } else { 3 };
            let colors = usize::from(num_colors) + 1;
            let palette = data.get(..colors * entry).ok_or_else(short)?;
            let stride = (width + 3) & !3;
            for row in 0..height {
                for column in 0..width {
                    let index = *data
                        .get(colors * entry + row * stride + column)
                        .ok_or_else(short)? as usize;
                    let color = palette.get(index * entry..index * entry + entry);
                    match color {
                        Some(color) if alpha => rgba.extend_from_slice(color),
                        Some(color) => rgba.extend_from_slice(&[color[0], color[1], color[2], 255]),
                        None => rgba.extend_from_slice(&[0, 0, 0, 0]),
                    }
                }
            }
        }
        swf::BitmapFormat::Rgb15 => {
            let stride = (width * 2 + 3) & !3;
            for row in 0..height {
                for column in 0..width {
                    let at = row * stride + column * 2;
                    let pixel = u16::from_be_bytes([
                        *data.get(at).ok_or_else(short)?,
                        *data.get(at + 1).ok_or_else(short)?,
                    ]);
                    let five = |shift: u16| (((pixel >> shift) & 0x1F) as u8) << 3;
                    rgba.extend_from_slice(&[five(10), five(5), five(0), 255]);
                }
            }
        }
        swf::BitmapFormat::Rgb32 => {
            for pixel in data.as_chunks::<4>().0.iter().take(width * height) {
                let a = if alpha { pixel[0] } else { 255 };
                // Lossless2 colours are premultiplied by their alpha.
                let straight = |c: u8| {
                    if alpha && a > 0 && a < 255 {
                        ((u16::from(c) * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8
                    } else {
                        c
                    }
                };
                rgba.extend_from_slice(&[
                    straight(pixel[1]),
                    straight(pixel[2]),
                    straight(pixel[3]),
                    a,
                ]);
            }
            if rgba.len() < width * height * 4 {
                return Err(short());
            }
        }
    }
    Ok((bits.width, bits.height, rgba))
}

// ----------------------------------------------------------------- sound

/// IMA step sizes, shared by every ADPCM code width.
const ADPCM_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

fn adpcm_index_shift(bits: u32, magnitude: u32) -> i32 {
    const TWO: [i32; 2] = [-1, 2];
    const THREE: [i32; 4] = [-1, -1, 2, 4];
    const FOUR: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
    const FIVE: [i32; 16] = [-1, -1, -1, -1, -1, -1, -1, -1, 1, 2, 4, 6, 8, 10, 13, 16];
    let table: &[i32] = match bits {
        2 => &TWO,
        3 => &THREE,
        4 => &FOUR,
        _ => &FIVE,
    };
    table.get(magnitude as usize).copied().unwrap_or(-1)
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl BitReader<'_> {
    fn read(&mut self, count: u32) -> Option<u32> {
        let mut value = 0_u32;
        for _ in 0..count {
            let byte = *self.data.get(self.bit / 8)?;
            value = (value << 1) | u32::from((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Some(value)
    }
}

/// Flash's ADPCM: a code width, then packets of 4096 samples per channel,
/// each opening with a 16-bit sample and a 6-bit step index per channel.
fn decode_adpcm(data: &[u8], channels: usize, max_samples: Option<usize>) -> Vec<i16> {
    let mut reader = BitReader { data, bit: 0 };
    let Some(code_bits) = reader.read(2).map(|value| value + 2) else {
        return Vec::new();
    };
    let sign = 1_u32 << (code_bits - 1);
    let mut out = Vec::new();
    let limit = max_samples.unwrap_or(usize::MAX);
    'packets: loop {
        let mut predicted = vec![0_i32; channels];
        let mut index = vec![0_i32; channels];
        for channel in 0..channels {
            let (Some(sample), Some(step)) = (reader.read(16), reader.read(6)) else {
                break 'packets;
            };
            predicted[channel] = i32::from(sample as u16 as i16);
            index[channel] = step as i32;
            out.push(predicted[channel] as i16);
        }
        for _ in 1..4096 {
            if out.len() / channels >= limit {
                break 'packets;
            }
            for channel in 0..channels {
                let Some(code) = reader.read(code_bits) else {
                    break 'packets;
                };
                let step = ADPCM_STEPS[index[channel].clamp(0, 88) as usize];
                let mut difference = step >> (code_bits - 1);
                let mut mask = sign >> 1;
                let mut part = step;
                while mask != 0 {
                    if code & mask != 0 {
                        difference += part;
                    }
                    part >>= 1;
                    mask >>= 1;
                }
                if code & sign != 0 {
                    predicted[channel] -= difference;
                } else {
                    predicted[channel] += difference;
                }
                predicted[channel] = predicted[channel].clamp(-32768, 32767);
                index[channel] =
                    (index[channel] + adpcm_index_shift(code_bits, code & (sign - 1))).clamp(0, 88);
                out.push(predicted[channel] as i16);
            }
        }
    }
    out
}

/// A SWF sound as q0v bytes, its length in samples and its sample rate.
fn decode_sound(
    format: &SoundFormat,
    data: &[u8],
    samples: Option<u32>,
) -> Result<(Vec<u8>, u64, u32), String> {
    let channels: u16 = if format.is_stereo { 2 } else { 1 };
    let rate = u32::from(format.sample_rate);
    let pcm: Vec<i16> = match format.compression {
        AudioCompression::Mp3 => {
            let decoded = q0video::audio::decode_source(data)?;
            let samples = decoded.samples_per_channel();
            let bytes = crate::audio::wrap_audio(
                data,
                true,
                decoded.sample_rate,
                decoded.channels,
                samples,
                &decoded.pcm_le_bytes,
            )?;
            return Ok((bytes, samples, decoded.sample_rate));
        }
        AudioCompression::Adpcm => decode_adpcm(
            data,
            usize::from(channels),
            samples.map(|count| count as usize),
        ),
        AudioCompression::Uncompressed | AudioCompression::UncompressedUnknownEndian => {
            if format.is_16_bit {
                data.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| i16::from_le_bytes(*pair))
                    .collect()
            } else {
                data.iter()
                    .map(|byte| (i16::from(*byte) - 128) << 8)
                    .collect()
            }
        }
        other => return Err(format!("{other:?} sound is not supported")),
    };
    let frames = (pcm.len() / usize::from(channels)) as u64;
    if frames == 0 {
        return Err("the sound has no samples".into());
    }
    let bytes: Vec<u8> = pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect();
    Ok((
        crate::audio::wrap_audio(&[], false, rate, channels, frames, &bytes)?,
        frames,
        rate,
    ))
}

// ---------------------------------------------------------- frame code

/// Frame code: q0lang where it says the same thing, `# AS:` lines otherwise.
struct Translation {
    source: String,
    complete: bool,
}

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn describe(action: &Action<'_>) -> String {
    let text = format!("{action:?}")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    if text.chars().count() > 160 {
        let cut: String = text.chars().take(157).collect();
        format!("{cut}...")
    } else {
        text
    }
}

impl Converter {
    fn text(&self, value: &SwfStr) -> String {
        value
            .to_str_lossy(SwfStr::encoding_for_version(self.version))
            .into_owned()
    }

    fn note(&mut self, message: impl Into<String>) {
        *self.notes.entry(message.into()).or_default() += 1;
    }

    fn alloc_asset(&mut self) -> Option<u16> {
        let id = self.next_asset;
        self.next_asset = self.next_asset.checked_add(1)?;
        Some(id)
    }

    fn alloc_q0rg(&mut self) -> Option<u16> {
        let id = self.next_q0rg;
        self.next_q0rg = self.next_q0rg.checked_add(1)?;
        Some(id)
    }

    fn alloc_instance(&mut self) -> u32 {
        let id = self.next_instance;
        self.next_instance = self.next_instance.wrapping_add(1).max(1);
        id
    }

    fn item_name(&self, id: CharacterId, kind: &str) -> String {
        self.exports
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("{kind} {id}"))
    }

    fn collect_exports(&mut self, tags: &[Tag<'_>]) {
        for tag in tags {
            if let Tag::ExportAssets(assets) = tag {
                for asset in assets {
                    let name = self.text(asset.name);
                    self.exports.insert(asset.id, name);
                }
            }
        }
    }

    fn define_all(&mut self, tags: &[Tag<'_>]) {
        for tag in tags {
            match tag {
                Tag::DefineShape(shape) => self.shape(shape),
                Tag::DefineSprite(sprite) => self.sprite(sprite),
                Tag::JpegTables(tables) => self.jpeg_tables = Some(tables.to_vec()),
                Tag::DefineBits { id, jpeg_data } => {
                    let joined = match self.jpeg_tables.as_ref().filter(|tables| tables.len() > 4) {
                        Some(tables) => {
                            let tables = clean_jpeg(tables);
                            let tables = tables.strip_suffix(&[0xFF, 0xD9]).unwrap_or(tables);
                            let body = clean_jpeg(jpeg_data);
                            let body = body.strip_prefix(&[0xFF, 0xD8]).unwrap_or(body);
                            [tables, body].concat()
                        }
                        None => jpeg_data.to_vec(),
                    };
                    let decoded = decode_jpeg(&joined);
                    self.bitmap(*id, decoded);
                }
                Tag::DefineBitsJpeg2 { id, jpeg_data } => {
                    let decoded = decode_jpeg(jpeg_data);
                    self.bitmap(*id, decoded);
                }
                Tag::DefineBitsJpeg3(jpeg) => {
                    let decoded = decode_jpeg(jpeg.data).map(|(w, h, mut pixels)| {
                        if let Ok(alpha) = inflate(jpeg.alpha_data) {
                            for (pixel, a) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(alpha) {
                                pixel[3] = a;
                            }
                        }
                        (w, h, pixels)
                    });
                    self.bitmap(jpeg.id, decoded);
                }
                Tag::DefineBitsLossless(bits) => {
                    let decoded = decode_lossless(bits);
                    self.bitmap(bits.id, decoded);
                }
                Tag::DefineFont(font) => {
                    self.fonts.insert(
                        font.id,
                        FontGlyphs {
                            em: 1024.0,
                            glyphs: font.glyphs.clone(),
                            codes: Vec::new(),
                            advances: Vec::new(),
                            ascent: 880.0,
                            descent: 144.0,
                            leading: 0.0,
                        },
                    );
                }
                Tag::DefineFont2(font) => {
                    let em = if font.version >= 3 { 20_480.0 } else { 1024.0 };
                    let (ascent, descent, leading) =
                        font.layout
                            .as_ref()
                            .map_or((em * 0.86, em * 0.14, 0.0), |layout| {
                                (
                                    f32::from(layout.ascent),
                                    f32::from(layout.descent),
                                    f32::from(layout.leading),
                                )
                            });
                    self.fonts.insert(
                        font.id,
                        FontGlyphs {
                            em,
                            glyphs: font
                                .glyphs
                                .iter()
                                .map(|g| g.shape_records.clone())
                                .collect(),
                            codes: font.glyphs.iter().map(|g| g.code).collect(),
                            advances: font.glyphs.iter().map(|g| f32::from(g.advance)).collect(),
                            ascent,
                            descent,
                            leading,
                        },
                    );
                }
                Tag::DefineText(text) | Tag::DefineText2(text) => self.static_text(text),
                Tag::DefineEditText(text) => self.edit_text(text),
                Tag::DefineButton(button) | Tag::DefineButton2(button) => self.button(button),
                Tag::DefineSound(sound) => {
                    let decoded =
                        decode_sound(&sound.format, sound_data(sound), Some(sound.num_samples));
                    self.sound(sound.id, decoded);
                }
                Tag::DefineMorphShape(_) => {
                    self.note("shape tweens made in Flash (morph shapes) are not converted yet");
                }
                Tag::DefineVideoStream(_) => {
                    self.note("embedded video is not converted yet");
                }
                Tag::DoInitAction { action_data, .. } => {
                    self.scripts.class_blocks += 1;
                    self.scripts.class_bytes += action_data.len();
                }
                _ => {}
            }
        }
    }

    fn file_under(&mut self, item: LibraryItemReference, folder: &'static str) {
        self.folders.push((item, folder));
    }

    fn bitmap(&mut self, id: CharacterId, decoded: Result<(u16, u16, Vec<u8>), String>) {
        match decoded {
            Ok((width, height, pixels)) if width > 0 && height > 0 => {
                let Some(asset_id) = self.alloc_asset() else {
                    self.note("the movie has more items than one q0s project holds");
                    return;
                };
                self.project.assets.push(Asset::Bitmap(BitmapAsset {
                    asset_id,
                    width,
                    height,
                    rgba: Arc::new(pixels),
                }));
                let name = self.item_name(id, "Bitmap");
                self.project.asset_names.insert(asset_id, name);
                self.bitmaps.insert(id, asset_id);
                self.file_under(LibraryItemReference::Asset(asset_id), "Bitmaps");
            }
            Ok(_) => self.note("an empty bitmap was skipped"),
            Err(error) => self.note(format!("a bitmap could not be decoded: {error}")),
        }
    }

    fn sound(&mut self, id: CharacterId, decoded: Result<(Vec<u8>, u64, u32), String>) {
        match decoded {
            Ok((bytes, samples, rate)) => {
                let Some(asset_id) = self.alloc_asset() else {
                    self.note("the movie has more items than one q0s project holds");
                    return;
                };
                self.project.assets.push(Asset::Q0v(Q0vAsset {
                    asset_id,
                    bytes: bytes.into(),
                }));
                let name = self.item_name(id, "Sound");
                self.project.asset_names.insert(asset_id, name);
                self.sounds.insert(
                    id,
                    ImportedSound {
                        asset_id,
                        samples,
                        rate,
                    },
                );
                self.file_under(LibraryItemReference::Asset(asset_id), "Sounds");
            }
            Err(error) => self.note(format!("a sound could not be converted: {error}")),
        }
    }

    /// Makes `targets` one placeable item: itself when it is one, a q0rg
    /// holding them all otherwise.
    fn character(
        &mut self,
        id: CharacterId,
        name: String,
        targets: Vec<Target>,
        folder: &'static str,
    ) {
        match targets.as_slice() {
            [] => {}
            [single] => {
                if let Target::Asset(asset_id) = single {
                    self.project.asset_names.insert(*asset_id, name);
                    self.file_under(LibraryItemReference::Asset(*asset_id), folder);
                } else if let Target::Q0rg(q0rg_id) = single {
                    self.file_under(LibraryItemReference::Q0rg(*q0rg_id), folder);
                }
                self.characters.insert(id, *single);
            }
            many => {
                let Some(q0rg_id) = self.alloc_q0rg() else {
                    self.note("the movie has more symbols than one q0s project holds");
                    return;
                };
                let placements = many
                    .iter()
                    .map(|target| Placement {
                        pivot: None,
                        instance_id: 0,
                        frame: 0,
                        target: *target,
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: PlacementFx::default(),
                    })
                    .collect();
                self.project.q0rgs.push(Q0rg {
                    q0rg_id,
                    name,
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        timeline_frame_count: None,
                        name: "Layer 1".into(),
                        explicit_keyframes: vec![0],
                        placements,
                    }],
                });
                self.file_under(LibraryItemReference::Q0rg(q0rg_id), folder);
                self.characters.insert(id, Target::Q0rg(q0rg_id));
            }
        }
    }

    fn targets(&mut self, shape: &RawShape, name: &str) -> Vec<Target> {
        let mut warnings = Vec::new();
        let bitmaps = &self.bitmaps;
        let resolve = |fill: &RawFillStyle| match fill {
            RawFillStyle::Bitmap { reference, .. } => u16::try_from(*reference)
                .ok()
                .and_then(|id| bitmaps.get(&id).copied()),
            _ => None,
        };
        let targets = shape_targets(
            shape,
            name,
            &mut self.project,
            &mut self.next_asset,
            &mut self.next_q0rg,
            &mut warnings,
            &resolve,
        );
        for warning in warnings {
            // The shape's own name makes every message unique; count by kind.
            let kind = warning
                .split_once(": ")
                .map_or(warning.as_str(), |(_, rest)| rest);
            self.note(kind.to_string());
        }
        match targets {
            Ok(targets) => targets,
            Err(error) => {
                self.note(format!("a shape could not be converted: {error}"));
                Vec::new()
            }
        }
    }

    fn shape(&mut self, shape: &swf::Shape) {
        let raw = raw_shape(&shape.styles, &shape.shape);
        let name = self.item_name(shape.id, "Shape");
        let targets = self.targets(&raw, &name);
        self.character(shape.id, name, targets, "Shapes");
    }

    fn static_text(&mut self, text: &swf::Text) {
        let mut raw = empty_shape();
        let mut styles: HashMap<[u8; 4], u16> = HashMap::new();
        let text_matrix = matrix_px(&text.matrix);
        let (mut font, mut color) = (
            None,
            Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
        );
        let (mut x, mut y, mut height) = (0_i32, 0_i32, 240_i32);
        let mut missing = false;
        for record in &text.records {
            if let Some(id) = record.font_id {
                font = Some(id);
            }
            if let Some(value) = &record.color {
                color = rgba(value);
            }
            if let Some(value) = record.x_offset {
                x = value.get();
            }
            if let Some(value) = record.y_offset {
                y = value.get();
            }
            if let Some(value) = record.height {
                height = value.get();
            }
            let Some(glyphs) = font.and_then(|id| self.fonts.get(&id)) else {
                missing = true;
                continue;
            };
            let next = styles.len() as u16 + 1;
            let style = *styles
                .entry([color.r, color.g, color.b, color.a])
                .or_insert(next);
            raw.fills.insert(style, RawFillStyle::Solid(color));
            let scale = height as f32 / glyphs.em;
            for entry in &record.glyphs {
                if let Some(records) = glyphs.glyphs.get(entry.index as usize) {
                    // Glyph units to twips, then the text's own matrix.
                    let glyph = Affine {
                        a11: scale / 20.0,
                        a12: 0.0,
                        a21: 0.0,
                        a22: scale / 20.0,
                        tx: x as f32 / 20.0,
                        ty: y as f32 / 20.0,
                    };
                    glyph_edges(
                        records,
                        &Affine::compose(text_matrix, glyph),
                        style,
                        &mut raw.edges,
                    );
                }
                x += entry.advance;
            }
        }
        if missing {
            self.note("some text uses a font that is not embedded; that text was left out");
        }
        let name = self.item_name(text.id, "Text");
        let targets = self.targets(&raw, &name);
        self.character(text.id, name, targets, "Text");
    }

    fn edit_text(&mut self, field: &swf::EditText<'_>) {
        if !field.variable_name().is_empty() {
            self.scripts.dynamic_text += 1;
        }
        let mut text = field
            .initial_text()
            .map(|value| self.text(value))
            .unwrap_or_default();
        if field.is_html() {
            text = strip_html(&text);
        }
        let bounds = field.bounds();
        let name = self.item_name(field.id(), "Text field");
        let Some(glyphs) = field.font_id().and_then(|id| self.fonts.get(&id)) else {
            if !text.trim().is_empty() {
                self.note("a text field uses a device font; its text was left out");
            }
            return;
        };
        if glyphs.codes.is_empty() || text.trim().is_empty() {
            self.character(field.id(), name, Vec::new(), "Text");
            return;
        }
        let height = field.height().map_or(240.0, |value| value.get() as f32);
        let scale = height / glyphs.em;
        let color = field.color().map_or(
            Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            |value| rgba(&value),
        );
        let mut raw = empty_shape();
        raw.fills.insert(1, RawFillStyle::Solid(color));
        let box_left = bounds.x_min.get() as f32 + 40.0;
        let box_width = (bounds.x_max.get() - bounds.x_min.get()) as f32 - 80.0;
        let line_height = (glyphs.ascent + glyphs.descent + glyphs.leading) * scale;
        let mut baseline = bounds.y_min.get() as f32 + 40.0 + glyphs.ascent * scale;
        let align = field
            .layout()
            .map_or(TextAlign::Left, |layout| layout.align);
        for line in text.split(['\r', '\n']) {
            let indices: Vec<Option<usize>> = line
                .encode_utf16()
                .map(|code| glyphs.codes.iter().position(|candidate| *candidate == code))
                .collect();
            let width: f32 = indices
                .iter()
                .map(|index| {
                    index
                        .and_then(|i| glyphs.advances.get(i))
                        .copied()
                        .unwrap_or(glyphs.em * 0.5)
                        * scale
                })
                .sum();
            let mut x = match align {
                TextAlign::Center => box_left + (box_width - width) / 2.0,
                TextAlign::Right => box_left + box_width - width,
                _ => box_left,
            };
            for index in indices {
                if let Some(index) = index {
                    if let Some(records) = glyphs.glyphs.get(index) {
                        let affine = Affine {
                            a11: scale / 20.0,
                            a12: 0.0,
                            a21: 0.0,
                            a22: scale / 20.0,
                            tx: x / 20.0,
                            ty: baseline / 20.0,
                        };
                        glyph_edges(records, &affine, 1, &mut raw.edges);
                    }
                }
                x += index
                    .and_then(|i| glyphs.advances.get(i))
                    .copied()
                    .unwrap_or(glyphs.em * 0.5)
                    * scale;
            }
            baseline += line_height;
        }
        let targets = self.targets(&raw, &name);
        self.character(field.id(), name, targets, "Text");
    }

    fn button(&mut self, button: &swf::Button<'_>) {
        self.scripts.handlers += button.actions.len();
        let Some(q0rg_id) = self.alloc_q0rg() else {
            self.note("the movie has more symbols than one q0s project holds");
            return;
        };
        let mut records: Vec<_> = button
            .records
            .iter()
            .filter(|record| record.states.contains(ButtonState::UP))
            .collect();
        records.sort_by_key(|record| record.depth);
        let mut layers = Vec::new();
        for record in records {
            let Some(target) = self.characters.get(&record.id).copied() else {
                continue;
            };
            let transform =
                affine_to_transform(matrix_px(&record.matrix)).unwrap_or(Transform2D::IDENTITY);
            let layer_id = layers.len() as u16 + 1;
            layers.push(Layer {
                layer_id,
                timeline_frame_count: None,
                name: format!("depth {}", record.depth),
                explicit_keyframes: vec![0],
                placements: vec![Placement {
                    pivot: None,
                    instance_id: 0,
                    frame: 0,
                    target,
                    transform,
                    tween: Tween::None,
                    fx: color_fx(&record.color_transform),
                }],
            });
        }
        if layers.is_empty() {
            layers.push(Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "Layer 1".into(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            });
        }
        let name = self.item_name(button.id, "Button");
        self.project.q0rgs.push(Q0rg {
            q0rg_id,
            name,
            frame_count: 1,
            script: String::new(),
            layers,
        });
        self.file_under(LibraryItemReference::Q0rg(q0rg_id), "Buttons");
        self.characters.insert(button.id, Target::Q0rg(q0rg_id));
    }

    fn sprite(&mut self, sprite: &swf::Sprite<'_>) {
        let Some(q0rg_id) = self.alloc_q0rg() else {
            self.note("the movie has more symbols than one q0s project holds");
            return;
        };
        let name = self.item_name(sprite.id, "Movie clip");
        let q0rg = self.timeline(q0rg_id, &name, &sprite.tags, sprite.num_frames);
        self.project.q0rgs.push(q0rg);
        self.movie_clips.insert(q0rg_id);
        self.file_under(LibraryItemReference::Q0rg(q0rg_id), "Movie clips");
        self.characters.insert(sprite.id, Target::Q0rg(q0rg_id));
    }

    fn place(&mut self, live: &mut BTreeMap<u16, Shown>, place: &PlaceObject<'_>) {
        let fresh = |character, instance| Shown {
            character,
            matrix: Affine::IDENTITY,
            fx: PlacementFx::default(),
            name: None,
            clip_depth: None,
            instance,
        };
        match place.action {
            PlaceObjectAction::Place(character) | PlaceObjectAction::Replace(character) => {
                let instance = self.alloc_instance();
                match live.get_mut(&place.depth) {
                    Some(shown) if matches!(place.action, PlaceObjectAction::Replace(_)) => {
                        shown.character = character;
                        shown.instance = instance;
                    }
                    _ => {
                        live.insert(place.depth, fresh(character, instance));
                    }
                }
            }
            PlaceObjectAction::Modify | PlaceObjectAction::None => {}
        }
        let Some(shown) = live.get_mut(&place.depth) else {
            return;
        };
        if let Some(matrix) = &place.matrix {
            shown.matrix = matrix_px(matrix);
        }
        if let Some(transform) = &place.color_transform {
            let blend = shown.fx.blend_mode;
            shown.fx = PlacementFx {
                blend_mode: blend,
                ..color_fx(transform)
            };
        }
        if let Some(name) = place.name {
            shown.name = Some(self.text(name));
        }
        if let Some(depth) = place.clip_depth {
            shown.clip_depth = Some(depth);
        }
        if let Some(mode) = place.blend_mode {
            match blend_mode(mode) {
                Some(mode) => shown.fx.blend_mode = mode,
                None => self.note(format!(
                    "blend mode {mode:?} has no q0s match; Normal is used"
                )),
            }
        }
        if place.is_visible == Some(false) {
            shown.fx.opacity = 0.0;
        }
        if let Some(filters) = &place.filters {
            shown.fx.blur = None;
            shown.fx.glow = None;
            shown.fx.shadow = None;
            let mut skipped = Vec::new();
            for filter in filters {
                match filter {
                    swf::Filter::BlurFilter(blur) => {
                        shown.fx.blur = Some(BlurFx {
                            radius_x: blur.blur_x.to_f32() / 2.0,
                            radius_y: blur.blur_y.to_f32() / 2.0,
                            quality: blur.num_passes().clamp(1, 3),
                        });
                    }
                    swf::Filter::GlowFilter(glow) if !glow.is_inner() => {
                        shown.fx.glow = Some(GlowFx {
                            color: rgba(&glow.color),
                            radius_x: glow.blur_x.to_f32() / 2.0,
                            radius_y: glow.blur_y.to_f32() / 2.0,
                            strength: glow.strength.to_f32(),
                            quality: glow.num_passes().clamp(1, 3),
                        });
                    }
                    swf::Filter::DropShadowFilter(shadow) if !shadow.is_inner() => {
                        let angle = shadow.angle.to_f32();
                        let distance = shadow.distance.to_f32();
                        shown.fx.shadow = Some(DropShadowFx {
                            color: rgba(&shadow.color),
                            blur_radius: (shadow.blur_x.to_f32() + shadow.blur_y.to_f32()) / 4.0,
                            offset_x: angle.cos() * distance,
                            offset_y: angle.sin() * distance,
                            strength: shadow.strength.to_f32(),
                            quality: shadow.num_passes().clamp(1, 3),
                        });
                    }
                    other => skipped.push(match other {
                        swf::Filter::GlowFilter(_) => "inner glow",
                        swf::Filter::DropShadowFilter(_) => "inner shadow",
                        swf::Filter::BevelFilter(_) => "bevel",
                        swf::Filter::GradientGlowFilter(_) => "gradient glow",
                        swf::Filter::GradientBevelFilter(_) => "gradient bevel",
                        swf::Filter::ConvolutionFilter(_) => "convolution",
                        _ => "colour matrix",
                    }),
                }
            }
            for name in skipped {
                self.note(format!(
                    "the {name} filter has no q0s match yet and was left off"
                ));
            }
        }
        if let Some(actions) = &place.clip_actions {
            self.scripts.handlers += actions.records.len();
        }
    }

    fn timeline(&mut self, q0rg_id: u16, name: &str, tags: &[Tag<'_>], num_frames: u16) -> Q0rg {
        let mut live: BTreeMap<u16, Shown> = BTreeMap::new();
        let mut changes: BTreeMap<u16, Vec<(u16, Option<Shown>)>> = BTreeMap::new();
        let mut dirty: BTreeSet<u16> = BTreeSet::new();
        let mut frame: u16 = 0;
        let mut labels = Vec::new();
        let mut scripts: Vec<(u16, Translation)> = Vec::new();
        let mut starts: Vec<(u16, swf::StartSound)> = Vec::new();
        let mut stream: Option<swf::SoundStreamHead> = None;
        let mut blocks: Vec<(u16, Vec<u8>)> = Vec::new();
        for tag in tags {
            match tag {
                Tag::PlaceObject(place) => {
                    self.place(&mut live, place);
                    dirty.insert(place.depth);
                }
                Tag::RemoveObject(remove) => {
                    live.remove(&remove.depth);
                    dirty.insert(remove.depth);
                }
                Tag::ShowFrame => {
                    for depth in std::mem::take(&mut dirty) {
                        changes
                            .entry(depth)
                            .or_default()
                            .push((frame, live.get(&depth).cloned()));
                    }
                    frame = frame.saturating_add(1);
                }
                Tag::FrameLabel(label) => labels.push((frame, self.text(label.label))),
                Tag::DoAction(actions) => {
                    let translation = self.translate(actions);
                    scripts.push((frame, translation));
                }
                Tag::StartSound(start) => starts.push((frame, start.clone())),
                Tag::SoundStreamHead(head) | Tag::SoundStreamHead2(head) => {
                    stream = Some(head.clone())
                }
                Tag::SoundStreamBlock(block) => blocks.push((frame, block.to_vec())),
                _ => {}
            }
        }
        if !dirty.is_empty() {
            for depth in std::mem::take(&mut dirty) {
                changes
                    .entry(depth)
                    .or_default()
                    .push((frame, live.get(&depth).cloned()));
            }
            frame = frame.saturating_add(1);
        }
        let frame_count = num_frames.max(frame).max(1);

        let mut layers = Vec::new();
        let mut depth_layers: Vec<(u16, u16)> = Vec::new();
        let mut masks: Vec<(u16, u16, u16)> = Vec::new();
        let mut names: HashMap<String, u32> = HashMap::new();
        for (depth, entries) in &changes {
            let layer_id = layers.len() as u16 + 1;
            let mut placements = Vec::new();
            let mut keys = BTreeSet::new();
            let mut previous: Option<&Option<Shown>> = None;
            let mut layer_name = None;
            let mut clip = None;
            for (at, shown) in entries {
                if previous == Some(shown) {
                    continue;
                }
                previous = Some(shown);
                keys.insert(*at);
                let Some(shown) = shown else {
                    continue;
                };
                let Some(target) = self.characters.get(&shown.character).copied() else {
                    continue;
                };
                if let Some(depth) = shown.clip_depth {
                    clip = Some(depth);
                }
                if layer_name.is_none() {
                    layer_name = shown.name.clone();
                }
                let (transform, hidden) = match affine_to_transform(shown.matrix) {
                    Some(transform) => (transform, false),
                    // Flash hides things by scaling them to nothing.
                    None => (
                        Transform2D {
                            tx: shown.matrix.tx,
                            ty: shown.matrix.ty,
                            sx: 0.001,
                            sy: 0.001,
                            ..Transform2D::IDENTITY
                        },
                        true,
                    ),
                };
                let mut fx = shown.fx;
                if hidden {
                    fx.opacity = 0.0;
                }
                if let Target::Q0rg(id) = target {
                    if self.movie_clips.contains(&id) {
                        // A movie clip runs its own timeline from when it appears.
                        fx.q0rg_playback = Q0rgPlayback {
                            placement_relative: true,
                            ..Q0rgPlayback::default()
                        };
                    }
                }
                placements.push(Placement {
                    pivot: None,
                    instance_id: shown.instance,
                    frame: *at,
                    target,
                    transform,
                    tween: Tween::None,
                    fx,
                });
                if let Some(name) = &shown.name {
                    // q0s keeps names unique per timeline; Flash reuses them
                    // for different objects on different frames.
                    match names.get(name) {
                        Some(owner) if *owner != shown.instance => {
                            self.note(
                                "an instance name used by several objects names only the first",
                            );
                        }
                        _ => {
                            names.insert(name.clone(), shown.instance);
                            self.project.runtime.instance_names.insert(
                                InstanceKey {
                                    q0rg_id,
                                    instance_id: shown.instance,
                                },
                                name.clone(),
                            );
                        }
                    }
                }
            }
            if placements.is_empty() {
                continue;
            }
            self.keyframes += keys.len();
            if let Some(clip) = clip {
                masks.push((layer_id, *depth, clip));
            }
            depth_layers.push((*depth, layer_id));
            layers.push(Layer {
                layer_id,
                timeline_frame_count: None,
                name: layer_name.unwrap_or_else(|| format!("depth {depth}")),
                explicit_keyframes: keys.into_iter().filter(|key| *key < frame_count).collect(),
                placements: placements
                    .into_iter()
                    .filter(|placement| placement.frame < frame_count)
                    .collect(),
            });
        }
        for (mask_layer, depth, clip) in masks {
            self.project.layer_metadata.insert(
                LayerKey {
                    q0rg_id,
                    layer_id: mask_layer,
                },
                LayerMetadata {
                    kind: LayerKind::Mask,
                    ..LayerMetadata::default()
                },
            );
            for (masked_depth, layer_id) in &depth_layers {
                if *masked_depth > depth && *masked_depth <= clip {
                    let key = LayerKey {
                        q0rg_id,
                        layer_id: *layer_id,
                    };
                    let entry = self.project.layer_metadata.entry(key).or_default();
                    if entry.kind == LayerKind::Normal && entry.mask_layer_id.is_none() {
                        entry.mask_layer_id = Some(mask_layer);
                    }
                }
            }
        }

        // Frame code on a layer of its own, as Flash authors keep it.
        if !scripts.is_empty() {
            let layer_id = layers.len() as u16 + 1;
            let mut by_frame: BTreeMap<u16, Vec<Translation>> = BTreeMap::new();
            for (at, translation) in scripts {
                by_frame
                    .entry(at.min(frame_count - 1))
                    .or_default()
                    .push(translation);
            }
            for (at, parts) in &by_frame {
                let complete = parts.iter().all(|part| part.complete);
                if complete {
                    self.scripts.translated += 1;
                } else {
                    self.scripts.partly += 1;
                }
                let mut source = String::new();
                for part in parts {
                    source.push_str(&part.source);
                }
                self.project.runtime.frame_scripts.push(FrameScript {
                    q0rg_id,
                    layer_id,
                    frame: *at,
                    source,
                });
            }
            layers.push(Layer {
                layer_id,
                timeline_frame_count: None,
                name: "actions".into(),
                explicit_keyframes: by_frame.keys().copied().collect(),
                placements: Vec::new(),
            });
        }
        // Flash resolves a label to its first frame; q0s keeps names unique.
        let mut seen = HashSet::new();
        for (at, label) in labels {
            if !seen.insert(label.clone()) {
                continue;
            }
            self.project.runtime.frame_labels.insert(
                FrameLabelKey {
                    q0rg_id,
                    frame: at.min(frame_count - 1),
                },
                label,
            );
        }
        let sounds_layer = layers.len() as u16 + 1;
        let mut has_sound = false;
        for (at, start) in starts {
            let Some(sound) = self
                .sounds
                .get(&start.id)
                .map(|sound| ImportedSound { ..*sound })
            else {
                continue;
            };
            if start.sound_info.event == SoundEvent::Stop {
                let stop_at = at.min(frame_count - 1);
                for clip in &mut self.project.audio_clips {
                    if clip.q0rg_id == q0rg_id
                        && clip.asset_id == sound.asset_id
                        && clip.start_frame < stop_at
                    {
                        let length = stop_at - clip.start_frame;
                        clip.duration_frames = Some(
                            clip.duration_frames
                                .map_or(length, |frames| frames.min(length)),
                        );
                    }
                }
                continue;
            }
            if start.sound_info.num_loops > 1 {
                self.note("looping sounds play once");
            }
            let frames = ((sound.samples as f64 / f64::from(sound.rate.max(1)))
                * f64::from(self.project.meta.fps))
            .ceil()
            .clamp(1.0, f64::from(u16::MAX)) as u16;
            self.project.audio_clips.push(AudioClip {
                q0rg_id,
                layer_id: sounds_layer,
                start_frame: at.min(frame_count - 1),
                asset_id: sound.asset_id,
                gain: 1.0,
                muted: false,
                source_offset_samples: 0,
                duration_frames: Some(frames),
                pan: 0.0,
                fade_in_ms: 0,
                fade_out_ms: 0,
            });
            has_sound = true;
        }
        if let Some(head) = stream {
            has_sound |=
                self.stream_sound(q0rg_id, sounds_layer, name, &head, &blocks, frame_count);
        }
        if has_sound {
            layers.push(Layer {
                layer_id: sounds_layer,
                timeline_frame_count: None,
                name: "sounds".into(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            });
        }
        if layers.is_empty() {
            layers.push(Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "Layer 1".into(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            });
        }
        self.layers += layers.len();
        Q0rg {
            q0rg_id,
            name: name.to_string(),
            frame_count,
            script: String::new(),
            layers,
        }
    }

    /// Stream sound: one clip per unbroken run of blocks, synced to frames.
    fn stream_sound(
        &mut self,
        q0rg_id: u16,
        layer_id: u16,
        name: &str,
        head: &swf::SoundStreamHead,
        blocks: &[(u16, Vec<u8>)],
        frame_count: u16,
    ) -> bool {
        let mut added = false;
        let mut runs: Vec<(u16, u16, Vec<u8>)> = Vec::new();
        for (at, block) in blocks {
            // MP3 blocks open with a sample count and a seek offset.
            let data: &[u8] = if head.stream_format.compression == AudioCompression::Mp3 {
                block.get(4..).unwrap_or_default()
            } else {
                block
            };
            match runs.last_mut() {
                Some((_, end, bytes))
                    if *end + 1 >= *at
                        && head.stream_format.compression == AudioCompression::Mp3 =>
                {
                    *end = *at;
                    bytes.extend_from_slice(data);
                }
                _ => runs.push((*at, *at, data.to_vec())),
            }
        }
        if head.stream_format.compression != AudioCompression::Mp3 {
            // ADPCM and PCM blocks decode on their own; join their samples.
            let mut joined: Vec<(u16, u16, Vec<i16>)> = Vec::new();
            for (start, _, bytes) in runs {
                let pcm: Vec<i16> = match head.stream_format.compression {
                    AudioCompression::Adpcm => decode_adpcm(
                        &bytes,
                        if head.stream_format.is_stereo { 2 } else { 1 },
                        None,
                    ),
                    _ => bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| i16::from_le_bytes(*pair))
                        .collect(),
                };
                match joined.last_mut() {
                    Some((_, end, samples)) if *end + 1 >= start => {
                        *end = start;
                        samples.extend(pcm);
                    }
                    _ => joined.push((start, start, pcm)),
                }
            }
            for (start, end, pcm) in joined {
                let channels: u16 = if head.stream_format.is_stereo { 2 } else { 1 };
                let rate = u32::from(head.stream_format.sample_rate);
                let frames = (pcm.len() / usize::from(channels)) as u64;
                let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
                match crate::audio::wrap_audio(&[], false, rate, channels, frames, &bytes) {
                    Ok(q0v) => {
                        added |= self.stream_clip(
                            q0rg_id,
                            layer_id,
                            name,
                            q0v,
                            frames,
                            rate,
                            start,
                            end,
                            frame_count,
                        );
                    }
                    Err(error) => {
                        self.note(format!("a streamed sound could not be converted: {error}"))
                    }
                }
            }
            return added;
        }
        for (start, end, bytes) in runs {
            match decode_sound(&head.stream_format, &bytes, None) {
                Ok((q0v, samples, rate)) => {
                    added |= self.stream_clip(
                        q0rg_id,
                        layer_id,
                        name,
                        q0v,
                        samples,
                        rate,
                        start,
                        end,
                        frame_count,
                    );
                }
                Err(error) => {
                    self.note(format!("a streamed sound could not be converted: {error}"))
                }
            }
        }
        added
    }

    #[allow(clippy::too_many_arguments)]
    fn stream_clip(
        &mut self,
        q0rg_id: u16,
        layer_id: u16,
        name: &str,
        q0v: Vec<u8>,
        _samples: u64,
        _rate: u32,
        start: u16,
        end: u16,
        frame_count: u16,
    ) -> bool {
        let Some(asset_id) = self.alloc_asset() else {
            return false;
        };
        self.project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id,
            bytes: q0v.into(),
        }));
        self.project
            .asset_names
            .insert(asset_id, format!("{name} stream {}", start + 1));
        self.file_under(LibraryItemReference::Asset(asset_id), "Sounds");
        let start = start.min(frame_count - 1);
        self.project.audio_clips.push(AudioClip {
            q0rg_id,
            layer_id,
            start_frame: start,
            asset_id,
            gain: 1.0,
            muted: false,
            source_offset_samples: 0,
            duration_frames: Some(end.saturating_sub(start).saturating_add(1)),
            pan: 0.0,
            fade_in_ms: 0,
            fade_out_ms: 0,
        });
        true
    }

    /// AVM1 frame code to q0lang: expressions, variables, clip properties,
    /// `if`/`else` and timeline control. Code beyond that (calls, objects,
    /// loops, `&&` chains) keeps the whole script as `# AS:` lines.
    fn translate(&mut self, data: &[u8]) -> Translation {
        let mut reader = swf::avm1::read::Reader::new(data, self.version);
        let mut actions = Vec::new();
        loop {
            let offset = data.len() - reader.get_mut().len();
            match reader.read_action() {
                Ok(action) => actions.push((offset, action)),
                Err(_) => break,
            }
        }
        let end = data.len() - reader.get_mut().len();
        let mut decompiler = Decompiler {
            converter: self,
            actions: &actions,
            end,
            pool: Vec::new(),
            timeline: false,
            app: false,
        };
        let mut lines = Vec::new();
        let mut stack = Vec::new();
        let result = decompiler.block(0, actions.len(), &mut stack, &mut lines, 0);
        let (timeline, app) = (decompiler.timeline, decompiler.app);
        let mut source = String::new();
        if result.is_ok() {
            if timeline {
                source.push_str("import q0.timeline\n");
            }
            if app {
                source.push_str("import q0.app\n");
            }
            for line in lines {
                source.push_str(&line);
                source.push('\n');
            }
            return Translation {
                source,
                complete: true,
            };
        }
        source.push_str("# Flash2Q0: this frame's ActionScript is not translated yet.\n");
        for (_, action) in actions.iter().take(400) {
            source.push_str("# AS: ");
            source.push_str(&describe(action));
            source.push('\n');
        }
        if actions.len() > 400 {
            source.push_str(&format!("# AS: ... {} more actions\n", actions.len() - 400));
        }
        Translation {
            source,
            complete: false,
        }
    }

    fn finish(mut self) -> Result<ImportedProject, ImportError> {
        // Every placed character got its library place; file them.
        for (_, folder) in &self.folders {
            if !self
                .project
                .runtime
                .library_folders
                .iter()
                .any(|f| f == folder)
            {
                self.project
                    .runtime
                    .library_folders
                    .push((*folder).to_string());
            }
        }
        for (item, folder) in std::mem::take(&mut self.folders) {
            self.project
                .runtime
                .library_item_folders
                .insert(item, folder.to_string());
        }
        self.project.q0rgs[1..].sort_by_key(|q0rg| q0rg.q0rg_id);

        let mut warnings: Vec<String> = self
            .notes
            .iter()
            .map(|(text, count)| {
                if *count > 1 {
                    format!("{text} ({count}x)")
                } else {
                    text.clone()
                }
            })
            .collect();
        let scripts = &self.scripts;
        if scripts.translated + scripts.partly > 0 {
            warnings.push(format!(
                "frame code: {} script(s) translated to q0lang, {} kept as ActionScript comments for now",
                scripts.translated, scripts.partly
            ));
        }
        if scripts.class_blocks > 0 {
            warnings.push(format!(
                "{} ActionScript class/#initclip block(s) ({} KB of bytecode) are not translated yet; the game logic waits for the q0lang ActionScript upgrade",
                scripts.class_blocks,
                scripts.class_bytes / 1024
            ));
        }
        if scripts.handlers > 0 {
            warnings.push(format!(
                "{} button or clip event handler(s) (on/onClipEvent) are not translated yet",
                scripts.handlers
            ));
        }
        if scripts.dynamic_text > 0 {
            warnings.push(format!(
                "{} text field(s) bound to variables show their starting text only",
                scripts.dynamic_text
            ));
        }
        q0s_format::v2::validate(&self.project).map_err(|error| {
            ImportError::Invalid(format!(
                "the converted movie does not form a valid project: {error} (deepest nesting: {})",
                deepest_chain(&self.project).join(" > ")
            ))
        })?;
        Ok(ImportedProject {
            project: self.project,
            report: ImportReport {
                source_kind: "swf".into(),
                imported_layers: self.layers,
                imported_keyframes: self.keyframes,
                warnings,
            },
            pending_text: Vec::new(),
        })
    }
}

// ------------------------------------------------------- AVM1 to q0lang

/// Words q0lang reserves; a Flash name that is one cannot be a q0lang name.
const Q0LANG_WORDS: &[&str] = &[
    "if", "else", "while", "for", "in", "break", "continue", "return", "pb", "pr", "func",
    "import", "true", "false", "null", "and", "or", "not", "listen", "xlisten", "range", "do",
];

/// MovieClip property numbers of GetProperty and SetProperty.
const CLIP_PROPERTIES: [&str; 22] = [
    "_x",
    "_y",
    "_xscale",
    "_yscale",
    "_currentframe",
    "_totalframes",
    "_alpha",
    "_visible",
    "_width",
    "_height",
    "_rotation",
    "_target",
    "_framesloaded",
    "_name",
    "_droptarget",
    "_url",
    "_highquality",
    "_focusrect",
    "_soundbuftime",
    "_quality",
    "_xmouse",
    "_ymouse",
];

/// A value on the AVM1 stack, as q0lang source.
#[derive(Clone)]
struct Expr {
    code: String,
    /// The string it is, when it is a constant string.
    text: Option<String>,
    /// The number it is, when it is a constant number.
    number: Option<f64>,
}

impl Expr {
    fn code(code: String) -> Self {
        Self {
            code,
            text: None,
            number: None,
        }
    }
}

type Untranslatable = ();

struct Decompiler<'a, 'b> {
    converter: &'a Converter,
    actions: &'a [(usize, Action<'b>)],
    /// Byte offset just past the last action.
    end: usize,
    pool: Vec<String>,
    timeline: bool,
    app: bool,
}

fn q0lang_name(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !Q0LANG_WORDS.contains(&segment)
}

/// A Flash variable path as a q0lang (dotted) name: `_root.a.b`, `_root:a`
/// and `/:a` all become `a`; `this.x` becomes `x`.
fn variable_path(name: &str) -> Option<String> {
    let mut name = name.trim();
    for prefix in ["_root.", "_level0.", "this.", "_root:", "_level0:", "/:"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            name = rest;
            break;
        }
    }
    let name = name.replace(':', ".");
    (!name.is_empty() && name.split('.').all(q0lang_name)).then_some(name)
}

fn number_code(value: f64) -> Option<String> {
    value.is_finite().then(|| {
        if value.fract() == 0.0 && value.abs() < 1e15 {
            format!("{}", value as i64)
        } else {
            format!("{value}")
        }
    })
}

fn negate(condition: &str) -> String {
    match condition
        .strip_prefix("not (")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        // Only when the parentheses enclose the whole condition.
        Some(inner) if balanced(inner) => inner.to_string(),
        _ => format!("not ({condition})"),
    }
}

fn balanced(code: &str) -> bool {
    let mut depth = 0_i32;
    let mut quoted = None;
    let mut escaped = false;
    for ch in code.chars() {
        match quoted {
            Some(quote) => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == quote {
                    quoted = None;
                }
            }
            None => match ch {
                '"' | '\'' => quoted = Some(ch),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return false;
                    }
                }
                _ => {}
            },
        }
    }
    depth == 0
}

impl Decompiler<'_, '_> {
    fn index_of(&self, offset: usize) -> Option<usize> {
        if offset == self.end {
            return Some(self.actions.len());
        }
        self.actions.iter().position(|(at, _)| *at == offset)
    }

    /// The index an If or Jump at `index` lands on.
    fn target(&self, index: usize, offset: i16) -> Option<usize> {
        let after = self.actions.get(index + 1).map_or(self.end, |(at, _)| *at);
        let target = after as i64 + i64::from(offset);
        usize::try_from(target)
            .ok()
            .and_then(|target| self.index_of(target))
    }

    fn pop(stack: &mut Vec<Expr>) -> Result<Expr, Untranslatable> {
        stack.pop().ok_or(())
    }

    fn binary(stack: &mut Vec<Expr>, operator: &str) -> Result<(), Untranslatable> {
        let right = Self::pop(stack)?;
        let left = Self::pop(stack)?;
        stack.push(Expr::code(format!(
            "({} {operator} {})",
            left.code, right.code
        )));
        Ok(())
    }

    fn path_of(expr: &Expr) -> Result<String, Untranslatable> {
        expr.text.as_deref().and_then(variable_path).ok_or(())
    }

    fn goto_line(&mut self, play: bool, target: String) -> String {
        self.timeline = true;
        format!("{} {target}", if play { "gorun!" } else { "gostop!" })
    }

    /// Translates actions `start..end` into `out`, indented by `depth`.
    fn block(
        &mut self,
        start: usize,
        end: usize,
        stack: &mut Vec<Expr>,
        out: &mut Vec<String>,
        depth: usize,
    ) -> Result<(), Untranslatable> {
        let indent = "  ".repeat(depth);
        let mut index = start;
        while index < end {
            let next_play = |at: usize| match self.actions.get(at + 1).map(|(_, action)| action) {
                Some(Action::Play) => Some(true),
                Some(Action::Stop) => Some(false),
                _ => None,
            };
            match &self.actions[index].1 {
                Action::End => break,
                Action::ConstantPool(constants) => {
                    self.pool = constants
                        .strings
                        .iter()
                        .map(|text| self.converter.text(text))
                        .collect();
                }
                Action::Push(push) => {
                    for value in &push.values {
                        let text = match value {
                            AsValue::Str(text) => Some(self.converter.text(text)),
                            AsValue::ConstantPool(at) => {
                                Some(self.pool.get(usize::from(*at)).ok_or(())?.clone())
                            }
                            _ => None,
                        };
                        let number = match value {
                            AsValue::Int(number) => Some(f64::from(*number)),
                            AsValue::Float(number) => Some(f64::from(*number)),
                            AsValue::Double(number) => Some(*number),
                            _ => None,
                        };
                        let code = match (value, &text, number) {
                            (_, Some(text), _) => quote(text),
                            (_, _, Some(number)) => number_code(number).ok_or(())?,
                            (AsValue::Bool(value), _, _) => value.to_string(),
                            (AsValue::Null | AsValue::Undefined, _, _) => "null".to_string(),
                            _ => return Err(()),
                        };
                        stack.push(Expr { code, text, number });
                    }
                }
                Action::Pop => {
                    Self::pop(stack)?;
                }
                Action::GetVariable => {
                    let name = Self::pop(stack)?;
                    let path = Self::path_of(&name)?;
                    stack.push(Expr::code(path));
                }
                Action::SetVariable => {
                    let value = Self::pop(stack)?;
                    let name = Self::pop(stack)?;
                    out.push(format!(
                        "{indent}{} = {}",
                        Self::path_of(&name)?,
                        value.code
                    ));
                }
                Action::GetMember => {
                    let member = Self::pop(stack)?;
                    let object = Self::pop(stack)?;
                    let member = member.text.filter(|name| q0lang_name(name)).ok_or(())?;
                    // Only a named path can carry a member in q0lang.
                    if !object.code.split('.').all(q0lang_name) {
                        return Err(());
                    }
                    stack.push(Expr::code(format!("{}.{member}", object.code)));
                }
                Action::SetMember => {
                    let value = Self::pop(stack)?;
                    let member = Self::pop(stack)?;
                    let object = Self::pop(stack)?;
                    let member = member.text.filter(|name| q0lang_name(name)).ok_or(())?;
                    if !object.code.split('.').all(q0lang_name) {
                        return Err(());
                    }
                    out.push(format!("{indent}{}.{member} = {}", object.code, value.code));
                }
                action @ (Action::GetProperty | Action::SetProperty) => {
                    let value = if matches!(action, Action::SetProperty) {
                        Some(Self::pop(stack)?)
                    } else {
                        None
                    };
                    let property = Self::pop(stack)?.number.ok_or(())? as usize;
                    let target = Self::pop(stack)?;
                    let property = *CLIP_PROPERTIES.get(property).ok_or(())?;
                    let target = target.text.ok_or(())?;
                    let name = if target.is_empty() || target == "this" {
                        property.to_string()
                    } else {
                        format!("{}.{property}", variable_path(&target).ok_or(())?)
                    };
                    match value {
                        Some(value) => out.push(format!("{indent}{name} = {}", value.code)),
                        None => stack.push(Expr::code(name)),
                    }
                }
                Action::Equals | Action::Equals2 | Action::StrictEquals | Action::StringEquals => {
                    Self::binary(stack, "==")?;
                }
                Action::Less | Action::Less2 | Action::StringLess => Self::binary(stack, "<")?,
                Action::Greater | Action::StringGreater => Self::binary(stack, ">")?,
                Action::Add | Action::Add2 | Action::StringAdd => Self::binary(stack, "+")?,
                Action::Subtract => Self::binary(stack, "-")?,
                Action::Multiply => Self::binary(stack, "*")?,
                Action::Divide => Self::binary(stack, "/")?,
                Action::Modulo => Self::binary(stack, "%")?,
                Action::And => Self::binary(stack, "and")?,
                Action::Or => Self::binary(stack, "or")?,
                Action::Not => {
                    let value = Self::pop(stack)?;
                    stack.push(Expr::code(negate(&value.code)));
                }
                Action::Increment => {
                    let value = Self::pop(stack)?;
                    stack.push(Expr::code(format!("({} + 1)", value.code)));
                }
                Action::Decrement => {
                    let value = Self::pop(stack)?;
                    stack.push(Expr::code(format!("({} - 1)", value.code)));
                }
                Action::Stop => {
                    self.timeline = true;
                    out.push(format!("{indent}stop!"));
                }
                Action::Play => {
                    self.timeline = true;
                    out.push(format!("{indent}play!"));
                }
                Action::NextFrame => {
                    self.timeline = true;
                    out.push(format!("{indent}gonext!"));
                }
                Action::PreviousFrame => {
                    self.timeline = true;
                    out.push(format!("{indent}goprev!"));
                }
                Action::GotoFrame(goto) => {
                    let play = next_play(index);
                    let line = self.goto_line(play == Some(true), goto.frame.to_string());
                    out.push(format!("{indent}{line}"));
                    if play.is_some() {
                        index += 1;
                    }
                }
                Action::GotoLabel(goto) => {
                    let play = next_play(index);
                    let label = quote(&self.converter.text(goto.label));
                    let line = self.goto_line(play == Some(true), label);
                    out.push(format!("{indent}{line}"));
                    if play.is_some() {
                        index += 1;
                    }
                }
                Action::GotoFrame2(goto) => {
                    let target = Self::pop(stack)?;
                    // A number is a 1-based frame; a string a label.
                    let target = match target.number {
                        Some(number) => {
                            ((number as i64 - 1).max(0) + i64::from(goto.scene_offset)).to_string()
                        }
                        None if target.text.is_some() => target.code,
                        None => format!("({} - 1)", target.code),
                    };
                    let line = self.goto_line(goto.set_playing, target);
                    out.push(format!("{indent}{line}"));
                }
                Action::GetUrl(get) => {
                    let url = self.converter.text(get.url);
                    if !(url.starts_with("http://") || url.starts_with("https://")) {
                        return Err(());
                    }
                    self.app = true;
                    out.push(format!("{indent}q0app.open! {}", quote(&url)));
                }
                Action::Trace => {
                    let value = Self::pop(stack)?;
                    self.app = true;
                    out.push(format!("{indent}q0app.log! {}", value.code));
                }
                Action::If(branch) => {
                    let target = self.target(index, branch.offset).ok_or(())?;
                    if target <= index || target > end {
                        return Err(());
                    }
                    // `If` jumps over the block when its value holds, so
                    // the block runs when it does not.
                    let condition = negate(&Self::pop(stack)?.code);
                    let height = stack.len();
                    // A Jump closing the block skips an else block.
                    let mut after = target;
                    let mut then_end = target;
                    let mut else_block = None;
                    if target > index + 1 {
                        if let (_, Action::Jump(jump)) = &self.actions[target - 1] {
                            let skip = self.target(target - 1, jump.offset).ok_or(())?;
                            if skip > target && skip <= end {
                                then_end = target - 1;
                                else_block = Some((target, skip));
                                after = skip;
                            } else if skip != target {
                                return Err(());
                            }
                        }
                    }
                    out.push(format!("{indent}if {condition} {{"));
                    self.block(index + 1, then_end, stack, out, depth + 1)?;
                    if let Some((from, to)) = else_block {
                        out.push(format!("{indent}}} else {{"));
                        self.block(from, to, stack, out, depth + 1)?;
                    }
                    out.push(format!("{indent}}}"));
                    if stack.len() != height {
                        return Err(());
                    }
                    index = after;
                    continue;
                }
                _ => return Err(()),
            }
            index += 1;
        }
        Ok(())
    }
}

/// The longest chain of symbols inside symbols, by name.
fn deepest_chain(project: &ProjectV2) -> Vec<String> {
    fn walk(project: &ProjectV2, id: u16, seen: &mut Vec<u16>, best: &mut Vec<u16>) {
        if seen.contains(&id) || seen.len() > 64 {
            return;
        }
        seen.push(id);
        if seen.len() > best.len() {
            *best = seen.clone();
        }
        if let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == id) {
            let children: BTreeSet<u16> = q0rg
                .layers
                .iter()
                .flat_map(|layer| &layer.placements)
                .filter_map(|placement| match placement.target {
                    Target::Q0rg(child) => Some(child),
                    Target::Asset(_) => None,
                })
                .collect();
            for child in children {
                walk(project, child, seen, best);
            }
        }
        seen.pop();
    }
    let mut best = Vec::new();
    walk(project, MAIN, &mut Vec::new(), &mut best);
    best.iter()
        .map(|id| {
            project
                .q0rgs
                .iter()
                .find(|q0rg| q0rg.q0rg_id == *id)
                .map_or_else(|| id.to_string(), |q0rg| q0rg.name.clone())
        })
        .collect()
}

fn sound_data<'a>(sound: &swf::Sound<'a>) -> &'a [u8] {
    // DefineSound MP3 data opens with a two-byte seek-sample count.
    if sound.format.compression == AudioCompression::Mp3 {
        sound.data.get(2..).unwrap_or_default()
    } else {
        sound.data
    }
}

/// Text of an HTML text field without its tags.
fn strip_html(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name = tag.trim_start_matches('/').to_ascii_lowercase();
                if name.starts_with("br") || (tag.starts_with('/') && name.starts_with('p')) {
                    text.push('\n');
                }
            }
            _ if in_tag => tag.push(ch),
            _ => text.push(ch),
        }
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .trim_end_matches('\n')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Converts the SWF named by Q0FLA_SWF and prints the report:
    /// Q0FLA_SWF=movie.swf cargo test -p q0fla --release -- --ignored --nocapture
    #[test]
    #[ignore]
    fn convert_swf_from_env() {
        let Ok(path) = std::env::var("Q0FLA_SWF") else {
            return;
        };
        let started = std::time::Instant::now();
        let imported = import_swf(Path::new(&path)).expect("conversion");
        println!(
            "{} in {:.1}s",
            imported.report.summary(),
            started.elapsed().as_secs_f32()
        );
        for warning in &imported.report.warnings {
            println!("  - {warning}");
        }
        let project = &imported.project;
        println!(
            "{} q0rgs, {} assets, {} clips, {} scripts, {} labels",
            project.q0rgs.len(),
            project.assets.len(),
            project.audio_clips.len(),
            project.runtime.frame_scripts.len(),
            project.runtime.frame_labels.len()
        );
        if let Ok(out) = std::env::var("Q0FLA_OUT") {
            let bytes = q0s_format::v2::write_packed(project).expect("write");
            std::fs::write(&out, &bytes).expect("save");
            println!("wrote {out} ({:.1} MB)", bytes.len() as f32 / 1_048_576.0);
            let movie = q0s_format::write_q0s_v2(project).expect("write q0s");
            let movie_path = Path::new(&out).with_extension("q0s");
            std::fs::write(&movie_path, &movie).expect("save q0s");
            println!(
                "wrote {} ({:.1} MB)",
                movie_path.display(),
                movie.len() as f32 / 1_048_576.0
            );
        }
        if std::env::var("Q0FLA_DUMP").is_ok() {
            let mut labels: Vec<_> = project
                .runtime
                .frame_labels
                .iter()
                .filter(|(key, _)| key.q0rg_id == MAIN)
                .map(|(key, label)| (key.frame, label.clone()))
                .collect();
            labels.sort();
            println!("main labels: {labels:?}");
            for (key, name) in &project.runtime.instance_names {
                if name == "introMovie" {
                    let target = project
                        .q0rgs
                        .iter()
                        .find(|q| q.q0rg_id == key.q0rg_id)
                        .and_then(|q| {
                            q.layers
                                .iter()
                                .flat_map(|l| &l.placements)
                                .find(|p| p.instance_id == key.instance_id)
                                .map(|p| (p.frame, p.target))
                        });
                    if let Some((placed, q0s_format::v2::Target::Q0rg(id))) = target {
                        let clip = project.q0rgs.iter().find(|q| q.q0rg_id == id).unwrap();
                        let scripts: Vec<_> = project
                            .runtime
                            .frame_scripts
                            .iter()
                            .filter(|s| s.q0rg_id == id)
                            .map(|s| {
                                (
                                    s.frame,
                                    s.source.lines().take(3).collect::<Vec<_>>().join(" | "),
                                )
                            })
                            .collect();
                        println!("introMovie in q0rg {} at frame {placed}: q0rg {id} '{}' {} frames, scripts {scripts:?}", key.q0rg_id, clip.name, clip.frame_count);
                    }
                }
            }
            // Clips on the main timeline's first frames, and their own code.
            let main = &project.q0rgs[0];
            let mut seen = BTreeSet::new();
            for layer in &main.layers {
                for placement in layer.placements.iter().filter(|p| p.frame < 10) {
                    if let q0s_format::v2::Target::Q0rg(id) = placement.target {
                        if !seen.insert(id) {
                            continue;
                        }
                        let clip = project.q0rgs.iter().find(|q| q.q0rg_id == id).unwrap();
                        let scripts: Vec<_> = project
                            .runtime
                            .frame_scripts
                            .iter()
                            .filter(|s| s.q0rg_id == id && !s.source.trim().is_empty())
                            .collect();
                        println!(
                            "clip {id} '{}' on main frame {} ({} frames, {} scripts)",
                            clip.name,
                            placement.frame,
                            clip.frame_count,
                            scripts.len()
                        );
                        for script in scripts.iter().take(4) {
                            println!(
                                "  -- frame {}
{}",
                                script.frame,
                                script.source.lines().take(24).collect::<Vec<_>>().join(
                                    "
"
                                )
                            );
                        }
                    }
                }
            }
            for script in project
                .runtime
                .frame_scripts
                .iter()
                .filter(|s| s.q0rg_id == MAIN && s.frame < 40)
            {
                println!(
                    "--- main frame {}
{}",
                    script.frame, script.source
                );
            }
        }
        for script in project
            .runtime
            .frame_scripts
            .iter()
            .filter(|s| s.q0rg_id == MAIN)
            .take(8)
        {
            let first: Vec<&str> = script.source.lines().take(3).collect();
            println!("  main frame {}: {}", script.frame, first.join(" | "));
        }
    }

    #[test]
    fn a_wait_for_a_clip_to_end_becomes_an_if() {
        // if (introMovie._currentframe == introMovie._totalframes) { gotoAndPlay("mainMenu"); }
        let mut code = Vec::new();
        let push_pool =
            |code: &mut Vec<u8>, index: u8| code.extend_from_slice(&[0x96, 2, 0, 8, index]);
        // ConstantPool ["introMovie", "_currentframe", "_totalframes"]
        let strings = ["introMovie", "_currentframe", "_totalframes"];
        let mut pool = vec![3, 0];
        for text in strings {
            pool.extend_from_slice(text.as_bytes());
            pool.push(0);
        }
        code.push(0x88);
        code.extend_from_slice(&(pool.len() as u16).to_le_bytes());
        code.extend_from_slice(&pool);
        push_pool(&mut code, 0);
        code.push(0x1C); // GetVariable
        push_pool(&mut code, 1);
        code.push(0x4E); // GetMember
        push_pool(&mut code, 0);
        code.push(0x1C);
        push_pool(&mut code, 2);
        code.push(0x4E);
        code.push(0x49); // Equals2
        code.push(0x12); // Not
        let label = b"mainMenu\0";
        let skip = (3 + label.len() + 1) as i16;
        code.push(0x9D); // If
        code.extend_from_slice(&[2, 0]);
        code.extend_from_slice(&skip.to_le_bytes());
        code.push(0x8C); // GotoLabel
        code.extend_from_slice(&(label.len() as u16).to_le_bytes());
        code.extend_from_slice(label);
        code.push(0x06); // Play
        code.push(0x00); // End
        let mut converter = test_converter();
        let translation = converter.translate(&code);
        assert!(translation.complete, "{}", translation.source);
        assert_eq!(
            translation.source,
            "import q0.timeline\nif (introMovie._currentframe == introMovie._totalframes) {\n  gorun! \"mainMenu\"\n}\n"
        );
        let program = q0s_format::q0lang::parser::parse(&translation.source);
        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    }

    fn test_converter() -> Converter {
        Converter {
            version: 9,
            project: ProjectV2 {
                meta: ProjectMeta {
                    name: "t".into(),
                    fps: 30,
                    stage_width: 100,
                    stage_height: 100,
                    background_color: Rgba {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    },
                    entry_q0rg_id: MAIN,
                },
                assets: Vec::new(),
                asset_names: HashMap::new(),
                asset_appearances: Default::default(),
                layer_metadata: HashMap::new(),
                audio_clips: Vec::new(),
                runtime: Default::default(),
                q0rgs: Vec::new(),
            },
            next_asset: 1,
            next_q0rg: 2,
            next_instance: 1,
            characters: HashMap::new(),
            movie_clips: HashSet::new(),
            bitmaps: HashMap::new(),
            fonts: HashMap::new(),
            sounds: HashMap::new(),
            exports: HashMap::new(),
            jpeg_tables: None,
            folders: Vec::new(),
            notes: BTreeMap::new(),
            layers: 0,
            keyframes: 0,
            scripts: ScriptCounts::default(),
        }
    }

    #[test]
    fn html_text_loses_its_tags_but_keeps_its_lines() {
        assert_eq!(
            strip_html("<p align=\"left\"><font size=\"12\">Hi &amp; bye</font></p><p>two</p>"),
            "Hi & bye\ntwo"
        );
    }

    #[test]
    fn adpcm_keeps_its_first_sample_and_moves_from_it() {
        // 4-bit codes, one channel: sample 1000, step index 20, then codes.
        let mut bits = String::from("10");
        bits.push_str(&format!("{:016b}", 1000_u16));
        bits.push_str(&format!("{:06b}", 20_u8));
        for _ in 0..8 {
            bits.push_str("0111");
        }
        while bits.len() % 8 != 0 {
            bits.push('0');
        }
        let bytes: Vec<u8> = bits
            .as_bytes()
            .chunks(8)
            .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 2).unwrap())
            .collect();
        let samples = decode_adpcm(&bytes, 1, Some(9));
        assert_eq!(samples[0], 1000);
        assert!(samples[1] > 1000 && samples.windows(2).all(|pair| pair[1] >= pair[0]));
    }

    #[test]
    fn colour_transforms_become_opacity_and_tint() {
        let half = swf::Fixed8::from_f32(0.5);
        let one = swf::Fixed8::ONE;
        let fx = color_fx(&ColorTransform {
            r_multiply: half,
            g_multiply: half,
            b_multiply: half,
            a_multiply: half,
            r_add: 100,
            g_add: 0,
            b_add: 0,
            a_add: 0,
        });
        assert!((fx.opacity - 0.5).abs() < 0.01);
        let tint = fx.tint.expect("a tint");
        assert!((tint.amount - 0.5).abs() < 0.01);
        assert_eq!(tint.color.r, 200);
        let plain = color_fx(&ColorTransform {
            r_multiply: one,
            g_multiply: one,
            b_multiply: one,
            a_multiply: one,
            r_add: 0,
            g_add: 0,
            b_add: 0,
            a_add: 0,
        });
        assert!(plain.tint.is_none() && (plain.opacity - 1.0).abs() < 1e-6);
    }
}
