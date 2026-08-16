use q0s_format::{
    is_q0s_v2, parse_q0s, parse_q0s_v2, parse_q1s, q1s_to_movie,
    raster::rasterize_q0rg_frame_scaled,
    v2::{self, Asset, ProjectDependencyKind, ProjectDependencySource, ProjectV2},
};
use q0video::q0v::Q0vFile;

use crate::{
    document::{Document, DocumentData, PreviewRef},
    swf::parse_swf,
};

const MAX_PREVIEW_WIDTH: u32 = 720;
const MAX_PREVIEW_HEIGHT: u32 = 460;
const MAX_TEXT_PREVIEW_BYTES: usize = 128 * 1024;
const MAX_HEX_PREVIEW_BYTES: usize = 4096;

#[derive(Debug, Clone)]
pub struct VectorPreview {
    pub paths: Vec<v2::Path>,
    pub fill: Option<v2::Rgba>,
    pub stroke: Option<v2::Stroke>,
    pub bounds: (f32, f32, f32, f32),
    pub caption: String,
}

#[derive(Debug, Clone)]
pub enum PreviewContent {
    Vector(VectorPreview),
    Image {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
        caption: String,
    },
    Text {
        text: String,
        caption: String,
    },
}

pub fn preview_frame_count(document: &Document, reference: &PreviewRef) -> u32 {
    match (reference, &document.data) {
        (PreviewRef::QProjectQ0rg { path, q0rg_id }, DocumentData::QProject(root)) => {
            resolve_project(root, path)
                .ok()
                .and_then(|resolved| {
                    resolved
                        .as_project()
                        .q0rgs
                        .iter()
                        .find(|q0rg| q0rg.q0rg_id == *q0rg_id)
                        .map(|q0rg| u32::from(q0rg.frame_count.max(1)))
                })
                .unwrap_or(1)
        }
        (PreviewRef::QProjectAsset { path, asset_id, .. }, DocumentData::QProject(root)) => {
            resolve_project(root, path)
                .ok()
                .and_then(|resolved| {
                    resolved
                        .as_project()
                        .assets
                        .iter()
                        .find(|asset| asset.id() == *asset_id)
                        .and_then(|asset| match asset {
                            Asset::Q0v(media) => q0video::q0v::probe_header(&media.bytes).ok(),
                            _ => None,
                        })
                })
                .filter(|header| header.spec.video)
                .map(|header| header.spec.timeline_frames.max(1))
                .unwrap_or(1)
        }
        (PreviewRef::Q0vStandaloneVideo, DocumentData::Q0v(media)) => {
            u32::try_from(media.frames.len()).unwrap_or(u32::MAX).max(1)
        }
        _ => 1,
    }
}

pub fn build_preview(
    document: &Document,
    reference: &PreviewRef,
    frame: u32,
) -> Result<PreviewContent, String> {
    match (reference, &document.data) {
        (PreviewRef::QProjectAsset { path, asset_id, .. }, DocumentData::QProject(root)) => {
            let resolved = resolve_project(root, path)?;
            preview_project_asset(resolved.as_project(), *asset_id, frame)
        }
        (PreviewRef::QProjectQ0rg { path, q0rg_id }, DocumentData::QProject(root)) => {
            let resolved = resolve_project(root, path)?;
            preview_q0rg(resolved.as_project(), *q0rg_id, frame)
        }
        (PreviewRef::QProjectEmbedded { path, node_id }, DocumentData::QProject(root)) => {
            let resolved = resolve_project(root, path)?;
            let project = resolved.as_project();
            let node = project
                .runtime
                .project_graph
                .nodes
                .iter()
                .find(|node| node.node_id == *node_id)
                .ok_or_else(|| format!("embedded project node {node_id} is missing"))?;
            let ProjectDependencySource::Embedded(bytes) = &node.source else {
                return Err("project node is not embedded".to_string());
            };
            if node.kind == ProjectDependencyKind::Q0lang {
                return Ok(PreviewContent::Text {
                    text: text_preview(bytes),
                    caption: format!("embedded q0lang • {} bytes", bytes.len()),
                });
            }
            preview_embedded_bytes(&node.alias, bytes, frame)
        }
        (PreviewRef::Q1LegacyBitmap(asset_id), DocumentData::Q1Legacy(project)) => {
            let bitmap = project
                .assets
                .iter()
                .find(|asset| asset.asset_id == *asset_id)
                .ok_or_else(|| format!("legacy bitmap {asset_id} is missing"))?;
            Ok(PreviewContent::Image {
                width: usize::from(bitmap.width),
                height: usize::from(bitmap.height),
                rgba: bitmap.rgba.clone(),
                caption: format!("bitmap #{asset_id} • {}×{}", bitmap.width, bitmap.height),
            })
        }
        (PreviewRef::Q0LegacyBitmap(asset_id), DocumentData::Q0Legacy(movie)) => {
            let bitmap = movie
                .bitmaps
                .get(asset_id)
                .ok_or_else(|| format!("legacy bitmap {asset_id} is missing"))?;
            Ok(PreviewContent::Image {
                width: usize::from(bitmap.width),
                height: usize::from(bitmap.height),
                rgba: bitmap.rgba.clone(),
                caption: format!("bitmap #{asset_id} • {}×{}", bitmap.width, bitmap.height),
            })
        }
        (PreviewRef::Q0vStandaloneVideo, DocumentData::Q0v(media)) => {
            preview_q0v_media(media, frame, false)
        }
        (PreviewRef::Q0vStandaloneAudio, DocumentData::Q0v(media)) => {
            preview_q0v_media(media, frame, true)
        }
        (PreviewRef::SwfTag(index), DocumentData::Swf(movie)) => {
            preview_swf_tag(movie, *index, frame)
        }
        (PreviewRef::SwfTag(index), DocumentData::Projector { movie, .. }) => {
            preview_swf_tag(movie, *index, frame)
        }
        _ => Err("selected tree node does not belong to this document".to_string()),
    }
}

enum ResolvedProject<'a> {
    Root(&'a ProjectV2),
    Embedded(Box<ProjectV2>),
}

impl ResolvedProject<'_> {
    fn as_project(&self) -> &ProjectV2 {
        match self {
            Self::Root(project) => project,
            Self::Embedded(project) => project,
        }
    }
}

fn resolve_project<'a>(root: &'a ProjectV2, path: &[u16]) -> Result<ResolvedProject<'a>, String> {
    if path.is_empty() {
        return Ok(ResolvedProject::Root(root));
    }

    let mut current: Option<ProjectV2> = None;
    for node_id in path {
        let container = current.as_ref().unwrap_or(root);
        let node = container
            .runtime
            .project_graph
            .nodes
            .iter()
            .find(|node| node.node_id == *node_id)
            .ok_or_else(|| format!("embedded project path references missing node {node_id}"))?;
        if node.kind != ProjectDependencyKind::Movie {
            return Err(format!("project path node {node_id} is not a movie"));
        }
        let ProjectDependencySource::Embedded(bytes) = &node.source else {
            return Err(format!("project path node {node_id} is external"));
        };
        let next = parse_nested_project(bytes)?;
        current = Some(next);
    }
    Ok(ResolvedProject::Embedded(Box::new(current.expect(
        "non-empty project path resolves an embedded project",
    ))))
}

fn parse_nested_project(bytes: &[u8]) -> Result<ProjectV2, String> {
    if bytes.starts_with(b"Q0S\0") {
        if !is_q0s_v2(bytes) {
            return Err("legacy q0s cannot be traversed as a q0s v2 project".to_string());
        }
        return parse_q0s_v2(bytes).map_err(|error| format!("parse nested q0s: {error}"));
    }
    if bytes.starts_with(b"Q1S\0") {
        if bytes.get(4..6) == Some(&1u16.to_le_bytes()) {
            return Err("legacy q1s cannot be traversed as a q1s v2 project".to_string());
        }
        return v2::parse(bytes).map_err(|error| format!("parse nested q1s: {error}"));
    }
    Err("nested movie is not q0s/q1s".to_string())
}
fn preview_project_asset(
    project: &ProjectV2,
    asset_id: u16,
    frame: u32,
) -> Result<PreviewContent, String> {
    let asset = project
        .assets
        .iter()
        .find(|asset| asset.id() == asset_id)
        .ok_or_else(|| format!("asset {asset_id} is missing"))?;
    match asset {
        Asset::Bitmap(bitmap) => Ok(PreviewContent::Image {
            width: usize::from(bitmap.width),
            height: usize::from(bitmap.height),
            rgba: bitmap.rgba.clone(),
            caption: format!("bitmap #{asset_id} • {}×{}", bitmap.width, bitmap.height),
        }),
        Asset::Vector(_) => preview_vector_asset(project, asset_id),
        Asset::Q0v(media) => {
            let media = Q0vFile::parse(media.bytes.clone())
                .map_err(|error| format!("parse embedded q0v: {error}"))?;
            preview_q0v_media(&media, frame, !media.spec.video)
        }
        Asset::Rig(rig) => Ok(PreviewContent::Text {
            text: format!(
                "rig asset #{asset_id}\nowner q0rg: {}\nnodes: {}\ncontrols: {}\nconstraints: {}\nchannels: {}\ndeformers: {}\nvariants: {}",
                rig.owner_q0rg_id,
                rig.nodes.len(),
                rig.controls.len(),
                rig.constraints.len(),
                rig.channels.len(),
                rig.deformers.len(),
                rig.variants.len()
            ),
            caption: "rig structure".to_string(),
        }),
    }
}

fn preview_vector_asset(project: &ProjectV2, asset_id: u16) -> Result<PreviewContent, String> {
    let source = project
        .assets
        .iter()
        .find(|asset| asset.id() == asset_id)
        .ok_or_else(|| format!("vector asset {asset_id} is missing"))?;
    let Asset::Vector(vector) = source else {
        return Err(format!("asset {asset_id} is not a vector"));
    };
    let bounds = vector_source_bounds(vector).unwrap_or((0.0, 0.0, 1.0, 1.0));
    Ok(PreviewContent::Vector(VectorPreview {
        paths: vector.paths.clone(),
        fill: vector.fill,
        stroke: vector.stroke,
        bounds,
        caption: format!("vector asset #{asset_id} • live vector geometry"),
    }))
}

fn vector_source_bounds(vector: &v2::VectorAsset) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for path in &vector.paths {
        for anchor in &path.anchors {
            for point in [Some(anchor.point), anchor.in_handle, anchor.out_handle]
                .into_iter()
                .flatten()
            {
                if point.x.is_finite() && point.y.is_finite() {
                    min_x = min_x.min(point.x);
                    min_y = min_y.min(point.y);
                    max_x = max_x.max(point.x);
                    max_y = max_y.max(point.y);
                }
            }
        }
    }
    if !min_x.is_finite() {
        return None;
    }
    let pad = vector
        .stroke
        .map(|stroke| stroke.width.max(0.0) * 0.5)
        .unwrap_or(0.0);
    Some((min_x - pad, min_y - pad, max_x + pad, max_y + pad))
}
fn preview_q0rg(project: &ProjectV2, q0rg_id: u16, frame: u32) -> Result<PreviewContent, String> {
    let q0rg = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .ok_or_else(|| format!("q0rg {q0rg_id} is missing"))?;
    let local_frame = if q0rg.frame_count == 0 {
        0
    } else {
        (frame % u32::from(q0rg.frame_count)) as u16
    };
    let (width, height) = fitted_size(
        u32::from(project.meta.stage_width.max(1)),
        u32::from(project.meta.stage_height.max(1)),
    );
    let rgba = rasterize_q0rg_frame_scaled(
        project,
        q0rg_id,
        local_frame,
        width,
        height,
        1,
        [0, 0, 0, 0],
    );
    let (width, height, rgba) = crop_visible_rgba(width as usize, height as usize, rgba)
        .unwrap_or((width as usize, height as usize, Vec::new()));
    let rgba = if rgba.is_empty() {
        rasterize_q0rg_frame_scaled(
            project,
            q0rg_id,
            local_frame,
            width as u32,
            height as u32,
            1,
            [0, 0, 0, 0],
        )
    } else {
        rgba
    };
    Ok(PreviewContent::Image {
        width,
        height,
        rgba,
        caption: format!(
            "q0rg {} • frame {}/{}",
            q0rg.name,
            local_frame + 1,
            q0rg.frame_count.max(1)
        ),
    })
}

fn preview_q0v_media(
    media: &Q0vFile,
    frame: u32,
    prefer_audio: bool,
) -> Result<PreviewContent, String> {
    if media.spec.audio && (prefer_audio || !media.spec.video) {
        return Ok(waveform_preview(
            media.audio_pcm_le_bytes(),
            media.spec.audio_channels,
            media.spec.audio_sample_rate,
        ));
    }
    if media.spec.video {
        if media.frames.is_empty() {
            return Err("q0v video stream has no frames".to_string());
        }
        let index = usize::try_from(frame).unwrap_or(usize::MAX) % media.frames.len();
        let rgba = media
            .decode_frame_rgba(index)
            .map_err(|error| format!("decode q0v frame {index}: {error}"))?;
        return Ok(PreviewContent::Image {
            width: media.spec.width as usize,
            height: media.spec.height as usize,
            rgba,
            caption: format!(
                "q0v video • frame {}/{} • {}×{}",
                index + 1,
                media.frames.len(),
                media.spec.width,
                media.spec.height
            ),
        });
    }
    Err("q0v contains neither a previewable video nor audio stream".to_string())
}

fn preview_embedded_bytes(name: &str, bytes: &[u8], frame: u32) -> Result<PreviewContent, String> {
    if let Ok(image) = image::load_from_memory(bytes) {
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        return Ok(PreviewContent::Image {
            width: width as usize,
            height: height as usize,
            rgba: rgba.into_raw(),
            caption: format!("embedded image {name} • {width}×{height}"),
        });
    }

    if bytes.starts_with(b"Q0V\0") {
        let media =
            Q0vFile::parse(bytes.to_vec()).map_err(|error| format!("parse q0v: {error}"))?;
        return preview_q0v_media(&media, frame, !media.spec.video);
    }
    if bytes.starts_with(b"Q0S\0") {
        if is_q0s_v2(bytes) {
            let project =
                parse_q0s_v2(bytes).map_err(|error| format!("parse embedded q0s: {error}"))?;
            return preview_q0rg(&project, project.meta.entry_q0rg_id, frame);
        }
        let movie =
            parse_q0s(bytes).map_err(|error| format!("parse embedded legacy q0s: {error}"))?;
        return Ok(PreviewContent::Text {
            text: format!(
                "legacy q0s\nframes: {}\nfps: {}\nbitmaps: {}\nplacements: {}",
                movie.header.frame_count,
                movie.header.fps,
                movie.bitmaps.len(),
                movie
                    .placements_by_frame
                    .iter()
                    .map(Vec::len)
                    .sum::<usize>()
            ),
            caption: format!("embedded legacy q0s {name}"),
        });
    }
    if bytes.starts_with(b"Q1S\0") {
        if bytes.get(4..6) == Some(&1u16.to_le_bytes()) {
            let project =
                parse_q1s(bytes).map_err(|error| format!("parse embedded q1s v1: {error}"))?;
            let movie =
                q1s_to_movie(&project).map_err(|error| format!("convert embedded q1s: {error}"))?;
            return Ok(PreviewContent::Text {
                text: format!(
                    "legacy q1s\nproject: {}\nstage: {}×{}\nframes: {}\nbitmaps: {}",
                    project.meta.name,
                    project.meta.stage_width,
                    project.meta.stage_height,
                    movie.header.frame_count,
                    movie.bitmaps.len()
                ),
                caption: format!("embedded q1s {name}"),
            });
        }
        let project = v2::parse(bytes).map_err(|error| format!("parse embedded q1s: {error}"))?;
        return preview_q0rg(&project, project.meta.entry_q0rg_id, frame);
    }
    if matches!(bytes.get(0..3), Some(b"FWS" | b"CWS" | b"ZWS")) {
        let movie = parse_swf(bytes).map_err(|error| format!("parse embedded swf: {error}"))?;
        return Ok(PreviewContent::Text {
            text: format!(
                "swf v{}\ncompression: {}\nstage: {:.1}×{:.1}\nfps: {:.3}\nframes: {}\ntags: {}",
                movie.version,
                movie.compression.label(),
                movie.width_px,
                movie.height_px,
                movie.frame_rate,
                movie.frame_count,
                movie.tags.len()
            ),
            caption: format!("embedded swf {name}"),
        });
    }
    if looks_like_text(bytes) {
        return Ok(PreviewContent::Text {
            text: text_preview(bytes),
            caption: format!("embedded text {name} • {} bytes", bytes.len()),
        });
    }
    Ok(PreviewContent::Text {
        text: hex_preview(bytes),
        caption: format!("embedded binary {name} • {} bytes", bytes.len()),
    })
}

fn preview_swf_tag(
    movie: &crate::swf::SwfMovie,
    index: usize,
    frame: u32,
) -> Result<PreviewContent, String> {
    let tag = movie
        .tags
        .iter()
        .find(|tag| tag.index == index)
        .ok_or_else(|| format!("swf tag {index} is missing"))?;
    match tag.code {
        21 if tag.payload.len() > 2 => preview_embedded_bytes(tag.name, &tag.payload[2..], frame),
        87 if tag.payload.len() > 6 => preview_embedded_bytes(tag.name, &tag.payload[6..], frame),
        _ => Ok(PreviewContent::Text {
            text: hex_preview(&tag.payload),
            caption: format!(
                "swf tag {} [{}] • {} bytes",
                tag.name,
                tag.code,
                tag.payload.len()
            ),
        }),
    }
}

fn waveform_preview(pcm: &[u8], channels: u16, sample_rate: u32) -> PreviewContent {
    const WIDTH: usize = 720;
    const HEIGHT: usize = 180;
    let mut rgba = vec![0u8; WIDTH * HEIGHT * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[18, 18, 18, 255]);
    }
    let center = HEIGHT / 2;
    for x in 0..WIDTH {
        let base = (center * WIDTH + x) * 4;
        rgba[base..base + 4].copy_from_slice(&[68, 68, 68, 255]);
    }

    let channels = usize::from(channels.max(1));
    let samples = pcm.len() / 2;
    let frames = samples / channels;
    if frames > 0 {
        for x in 0..WIDTH {
            let start = x * frames / WIDTH;
            let end = ((x + 1) * frames / WIDTH).max(start + 1).min(frames);
            let mut peak = 0i32;
            let stride = ((end - start) / 128).max(1);
            for frame in (start..end).step_by(stride) {
                for channel in 0..channels {
                    let sample_index = frame * channels + channel;
                    let byte_index = sample_index * 2;
                    if byte_index + 1 >= pcm.len() {
                        break;
                    }
                    let sample = i16::from_le_bytes([pcm[byte_index], pcm[byte_index + 1]]);
                    peak = peak.max(i32::from(sample).abs());
                }
            }
            let half = ((peak as f32 / 32768.0) * (HEIGHT as f32 * 0.46)).round() as usize;
            let y0 = center.saturating_sub(half);
            let y1 = (center + half).min(HEIGHT - 1);
            for y in y0..=y1 {
                let base = (y * WIDTH + x) * 4;
                rgba[base..base + 4].copy_from_slice(&[200, 16, 46, 255]);
            }
        }
    }
    let duration = if sample_rate == 0 {
        0.0
    } else {
        frames as f64 / f64::from(sample_rate)
    };
    PreviewContent::Image {
        width: WIDTH,
        height: HEIGHT,
        rgba,
        caption: format!("pcm16 waveform • {channels} ch • {sample_rate} hz • {duration:.2}s"),
    }
}

fn crop_visible_rgba(
    width: usize,
    height: usize,
    rgba: Vec<u8>,
) -> Option<(usize, usize, Vec<u8>)> {
    if width == 0 || height == 0 || rgba.len() != width.checked_mul(height)?.checked_mul(4)? {
        return None;
    }
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    let mut any = false;
    for y in 0..height {
        for x in 0..width {
            if rgba[(y * width + x) * 4 + 3] == 0 {
                continue;
            }
            any = true;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if !any {
        return Some((width, height, rgba));
    }
    let padding = 8usize;
    min_x = min_x.saturating_sub(padding);
    min_y = min_y.saturating_sub(padding);
    max_x = (max_x + padding).min(width - 1);
    max_y = (max_y + padding).min(height - 1);
    let crop_width = max_x - min_x + 1;
    let crop_height = max_y - min_y + 1;
    let mut cropped = Vec::with_capacity(crop_width * crop_height * 4);
    for y in min_y..=max_y {
        let start = (y * width + min_x) * 4;
        let end = start + crop_width * 4;
        cropped.extend_from_slice(&rgba[start..end]);
    }
    Some((crop_width, crop_height, cropped))
}
fn fitted_size(width: u32, height: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let scale = (MAX_PREVIEW_WIDTH as f64 / width as f64)
        .min(MAX_PREVIEW_HEIGHT as f64 / height as f64)
        .min(1.0);
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

fn looks_like_text(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8192)];
    let Ok(text) = std::str::from_utf8(sample) else {
        return false;
    };
    if text.is_empty() {
        return true;
    }
    let controls = text
        .chars()
        .filter(|ch| ch.is_control() && !matches!(*ch, '\n' | '\r' | '\t'))
        .count();
    controls * 20 < text.chars().count().max(1)
}

fn text_preview(bytes: &[u8]) -> String {
    let limited = &bytes[..bytes.len().min(MAX_TEXT_PREVIEW_BYTES)];
    let mut text = String::from_utf8_lossy(limited).into_owned();
    if bytes.len() > limited.len() {
        text.push_str("\n\n… preview truncated …");
    }
    text
}

fn hex_preview(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let limited = &bytes[..bytes.len().min(MAX_HEX_PREVIEW_BYTES)];
    let mut out = String::new();
    for (row, chunk) in limited.chunks(16).enumerate() {
        let _ = write!(out, "{:08x}  ", row * 16);
        for index in 0..16 {
            if let Some(byte) = chunk.get(index) {
                let _ = write!(out, "{byte:02x} ");
            } else {
                out.push_str("   ");
            }
            if index == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        for byte in chunk {
            out.push(if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte)
            } else {
                '.'
            });
        }
        out.push('\n');
    }
    if bytes.len() > limited.len() {
        out.push_str("… preview truncated …\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{Layer, Placement, ProjectMeta, Q0rg, Target, Transform2D, Tween};
    use q0video::q0v::{Q0vSpec, Q0vWriter};
    use std::collections::HashMap;
    use std::io::Cursor;

    #[test]
    fn text_sniffer_rejects_binary_and_accepts_q0lang() {
        assert!(looks_like_text(b"set x = 1\ngostop! 0\n"));
        assert!(!looks_like_text(&[0, 1, 2, 3, 0xff, 0xfe]));
    }

    #[test]
    fn audio_only_q0v_gets_a_waveform_preview() {
        let spec = Q0vSpec {
            width: 0,
            height: 0,
            fps: 48_000,
            timeline_frames: 4,
            video: false,
            audio: true,
            audio_sample_rate: 48_000,
            audio_channels: 1,
        };
        let mut writer = Q0vWriter::new(Cursor::new(Vec::new()), spec).unwrap();
        writer
            .write_audio_pcm_i16(&[0, i16::MAX, i16::MIN + 1, 0])
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let media = Q0vFile::parse(bytes).unwrap();
        let preview = preview_q0v_media(&media, 0, true).unwrap();
        assert!(matches!(
            preview,
            PreviewContent::Image {
                width: 720,
                height: 180,
                ..
            }
        ));
    }

    fn visual_project() -> ProjectV2 {
        let vector = v2::VectorAsset {
            asset_id: 7,
            paths: vec![v2::Path {
                anchors: vec![
                    v2::Anchor {
                        point: v2::Vec2::new(8.0, 8.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    v2::Anchor {
                        point: v2::Vec2::new(46.0, 8.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    v2::Anchor {
                        point: v2::Vec2::new(24.0, 44.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: true,
            }],
            fill: Some(v2::Rgba {
                r: 240,
                g: 40,
                b: 70,
                a: 255,
            }),
            stroke: None,
        };
        let child = Q0rg {
            q0rg_id: 2,
            name: "child".to_string(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                name: "child".to_string(),
                explicit_keyframes: vec![0],
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(7),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
                }],
            }],
        };
        let parent = Q0rg {
            q0rg_id: 1,
            name: "parent".to_string(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 2,
                name: "parent".to_string(),
                explicit_keyframes: vec![0],
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Q0rg(2),
                    transform: Transform2D {
                        tx: 5.0,
                        ty: 3.0,
                        ..Transform2D::IDENTITY
                    },
                    tween: Tween::None,
                    fx: Default::default(),
                }],
            }],
        };
        ProjectV2 {
            meta: ProjectMeta {
                name: "preview test".to_string(),
                fps: 24,
                stage_width: 64,
                stage_height: 64,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(vector)],
            asset_names: HashMap::new(),
            asset_appearances: HashMap::new(),
            layer_metadata: HashMap::new(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![parent, child],
        }
    }

    fn image_has_visible_alpha(preview: PreviewContent) -> bool {
        match preview {
            PreviewContent::Image { rgba, .. } => rgba.chunks_exact(4).any(|pixel| pixel[3] != 0),
            PreviewContent::Vector(_) | PreviewContent::Text { .. } => false,
        }
    }

    #[test]
    fn vector_asset_preview_preserves_source_bezier_geometry_without_rasterizing() {
        let project = visual_project();
        let preview = preview_vector_asset(&project, 7).unwrap();
        let PreviewContent::Vector(vector) = preview else {
            panic!("vector preview became a bitmap/text preview");
        };
        assert_eq!(
            vector.paths,
            match &project.assets[0] {
                Asset::Vector(asset) => asset.paths.clone(),
                _ => unreachable!(),
            }
        );
        assert_eq!(
            vector.fill,
            Some(v2::Rgba {
                r: 240,
                g: 40,
                b: 70,
                a: 255
            })
        );
        assert!(vector.stroke.is_none());
    }

    #[test]
    fn q0rg_preview_renders_nested_q0rg_content() {
        let project = visual_project();
        assert!(image_has_visible_alpha(
            preview_q0rg(&project, 1, 0).unwrap()
        ));
    }

    #[test]
    fn embedded_text_and_binary_always_have_a_preview() {
        assert!(matches!(
            preview_embedded_bytes("logic.q0l", b"set x = 1\n", 0).unwrap(),
            PreviewContent::Text { ref caption, .. } if caption.contains("text")
        ));
        assert!(matches!(
            preview_embedded_bytes("mystery.bin", &[0, 1, 2, 0xff, 0xfe], 0).unwrap(),
            PreviewContent::Text { ref caption, .. } if caption.contains("binary")
        ));
    }
}
