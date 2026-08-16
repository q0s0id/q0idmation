use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use q0s_format::v2::{Asset, ProjectDependencyKind, ProjectDependencySource, ProjectV2, Rgba};

use crate::{
    document::{Document, DocumentData},
    swf::{SwfMovie, SwfTag},
};

pub fn extract_document(document: &Document, output: &Path) -> Result<ExtractionReport, String> {
    fs::create_dir_all(output)
        .map_err(|error| format!("create extraction directory {}: {error}", output.display()))?;
    fs::write(output.join("manifest.txt"), &document.summary)
        .map_err(|error| format!("write manifest: {error}"))?;

    let mut files = 1usize;
    match &document.data {
        DocumentData::Q1Legacy(project) => {
            let dir = output.join("assets");
            fs::create_dir_all(&dir)
                .map_err(|error| format!("create assets directory: {error}"))?;
            for asset in &project.assets {
                let path = dir.join(format!("bitmap_{:05}.png", asset.asset_id));
                save_rgba_png(&path, asset.width.into(), asset.height.into(), &asset.rgba)?;
                files += 1;
            }
        }
        DocumentData::QProject(project) => {
            files += extract_q_project(project, output)?;
        }
        DocumentData::Q0Legacy(movie) => {
            let dir = output.join("bitmaps");
            fs::create_dir_all(&dir)
                .map_err(|error| format!("create bitmaps directory: {error}"))?;
            let mut bitmaps = movie.bitmaps.values().collect::<Vec<_>>();
            bitmaps.sort_by_key(|bitmap| bitmap.id);
            for bitmap in bitmaps {
                let path = dir.join(format!("bitmap_{:05}.png", bitmap.id));
                save_rgba_png(
                    &path,
                    bitmap.width.into(),
                    bitmap.height.into(),
                    &bitmap.rgba,
                )?;
                files += 1;
            }
        }
        DocumentData::Q0v(media) => {
            files += extract_q0v(media, output)?;
        }
        DocumentData::Swf(movie) | DocumentData::Projector { movie, .. } => {
            files += extract_swf(movie, output)?;
        }
    }

    Ok(ExtractionReport {
        output: output.to_path_buf(),
        files_written: files,
    })
}

#[derive(Debug, Clone)]
pub struct ExtractionReport {
    pub output: PathBuf,
    pub files_written: usize,
}

fn extract_q_project(project: &ProjectV2, output: &Path) -> Result<usize, String> {
    let mut files = 0usize;
    let assets_dir = output.join("assets");
    fs::create_dir_all(&assets_dir).map_err(|error| format!("create assets directory: {error}"))?;

    for asset in &project.assets {
        let id = asset.id();
        let friendly = project
            .asset_names
            .get(&id)
            .map(|name| safe_component(name))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("asset_{id:05}"));
        match asset {
            Asset::Bitmap(bitmap) => {
                let path = assets_dir.join(format!("{id:05}_{friendly}.png"));
                save_rgba_png(
                    &path,
                    bitmap.width.into(),
                    bitmap.height.into(),
                    &bitmap.rgba,
                )?;
                files += 1;
            }
            Asset::Vector(vector) => {
                let path = assets_dir.join(format!("{id:05}_{friendly}.svg"));
                fs::write(&path, vector_to_svg(vector))
                    .map_err(|error| format!("write {}: {error}", path.display()))?;
                files += 1;
            }
            Asset::Q0v(media) => {
                let path = assets_dir.join(format!("{id:05}_{friendly}.q0v"));
                fs::write(&path, &media.bytes)
                    .map_err(|error| format!("write {}: {error}", path.display()))?;
                files += 1;
            }
            Asset::Rig(rig) => {
                let path = assets_dir.join(format!("{id:05}_{friendly}.rig.txt"));
                let text = format!(
                    "rig asset {id}\nowner q0rg: {}\nnodes: {:#?}\ncontrols: {:#?}\nconstraints: {:#?}\nchannels: {:#?}\ndrivers: {:#?}\nposes: {:#?}\ndeformers: {:#?}\npose drivers: {:#?}\nmirror pairs: {:#?}\nvariants: {:#?}\n",
                    rig.owner_q0rg_id,
                    rig.nodes,
                    rig.controls,
                    rig.constraints,
                    rig.channels,
                    rig.drivers,
                    rig.poses,
                    rig.deformers,
                    rig.pose_drivers,
                    rig.mirror_pairs,
                    rig.variants
                );
                fs::write(&path, text)
                    .map_err(|error| format!("write {}: {error}", path.display()))?;
                files += 1;
            }
        }
    }

    let scripts_dir = output.join("scripts");
    let mut made_scripts_dir = false;
    for q0rg in &project.q0rgs {
        if q0rg.script.trim().is_empty() {
            continue;
        }
        if !made_scripts_dir {
            fs::create_dir_all(&scripts_dir)
                .map_err(|error| format!("create scripts directory: {error}"))?;
            made_scripts_dir = true;
        }
        let path = scripts_dir.join(format!(
            "q0rg_{:05}_{}.q0l",
            q0rg.q0rg_id,
            safe_component(&q0rg.name)
        ));
        fs::write(&path, &q0rg.script)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        files += 1;
    }
    for (index, script) in project.runtime.frame_scripts.iter().enumerate() {
        if !made_scripts_dir {
            fs::create_dir_all(&scripts_dir)
                .map_err(|error| format!("create scripts directory: {error}"))?;
            made_scripts_dir = true;
        }
        let path = scripts_dir.join(format!(
            "frame_{index:05}_q{}_l{}_f{}.q0l",
            script.q0rg_id, script.layer_id, script.frame
        ));
        fs::write(&path, &script.source)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        files += 1;
    }

    let graph_dir = output.join("project_graph");
    let mut made_graph_dir = false;
    for node in &project.runtime.project_graph.nodes {
        let ProjectDependencySource::Embedded(bytes) = &node.source else {
            continue;
        };
        if !made_graph_dir {
            fs::create_dir_all(&graph_dir)
                .map_err(|error| format!("create project_graph directory: {error}"))?;
            made_graph_dir = true;
        }
        let ext = match node.kind {
            ProjectDependencyKind::Movie => "q0s",
            ProjectDependencyKind::Q0lang => "q0l",
        };
        let path = graph_dir.join(format!(
            "{:05}_{}.{}",
            node.node_id,
            safe_component(&node.alias),
            ext
        ));
        fs::write(&path, bytes).map_err(|error| format!("write {}: {error}", path.display()))?;
        files += 1;
    }

    let timeline_path = output.join("timelines.txt");
    let mut timeline = String::new();
    writeln!(
        timeline,
        "project {} • {}x{} @ {} fps • entry q0rg {}",
        project.meta.name,
        project.meta.stage_width,
        project.meta.stage_height,
        project.meta.fps,
        project.meta.entry_q0rg_id
    )
    .unwrap();
    for q0rg in &project.q0rgs {
        writeln!(
            timeline,
            "\nq0rg {} '{}' • {} frames • {} layers",
            q0rg.q0rg_id,
            q0rg.name,
            q0rg.frame_count,
            q0rg.layers.len()
        )
        .unwrap();
        for layer in &q0rg.layers {
            writeln!(
                timeline,
                "  layer {} '{}' • keys {:?} • {} placements",
                layer.layer_id,
                layer.name,
                layer.keyframe_frames(),
                layer.placements.len()
            )
            .unwrap();
            for placement in &layer.placements {
                writeln!(timeline, "    frame {} • {:?}", placement.frame, placement).unwrap();
            }
        }
    }
    fs::write(&timeline_path, timeline)
        .map_err(|error| format!("write {}: {error}", timeline_path.display()))?;
    files += 1;

    Ok(files)
}

fn extract_q0v(media: &q0video::q0v::Q0vFile, output: &Path) -> Result<usize, String> {
    let mut files = 0usize;
    if media.spec.video {
        let dir = output.join("frames");
        fs::create_dir_all(&dir).map_err(|error| format!("create frames directory: {error}"))?;
        for (index, _) in media.frames.iter().enumerate() {
            let png = media
                .frame_png(index)
                .ok_or_else(|| format!("q0v frame {index} has invalid bounds"))?;
            let path = dir.join(format!("{index:06}.png"));
            fs::write(&path, png).map_err(|error| format!("write {}: {error}", path.display()))?;
            files += 1;
        }
    }
    if media.spec.audio {
        let path = output.join("audio.wav");
        write_pcm16_wav(
            &path,
            media.spec.audio_sample_rate,
            media.spec.audio_channels,
            media.audio_pcm_le_bytes(),
        )?;
        files += 1;
    }
    Ok(files)
}

fn extract_swf(movie: &SwfMovie, output: &Path) -> Result<usize, String> {
    let mut files = 0usize;
    let movie_path = output.join("movie.fws.swf");
    fs::write(&movie_path, &movie.normalized_fws)
        .map_err(|error| format!("write {}: {error}", movie_path.display()))?;
    files += 1;

    let tags_dir = output.join("tags");
    fs::create_dir_all(&tags_dir).map_err(|error| format!("create tags directory: {error}"))?;
    for tag in &movie.tags {
        let path = tags_dir.join(format!(
            "{:05}_{:03}_{}.bin",
            tag.index,
            tag.code,
            safe_component(tag.name)
        ));
        fs::write(&path, &tag.payload)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        files += 1;
        files += extract_known_swf_payload(tag, output)?;
    }
    Ok(files)
}

fn extract_known_swf_payload(tag: &SwfTag, output: &Path) -> Result<usize, String> {
    match tag.code {
        21 => {
            if tag.payload.len() < 2 {
                return Ok(0);
            }
            let character_id = u16::from_le_bytes([tag.payload[0], tag.payload[1]]);
            let image = &tag.payload[2..];
            let ext = sniff_image_extension(image).unwrap_or("img");
            let dir = output.join("images");
            fs::create_dir_all(&dir)
                .map_err(|error| format!("create images directory: {error}"))?;
            let path = dir.join(format!("jpeg2_{character_id:05}.{ext}"));
            fs::write(&path, image)
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            Ok(1)
        }
        82 => {
            if tag.payload.len() < 4 {
                return Ok(0);
            }
            let name_end = tag.payload[4..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|position| position + 4)
                .unwrap_or(tag.payload.len());
            let abc_start = name_end.saturating_add(1).min(tag.payload.len());
            if abc_start >= tag.payload.len() {
                return Ok(0);
            }
            let name = String::from_utf8_lossy(&tag.payload[4..name_end]);
            let dir = output.join("abc");
            fs::create_dir_all(&dir).map_err(|error| format!("create abc directory: {error}"))?;
            let path = dir.join(format!(
                "{:05}_{}.abc",
                tag.index,
                safe_component(if name.is_empty() { "unnamed" } else { &name })
            ));
            fs::write(&path, &tag.payload[abc_start..])
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            Ok(1)
        }
        87 => {
            if tag.payload.len() < 6 {
                return Ok(0);
            }
            let character_id = u16::from_le_bytes([tag.payload[0], tag.payload[1]]);
            let dir = output.join("binary");
            fs::create_dir_all(&dir)
                .map_err(|error| format!("create binary directory: {error}"))?;
            let path = dir.join(format!("binary_{character_id:05}.bin"));
            fs::write(&path, &tag.payload[6..])
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            Ok(1)
        }
        _ => Ok(0),
    }
}

fn vector_to_svg(vector: &q0s_format::v2::VectorAsset) -> String {
    let mut points = Vec::new();
    for path in &vector.paths {
        for anchor in &path.anchors {
            points.push(anchor.point);
            if let Some(point) = anchor.in_handle {
                points.push(point);
            }
            if let Some(point) = anchor.out_handle {
                points.push(point);
            }
        }
    }
    let (min_x, min_y, max_x, max_y) = if let Some(first) = points.first().copied() {
        points.iter().copied().fold(
            (first.x, first.y, first.x, first.y),
            |(min_x, min_y, max_x, max_y), point| {
                (
                    min_x.min(point.x),
                    min_y.min(point.y),
                    max_x.max(point.x),
                    max_y.max(point.y),
                )
            },
        )
    } else {
        (0.0, 0.0, 1.0, 1.0)
    };
    let pad = vector
        .stroke
        .map(|stroke| stroke.width * 0.75)
        .unwrap_or(1.0)
        .max(1.0);
    let width = (max_x - min_x + pad * 2.0).max(1.0);
    let height = (max_y - min_y + pad * 2.0).max(1.0);

    let fill = vector
        .fill
        .map(svg_color)
        .unwrap_or_else(|| "none".to_string());
    let fill_opacity = vector
        .fill
        .map(|color| f32::from(color.a) / 255.0)
        .unwrap_or(1.0);
    let (stroke, stroke_opacity, stroke_width) = vector
        .stroke
        .map(|stroke| {
            (
                svg_color(stroke.color),
                f32::from(stroke.color.a) / 255.0,
                stroke.width,
            )
        })
        .unwrap_or_else(|| ("none".to_string(), 1.0, 0.0));

    let mut out = String::new();
    writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\">",
        min_x - pad,
        min_y - pad,
        width,
        height
    )
    .unwrap();
    writeln!(
        out,
        "  <g fill=\"{fill}\" fill-opacity=\"{fill_opacity:.6}\" stroke=\"{stroke}\" stroke-opacity=\"{stroke_opacity:.6}\" stroke-width=\"{stroke_width}\">"
    )
    .unwrap();
    for path in &vector.paths {
        if path.anchors.is_empty() {
            continue;
        }
        let mut data = String::new();
        let first = &path.anchors[0];
        write!(data, "M {} {}", first.point.x, first.point.y).unwrap();
        for pair in path.anchors.windows(2) {
            append_svg_segment(&mut data, &pair[0], &pair[1]);
        }
        if path.closed {
            let last = path.anchors.last().unwrap();
            append_svg_segment(&mut data, last, first);
            data.push_str(" Z");
        }
        writeln!(out, "    <path d=\"{data}\"/>").unwrap();
    }
    out.push_str("  </g>\n</svg>\n");
    out
}

fn append_svg_segment(
    data: &mut String,
    current: &q0s_format::v2::Anchor,
    next: &q0s_format::v2::Anchor,
) {
    if current.out_handle.is_some() || next.in_handle.is_some() {
        let c1 = current.out_handle.unwrap_or(current.point);
        let c2 = next.in_handle.unwrap_or(next.point);
        write!(
            data,
            " C {} {}, {} {}, {} {}",
            c1.x, c1.y, c2.x, c2.y, next.point.x, next.point.y
        )
        .unwrap();
    } else {
        write!(data, " L {} {}", next.point.x, next.point.y).unwrap();
    }
}

fn svg_color(color: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

fn save_rgba_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    image::save_buffer(path, rgba, width, height, image::ColorType::Rgba8)
        .map_err(|error| format!("write {}: {error}", path.display()))
}

fn write_pcm16_wav(path: &Path, sample_rate: u32, channels: u16, pcm: &[u8]) -> Result<(), String> {
    if !pcm.len().is_multiple_of(usize::from(channels).max(1) * 2) {
        return Err("q0v pcm payload is not aligned to whole channel samples".to_string());
    }
    let data_len = u32::try_from(pcm.len()).map_err(|_| "wav payload exceeds 4 GiB".to_string())?;
    let byte_rate = sample_rate
        .checked_mul(u32::from(channels))
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| "wav byte rate overflow".to_string())?;
    let block_align = channels
        .checked_mul(2)
        .ok_or_else(|| "wav block alignment overflow".to_string())?;
    let riff_len = 36u32
        .checked_add(data_len)
        .ok_or_else(|| "wav RIFF length overflow".to_string())?;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_len.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    fs::write(path, wav).map_err(|error| format!("write {}: {error}", path.display()))
}

fn sniff_image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else {
        None
    }
}

fn safe_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len().min(80));
    for ch in value.chars().take(80) {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | ' ') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out.trim().trim_matches('.').replace(' ', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_sanitizer_removes_path_syntax() {
        assert_eq!(safe_component("../hello:world\\x"), "___hello_world_x");
    }

    #[test]
    fn jpeg_sniffer_recognizes_common_payloads() {
        assert_eq!(
            sniff_image_extension(&[0xff, 0xd8, 0xff, 0xe0]),
            Some("jpg")
        );
        assert_eq!(sniff_image_extension(b"\x89PNG\r\n\x1a\nxxx"), Some("png"));
    }

    #[test]
    fn q0v_extraction_preserves_frame_payload_and_writes_pcm_wav() {
        use q0video::q0v::{Q0vFile, Q0vSpec, Q0vWriter};
        use std::io::Cursor;
        use std::time::{SystemTime, UNIX_EPOCH};

        let spec = Q0vSpec {
            width: 1,
            height: 1,
            fps: 24,
            timeline_frames: 1,
            video: true,
            audio: true,
            audio_sample_rate: 48_000,
            audio_channels: 1,
        };
        let mut writer = Q0vWriter::new(Cursor::new(Vec::new()), spec).expect("q0v writer");
        writer
            .write_video_frame(0, b"frame-payload")
            .expect("video frame");
        writer
            .write_audio_pcm_i16(&[123, -456])
            .expect("audio samples");
        let bytes = writer.finish().expect("finish q0v").into_inner();
        let media = Q0vFile::parse(bytes).expect("parse q0v");

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "canripper-q0v-extract-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("temp output");
        let written = extract_q0v(&media, &root).expect("extract q0v");
        assert_eq!(written, 2);
        assert_eq!(
            fs::read(root.join("frames/000000.png")).expect("frame output"),
            b"frame-payload"
        );
        let wav = fs::read(root.join("audio.wav")).expect("wav output");
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(
            &wav[44..],
            &[123_i16.to_le_bytes(), (-456_i16).to_le_bytes()].concat()
        );
        fs::remove_dir_all(root).expect("cleanup temp output");
    }
}
