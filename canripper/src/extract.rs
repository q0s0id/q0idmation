use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use q0s_format::v2::{Asset, ProjectDependencyKind, ProjectDependencySource, ProjectV2, Rgba};

use crate::{
    document::{Document, DocumentData, PreviewRef},
    preview::{PreviewContent, VectorLayer},
    swf::{SwfMovie, SwfTag},
    swf_assets::{self, Category, Library},
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
                fs::write(&path, media.bytes.as_slice())
                    .map_err(|error| format!("write {}: {error}", path.display()))?;
                files += 1;
            }
            Asset::Model(model) => {
                let path = assets_dir.join(format!("{id:05}_{friendly}.q0model"));
                let bytes = q0s_format::three::wire::model_to_bytes(&model.data)
                    .map_err(|error| format!("encode model {id}: {error}"))?;
                fs::write(&path, bytes)
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
            media.audio_pcm_le_bytes()?,
        )?;
        files += 1;
    }
    Ok(files)
}

/// The decompressed movie, then every character decoded into a folder for
/// its kind, the way JPEXS exports: images as PNG, shapes, sprites and
/// buttons as SVG, sounds as MP3 or WAV, scripts as P-code or ABC. What
/// cannot be decoded is kept as raw tag payloads. Timeline tags are left
/// out: the movie holds them.
fn extract_swf(movie: &SwfMovie, output: &Path) -> Result<usize, String> {
    let mut files = 0usize;
    let movie_path = output.join("movie.fws.swf");
    fs::write(&movie_path, &movie.normalized_fws)
        .map_err(|error| format!("write {}: {error}", movie_path.display()))?;
    files += 1;
    let library = Library::new(movie);
    for tag in &movie.tags {
        if Category::of(tag.code) == Category::Control {
            continue;
        }
        if let Some((folder, name, bytes)) = export_swf_tag(&library, tag, 0) {
            let dir = output.join(folder);
            fs::create_dir_all(&dir)
                .map_err(|error| format!("create {}: {error}", dir.display()))?;
            let path = dir.join(name);
            fs::write(&path, bytes)
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            files += 1;
        }
    }
    Ok(files)
}

/// A tag as the file it exports to: its folder, file name and bytes.
fn export_swf_tag(
    library: &Library,
    tag: &SwfTag,
    frame: u32,
) -> Option<(&'static str, String, Vec<u8>)> {
    let id = swf_assets::character_id(tag);
    let stem = match id {
        Some(id) => match library.name(id) {
            Some(name) => format!("{id}_{}", safe_component(name)),
            None => id.to_string(),
        },
        None => format!("{:05}_{}", tag.index, safe_component(tag.name)),
    };
    let raw = || Some(("tags", format!("{stem}.bin"), tag.payload.clone()));
    match Category::of(tag.code) {
        Category::Image => match library.decode_image(tag) {
            Ok((width, height, rgba)) => png_bytes(width, height, &rgba)
                .ok()
                .map(|png| ("images", format!("{stem}.png"), png)),
            Err(_) => raw(),
        },
        category @ (Category::Shape | Category::Sprite | Category::Button) => {
            let layers = library.draw(tag, frame).ok()?;
            let bounds = swf_assets::layer_bounds(&layers)?;
            let svg = layers_to_svg(
                &layers,
                (bounds.x_min, bounds.y_min, bounds.x_max, bounds.y_max),
                None,
            );
            Some((category.folder(), format!("{stem}.svg"), svg.into_bytes()))
        }
        Category::Sound => {
            let sound = swf_assets::parse_sound(tag).ok()?;
            let (extension, bytes) = sound.file(tag);
            Some(("sounds", format!("{stem}.{extension}"), bytes))
        }
        Category::BinaryData => {
            let data = tag.payload.get(6..)?;
            let extension = sniff_extension(data).unwrap_or("bin");
            Some(("binaryData", format!("{stem}.{extension}"), data.to_vec()))
        }
        Category::Script => {
            if let Some(code) = swf_assets::action_code(tag) {
                return Some((
                    "scripts",
                    format!("{stem}.pcode.txt"),
                    swf_assets::disassemble(code).into_bytes(),
                ));
            }
            let mut reader = swf_assets::Reader::new(&tag.payload);
            reader.u32().ok()?;
            let name = reader.string().ok()?;
            let abc = reader.rest().to_vec();
            let name = if name.is_empty() {
                stem.clone()
            } else {
                safe_component(&name)
            };
            Some(("scripts", format!("{name}.abc"), abc))
        }
        Category::Text if tag.code == 37 => {
            let text = swf_assets::parse_edit_text(tag).ok()?;
            Some((
                "texts",
                format!("{stem}.txt"),
                text.text.unwrap_or_default().into_bytes(),
            ))
        }
        Category::Control => None,
        _ => raw(),
    }
}

/// What the selected node saves as: a suggested file name and its bytes.
pub struct Exported {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// The selected node as a file of its own: decoded where CanRipper can
/// decode it, else as what its preview shows.
pub fn export_selection(
    document: &Document,
    reference: &PreviewRef,
    frame: u32,
    label: &str,
) -> Result<Exported, String> {
    let stem = {
        let cleaned = safe_component(label);
        if cleaned.is_empty() {
            "export".to_string()
        } else {
            cleaned.chars().take(60).collect()
        }
    };
    if let (PreviewRef::SwfTag(index), DocumentData::Swf(movie))
    | (PreviewRef::SwfTag(index), DocumentData::Projector { movie, .. }) =
        (reference, &document.data)
    {
        let library = Library::new(movie);
        let tag = movie.tags.get(*index).ok_or("this tag is missing")?;
        if let Some((_, name, bytes)) = export_swf_tag(&library, tag, frame) {
            return Ok(Exported { name, bytes });
        }
    }
    let (bytes, _, what) = crate::preview::node_bytes(document, Some(reference));
    if what == "q0v media" || what == "embedded file" {
        let extension = sniff_extension(&bytes).unwrap_or("bin");
        return Ok(Exported {
            name: format!("{stem}.{extension}"),
            bytes: bytes.into_owned(),
        });
    }
    match crate::preview::build_preview(document, reference, frame)? {
        PreviewContent::Image {
            width,
            height,
            rgba,
            ..
        } => Ok(Exported {
            name: format!("{stem}.png"),
            bytes: png_bytes(width as u32, height as u32, &rgba)?,
        }),
        PreviewContent::Vector(vector) => Ok(Exported {
            name: format!("{stem}.svg"),
            bytes: layers_to_svg(&vector.layers, vector.bounds, vector.background).into_bytes(),
        }),
        PreviewContent::Text { text, .. } => Ok(Exported {
            name: format!("{stem}.txt"),
            bytes: text.into_bytes(),
        }),
    }
}

pub fn png_bytes(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or("image size does not match its pixels")?;
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageOutputFormat::Png)
        .map_err(|error| format!("encode png: {error}"))?;
    Ok(out.into_inner())
}

/// Vector layers as an SVG document showing `bounds`.
pub fn layers_to_svg(
    layers: &[VectorLayer],
    bounds: (f32, f32, f32, f32),
    background: Option<Rgba>,
) -> String {
    let (min_x, min_y, max_x, max_y) = bounds;
    let (width, height) = ((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));
    let mut out = String::new();
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{min_x} {min_y} {width} {height}\" width=\"{width}\" height=\"{height}\">"
    );
    if let Some(color) = background {
        let _ = writeln!(
            out,
            "  <rect x=\"{min_x}\" y=\"{min_y}\" width=\"{width}\" height=\"{height}\" fill=\"{}\"/>",
            svg_color(color)
        );
    }
    for layer in layers {
        let mut data = String::new();
        for path in &layer.paths {
            let Some(first) = path.anchors.first() else {
                continue;
            };
            let _ = write!(data, "M {} {}", first.point.x, first.point.y);
            for pair in path.anchors.windows(2) {
                append_svg_segment(&mut data, &pair[0], &pair[1]);
            }
            if path.closed {
                if let Some(last) = path.anchors.last() {
                    append_svg_segment(&mut data, last, first);
                }
                data.push_str(" Z ");
            } else {
                data.push(' ');
            }
        }
        if data.is_empty() {
            continue;
        }
        let fill = layer.fill.map_or("none".to_string(), |color| {
            format!(
                "{}\" fill-opacity=\"{:.3}",
                svg_color(color),
                f32::from(color.a) / 255.0
            )
        });
        let stroke = layer.stroke.map_or(String::new(), |stroke| {
            format!(
                " stroke=\"{}\" stroke-opacity=\"{:.3}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"",
                svg_color(stroke.color),
                f32::from(stroke.color.a) / 255.0,
                stroke.width
            )
        });
        let _ = writeln!(
            out,
            "  <path d=\"{}\" fill=\"{fill}\" fill-rule=\"nonzero\"{stroke}/>",
            data.trim_end()
        );
    }
    out.push_str("</svg>\n");
    out
}

/// The extension a file's first bytes say it is.
fn sniff_extension(bytes: &[u8]) -> Option<&'static str> {
    if let Some(image) = sniff_image_extension(bytes) {
        return Some(image);
    }
    if bytes.starts_with(b"Q0S\0") {
        Some("q0s")
    } else if bytes.starts_with(b"Q1S\0") {
        Some("q1s")
    } else if bytes.starts_with(b"Q0V\0") {
        Some("q0v")
    } else if bytes.starts_with(b"FWS") || bytes.starts_with(b"CWS") || bytes.starts_with(b"ZWS") {
        Some("swf")
    } else if bytes.starts_with(b"ID3") || bytes.starts_with(&[0xff, 0xfb]) {
        Some("mp3")
    } else if bytes.starts_with(b"<?xml") {
        Some("xml")
    } else {
        None
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
    fn exported_files_are_named_for_what_their_bytes_hold() {
        assert_eq!(sniff_extension(b"Q0S\0\x2a\0"), Some("q0s"));
        assert_eq!(sniff_extension(b"CWS\x08...."), Some("swf"));
        assert_eq!(sniff_extension(b"ID3\x03"), Some("mp3"));
        assert_eq!(sniff_extension(b"nothing known"), None);
    }

    #[test]
    fn vector_layers_become_svg_paths_in_their_colours() {
        use q0s_format::v2::{Anchor, Path as VPath, Vec2};
        let corner = |x: f32, y: f32| Anchor {
            point: Vec2::new(x, y),
            in_handle: None,
            out_handle: None,
        };
        let layer = VectorLayer {
            paths: vec![VPath {
                anchors: vec![corner(0.0, 0.0), corner(10.0, 0.0), corner(10.0, 10.0)],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 128,
            }),
            stroke: None,
        };
        let white = Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
        let svg = layers_to_svg(&[layer], (0.0, 0.0, 10.0, 10.0), Some(white));
        assert!(svg.contains("<rect"), "{svg}");
        assert!(
            svg.contains("fill=\"#ff0000\" fill-opacity=\"0.502\""),
            "{svg}"
        );
        assert!(svg.contains("M 0 0 L 10 0 L 10 10 L 0 0 Z"), "{svg}");
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
