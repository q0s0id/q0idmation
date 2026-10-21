use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use cfb::CompoundFile;
use q0s_format::geom::CapShape;
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Layer, Placement, PlacementFx, ProjectMeta, ProjectRuntimeData, ProjectScene, ProjectV2, Q0rg,
    Rgba, Stroke, Target, Transform2D, Tween, Vec2,
};

use crate::xfl_content::{self, RawEdge, RawFillStyle, RawShape, Segment};
use crate::{ImportError, ImportReport, ImportedProject};

#[path = "binary_archive.rs"]
mod archive;
#[path = "binary_project.rs"]
mod project;

pub(crate) const OLE2_MAGIC: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const MAX_STREAM_BYTES: u64 = 512 * 1024 * 1024;
const MAX_BINARY_SHAPES: usize = 100_000;
const MAX_STYLE_COUNT: usize = 16_384;
const MAX_EDGE_COUNT: usize = 2_000_000;

#[derive(Clone)]
struct StreamInfo {
    name: String,
    path: PathBuf,
    len: u64,
}

struct RecoveredShape {
    shape: RawShape,
    start: usize,
    end: usize,
}

pub(crate) fn import_legacy_fla(path: &Path) -> Result<ImportedProject, ImportError> {
    project::import(path)
}

#[derive(Default)]
struct LegacyNames {
    scenes: HashMap<u32, String>,
    symbols: HashMap<u32, String>,
}

struct LegacyMetadata {
    names: LegacyNames,
    width: u16,
    height: u16,
    fps: u16,
    background: Rgba,
}

fn read_legacy_metadata(contents: &[u8]) -> LegacyMetadata {
    let strings = read_utf16_ascii_runs(contents);
    let mut names = LegacyNames::default();
    for (index, (_, key)) in strings.iter().enumerate() {
        if let Some(number) = numbered_name(key, "Symbol ") {
            if let Some((_, name)) = strings.get(index + 1) {
                if !name.is_empty() && !name.starts_with("Symbol ") && !name.starts_with("Page ") {
                    names.symbols.entry(number).or_insert_with(|| name.clone());
                }
            }
        } else if let Some(number) = numbered_name(key, "Page ") {
            if let Some((_, name)) = strings.get(index + 1) {
                if !name.is_empty() && !name.starts_with("Symbol ") && !name.starts_with("Page ") {
                    names.scenes.entry(number).or_insert_with(|| name.clone());
                }
            }
        }
    }
    let mut width = 550_u16;
    let mut height = 400_u16;
    for (index, (_, key)) in strings.iter().enumerate() {
        let Some((_, value)) = strings.get(index + 1) else {
            continue;
        };
        let destination = if key.ends_with("::Width") && key.contains("Html") {
            Some(&mut width)
        } else if key.ends_with("::Height") && key.contains("Html") {
            Some(&mut height)
        } else {
            None
        };
        if let Some(destination) = destination {
            if let Ok(parsed) = value.parse::<u16>() {
                if (1..=16_384).contains(&parsed) {
                    *destination = parsed;
                }
            }
        }
    }
    let mut fps = 24_u16;
    let mut background = Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    for index in 100..contents.len().saturating_sub(12) {
        if contents[index + 3] != 0xff || contents[index + 7] != 0xff {
            continue;
        }
        let pad = u16::from_le_bytes([contents[index + 8], contents[index + 9]]);
        let rate = u16::from_le_bytes([contents[index + 10], contents[index + 11]]);
        if pad == 0 && (10..=60).contains(&rate) {
            background = Rgba {
                r: contents[index],
                g: contents[index + 1],
                b: contents[index + 2],
                a: contents[index + 3],
            };
            fps = rate;
            break;
        }
    }
    LegacyMetadata {
        names,
        width,
        height,
        fps,
        background,
    }
}

fn read_utf16_ascii_runs(bytes: &[u8]) -> Vec<(usize, String)> {
    let mut result = Vec::new();
    let mut index = 0;
    while index + 5 < bytes.len() {
        let start = index;
        let mut units = Vec::new();
        while index + 1 < bytes.len() {
            let unit = u16::from_le_bytes([bytes[index], bytes[index + 1]]);
            if (0x20..=0x7e).contains(&unit) || unit == 0x09 || unit == 0x0a || unit == 0x0d {
                units.push(unit);
                index += 2;
            } else {
                break;
            }
        }
        if units.len() >= 3 {
            result.push((start, String::from_utf16_lossy(&units)));
        } else {
            index = start + 1;
        }
    }
    let mut cursor = 0;
    while cursor + 4 <= bytes.len() {
        let Some(offset) = bytes[cursor..]
            .windows(3)
            .position(|window| window == [0xff, 0xfe, 0xff])
        else {
            break;
        };
        let marker = cursor + offset;
        cursor = marker + 3;
        let Some(&char_count) = bytes.get(cursor) else {
            break;
        };
        cursor += 1;
        let byte_count = usize::from(char_count).saturating_mul(2);
        let Some(raw) = bytes.get(cursor..cursor.saturating_add(byte_count)) else {
            continue;
        };
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let value = String::from_utf16_lossy(&units);
        if !value.is_empty() {
            result.push((marker, value));
        }
        cursor += byte_count;
    }
    result.sort_by_key(|(offset, _)| *offset);
    result.dedup_by(|left, right| left.0 == right.0);
    result
}

fn numbered_name(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.parse().ok()
}

fn numbered_stream(name: &str, prefix: &str) -> Option<u32> {
    let compact_prefix = match prefix {
        "Page " => Some("P "),
        "Symbol " => Some("S "),
        _ => None,
    };
    numbered_stream_id(name, prefix).or_else(|| {
        compact_prefix.and_then(|compact_prefix| numbered_stream_id(name, compact_prefix))
    })
}

fn numbered_stream_id(name: &str, prefix: &str) -> Option<u32> {
    let actual_prefix = name.get(..prefix.len())?;
    if !actual_prefix.eq_ignore_ascii_case(prefix) {
        return None;
    }
    name[prefix.len()..].split_whitespace().next()?.parse().ok()
}

fn read_stream(
    compound: &mut CompoundFile<File>,
    stream: &StreamInfo,
) -> Result<Vec<u8>, ImportError> {
    if stream.len > MAX_STREAM_BYTES {
        return Err(ImportError::Invalid(format!(
            "legacy FLA stream '{}' exceeds the {} byte safety limit",
            stream.name, MAX_STREAM_BYTES
        )));
    }
    let mut reader = compound.open_stream(&stream.path).map_err(|error| {
        ImportError::Invalid(format!(
            "could not open legacy FLA stream '{}': {error}",
            stream.name
        ))
    })?;
    let mut bytes = Vec::with_capacity(stream.len as usize);
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn empty_layer(layer_id: u16, name: &str) -> Layer {
    Layer {
        layer_id,
        timeline_frame_count: None,
        name: name.to_string(),
        explicit_keyframes: vec![0],
        placements: Vec::new(),
    }
}

fn add_shapes_to_layer(
    shapes: Vec<RecoveredShape>,
    frame: u16,
    owner: &str,
    layer: &mut Layer,
    project: &mut ProjectV2,
    next_asset_id: &mut u16,
    warnings: &mut Vec<String>,
) -> Result<(), ImportError> {
    for recovered in shapes {
        let before = project.assets.len();
        let asset_ids =
            xfl_content::shape_assets(&recovered.shape, owner, project, next_asset_id, warnings)?;
        for asset_id in asset_ids {
            layer.push_placement(Placement {
                pivot: None,
                instance_id: 0,
                frame,
                target: Target::Asset(asset_id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: PlacementFx::default(),
            });
        }
        if project.assets.len() == before {
            warnings.push(format!(
                "{owner}: vector shape at byte {}..{} had no styles that q1s can render",
                recovered.start, recovered.end
            ));
        }
    }
    Ok(())
}

fn recover_shapes(bytes: &[u8], limit: usize) -> Vec<RecoveredShape> {
    if limit == 0 || bytes.len() < 40 {
        return Vec::new();
    }
    let mut shapes = Vec::new();
    let mut covered_until = 0_usize;
    let signature = b"\xff\xff\x01\x00\x09\x00CPicShape";
    for (index, window) in bytes.windows(signature.len()).enumerate() {
        if window != signature || index < covered_until {
            continue;
        }
        let start = index + signature.len();
        if let Some((shape, end)) = parse_shape(bytes, start) {
            covered_until = end;
            shapes.push(RecoveredShape { shape, start, end });
            if shapes.len() >= limit {
                return shapes;
            }
        }
    }

    let signature = [0, 0, 0, 0, 0, 0x80, 0, 0, 0, 0x80];
    let mut cursor = 0_usize;
    while cursor + signature.len() < bytes.len() && shapes.len() < limit {
        let Some(offset) = bytes[cursor..]
            .windows(signature.len())
            .position(|window| window == signature)
        else {
            break;
        };
        let marker = cursor + offset;
        cursor = marker + 1;
        if marker < 2 || marker - 2 < covered_until {
            continue;
        }
        if let Some((shape, end)) = parse_shape(bytes, marker - 2) {
            if end > marker + signature.len() && end <= bytes.len() {
                covered_until = end;
                shapes.push(RecoveredShape {
                    shape,
                    start: marker - 2,
                    end,
                });
                cursor = end;
            }
        }
    }
    shapes
}

fn parse_shape(bytes: &[u8], start: usize) -> Option<(RawShape, usize)> {
    let mut reader = ByteReader::at(bytes, start);
    let object_schema = reader.u8()?;
    let _flags = reader.u8()?;
    if object_schema > 8 || reader.u16()? != 0 {
        return None;
    }
    let _point_x = reader.s32()?;
    let _point_y = reader.s32()?;
    if object_schema > 2 {
        reader.u8()?;
    }
    if object_schema > 3 {
        reader.u8()?;
    }
    let shape = parse_shape_tail(&mut reader, 0)?;
    if shape.edges.is_empty() || (shape.fills.is_empty() && shape.strokes.is_empty()) {
        return None;
    }
    Some((shape, reader.position))
}

fn parse_shape_tail(reader: &mut ByteReader<'_>, gradient_extra: usize) -> Option<RawShape> {
    let shape_schema = reader.u8()?;
    if shape_schema > 16 {
        return None;
    }
    let matrix = read_matrix(reader)?;
    if !valid_affine(matrix) {
        return None;
    }
    let shape_data_schema = reader.u8()?;
    if shape_data_schema > 8 {
        return None;
    }
    let _edge_hint = reader.u32()?;
    let fill_count = usize::from(reader.u16()?);
    if fill_count > MAX_STYLE_COUNT {
        return None;
    }
    let mut fills = HashMap::with_capacity(fill_count);
    for style in 1..=fill_count {
        let fill = if shape_data_schema < 3 {
            let color = read_color(reader.u32()?);
            reader.u16()?;
            RawFillStyle::Solid(color)
        } else {
            parse_fill_style(reader, shape_schema > 2, gradient_extra)?
        };
        fills.insert(u16::try_from(style).ok()?, fill);
    }
    let line_count = usize::from(reader.u16()?);
    if line_count > MAX_STYLE_COUNT {
        return None;
    }
    let mut strokes = HashMap::with_capacity(line_count);
    for style in 1..=line_count {
        let color = read_color(reader.u32()?);
        let width_twips = reader.u16()?;
        reader.skip(4)?;
        if shape_schema > 2 {
            reader.skip(4)?;
            reader.u16()?;
            let _line_fill = parse_fill_style(reader, true, gradient_extra)?;
        }
        let width = if width_twips == 0 {
            1.0
        } else {
            f32::from(width_twips) / 20.0
        };
        if width.is_finite() && width > 0.0 {
            strokes.insert(
                u16::try_from(style).ok()?,
                Stroke {
                    color,
                    width,
                    cap: CapShape::Round,
                    join: q0s_format::geom::JoinShape::Bevel,
                },
            );
        }
    }

    let mut edges = Vec::new();
    if shape_data_schema >= 2 {
        let (edge_result, consumed) = parse_edges(reader)?;
        edges = edge_result;
        if consumed == 0 || edges.len() > MAX_EDGE_COUNT {
            return None;
        }
    }
    if shape_data_schema > 4 {
        let cubic_count = reader.s32()?;
        if !(0..=i32::try_from(MAX_EDGE_COUNT).ok()?).contains(&cubic_count) {
            return None;
        }
        if cubic_count > 0 {
            // Cubics are a newer supplemental stream. Older edge records stay
            // usable, so skip the supplemental points without misaligning the
            // next object in the OLE stream.
            reader.skip(usize::try_from(cubic_count).ok()?.checked_mul(32)?)?;
        }
    }
    if edges.iter().any(|edge| {
        edge.fill0.is_some_and(|id| usize::from(id) > fill_count)
            || edge.fill1.is_some_and(|id| usize::from(id) > fill_count)
            || edge.stroke.is_some_and(|id| usize::from(id) > line_count)
    }) {
        return None;
    }
    let shape = RawShape {
        affine: matrix,
        fills,
        strokes,
        edges,
        widths: HashMap::new(),
    };

    Some(shape)
}

fn parse_fill_style(
    reader: &mut ByteReader<'_>,
    caps: bool,
    gradient_extra: usize,
) -> Option<RawFillStyle> {
    let color = read_color(reader.u32()?);
    let subtype = reader.u8()?;
    reader.u8()?;
    if subtype & 0x70 == 0x10 {
        let mut transform = read_matrix(reader)?;
        let stop_count = usize::from(reader.u8()?);
        if stop_count > 64 {
            return None;
        }
        if caps {
            reader.u16()?;
            reader.u8()?;
            reader.skip(gradient_extra)?;
        }
        // The gradient's canonical coordinates are +/-16384 twips.
        // q1s samples gradients in a normalized +/-1 domain in stage pixels.
        let radius = 16_384.0 / 20.0;
        transform.a11 *= radius;
        transform.a12 *= radius;
        transform.a21 *= radius;
        transform.a22 *= radius;
        let mut stops = Vec::with_capacity(stop_count);
        for _ in 0..stop_count {
            stops.push(q0s_format::v2::GradientStop {
                position: f32::from(reader.u8()?) / 255.0,
                color: read_color(reader.u32()?),
            });
        }
        if stops.is_empty() {
            stops.push(q0s_format::v2::GradientStop {
                position: 0.0,
                color,
            });
            stops.push(q0s_format::v2::GradientStop {
                position: 1.0,
                color,
            });
        } else if stops.len() == 1 {
            let only = stops[0];
            stops.push(q0s_format::v2::GradientStop {
                position: 1.0,
                color: only.color,
            });
        }
        return Some(RawFillStyle::Gradient(q0s_format::v2::GradientFill {
            kind: if subtype & 2 == 0 {
                q0s_format::v2::GradientKind::Linear
            } else {
                q0s_format::v2::GradientKind::Radial
            },
            stops,
            transform,
        }));
    }
    if subtype & 0x70 == 0x20 {
        read_matrix(reader)?;
        reader.u32()?;
        reader.skip(8)?;
        return Some(RawFillStyle::Solid(color));
    }
    if subtype & 0x70 == 0x40 {
        let mut transform = read_matrix(reader)?;
        let reference = u32::from(reader.u16()?);
        // Bitmap fill bases map source pixels to twips, unlike object bases.
        transform.a11 /= 20.0;
        transform.a12 /= 20.0;
        transform.a21 /= 20.0;
        transform.a22 /= 20.0;
        return Some(RawFillStyle::Bitmap {
            reference,
            name: None,
            transform,
            tiled: subtype & 1 == 0,
        });
    }
    Some(RawFillStyle::Solid(color))
}

fn parse_edges(reader: &mut ByteReader<'_>) -> Option<(Vec<RawEdge>, usize)> {
    let initial = reader.position;
    let mut edges = Vec::new();
    let mut x = 0_i64;
    let mut y = 0_i64;
    let mut fill0 = 0_u16;
    let mut fill1 = 0_u16;
    let mut stroke = 0_u16;
    loop {
        let flags = reader.u8()?;
        if flags == 0 {
            break;
        }
        if flags & 0x40 != 0 {
            let values = if flags & 0x80 != 0 {
                [
                    u16::from(reader.u8()?),
                    u16::from(reader.u8()?),
                    u16::from(reader.u8()?),
                ]
            } else {
                [reader.u16()?, reader.u16()?, reader.u16()?]
            };
            stroke = values[0] & 0x7fff;
            fill0 = values[1] & 0x7fff;
            fill1 = values[2] & 0x7fff;
        }
        let (dx1, dy1) = read_delta(reader, flags & 3)?;
        let (dx2, dy2) = read_delta(reader, (flags >> 2) & 3)?;
        let (dx3, dy3) = read_delta(reader, (flags >> 4) & 3)?;
        let sx = x + dx1;
        let sy = y + dy1;
        let ex = sx + dx3;
        let ey = sy + dy3;
        let (cx, cy) = if (flags >> 2) & 3 == 0 {
            ((sx + ex) / 2, (sy + ey) / 2)
        } else {
            (sx + dx2, sy + dy2)
        };
        if [sx, sy, ex, ey, cx, cy]
            .into_iter()
            .any(|coordinate| coordinate.unsigned_abs() > 1_000_000_000)
        {
            return None;
        }
        let segment = Segment {
            // Edges use 8.8 fixed-point twips: 256 * 20 units per pixel.
            start: Vec2::new(sx as f32 / 5120.0, sy as f32 / 5120.0),
            end: Vec2::new(ex as f32 / 5120.0, ey as f32 / 5120.0),
            control: (((flags >> 2) & 3) != 0)
                .then_some(Vec2::new(cx as f32 / 5120.0, cy as f32 / 5120.0)),
        };
        edges.push(RawEdge {
            fill0: (fill0 != 0).then_some(fill0),
            fill1: (fill1 != 0).then_some(fill1),
            stroke: (stroke != 0).then_some(stroke),
            segments: vec![segment],
        });
        x = ex;
        y = ey;
        if edges.len() > MAX_EDGE_COUNT {
            return None;
        }
    }
    Some((edges, reader.position.saturating_sub(initial)))
}

fn read_delta(reader: &mut ByteReader<'_>, kind: u8) -> Option<(i64, i64)> {
    match kind {
        0 => Some((0, 0)),
        1 => Some((i64::from(reader.s16()?), i64::from(reader.s16()?))),
        2 => Some((i64::from(reader.s32()?), i64::from(reader.s32()?))),
        3 => Some((i64::from(reader.s16()?) << 7, i64::from(reader.s16()?) << 7)),
        _ => None,
    }
}

fn read_matrix(reader: &mut ByteReader<'_>) -> Option<Affine> {
    let a = reader.s32()? as f32 / 65536.0;
    let b = reader.s32()? as f32 / 65536.0;
    let c = reader.s32()? as f32 / 65536.0;
    let d = reader.s32()? as f32 / 65536.0;
    let tx = reader.s32()? as f32 / 20.0;
    let ty = reader.s32()? as f32 / 20.0;
    Some(Affine {
        a11: a,
        a12: c,
        a21: b,
        a22: d,
        tx,
        ty,
    })
}

fn valid_affine(matrix: Affine) -> bool {
    [
        matrix.a11, matrix.a12, matrix.a21, matrix.a22, matrix.tx, matrix.ty,
    ]
    .into_iter()
    .all(|value| value.is_finite() && value.abs() <= 1_000_000.0)
}

fn read_color(rgba: u32) -> Rgba {
    Rgba {
        r: (rgba & 0xff) as u8,
        g: ((rgba >> 8) & 0xff) as u8,
        b: ((rgba >> 16) & 0xff) as u8,
        a: (rgba >> 24) as u8,
    }
}

#[derive(Clone)]
struct ByteReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ByteReader<'a> {
    fn at(bytes: &'a [u8], position: usize) -> Self {
        Self { bytes, position }
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.position.checked_add(count)?;
        let value = self.bytes.get(self.position..end)?;
        self.position = end;
        Some(value)
    }

    fn skip(&mut self, count: usize) -> Option<()> {
        self.take(count).map(|_| ())
    }

    fn u8(&mut self) -> Option<u8> {
        Some(*self.take(1)?.first()?)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn s16(&mut self) -> Option<i16> {
        Some(i16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn s32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}
