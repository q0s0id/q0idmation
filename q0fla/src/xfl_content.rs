use std::collections::{HashMap, HashSet};
use std::io::Read;

use flate2::read::ZlibDecoder;
use q0s_format::geom::CapShape;
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Anchor, Asset, BitmapAsset, BlendMode, BlurFx, CameraKey, CameraState, CameraTrack, Easing,
    EasingFamily, EasingMode, GlowFx, GradientFill, GradientKind, GradientStop, Layer, LayerKey,
    LayerKind, LayerMetadata, Path as VectorPath, Placement, PlacementFx, ProjectScene, ProjectV2,
    Q0rg, Q0rgPlayback, Q0rgPlaybackMode, Rgba, Stroke, Target, Transform2D, Tween, Vec2,
    VectorAppearance, VectorAsset, VectorMaterial,
};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::{ImportError, ImportedProject};

#[derive(Debug, Clone)]
struct RawTimeline {
    name: String,
    layers: Vec<RawLayer>,
    frame_count: u16,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct RawLayer {
    name: String,
    kind: LayerKind,
    masked: bool,
    mask_source_index: Option<usize>,
    is_camera: bool,
    parent_index: Option<usize>,
    hidden: bool,
    locked: bool,
    frames: Vec<RawFrame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawTweenType {
    None,
    Motion,
    Shape,
}

#[derive(Debug, Clone)]
struct RawFrame {
    index: u16,
    duration: u16,
    tween_type: RawTweenType,
    ease_method: Option<String>,
    ease_intensity: f32,
    custom_ease_points: Vec<[f32; 2]>,
    elements: Vec<RawElement>,
}

#[derive(Debug, Clone)]
enum RawElement {
    Symbol(RawSymbolInstance),
    Bitmap(RawBitmapInstance),
    Shape(RawShape),
    Text(RawText),
}

#[derive(Debug, Clone)]
pub(crate) struct RawText {
    pub(crate) affine: Affine,
    pub(crate) offset: Vec2,
    pub(crate) text: String,
    pub(crate) font: String,
    pub(crate) size: f32,
    pub(crate) color: Rgba,
}

#[derive(Debug, Clone)]
struct RawSymbolInstance {
    library_item_name: String,
    affine: Affine,
    playback: Q0rgPlayback,
    opacity: f32,
    tint: Option<q0s_format::v2::TintFx>,
    blend_mode: BlendMode,
    unsupported_blend_mode: Option<String>,
    blur: Option<BlurFx>,
    glow: Option<GlowFx>,
}

#[derive(Debug, Clone)]
struct RawBitmapInstance {
    library_item_name: String,
    affine: Affine,
}

#[derive(Debug, Clone)]
struct RawBitmapMedia {
    name: String,
    data_href: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RawShape {
    pub(crate) affine: Affine,
    pub(crate) fills: HashMap<u16, RawFillStyle>,
    pub(crate) strokes: HashMap<u16, Stroke>,
    pub(crate) edges: Vec<RawEdge>,
}

#[derive(Debug, Clone)]
pub(crate) enum RawFillStyle {
    Solid(Rgba),
    Gradient(GradientFill),
    Bitmap {
        reference: u32,
        transform: Affine,
        tiled: bool,
    },
}

#[derive(Debug)]
struct GradientState {
    depth: usize,
    style_index: u16,
    kind: GradientKind,
    matrix: Affine,
    stops: Vec<GradientStop>,
}

#[derive(Debug, Clone)]
pub(crate) struct RawEdge {
    pub(crate) fill0: Option<u16>,
    pub(crate) fill1: Option<u16>,
    pub(crate) stroke: Option<u16>,
    pub(crate) segments: Vec<Segment>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Segment {
    pub(crate) start: Vec2,
    pub(crate) end: Vec2,
    pub(crate) control: Option<Vec2>,
}

impl Segment {
    fn reversed(self) -> Self {
        Self {
            start: self.end,
            end: self.start,
            control: self.control,
        }
    }

    fn transformed(self, affine: Affine) -> Self {
        Self {
            start: affine.apply(self.start),
            end: affine.apply(self.end),
            control: self.control.map(|point| affine.apply(point)),
        }
    }
}

#[derive(Debug)]
struct ShapeState {
    depth: usize,
    affine: Affine,
    fills: HashMap<u16, RawFillStyle>,
    strokes: HashMap<u16, Stroke>,
    edges: Vec<RawEdge>,
    fill_style: Option<(usize, u16)>,
    stroke_style: Option<(usize, u16)>,
    gradient: Option<GradientState>,
}

#[derive(Debug)]
struct InstanceState {
    depth: usize,
    instance: RawSymbolInstance,
}
#[derive(Debug)]
struct BitmapState {
    depth: usize,
    instance: RawBitmapInstance,
}

#[derive(Debug)]
struct TextState {
    depth: usize,
    affine: Affine,
    offset: Vec2,
    text: String,
    font: String,
    size: f32,
    color: Rgba,
    characters_depth: Option<usize>,
    run_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct GroupState {
    depth: usize,
    affine: Affine,
}

pub(crate) fn enrich_project(
    imported: &mut ImportedProject,
    dom_xml: &[u8],
    library_docs: &[(String, Vec<u8>)],
    binary_docs: &[(String, Vec<u8>)],
) -> Result<(), ImportError> {
    let document_scenes = parse_document_timelines(dom_xml)?;
    if document_scenes.is_empty() {
        return Err(ImportError::Invalid(
            "domdocument.xml has no readable scene timelines".to_string(),
        ));
    }

    let mut symbols = Vec::new();
    for (entry_path, xml) in library_docs {
        if let Some(timeline) = parse_timeline(xml, true)? {
            if !timeline.name.trim().is_empty() {
                symbols.push((library_item_path(entry_path), timeline));
            }
        }
    }

    let mut symbol_ids = HashMap::<String, u16>::new();
    let mut symbol_frame_counts_by_id = HashMap::<u16, u16>::new();
    let mut symbols_by_name = HashMap::<String, Vec<u16>>::new();
    let mut symbol_ids_by_path = Vec::with_capacity(symbols.len());
    for (path, symbol) in &symbols {
        let next = u16::try_from(symbol_ids_by_path.len() + document_scenes.len() + 1)
            .map_err(|_| ImportError::Invalid("too many fla symbols for q0rg ids".to_string()))?;
        symbol_ids.insert(normalize_library_reference(path), next);
        symbols_by_name
            .entry(normalize_library_reference(&symbol.name))
            .or_default()
            .push(next);
        symbol_ids_by_path.push(next);
        symbol_frame_counts_by_id.insert(next, symbol.frame_count.max(1));
    }
    // Bare names remain supported when they uniquely identify a library item;
    // folder-qualified references always resolve to their own symbol.
    for (name, ids) in symbols_by_name {
        if ids.len() == 1 {
            symbol_ids.entry(name).or_insert(ids[0]);
        } else {
            imported.report.warnings.push(format!(
                "library contains {} symbols named '{}'; folder-qualified references are used",
                ids.len(),
                name
            ));
        }
    }
    let symbol_frame_counts: HashMap<String, u16> = symbol_ids
        .iter()
        .filter_map(|(name, id)| {
            symbol_frame_counts_by_id
                .get(id)
                .map(|count| (name.clone(), *count))
        })
        .collect();

    imported.project.assets.clear();
    imported.project.asset_names.clear();
    imported.project.asset_appearances.clear();
    imported.project.q0rgs.clear();
    imported.project.layer_metadata.clear();
    imported.project.runtime.scenes.clear();
    imported.project.runtime.library_folders.clear();
    imported.project.runtime.library_item_folders.clear();
    let mut folder_reader = Reader::from_reader(dom_xml);
    loop {
        match folder_reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element))
                if element.name().as_ref() == b"DOMFolderItem" =>
            {
                if let Some(name) = attr(&folder_reader, &element, b"name")? {
                    ensure_library_folder_paths(&mut imported.project, &name.replace('\\', "/"));
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(ImportError::Invalid(format!("library folders: {error}"))),
            _ => {}
        }
    }
    imported.project.runtime.camera = CameraTrack::default();
    if let Some(entry_scene) = document_scenes.first() {
        imported.project.runtime.camera =
            camera_track_from_timeline(entry_scene, &mut imported.report.warnings);
    }
    if document_scenes
        .iter()
        .skip(1)
        .any(|scene| scene.layers.iter().any(|layer| layer.is_camera))
    {
        imported.report.warnings.push(
            "camera layers from scenes after the first were not imported because q1s stores one project camera track"
                .to_string(),
        );
    }

    let binary_by_name: HashMap<String, &[u8]> = binary_docs
        .iter()
        .map(|(name, bytes)| {
            (
                name.replace('\\', "/").to_ascii_lowercase(),
                bytes.as_slice(),
            )
        })
        .collect();
    let mut bitmap_folders = HashMap::<String, String>::new();
    for (entry_path, xml) in library_docs {
        let Some(folder) = library_folder_from_item_path(&library_item_path(entry_path)) else {
            continue;
        };
        for media in parse_bitmap_media(xml)? {
            bitmap_folders.insert(normalize_library_reference(&media.name), folder.clone());
        }
    }
    let mut bitmap_asset_ids = HashMap::<String, u16>::new();
    let mut next_asset_id = 1u16;
    for media in parse_bitmap_media(dom_xml)? {
        let archive_name =
            format!("bin/{}", media.data_href.replace('\\', "/")).to_ascii_lowercase();
        let Some(bytes) = binary_by_name.get(&archive_name).copied() else {
            imported.report.warnings.push(format!(
                "bitmap '{}' references missing '{}'; bitmap instances will be skipped",
                media.name, media.data_href
            ));
            continue;
        };
        match decode_lossless_bitmap_dat(bytes) {
            Ok((width, height, rgba)) => {
                let asset_id = next_asset_id;
                next_asset_id = next_asset_id.checked_add(1).ok_or_else(|| {
                    ImportError::Invalid("too many bitmap assets in fla".to_string())
                })?;
                imported.project.assets.push(Asset::Bitmap(BitmapAsset {
                    asset_id,
                    width,
                    height,
                    rgba,
                }));
                let normalized_name = media.name.replace('\\', "/");
                imported.project.asset_names.insert(
                    asset_id,
                    normalized_name
                        .rsplit('/')
                        .next()
                        .unwrap_or(&media.name)
                        .to_string(),
                );
                let name_folder = library_folder_from_item_path(&normalized_name);
                if let Some(folder) = name_folder
                    .as_ref()
                    .or_else(|| bitmap_folders.get(&normalize_library_reference(&media.name)))
                {
                    if let Some(folder) = ensure_library_folder_paths(&mut imported.project, folder)
                    {
                        imported.project.runtime.library_item_folders.insert(
                            q0s_format::v2::LibraryItemReference::Asset(asset_id),
                            folder,
                        );
                    }
                }
                bitmap_asset_ids.insert(normalize_library_reference(&media.name), asset_id);
            }
            Err(error) => imported.report.warnings.push(format!(
                "bitmap '{}' could not be decoded from '{}': {error}",
                media.name, media.data_href
            )),
        }
    }
    for (index, scene) in document_scenes.into_iter().enumerate() {
        let scene_id = u16::try_from(index + 1)
            .map_err(|_| ImportError::Invalid("too many fla scenes".to_string()))?;
        let scene_name = if scene.name.trim().is_empty() {
            format!("Scene {}", index + 1)
        } else {
            scene.name.trim().to_string()
        };
        let scene_built = build_q0rg(
            &scene,
            scene_id,
            &scene_name,
            &symbol_ids,
            &symbol_frame_counts,
            &bitmap_asset_ids,
            &mut imported.project,
            &mut next_asset_id,
        )?;
        imported.project.q0rgs.push(scene_built.q0rg);
        imported.project.layer_metadata.extend(scene_built.metadata);
        imported.project.runtime.scenes.push(ProjectScene {
            scene_id,
            q0rg_id: scene_id,
            name: scene_name,
        });
        imported.report.warnings.extend(scene.warnings);
        imported.report.warnings.extend(scene_built.warnings);
        imported.pending_text.extend(scene_built.pending_text);
    }

    for ((path, symbol), q0rg_id) in symbols.into_iter().zip(symbol_ids_by_path) {
        let normalized_name = symbol.name.replace('\\', "/");
        let symbol_name = normalized_name
            .rsplit('/')
            .next()
            .unwrap_or(&symbol.name)
            .to_string();
        let built = build_q0rg(
            &symbol,
            q0rg_id,
            &symbol_name,
            &symbol_ids,
            &symbol_frame_counts,
            &bitmap_asset_ids,
            &mut imported.project,
            &mut next_asset_id,
        )?;
        imported.project.q0rgs.push(built.q0rg);
        imported.project.layer_metadata.extend(built.metadata);
        if let Some(folder_path) = library_folder_from_item_path(&path) {
            if let Some(folder_path) =
                ensure_library_folder_paths(&mut imported.project, &folder_path)
            {
                imported.project.runtime.library_item_folders.insert(
                    q0s_format::v2::LibraryItemReference::Q0rg(q0rg_id),
                    folder_path,
                );
            }
        }
        imported.report.warnings.extend(symbol.warnings);
        imported.report.warnings.extend(built.warnings);
        imported.pending_text.extend(built.pending_text);
    }

    imported.report.imported_layers = imported
        .project
        .q0rgs
        .iter()
        .map(|q0rg| q0rg.layers.len())
        .sum();
    imported.report.imported_keyframes = imported
        .project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .map(|layer| layer.explicit_keyframes.len())
        .sum();
    imported.report.warnings.retain(|warning| {
        !warning.contains("first q0fla pass imports document/timeline structure only")
    });
    dedup_warnings(&mut imported.report.warnings);

    q0s_format::v2::validate(&imported.project).map_err(|error| {
        ImportError::Invalid(format!("enriched q1s model failed validation: {error}"))
    })?;
    Ok(())
}

fn library_item_path(entry_path: &str) -> String {
    let normalized = entry_path.replace('\\', "/");
    let without_library = normalized
        .get(..8)
        .filter(|prefix| prefix.eq_ignore_ascii_case("library/"))
        .map(|_| &normalized[8..])
        .unwrap_or(normalized.as_str());
    without_library
        .get(..without_library.len().saturating_sub(4))
        .filter(|_| without_library.to_ascii_lowercase().ends_with(".xml"))
        .unwrap_or(without_library)
        .to_string()
}

fn normalize_library_reference(name: &str) -> String {
    let normalized = name.trim().replace('\\', "/");
    let without_library = normalized
        .get(..8)
        .filter(|prefix| prefix.eq_ignore_ascii_case("library/"))
        .map(|_| &normalized[8..])
        .unwrap_or(normalized.as_str());
    let without_extension = if without_library.to_ascii_lowercase().ends_with(".xml") {
        &without_library[..without_library.len() - 4]
    } else {
        without_library
    };
    without_extension
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
        .to_lowercase()
}

fn library_folder_from_item_path(item_path: &str) -> Option<String> {
    item_path
        .rsplit_once('/')
        .and_then(|(folder, _)| normalize_library_folder_path(folder))
}

fn normalize_library_folder_path(path: &str) -> Option<String> {
    let segments = path
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    (!segments.is_empty()).then(|| segments.join("/"))
}

pub(crate) fn ensure_library_folder_paths(project: &mut ProjectV2, path: &str) -> Option<String> {
    let path = normalize_library_folder_path(path)?;
    let mut current = String::new();
    for segment in path.split('/') {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(segment);
        if !project
            .runtime
            .library_folders
            .iter()
            .any(|folder| folder == &current)
        {
            project.runtime.library_folders.push(current.clone());
        }
    }
    project.runtime.library_folders.sort();
    Some(path)
}

struct BuiltQ0rg {
    q0rg: Q0rg,
    metadata: HashMap<LayerKey, LayerMetadata>,
    warnings: Vec<String>,
    pending_text: Vec<crate::PendingText>,
}

fn import_frame_tween(
    frame: &RawFrame,
    next_frame: Option<u16>,
    q0rg_name: &str,
    layer_name: &str,
    warnings: &mut Vec<String>,
) -> Tween {
    // Motion and shape tweens share Animate's easing description; the caller
    // decides which artwork the result applies to.
    let (RawTweenType::Motion | RawTweenType::Shape, Some(to_frame)) =
        (frame.tween_type, next_frame)
    else {
        return Tween::None;
    };
    if to_frame <= frame.index {
        return Tween::None;
    }
    if frame.custom_ease_points.is_empty() {
        let Some(easing) = animate_easing(frame.ease_method.as_deref(), frame.ease_intensity)
        else {
            warnings.push(format!(
                "'{q0rg_name}' layer '{layer_name}' frame {} has unsupported Animate ease method '{}' or intensity {}; imported as linear",
                frame.index,
                frame.ease_method.as_deref().unwrap_or("classic"),
                frame.ease_intensity,
            ));
            return Tween::Linear { to_frame };
        };
        return Tween::Linear { to_frame }.with_easing(easing);
    }
    if frame.custom_ease_points.len() != 4 {
        warnings.push(format!(
            "'{q0rg_name}' layer '{layer_name}' frame {} has an unsupported Animate custom ease point count {}; imported as linear",
            frame.index,
            frame.custom_ease_points.len()
        ));
        return Tween::Linear { to_frame };
    }
    let p1 = frame.custom_ease_points[1];
    let p2 = frame.custom_ease_points[2];
    let easing = Easing::CubicBezier {
        x1: p1[0],
        y1: p1[1],
        x2: p2[0],
        y2: p2[1],
    };
    if easing.is_valid() {
        Tween::Eased { to_frame, easing }
    } else {
        warnings.push(format!(
            "'{q0rg_name}' layer '{layer_name}' frame {} has invalid Animate custom ease; imported as linear",
            frame.index
        ));
        Tween::Linear { to_frame }
    }
}

fn animate_easing(method: Option<&str>, intensity: f32) -> Option<Easing> {
    let method = method.unwrap_or("classic");
    if matches!(method, "none" | "linear") {
        return Some(Easing::Linear);
    }
    if matches!(method, "classic" | "") {
        if !intensity.is_finite() || !(-100.0..=100.0).contains(&intensity) {
            return None;
        }
        if intensity == 0.0 {
            return Some(Easing::Linear);
        }
        // Flash's classic ease blends linear progress with quadratic ease:
        // positive intensity decelerates; negative intensity accelerates.
        // Fixed x handles make this cubic exactly (1+s)*t - s*t*t.
        let strength = intensity / 100.0;
        return Some(Easing::CubicBezier {
            x1: 1.0 / 3.0,
            y1: (1.0 + strength) / 3.0,
            x2: 2.0 / 3.0,
            y2: (2.0 + strength) / 3.0,
        });
    }
    let (family, mode) = if let Some(family) = method.strip_suffix("InOut") {
        (family, EasingMode::InOut)
    } else if let Some(family) = method.strip_suffix("Out") {
        (family, EasingMode::Out)
    } else {
        (method.strip_suffix("In")?, EasingMode::In)
    };
    let family = match family {
        "quad" => EasingFamily::Quad,
        "cubic" => EasingFamily::Cubic,
        "quart" => EasingFamily::Quart,
        "quint" => EasingFamily::Quint,
        "sine" => EasingFamily::Sine,
        "expo" => EasingFamily::Expo,
        "circ" => EasingFamily::Circ,
        "back" => EasingFamily::Back,
        "elastic" => EasingFamily::Elastic,
        "bounce" => EasingFamily::Bounce,
        _ => return None,
    };
    Some(Easing::Preset { family, mode })
}

fn camera_track_from_timeline(timeline: &RawTimeline, warnings: &mut Vec<String>) -> CameraTrack {
    let camera_layers: Vec<_> = timeline
        .layers
        .iter()
        .filter(|layer| layer.is_camera)
        .collect();
    if camera_layers.is_empty() {
        return CameraTrack::default();
    }
    if camera_layers.len() > 1 {
        warnings.push(format!(
            "scene '{}' has {} camera layers; only the first one was imported",
            timeline.name,
            camera_layers.len()
        ));
    }

    let layer = camera_layers[0];
    let mut keyed_frames = Vec::new();
    for frame in &layer.frames {
        let camera_instance = frame.elements.iter().find_map(|element| match element {
            RawElement::Symbol(instance)
                if normalize_library_reference(&instance.library_item_name) == "__camera__" =>
            {
                Some(instance)
            }
            _ => None,
        });
        let Some(instance) = camera_instance else {
            warnings.push(format!(
                "scene '{}' camera layer '{}' frame {} has no __Camera__ transform and was skipped",
                timeline.name, layer.name, frame.index
            ));
            continue;
        };

        let affine = instance.affine;
        let scale_x = affine.a11.hypot(affine.a21);
        let scale_y = affine.a12.hypot(affine.a22);
        let determinant = affine.a11 * affine.a22 - affine.a12 * affine.a21;
        if ![
            affine.a11,
            affine.a12,
            affine.a21,
            affine.a22,
            affine.tx,
            affine.ty,
            scale_x,
            scale_y,
            determinant,
        ]
        .into_iter()
        .all(f32::is_finite)
            || scale_x <= 1.0e-6
            || scale_y <= 1.0e-6
            || determinant <= 1.0e-9
        {
            warnings.push(format!(
                "scene '{}' camera layer '{}' frame {} has an invalid transform and was skipped",
                timeline.name, layer.name, frame.index
            ));
            continue;
        }

        // Animate stores the transformed camera viewport size in the matrix:
        // a smaller instance means a tighter view. q1s stores magnification,
        // so convert the viewport scale to its reciprocal.
        let viewport_scale = (scale_x + scale_y) * 0.5;
        let zoom = viewport_scale.recip();
        if (scale_x - scale_y).abs() > viewport_scale * 0.01 {
            warnings.push(format!(
                "scene '{}' camera layer '{}' frame {} has non-uniform scale; camera zoom was approximated",
                timeline.name, layer.name, frame.index
            ));
        }
        keyed_frames.push((
            frame,
            CameraState {
                center_x: affine.tx,
                center_y: affine.ty,
                zoom,
                rotation: affine.a21.atan2(affine.a11),
            },
        ));
    }

    let mut track = CameraTrack {
        enabled: !keyed_frames.is_empty(),
        keys: Vec::with_capacity(keyed_frames.len()),
    };
    for (position, (frame, state)) in keyed_frames.iter().enumerate() {
        let next_frame = keyed_frames.get(position + 1).map(|(next, _)| next.index);
        let tween = import_frame_tween(frame, next_frame, &timeline.name, &layer.name, warnings);
        track.keys.push(CameraKey {
            frame: frame.index,
            state: *state,
            tween,
        });
    }
    track
}

#[allow(clippy::too_many_arguments)]
fn build_q0rg(
    timeline: &RawTimeline,
    q0rg_id: u16,
    q0rg_name: &str,
    symbol_ids: &HashMap<String, u16>,
    symbol_frame_counts: &HashMap<String, u16>,
    bitmap_asset_ids: &HashMap<String, u16>,
    project: &mut ProjectV2,
    next_asset_id: &mut u16,
) -> Result<BuiltQ0rg, ImportError> {
    let mut layers = Vec::with_capacity(timeline.layers.len());
    let mut metadata = HashMap::new();
    let mut warnings = Vec::new();
    let mut pending_text = Vec::new();

    for (original_index, raw_layer) in timeline.layers.iter().enumerate().rev() {
        if raw_layer.is_camera {
            continue;
        }
        let layer_id = u16::try_from(original_index + 1)
            .map_err(|_| ImportError::Invalid("layer id overflow".to_string()))?;
        let mut placements = Vec::new();
        let mut explicit_keyframes = Vec::new();
        if raw_layer.kind != LayerKind::Folder {
            for (frame_pos, frame) in raw_layer.frames.iter().enumerate() {
                explicit_keyframes.push(frame.index);
                let next_frame = raw_layer.frames.get(frame_pos + 1).map(|next| next.index);
                let frame_tween = import_frame_tween(
                    frame,
                    next_frame,
                    q0rg_name,
                    &raw_layer.name,
                    &mut warnings,
                );
                // A motion tween moves instances; a shape tween morphs the
                // drawn shapes and leaves instances on the frame held.
                let (motion_tween, shape_tween) = match (frame.tween_type, frame_tween.to_frame()) {
                    (RawTweenType::Shape, Some(to_frame)) => (
                        Tween::None,
                        Tween::Shape {
                            to_frame,
                            easing: frame_tween.easing(),
                        },
                    ),
                    (RawTweenType::Motion, _) => (frame_tween, Tween::None),
                    _ => (Tween::None, Tween::None),
                };
                for element in &frame.elements {
                    match element {
                        RawElement::Symbol(instance) => {
                            let reference =
                                normalize_library_reference(&instance.library_item_name);
                            if let Some(&target_id) = symbol_ids.get(&reference) {
                                let transform =
                                    affine_to_transform(instance.affine).ok_or_else(|| {
                                        ImportError::Invalid(format!(
                                            "symbol instance '{}' has a non-decomposable matrix",
                                            instance.library_item_name
                                        ))
                                    })?;
                                let mut playback = instance.playback;
                                let frame_count = symbol_frame_counts
                                    .get(&reference)
                                    .copied()
                                    .unwrap_or(1)
                                    .max(1);
                                let max_frame = frame_count - 1;
                                let original_playback = playback;
                                playback.first_frame = playback.first_frame.min(max_frame);
                                playback.single_frame = playback.single_frame.min(max_frame);
                                playback.last_frame = playback
                                    .last_frame
                                    .map(|last| last.min(max_frame).max(playback.first_frame));
                                if playback != original_playback {
                                    warnings.push(format!(
                                        "symbol '{}' playback range was adjusted to its {} available frames",
                                        instance.library_item_name, frame_count
                                    ));
                                }
                                if !matches!(
                                    playback.mode,
                                    Q0rgPlaybackMode::SingleFrame | Q0rgPlaybackMode::Manual
                                ) {
                                    playback.set_range(
                                        playback.first_frame,
                                        playback.last_frame.unwrap_or(max_frame),
                                        frame_count,
                                    );
                                }
                                if let Some(mode) = instance.unsupported_blend_mode.as_deref() {
                                    warnings.push(format!(
                                        "symbol '{}' uses unsupported Animate blendMode='{mode}'; imported as normal",
                                        instance.library_item_name
                                    ));
                                }
                                placements.push(Placement {
                                    instance_id: 0,
                                    frame: frame.index,
                                    target: Target::Q0rg(target_id),
                                    transform,
                                    tween: motion_tween,
                                    fx: PlacementFx {
                                        opacity: instance.opacity,
                                        tint: instance.tint,
                                        blend_mode: instance.blend_mode,
                                        blur: instance.blur,
                                        glow: instance.glow,
                                        q0rg_playback: playback,
                                        ..PlacementFx::default()
                                    },
                                });
                            } else {
                                warnings.push(format!(
                                    "missing library symbol '{}'; instance was skipped",
                                    instance.library_item_name
                                ));
                            }
                        }
                        RawElement::Bitmap(instance) => {
                            if let Some(&target_id) = bitmap_asset_ids
                                .get(&normalize_library_reference(&instance.library_item_name))
                            {
                                let transform =
                                    affine_to_transform(instance.affine).ok_or_else(|| {
                                        ImportError::Invalid(format!(
                                            "bitmap instance '{}' has a non-decomposable matrix",
                                            instance.library_item_name
                                        ))
                                    })?;
                                placements.push(Placement {
                                    instance_id: 0,
                                    frame: frame.index,
                                    target: Target::Asset(target_id),
                                    transform,
                                    tween: motion_tween,
                                    fx: PlacementFx::default(),
                                });
                            } else {
                                warnings.push(format!(
                                    "missing bitmap '{}'; instance was skipped",
                                    instance.library_item_name
                                ));
                            }
                        }
                        RawElement::Shape(shape) => {
                            for target in shape_assets(
                                shape,
                                q0rg_name,
                                project,
                                next_asset_id,
                                &mut warnings,
                            )? {
                                placements.push(Placement {
                                    instance_id: 0,
                                    frame: frame.index,
                                    target: Target::Asset(target),
                                    transform: Transform2D::IDENTITY,
                                    tween: shape_tween,
                                    fx: PlacementFx::default(),
                                });
                            }
                        }
                        RawElement::Text(text) => {
                            if let Some(target) =
                                text_asset(text, q0rg_name, project, next_asset_id, &mut warnings, &mut pending_text)?
                            {
                                placements.push(Placement {
                                    instance_id: 0,
                                    frame: frame.index,
                                    target: Target::Asset(target),
                                    transform: Transform2D::IDENTITY,
                                    tween: Tween::None,
                                    fx: PlacementFx::default(),
                                });
                            }
                        }
                    }
                }
            }
            explicit_keyframes.sort_unstable();
            explicit_keyframes.dedup();
            if explicit_keyframes.is_empty() {
                explicit_keyframes.push(0);
            }
        }
        layers.push(Layer {
            layer_id,
            timeline_frame_count: Some(if raw_layer.kind == LayerKind::Folder {
                0
            } else {
                raw_layer
                    .frames
                    .iter()
                    .map(|frame| frame.index.saturating_add(frame.duration))
                    .max()
                    .unwrap_or(1)
                    .min(timeline.frame_count)
            }),
            name: raw_layer.name.clone(),
            explicit_keyframes,
            placements,
        });

        let parent_folder_id = raw_layer.parent_index.and_then(|parent_index| {
            timeline
                .layers
                .get(parent_index)
                .filter(|parent| parent.kind == LayerKind::Folder)
                .and_then(|_| u16::try_from(parent_index + 1).ok())
        });
        let layer_metadata = LayerMetadata {
            kind: raw_layer.kind,
            parent_folder_id,
            mask_layer_id: raw_layer
                .parent_index
                .filter(|index| {
                    timeline
                        .layers
                        .get(*index)
                        .is_some_and(|parent| parent.kind == LayerKind::Mask)
                })
                .or(raw_layer.mask_source_index)
                .and_then(|mask_index| u16::try_from(mask_index + 1).ok()),
            collapsed: false,
            hidden: raw_layer.hidden,
            locked: raw_layer.locked,
        };
        if layer_metadata != LayerMetadata::default() {
            metadata.insert(LayerKey::new(q0rg_id, layer_id), layer_metadata);
        }
    }

    if layers.is_empty() {
        layers.push(Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "Layer 1".to_string(),
            explicit_keyframes: vec![0],
            placements: Vec::new(),
        });
    }

    let mut q0rg = Q0rg {
        q0rg_id,
        name: q0rg_name.to_string(),
        frame_count: timeline.frame_count.max(1),
        script: String::new(),
        layers,
    };
    crate::normalize_folder_layer_order(&mut q0rg, &metadata);

    Ok(BuiltQ0rg {
        q0rg,
        metadata,
        warnings,
        pending_text,
    })
}

pub(crate) fn shape_assets(
    shape: &RawShape,
    owner_name: &str,
    project: &mut ProjectV2,
    next_asset_id: &mut u16,
    warnings: &mut Vec<String>,
) -> Result<Vec<u16>, ImportError> {
    let mut fill_segments = HashMap::<u16, Vec<Segment>>::new();
    let mut stroke_segments = HashMap::<u16, Vec<Segment>>::new();
    for edge in &shape.edges {
        if edge.fill0 != edge.fill1 {
            if let Some(fill0) = edge.fill0.filter(|style| *style != 0) {
                fill_segments
                    .entry(fill0)
                    .or_default()
                    .extend(edge.segments.iter().rev().copied().map(Segment::reversed));
            }
            if let Some(fill1) = edge.fill1.filter(|style| *style != 0) {
                fill_segments
                    .entry(fill1)
                    .or_default()
                    .extend(edge.segments.iter().copied());
            }
        }
        if let Some(stroke) = edge.stroke.filter(|style| *style != 0) {
            stroke_segments
                .entry(stroke)
                .or_default()
                .extend(edge.segments.iter().copied());
        }
    }

    let mut result = Vec::new();
    for (style, segments) in fill_segments {
        let Some(fill_style) = shape.fills.get(&style).cloned() else {
            warnings.push(format!(
                "{owner_name}: unsupported or missing fill style {style}; fill was skipped"
            ));
            continue;
        };
        let paths = stitch_segments(
            segments
                .into_iter()
                .map(|segment| segment.transformed(shape.affine))
                .collect(),
            true,
        );
        if paths.is_empty() {
            continue;
        }
        let (fallback, gradient) = match fill_style {
            RawFillStyle::Solid(color) => (color, None),
            RawFillStyle::Bitmap { .. } => {
                warnings.push(format!(
                    "{owner_name}: bitmap fill requires its library media; fill was skipped"
                ));
                continue;
            }
            RawFillStyle::Gradient(mut gradient) => {
                gradient.transform = Affine::compose(shape.affine, gradient.transform);
                if regularize_gradient_transform(&mut gradient.transform) {
                    warnings.push(format!(
                        "{owner_name}: degenerate fill gradient transform was regularized; gradient colors and stops were retained"
                    ));
                }
                let fallback = gradient
                    .stops
                    .first()
                    .map(|stop| stop.color)
                    .unwrap_or(Rgba {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    });
                (fallback, Some(gradient))
            }
        };
        let id = allocate_asset_id(next_asset_id)?;
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: id,
            paths,
            fill: Some(fallback),
            stroke: None,
        }));
        if let Some(fill_gradient) = gradient {
            project.asset_appearances.insert(
                id,
                VectorAppearance {
                    material: VectorMaterial::Solid,
                    erase_mask: Vec::new(),
                    material_source: Vec::new(),
                    clip_mask: Vec::new(),
                    field_transform: Affine::IDENTITY,
                    fill_gradient: Some(fill_gradient),
                },
            );
        }
        result.push(id);
    }

    for (style, segments) in stroke_segments {
        let Some(mut stroke) = shape.strokes.get(&style).copied() else {
            warnings.push(format!(
                "{owner_name}: unsupported or missing stroke style {style}; stroke was skipped"
            ));
            continue;
        };
        stroke.width *= shape.affine.uniform_scale().abs().max(1.0e-4);
        let paths = stitch_segments(
            segments
                .into_iter()
                .map(|segment| segment.transformed(shape.affine))
                .collect(),
            false,
        );
        if paths.is_empty() {
            continue;
        }
        let id = allocate_asset_id(next_asset_id)?;
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: id,
            paths,
            fill: None,
            stroke: Some(stroke),
        }));
        result.push(id);
    }
    Ok(result)
}

/// Add the smallest practical perpendicular basis needed to make a collapsed
/// XFL gradient transform invertible. Its color stops and dominant direction
/// remain intact, while the renderer can safely map points back into gradient
/// space.
fn regularize_gradient_transform(transform: &mut Affine) -> bool {
    const MINIMUM_BASIS: f32 = 1.0e-4;
    const MINIMUM_DETERMINANT: f32 = 1.0e-9;

    let values = [
        transform.a11,
        transform.a12,
        transform.a21,
        transform.a22,
        transform.tx,
        transform.ty,
    ];
    if !values.into_iter().all(f32::is_finite) {
        return false;
    }

    let determinant = transform.a11 * transform.a22 - transform.a12 * transform.a21;
    if determinant.is_finite() && determinant.abs() >= MINIMUM_DETERMINANT {
        return false;
    }

    let x_axis_len = transform.a11.hypot(transform.a21);
    let y_axis_len = transform.a12.hypot(transform.a22);
    let dominant_len = x_axis_len.max(y_axis_len);
    if !dominant_len.is_finite() {
        transform.a11 = MINIMUM_BASIS;
        transform.a12 = 0.0;
        transform.a21 = 0.0;
        transform.a22 = MINIMUM_BASIS;
        return true;
    }
    if dominant_len <= f32::MIN_POSITIVE {
        transform.a11 = MINIMUM_BASIS;
        transform.a12 = 0.0;
        transform.a21 = 0.0;
        transform.a22 = MINIMUM_BASIS;
        return true;
    }

    // Keep the adjustment relative to the surviving basis, while staying well
    // above q1s' absolute determinant threshold after f32 rounding.
    let target_det = (dominant_len * 1.0e-6 * dominant_len).max(1.0e-8);
    let orientation = if determinant.is_sign_negative() {
        -1.0
    } else {
        1.0
    };
    let delta = (orientation * target_det - determinant) / dominant_len;

    if target_det.is_finite() && delta.is_finite() {
        if x_axis_len >= y_axis_len {
            // The columns are the transformed x and y bases. Nudge the weaker
            // column perpendicular to the stronger one so its existing component
            // along the stronger basis is preserved.
            let perp_x = -transform.a21 / dominant_len;
            let perp_y = transform.a11 / dominant_len;
            transform.a12 += perp_x * delta;
            transform.a22 += perp_y * delta;
        } else {
            let perp_x = transform.a22 / dominant_len;
            let perp_y = -transform.a12 / dominant_len;
            transform.a11 += perp_x * delta;
            transform.a21 += perp_y * delta;
        }

        let regularized_det = transform.a11 * transform.a22 - transform.a12 * transform.a21;
        if regularized_det.is_finite() && regularized_det.abs() >= MINIMUM_DETERMINANT {
            return true;
        }
    }

    // The small perpendicular adjustment can round away when the surviving
    // basis is poorly conditioned. Rebuild the weaker axis explicitly while
    // retaining the dominant direction and the gradient translation.
    if dominant_len >= MINIMUM_BASIS {
        let perpendicular_len = (2.0e-9 / dominant_len).max(MINIMUM_BASIS);
        if perpendicular_len.is_finite() {
            if x_axis_len >= y_axis_len {
                let perp_x = -transform.a21 / dominant_len;
                let perp_y = transform.a11 / dominant_len;
                transform.a12 = orientation * perp_x * perpendicular_len;
                transform.a22 = orientation * perp_y * perpendicular_len;
            } else {
                let perp_x = transform.a22 / dominant_len;
                let perp_y = -transform.a12 / dominant_len;
                transform.a11 = orientation * perp_x * perpendicular_len;
                transform.a21 = orientation * perp_y * perpendicular_len;
            }

            let fallback_det = transform.a11 * transform.a22 - transform.a12 * transform.a21;
            if fallback_det.is_finite() && fallback_det.abs() >= MINIMUM_DETERMINANT {
                return true;
            }
        }
    }

    // Last resort for zero, non-finite, or extremely tiny bases. This keeps
    // the imported gradient renderable and satisfies q1s validation.
    transform.a11 = MINIMUM_BASIS;
    transform.a12 = 0.0;
    transform.a21 = 0.0;
    transform.a22 = MINIMUM_BASIS;
    true
}

pub(crate) fn text_asset(
    text: &RawText,
    owner_name: &str,
    project: &mut ProjectV2,
    next_asset_id: &mut u16,
    warnings: &mut Vec<String>,
    pending_text: &mut Vec<crate::PendingText>,
) -> Result<Option<u16>, ImportError> {
    if text.text.is_empty() {
        return Ok(None);
    }
    let mut replacement = None;
    let paths =
        match text_paths(text) {
            Ok(paths) => paths,
            Err(error) => {
                let fallback = crate::fonts::default_name().ok_or_else(|| ImportError::Invalid(
                    format!("{owner_name}: {error}; no outline font is available for replacement")))?;
                let mut source = text.clone();
                source.font = fallback.to_string();
                let paths = text_paths(&source).map_err(|fallback_error| ImportError::Invalid(
                    format!("{owner_name}: {error}; fallback failed: {fallback_error}")))?;
                warnings.push(format!("{owner_name}: {error}; text preserved using '{fallback}' pending font replacement"));
                replacement = Some(fallback.to_string());
                paths
            }
        };
    if paths.is_empty() {
        return Ok(None);
    }
    let id = allocate_asset_id(next_asset_id)?;
    project.assets.push(Asset::Vector(VectorAsset {
        asset_id: id,
        paths,
        fill: Some(text.color),
        stroke: None,
    }));
    if let Some(replacement) = replacement {
        pending_text.push(crate::PendingText { asset_id: id, source: text.clone(), replacement });
    }
    Ok(Some(id))
}

pub(crate) fn text_paths(text: &RawText) -> Result<Vec<VectorPath>, String> {
    let mut paths = crate::text_outline::outline_installed_font(&text.font, &text.text, text.size)?;
    let affine = Affine::compose(
        text.affine,
        Affine {
            tx: text.offset.x,
            ty: text.offset.y,
            ..Affine::IDENTITY
        },
    );
    for path in &mut paths {
        for anchor in &mut path.anchors {
            anchor.point = affine.apply(anchor.point);
            anchor.in_handle = anchor.in_handle.map(|point| affine.apply(point));
            anchor.out_handle = anchor.out_handle.map(|point| affine.apply(point));
        }
    }
    if paths.is_empty() && !text.text.trim().is_empty() {
        return Err(format!("Font '{}' produced no outlines for imported text", text.font));
    }
    Ok(paths)
}

fn allocate_asset_id(next_asset_id: &mut u16) -> Result<u16, ImportError> {
    if *next_asset_id == 0 {
        return Err(ImportError::Invalid(
            "fla contains too many vector assets".to_string(),
        ));
    }
    let id = *next_asset_id;
    *next_asset_id = next_asset_id.checked_add(1).unwrap_or(0);
    Ok(id)
}

fn parse_timeline(xml: &[u8], symbol_document: bool) -> Result<Option<RawTimeline>, ImportError> {
    parse_timeline_index(xml, symbol_document, 0)
}

fn parse_document_timelines(xml: &[u8]) -> Result<Vec<RawTimeline>, ImportError> {
    let mut timelines = Vec::new();
    while let Some(timeline) = parse_timeline_index(xml, false, timelines.len())? {
        timelines.push(timeline);
        if timelines.len() >= usize::from(u16::MAX) {
            return Err(ImportError::Invalid("too many fla scenes".to_string()));
        }
    }
    Ok(timelines)
}

fn parse_timeline_index(
    xml: &[u8],
    symbol_document: bool,
    wanted_timeline: usize,
) -> Result<Option<RawTimeline>, ImportError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut timeline_depth = None;
    let mut saw_timeline = false;
    let mut timeline_index = 0usize;
    let mut root_name = String::new();
    let mut timeline_name = String::new();
    let mut layers = Vec::new();
    let mut current_layer: Option<(usize, RawLayer)> = None;
    let mut current_frame: Option<(usize, RawFrame)> = None;
    let mut custom_ease_depth: Option<usize> = None;
    let mut instance: Option<InstanceState> = None;
    let mut bitmap: Option<BitmapState> = None;
    let mut shape: Option<ShapeState> = None;
    let mut text: Option<TextState> = None;
    let mut groups = Vec::<GroupState>::new();
    let mut max_frame_end = 1u32;
    let mut warnings = Vec::new();
    let mut unsupported = HashSet::<String>::new();

    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|error| ImportError::Xml(error.to_string()))?
        {
            Event::Start(element) => {
                depth += 1;
                let name_buf = element.name();
                let name = name_buf.as_ref();
                note_unsupported_frame_element(name, current_frame.is_some(), &mut unsupported);
                if symbol_document && name == b"DOMSymbolItem" && root_name.is_empty() {
                    root_name = attr(&reader, &element, b"name")?.unwrap_or_default();
                }
                if name == b"DOMTimeline" && !saw_timeline {
                    if timeline_index == wanted_timeline {
                        saw_timeline = true;
                        timeline_depth = Some(depth);
                        timeline_name = attr(&reader, &element, b"name")?.unwrap_or_default();
                    }
                    timeline_index = timeline_index.saturating_add(1);
                } else if timeline_depth.is_some() && name == b"DOMLayer" && current_layer.is_none()
                {
                    current_layer = Some((depth, raw_layer(&reader, &element, layers.len())?));
                } else if current_layer.is_some() && name == b"DOMFrame" && current_frame.is_none()
                {
                    let frame = raw_frame(&reader, &element)?;
                    if current_layer
                        .as_ref()
                        .is_some_and(|(_, layer)| layer.kind != LayerKind::Folder)
                    {
                        max_frame_end =
                            max_frame_end.max(u32::from(frame.index) + u32::from(frame.duration));
                    }
                    current_frame = Some((depth, frame));
                } else if current_frame.is_some() && name == b"CustomEase" {
                    custom_ease_depth = Some(depth);
                } else if current_frame.is_some() && name == b"Ease" {
                    if let Some((_, frame)) = current_frame.as_mut() {
                        read_frame_ease(&reader, &element, frame, &mut unsupported)?;
                    }
                } else if current_frame.is_some() && name == b"DOMGroup" {
                    groups.push(GroupState {
                        depth,
                        affine: Affine::IDENTITY,
                    });
                } else if current_frame.is_some()
                    && name == b"DOMSymbolInstance"
                    && instance.is_none()
                {
                    instance = Some(InstanceState {
                        depth,
                        instance: raw_instance(&reader, &element)?,
                    });
                } else if current_frame.is_some()
                    && name == b"DOMBitmapInstance"
                    && bitmap.is_none()
                {
                    bitmap = Some(BitmapState {
                        depth,
                        instance: raw_bitmap_instance(&reader, &element)?,
                    });
                } else if current_frame.is_some() && name == b"DOMShape" && shape.is_none() {
                    shape = Some(ShapeState {
                        depth,
                        affine: Affine::IDENTITY,
                        fills: HashMap::new(),
                        strokes: HashMap::new(),
                        edges: Vec::new(),
                        fill_style: None,
                        stroke_style: None,
                        gradient: None,
                    });
                } else if current_frame.is_some() && name == b"DOMStaticText" && text.is_none() {
                    let offset = Vec2::new(
                        parse_f32_attr(&reader, &element, b"left", 0.0)?,
                        parse_f32_attr(&reader, &element, b"top", 0.0)?,
                    );
                    if !offset.x.is_finite() || !offset.y.is_finite() {
                        return Err(ImportError::Invalid(
                            "static text has a non-finite local offset".to_string(),
                        ));
                    }
                    text = Some(TextState {
                        depth,
                        affine: Affine::IDENTITY,
                        offset,
                        text: String::new(),
                        font: "Arial".to_string(),
                        size: 12.0,
                        color: Rgba {
                            r: 0,
                            g: 0,
                            b: 0,
                            a: 255,
                        },
                        characters_depth: None,
                        run_count: 0,
                    });
                } else if name == b"DOMTextRun" {
                    if let Some(state) = text.as_mut() {
                        state.run_count += 1;
                        if state.run_count > 1 {
                            unsupported.insert(
                                "multi-run static text is imported using the first run style"
                                    .to_string(),
                            );
                        }
                    }
                } else if name == b"characters" {
                    if let Some(state) = text.as_mut() {
                        state.characters_depth = Some(depth);
                    }
                } else if name == b"DOMTextAttrs" {
                    if let Some(state) = text.as_mut() {
                        if state.run_count <= 1 {
                            apply_text_attrs(&reader, &element, state, &mut unsupported)?;
                        }
                    }
                }
                if let Some(state) = instance.as_mut() {
                    apply_instance_filter(&reader, &element, name, state, &mut unsupported)?;
                }
                handle_shape_start(&reader, &element, depth, &mut shape, &mut unsupported)?;
            }
            Event::Empty(element) => {
                let name_buf = element.name();
                let name = name_buf.as_ref();
                note_unsupported_frame_element(name, current_frame.is_some(), &mut unsupported);
                if let Some(state) = instance.as_mut() {
                    apply_instance_filter(&reader, &element, name, state, &mut unsupported)?;
                }
                if current_layer.is_some() && name == b"DOMFrame" && current_frame.is_none() {
                    let frame = raw_frame(&reader, &element)?;
                    if current_layer
                        .as_ref()
                        .is_some_and(|(_, layer)| layer.kind != LayerKind::Folder)
                    {
                        max_frame_end =
                            max_frame_end.max(u32::from(frame.index) + u32::from(frame.duration));
                    }
                    if let Some((_, layer)) = current_layer.as_mut() {
                        layer.frames.push(frame);
                    }
                } else if current_frame.is_some()
                    && name == b"DOMSymbolInstance"
                    && instance.is_none()
                {
                    let mut raw = raw_instance(&reader, &element)?;
                    raw.affine = Affine::compose(group_affine(&groups), raw.affine);
                    if let Some((_, frame)) = current_frame.as_mut() {
                        frame.elements.push(RawElement::Symbol(raw));
                    }
                } else if current_frame.is_some()
                    && name == b"DOMBitmapInstance"
                    && bitmap.is_none()
                {
                    let mut raw = raw_bitmap_instance(&reader, &element)?;
                    raw.affine = Affine::compose(group_affine(&groups), raw.affine);
                    if let Some((_, frame)) = current_frame.as_mut() {
                        frame.elements.push(RawElement::Bitmap(raw));
                    }
                } else if current_frame.is_some() && name == b"Ease" {
                    if let Some((_, frame)) = current_frame.as_mut() {
                        read_frame_ease(&reader, &element, frame, &mut unsupported)?;
                    }
                } else if name == b"Point" && custom_ease_depth.is_some() {
                    if let Some((_, frame)) = current_frame.as_mut() {
                        frame.custom_ease_points.push([
                            parse_f32_attr(&reader, &element, b"x", 0.0)?,
                            parse_f32_attr(&reader, &element, b"y", 0.0)?,
                        ]);
                    }
                } else if name == b"Color" && instance.is_some() {
                    if let Some(state) = instance.as_mut() {
                        if let Some(alpha) = attr(&reader, &element, b"alphaMultiplier")? {
                            let alpha = alpha.parse::<f32>().map_err(|_| {
                                ImportError::Invalid(format!("invalid alphaMultiplier '{alpha}'"))
                            })?;
                            state.instance.opacity = alpha.clamp(0.0, 1.0);
                        }
                        if let Some(amount) = attr(&reader, &element, b"tintMultiplier")? {
                            let amount = amount.parse::<f32>().map_err(|_| {
                                ImportError::Invalid(format!("invalid tintMultiplier '{amount}'"))
                            })?;
                            if !amount.is_finite() {
                                return Err(ImportError::Invalid(
                                    "non-finite tintMultiplier".to_string(),
                                ));
                            }
                            let color = attr(&reader, &element, b"tintColor")?
                                .map(|value| parse_hex_color(&value))
                                .transpose()?
                                .unwrap_or(Rgba {
                                    r: 0,
                                    g: 0,
                                    b: 0,
                                    a: 255,
                                });
                            state.instance.tint = Some(q0s_format::v2::TintFx {
                                color,
                                amount: amount.clamp(0.0, 1.0),
                            });
                        }
                        if attr(&reader, &element, b"brightness")?.is_some() {
                            unsupported.insert(
                                "symbol brightness color transforms are not imported yet"
                                    .to_string(),
                            );
                        }
                    }
                } else if name == b"DOMTextAttrs" {
                    if let Some(state) = text.as_mut() {
                        if state.run_count <= 1 {
                            apply_text_attrs(&reader, &element, state, &mut unsupported)?;
                        }
                    }
                } else if name == b"Matrix" {
                    let matrix = parse_matrix(&reader, &element)?;
                    if let Some(gradient) = shape.as_mut().and_then(|state| state.gradient.as_mut())
                    {
                        gradient.matrix = matrix;
                    } else if let Some(state) = instance.as_mut() {
                        state.instance.affine = matrix;
                    } else if let Some(state) = bitmap.as_mut() {
                        state.instance.affine = matrix;
                    } else if let Some(state) = shape.as_mut() {
                        state.affine = matrix;
                    } else if let Some(state) = text.as_mut() {
                        state.affine = matrix;
                    } else if let Some(group) = groups.last_mut() {
                        group.affine = matrix;
                    }
                }
                handle_shape_empty(&reader, &element, &mut shape, &mut unsupported)?;
            }
            Event::Text(value) => {
                if let Some(state) = text.as_mut() {
                    if state.characters_depth.is_some() {
                        let decoded = value
                            .decode()
                            .map_err(|error| ImportError::Xml(error.to_string()))?;
                        let unescaped = quick_xml::escape::unescape(&decoded)
                            .map_err(|error| ImportError::Xml(error.to_string()))?;
                        state.text.push_str(&unescaped);
                    }
                }
            }
            Event::GeneralRef(value) => {
                if let Some(state) = text.as_mut() {
                    if state.characters_depth.is_some() {
                        if let Some(character) = value
                            .resolve_char_ref()
                            .map_err(|error| ImportError::Xml(error.to_string()))?
                        {
                            state.text.push(character);
                        } else {
                            let name = value
                                .decode()
                                .map_err(|error| ImportError::Xml(error.to_string()))?;
                            let character = match name.as_ref() {
                                "amp" => Some('&'),
                                "lt" => Some('<'),
                                "gt" => Some('>'),
                                "apos" => Some('\''),
                                "quot" => Some('\"'),
                                _ => None,
                            };
                            if let Some(character) = character {
                                state.text.push(character);
                            } else {
                                return Err(ImportError::Xml(format!(
                                    "unsupported entity reference '&{name};' in text"
                                )));
                            }
                        }
                    }
                }
            }
            Event::End(element) => {
                let name_buf = element.name();
                let name = name_buf.as_ref();
                if name == b"CustomEase" && custom_ease_depth == Some(depth) {
                    custom_ease_depth = None;
                } else if name == b"characters"
                    && text
                        .as_ref()
                        .is_some_and(|state| state.characters_depth == Some(depth))
                {
                    if let Some(state) = text.as_mut() {
                        state.characters_depth = None;
                    }
                } else if name == b"DOMStaticText"
                    && text.as_ref().is_some_and(|state| state.depth == depth)
                {
                    if let Some(mut state) = text.take() {
                        state.affine = Affine::compose(group_affine(&groups), state.affine);
                        if let Some((_, frame)) = current_frame.as_mut() {
                            frame.elements.push(RawElement::Text(RawText {
                                affine: state.affine,
                                offset: state.offset,
                                text: state.text,
                                font: state.font,
                                size: state.size,
                                color: state.color,
                            }));
                        }
                    }
                } else if name == b"DOMSymbolInstance"
                    && instance.as_ref().is_some_and(|state| state.depth == depth)
                {
                    if let Some(mut state) = instance.take() {
                        state.instance.affine =
                            Affine::compose(group_affine(&groups), state.instance.affine);
                        if let Some((_, frame)) = current_frame.as_mut() {
                            frame.elements.push(RawElement::Symbol(state.instance));
                        }
                    }
                } else if name == b"DOMBitmapInstance"
                    && bitmap.as_ref().is_some_and(|state| state.depth == depth)
                {
                    if let Some(mut state) = bitmap.take() {
                        state.instance.affine =
                            Affine::compose(group_affine(&groups), state.instance.affine);
                        if let Some((_, frame)) = current_frame.as_mut() {
                            frame.elements.push(RawElement::Bitmap(state.instance));
                        }
                    }
                } else if name == b"DOMShape"
                    && shape.as_ref().is_some_and(|state| state.depth == depth)
                {
                    if let Some(mut state) = shape.take() {
                        state.affine = Affine::compose(group_affine(&groups), state.affine);
                        if let Some((_, frame)) = current_frame.as_mut() {
                            frame.elements.push(RawElement::Shape(RawShape {
                                affine: state.affine,
                                fills: state.fills,
                                strokes: state.strokes,
                                edges: state.edges,
                            }));
                        }
                    }
                } else if name == b"DOMFrame"
                    && current_frame.as_ref().is_some_and(|(d, _)| *d == depth)
                {
                    if let Some((_, frame)) = current_frame.take() {
                        if let Some((_, layer)) = current_layer.as_mut() {
                            layer.frames.push(frame);
                        }
                    }
                } else if name == b"DOMLayer"
                    && current_layer.as_ref().is_some_and(|(d, _)| *d == depth)
                {
                    if let Some((_, layer)) = current_layer.take() {
                        layers.push(layer);
                    }
                } else if name == b"DOMGroup"
                    && groups.last().is_some_and(|group| group.depth == depth)
                {
                    groups.pop();
                }
                handle_shape_end(name, depth, &mut shape);
                let completed_timeline = name == b"DOMTimeline" && timeline_depth == Some(depth);
                if completed_timeline {
                    timeline_depth = None;
                }
                depth = depth.saturating_sub(1);
                if completed_timeline {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    let mut masks_by_parent = HashMap::new();
    for index in 0..layers.len() {
        let parent = layers[index].parent_index;
        let explicit_mask =
            parent.filter(|p| layers.get(*p).is_some_and(|l| l.kind == LayerKind::Mask));
        layers[index].mask_source_index = explicit_mask.or_else(|| {
            layers[index]
                .masked
                .then(|| masks_by_parent.get(&parent).copied())
                .flatten()
        });
        if layers[index].kind == LayerKind::Mask {
            masks_by_parent.insert(parent, index);
        } else if !layers[index].masked && layers[index].kind != LayerKind::Folder {
            masks_by_parent.remove(&parent);
        }
    }

    if !saw_timeline {
        return Ok(None);
    }
    warnings.extend(unsupported);
    let frame_count = u16::try_from(max_frame_end.min(u32::from(u16::MAX)))
        .map_err(|_| ImportError::Invalid("timeline frame count overflow".to_string()))?;
    Ok(Some(RawTimeline {
        name: if symbol_document {
            root_name
        } else if timeline_name.is_empty() {
            if wanted_timeline == 0 {
                "Stage".to_string()
            } else {
                format!("Scene {}", wanted_timeline + 1)
            }
        } else {
            timeline_name
        },
        layers,
        frame_count: frame_count.max(1),
        warnings,
    }))
}

fn note_unsupported_frame_element(
    name: &[u8],
    inside_frame: bool,
    unsupported: &mut HashSet<String>,
) {
    if !inside_frame {
        return;
    }
    let warning = match name {
        b"DOMDynamicText" | b"DOMInputText" | b"DOMTLFText" => {
            Some("dynamic/input text elements are not imported yet")
        }
        b"DOMVideoInstance" => Some("video instances are not imported yet"),
        _ => None,
    };
    if let Some(warning) = warning {
        unsupported.insert(warning.to_string());
    }
}

fn group_affine(groups: &[GroupState]) -> Affine {
    groups.iter().fold(Affine::IDENTITY, |parent, group| {
        Affine::compose(parent, group.affine)
    })
}

fn raw_layer(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    ordinal: usize,
) -> Result<RawLayer, ImportError> {
    let name = attr(reader, element, b"name")?
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("Layer {}", ordinal + 1));
    let layer_type = attr(reader, element, b"layerType")?.unwrap_or_default();
    let kind = match layer_type.to_ascii_lowercase().as_str() {
        "folder" => LayerKind::Folder,
        "mask" => LayerKind::Mask,
        _ => LayerKind::Normal,
    };
    let masked = layer_type.eq_ignore_ascii_case("masked");
    let is_camera = layer_type.eq_ignore_ascii_case("camera");
    let parent_index = attr(reader, element, b"parentLayerIndex")?
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| ImportError::Invalid(format!("invalid parentLayerIndex '{value}'")))
        })
        .transpose()?;
    let hidden = matches!(
        attr(reader, element, b"visible")?.as_deref(),
        Some("false") | Some("0")
    );
    let locked = matches!(
        attr(reader, element, b"locked")?.as_deref(),
        Some("true") | Some("1")
    );
    Ok(RawLayer {
        name,
        kind,
        masked,
        mask_source_index: None,
        is_camera,
        parent_index,
        hidden,
        locked,
        frames: Vec::new(),
    })
}

fn raw_frame(reader: &Reader<&[u8]>, element: &BytesStart<'_>) -> Result<RawFrame, ImportError> {
    let index = parse_u16_attr(reader, element, b"index", 0)?;
    let duration = parse_u16_attr(reader, element, b"duration", 1)?.max(1);
    let tween_type = match attr(reader, element, b"tweenType")?.as_deref() {
        Some("motion") => RawTweenType::Motion,
        Some("shape") => RawTweenType::Shape,
        Some(_) | None => RawTweenType::None,
    };
    Ok(RawFrame {
        index,
        duration,
        tween_type,
        ease_method: attr(reader, element, b"easeMethodName")?,
        ease_intensity: parse_f32_attr(reader, element, b"tweenEasing", 0.0)?,
        custom_ease_points: Vec::new(),
        elements: Vec::new(),
    })
}

fn read_frame_ease(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    frame: &mut RawFrame,
    unsupported: &mut HashSet<String>,
) -> Result<(), ImportError> {
    let target = attr(reader, element, b"target")?.unwrap_or_else(|| "all".to_string());
    if target != "all" {
        unsupported.insert(format!(
            "property-specific tween ease for '{target}' is not imported yet; the shared frame ease is used"
        ));
        return Ok(());
    }
    // The <Ease> entry is authoritative; old XFL stores only its intensity,
    // while newer Animate stores the preset method here and on DOMFrame.
    frame.ease_method = attr(reader, element, b"method")?;
    frame.ease_intensity = parse_f32_attr(reader, element, b"intensity", frame.ease_intensity)?;
    Ok(())
}

fn raw_bitmap_instance(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<RawBitmapInstance, ImportError> {
    let library_item_name = attr(reader, element, b"libraryItemName")?.unwrap_or_default();
    if library_item_name.trim().is_empty() {
        return Err(ImportError::Invalid(
            "bitmap instance has no libraryItemName".to_string(),
        ));
    }
    Ok(RawBitmapInstance {
        library_item_name,
        affine: Affine::IDENTITY,
    })
}

fn parse_bitmap_media(xml: &[u8]) -> Result<Vec<RawBitmapMedia>, ImportError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut media = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|error| ImportError::Xml(error.to_string()))?
        {
            Event::Start(element) | Event::Empty(element)
                if element.name().as_ref() == b"DOMBitmapItem" =>
            {
                let name = attr(&reader, &element, b"name")?.unwrap_or_default();
                let data_href = attr(&reader, &element, b"bitmapDataHRef")?.unwrap_or_default();
                if !name.trim().is_empty() && !data_href.trim().is_empty() {
                    media.push(RawBitmapMedia { name, data_href });
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(media)
}

pub(crate) fn decode_lossless_bitmap_dat(bytes: &[u8]) -> Result<(u16, u16, Vec<u8>), ImportError> {
    if bytes.len() < 28 {
        return Err(ImportError::Unsupported(
            "bitmap .dat header is truncated".to_string(),
        ));
    }
    let indexed = match &bytes[0..2] {
        [0x03, 0x05] => false,
        [0x03, 0x03] => true,
        _ => {
            return Err(ImportError::Unsupported(format!(
                "unsupported Animate bitmap .dat format {:02X} {:02X}",
                bytes[0], bytes[1]
            )))
        }
    };
    let row_size = usize::from(u16::from_le_bytes([bytes[2], bytes[3]]));
    let width = u16::from_le_bytes([bytes[4], bytes[5]]);
    let height = u16::from_le_bytes([bytes[6], bytes[7]]);
    let bytes_per_pixel = if indexed { 1 } else { 4 };
    if width == 0 || height == 0 || row_size < usize::from(width) * bytes_per_pixel {
        return Err(ImportError::Invalid(format!(
            "invalid lossless bitmap dimensions {width}x{height} with row size {row_size}"
        )));
    }
    let has_alpha = bytes[24] & 1 != 0;
    let (palette, mut offset, compressed) = if indexed {
        let palette_count = if bytes[25] == 0 {
            256
        } else {
            usize::from(bytes[25])
        };
        let palette_end = 28usize
            .checked_add(palette_count * 4)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| ImportError::Invalid("truncated lossless bitmap palette".to_string()))?;
        let palette = bytes[28..palette_end]
            .chunks_exact(4)
            .map(|rgba| {
                [
                    rgba[0],
                    rgba[1],
                    rgba[2],
                    if has_alpha { rgba[3] } else { 255 },
                ]
            })
            .collect::<Vec<_>>();
        (Some(palette), palette_end, true)
    } else {
        (None, 26usize, bytes[25] != 0)
    };
    let mut payload = Vec::new();
    loop {
        if offset + 2 > bytes.len() {
            return Err(ImportError::Invalid(
                "truncated lossless bitmap chunk header".to_string(),
            ));
        }
        let chunk_len = usize::from(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]));
        offset += 2;
        if chunk_len == 0 {
            break;
        }
        let end = offset.checked_add(chunk_len).ok_or_else(|| {
            ImportError::Invalid("lossless bitmap chunk length overflow".to_string())
        })?;
        if end > bytes.len() {
            return Err(ImportError::Invalid(
                "truncated lossless bitmap chunk payload".to_string(),
            ));
        }
        payload.extend_from_slice(&bytes[offset..end]);
        offset = end;
    }

    let mut raw = Vec::new();
    if compressed {
        let mut decoder = ZlibDecoder::new(payload.as_slice());
        decoder.read_to_end(&mut raw).map_err(|error| {
            ImportError::Invalid(format!("invalid lossless bitmap zlib stream: {error}"))
        })?;
    } else {
        raw = payload;
    }
    let expected = row_size
        .checked_mul(usize::from(height))
        .ok_or_else(|| ImportError::Invalid("bitmap pixel byte length overflow".to_string()))?;
    if raw.len() < expected {
        return Err(ImportError::Invalid(format!(
            "lossless bitmap decoded {} bytes, expected at least {expected}",
            raw.len()
        )));
    }

    let pixel_count = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or_else(|| ImportError::Invalid("bitmap pixel count overflow".to_string()))?;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for y in 0..usize::from(height) {
        let row = &raw[y * row_size..y * row_size + row_size];
        for x in 0..usize::from(width) {
            if let Some(palette) = &palette {
                let index = usize::from(row[x]);
                let color = palette.get(index).ok_or_else(|| {
                    ImportError::Invalid(format!(
                        "lossless bitmap palette index {index} exceeds {} colors",
                        palette.len()
                    ))
                })?;
                rgba.extend_from_slice(color);
                continue;
            }
            let src = x * 4;
            let stored_alpha = if has_alpha { row[src] } else { 255 };
            let alpha = if has_alpha && stored_alpha != 0 && stored_alpha != 255 {
                stored_alpha - 1
            } else {
                stored_alpha
            };
            let mut red = row[src + 1];
            let mut green = row[src + 2];
            let mut blue = row[src + 3];
            if has_alpha && alpha > 0 && alpha < 255 {
                let unpremultiply = |value: u8| -> u8 {
                    ((u32::from(value) * 256 + u32::from(alpha) / 2) / u32::from(alpha)).min(255)
                        as u8
                };
                red = unpremultiply(red);
                green = unpremultiply(green);
                blue = unpremultiply(blue);
            }
            rgba.extend_from_slice(&[red, green, blue, alpha]);
        }
    }
    Ok((width, height, rgba))
}
fn raw_instance(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<RawSymbolInstance, ImportError> {
    let library_item_name = attr(reader, element, b"libraryItemName")?.unwrap_or_default();
    let first = parse_u16_attr(reader, element, b"firstFrame", 0)?;
    let last = optional_u16_attr(reader, element, b"lastFrame")?;
    let loop_mode = attr(reader, element, b"loop")?
        .unwrap_or_else(|| "loop".to_string())
        .to_ascii_lowercase();
    let mode = match loop_mode.as_str() {
        "single frame" | "singleframe" => Q0rgPlaybackMode::SingleFrame,
        "play once" | "playonce" => Q0rgPlaybackMode::PlayOnce,
        "loop reverse" | "loop reversed" | "reverse loop" => Q0rgPlaybackMode::LoopReversed,
        "play once reverse" | "play reversed once" | "reverse play once" => {
            Q0rgPlaybackMode::PlayReversedOnce
        }
        _ => Q0rgPlaybackMode::Loop,
    };
    let blend_name = attr(reader, element, b"blendMode")?
        .unwrap_or_else(|| "normal".to_string())
        .to_ascii_lowercase();
    let (blend_mode, unsupported_blend_mode) = match blend_name.as_str() {
        "normal" | "layer" => (BlendMode::Normal, None),
        "multiply" => (BlendMode::Multiply, None),
        "screen" => (BlendMode::Screen, None),
        "add" => (BlendMode::Add, None),
        "overlay" => (BlendMode::Overlay, None),
        other => (BlendMode::Normal, Some(other.to_string())),
    };
    Ok(RawSymbolInstance {
        library_item_name,
        affine: Affine::IDENTITY,
        opacity: 1.0,
        tint: None,
        blend_mode,
        unsupported_blend_mode,
        blur: None,
        glow: None,
        playback: Q0rgPlayback {
            mode,
            speed_percent: 100,
            placement_relative: true,
            first_frame: first,
            last_frame: last,
            single_frame: first,
        },
    })
}

fn apply_instance_filter(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    name: &[u8],
    state: &mut InstanceState,
    unsupported: &mut HashSet<String>,
) -> Result<(), ImportError> {
    match name {
        b"BlurFilter" => {
            if state.instance.blur.is_some() {
                unsupported.insert(
                    "multiple blur filters on one symbol are collapsed to the last filter"
                        .to_string(),
                );
            }
            state.instance.blur = Some(parse_blur_filter(reader, element)?);
        }
        b"GlowFilter" => {
            if state.instance.glow.is_some() {
                unsupported.insert(
                    "multiple glow filters on one symbol are collapsed to the last filter"
                        .to_string(),
                );
            }
            if attr_truthy(reader, element, b"inner")?
                || attr_truthy(reader, element, b"knockout")?
                || attr_truthy(reader, element, b"hideObject")?
            {
                unsupported.insert(
                    "inner/knockout/hideObject glow modes are not imported yet".to_string(),
                );
            }
            state.instance.glow = Some(parse_glow_filter(reader, element, unsupported)?);
        }
        _ => {}
    }
    Ok(())
}

fn parse_filter_quality(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<u8, ImportError> {
    let quality = parse_u16_attr(reader, element, b"quality", 1)?;
    if !(1..=15).contains(&quality) {
        return Err(ImportError::Invalid(format!(
            "filter quality must be between 1 and 15, got {quality}"
        )));
    }
    Ok(quality as u8)
}

fn validate_filter_radius(label: &str, value: f32) -> Result<f32, ImportError> {
    if !value.is_finite() || !(0.0..=512.0).contains(&value) {
        return Err(ImportError::Invalid(format!(
            "{label} must be finite and between 0 and 512, got {value}"
        )));
    }
    Ok(value)
}

fn parse_blur_filter(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<BlurFx, ImportError> {
    let radius_x = validate_filter_radius(
        "BlurFilter blurX",
        parse_f32_attr(reader, element, b"blurX", 4.0)?,
    )?;
    let radius_y = validate_filter_radius(
        "BlurFilter blurY",
        parse_f32_attr(reader, element, b"blurY", 4.0)?,
    )?;
    Ok(BlurFx {
        radius_x,
        radius_y,
        quality: parse_filter_quality(reader, element)?,
    })
}

fn parse_glow_filter(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    unsupported: &mut HashSet<String>,
) -> Result<GlowFx, ImportError> {
    let radius_x = validate_filter_radius(
        "GlowFilter blurX",
        parse_f32_attr(reader, element, b"blurX", 4.0)?,
    )?;
    let radius_y = validate_filter_radius(
        "GlowFilter blurY",
        parse_f32_attr(reader, element, b"blurY", 4.0)?,
    )?;
    let strength_attr = attr(reader, element, b"strength")?;
    let source_strength = match strength_attr.as_deref() {
        None => 1.0,
        Some(raw) => match raw.parse::<f32>() {
            Ok(value) if value.is_finite() => value,
            _ => {
                unsupported.insert(format!(
                    "invalid GlowFilter strength '{raw}' was replaced with the default"
                ));
                1.0
            }
        },
    };
    // Animate stores strengths above q0s' 0..=4 limit. Keep the project importable
    // and report whenever a source value has to be clamped to the supported range.
    let strength = source_strength.clamp(0.0, 4.0);
    if strength != source_strength {
        unsupported.insert(format!(
            "GlowFilter strength {source_strength} was clamped to the q0s range 0..=4"
        ));
    }
    let alpha = parse_f32_attr(reader, element, b"alpha", 1.0)?;
    if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
        return Err(ImportError::Invalid(format!(
            "GlowFilter alpha must be finite and between 0 and 1, got {alpha}"
        )));
    }
    let mut color = attr(reader, element, b"color")?
        .map(|value| parse_hex_color(&value))
        .transpose()?
        .unwrap_or(Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        });
    color.a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
    Ok(GlowFx {
        color,
        radius_x,
        radius_y,
        strength,
        quality: parse_filter_quality(reader, element)?,
    })
}

fn attr_truthy(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    key: &[u8],
) -> Result<bool, ImportError> {
    Ok(attr(reader, element, key)?
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "true" | "1")))
}

fn apply_text_attrs(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    state: &mut TextState,
    unsupported: &mut HashSet<String>,
) -> Result<(), ImportError> {
    if let Some(face) = attr(reader, element, b"face")?.filter(|face| !face.trim().is_empty()) {
        state.font = face;
    }
    if let Some(size) = attr(reader, element, b"size")? {
        let size = size
            .parse::<f32>()
            .map_err(|_| ImportError::Invalid(format!("invalid text size '{size}'")))?;
        if !size.is_finite() || size < 0.0 {
            return Err(ImportError::Invalid(format!("invalid text size '{size}'")));
        }
        if size == 0.0 {
            // Animate emits zero for text runs that inherit their parent's style.
            // Retain the previous run size (or the default size on the first run).
            unsupported.insert(
                "zero text size is treated as inherited from the current text style".to_string(),
            );
        } else {
            state.size = size;
        }
    }
    if let Some(color) = attr(reader, element, b"fillColor")? {
        state.color = parse_hex_color(&color)?;
    }
    Ok(())
}

fn handle_shape_start(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    depth: usize,
    shape: &mut Option<ShapeState>,
    unsupported: &mut HashSet<String>,
) -> Result<(), ImportError> {
    let Some(shape) = shape.as_mut() else {
        return Ok(());
    };
    match element.name().as_ref() {
        b"FillStyle" => {
            let index = parse_u16_attr(reader, element, b"index", 0)?;
            shape.fill_style = Some((depth, index));
        }
        b"StrokeStyle" => {
            let index = parse_u16_attr(reader, element, b"index", 0)?;
            shape.stroke_style = Some((depth, index));
            shape.strokes.entry(index).or_insert(Stroke {
                color: Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                width: 1.0,
                cap: CapShape::Round,
            });
        }
        b"SolidStroke" => apply_solid_stroke(reader, element, shape)?,
        b"SolidColor" => apply_solid_color(reader, element, shape)?,
        b"LinearGradient" | b"RadialGradient" => {
            if let Some((_, style_index)) = shape.fill_style {
                shape.gradient = Some(GradientState {
                    depth,
                    style_index,
                    kind: if element.name().as_ref() == b"LinearGradient" {
                        GradientKind::Linear
                    } else {
                        GradientKind::Radial
                    },
                    matrix: Affine::IDENTITY,
                    stops: Vec::new(),
                });
            }
        }
        b"BitmapFill" => {
            unsupported.insert("bitmap fills are not imported yet".to_string());
        }
        _ => {}
    }
    Ok(())
}

fn handle_shape_empty(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    shape: &mut Option<ShapeState>,
    unsupported: &mut HashSet<String>,
) -> Result<(), ImportError> {
    let Some(shape) = shape.as_mut() else {
        return Ok(());
    };
    match element.name().as_ref() {
        b"SolidStroke" => apply_solid_stroke(reader, element, shape)?,
        b"SolidColor" => apply_solid_color(reader, element, shape)?,
        b"GradientEntry" => apply_gradient_entry(reader, element, shape)?,
        b"Edge" => {
            let fill0 = optional_u16_attr(reader, element, b"fillStyle0")?;
            let fill1 = optional_u16_attr(reader, element, b"fillStyle1")?;
            let stroke = optional_u16_attr(reader, element, b"strokeStyle")?;
            let edges = attr(reader, element, b"edges")?.unwrap_or_default();
            shape.edges.push(RawEdge {
                fill0,
                fill1,
                stroke,
                segments: parse_edge_segments(&edges)?,
            });
        }
        b"BitmapFill" => {
            unsupported.insert("bitmap fills are not imported yet".to_string());
        }
        _ => {}
    }
    Ok(())
}

fn handle_shape_end(name: &[u8], depth: usize, shape: &mut Option<ShapeState>) {
    let Some(shape) = shape.as_mut() else {
        return;
    };
    if matches!(name, b"LinearGradient" | b"RadialGradient")
        && shape
            .gradient
            .as_ref()
            .is_some_and(|gradient| gradient.depth == depth)
    {
        if let Some(mut gradient) = shape.gradient.take() {
            gradient.stops.sort_by(|a, b| {
                a.position
                    .partial_cmp(&b.position)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if gradient.stops.len() >= 2 {
                // XFL/SWF gradients use a canonical +/-16384 *twip* domain, while
                // DOMShape edge coordinates above are normalized to stage pixels
                // (20 twips == 1 px). Keep both in the same local coordinate space.
                // Missing this /20 makes every imported gradient ~20x too large,
                // collapsing soft radial fields into almost-flat translucent blobs.
                const XFL_GRADIENT_RADIUS_PX: f32 = 16_384.0 / 20.0;
                gradient.matrix.a11 *= XFL_GRADIENT_RADIUS_PX;
                gradient.matrix.a12 *= XFL_GRADIENT_RADIUS_PX;
                gradient.matrix.a21 *= XFL_GRADIENT_RADIUS_PX;
                gradient.matrix.a22 *= XFL_GRADIENT_RADIUS_PX;
                shape.fills.insert(
                    gradient.style_index,
                    RawFillStyle::Gradient(GradientFill {
                        kind: gradient.kind,
                        stops: gradient.stops,
                        transform: gradient.matrix,
                    }),
                );
            }
        }
    }
    if name == b"FillStyle" && shape.fill_style.is_some_and(|(d, _)| d == depth) {
        shape.fill_style = None;
    }
    if name == b"StrokeStyle" && shape.stroke_style.is_some_and(|(d, _)| d == depth) {
        shape.stroke_style = None;
    }
}

fn apply_gradient_entry(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    shape: &mut ShapeState,
) -> Result<(), ImportError> {
    let Some(gradient) = shape.gradient.as_mut() else {
        return Ok(());
    };
    let position = parse_f32_attr(reader, element, b"ratio", 0.0)?.clamp(0.0, 1.0);
    let color = attr(reader, element, b"color")?
        .map(|value| parse_hex_color(&value))
        .transpose()?
        .unwrap_or(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        });
    let alpha = parse_f32_attr(reader, element, b"alpha", 1.0)?.clamp(0.0, 1.0);
    gradient.stops.push(GradientStop {
        position,
        color: Rgba {
            a: (alpha * 255.0).round() as u8,
            ..color
        },
    });
    Ok(())
}

fn apply_solid_color(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    shape: &mut ShapeState,
) -> Result<(), ImportError> {
    let color = attr(reader, element, b"color")?
        .map(|value| parse_hex_color(&value))
        .transpose()?
        .unwrap_or(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        });
    let alpha = attr(reader, element, b"alpha")?
        .map(|value| {
            value
                .parse::<f32>()
                .map_err(|_| ImportError::Invalid(format!("invalid alpha '{value}'")))
        })
        .transpose()?
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    let color = Rgba {
        a: (alpha * 255.0).round() as u8,
        ..color
    };
    if let Some((_, index)) = shape.stroke_style {
        shape
            .strokes
            .entry(index)
            .or_insert(Stroke {
                color,
                width: 1.0,
                cap: CapShape::Round,
            })
            .color = color;
    } else if let Some((_, index)) = shape.fill_style {
        shape.fills.insert(index, RawFillStyle::Solid(color));
    }
    Ok(())
}

fn apply_solid_stroke(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    shape: &mut ShapeState,
) -> Result<(), ImportError> {
    let Some((_, index)) = shape.stroke_style else {
        return Ok(());
    };
    let width = attr(reader, element, b"weight")?
        .map(|value| {
            value
                .parse::<f32>()
                .map_err(|_| ImportError::Invalid(format!("invalid stroke weight '{value}'")))
        })
        .transpose()?
        .unwrap_or(1.0)
        .max(0.01);
    let cap = match attr(reader, element, b"caps")?.as_deref() {
        Some("none") | Some("square") => CapShape::Butt,
        _ => CapShape::Round,
    };
    let stroke = shape.strokes.entry(index).or_insert(Stroke {
        color: Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        },
        width,
        cap,
    });
    stroke.width = width;
    stroke.cap = cap;
    Ok(())
}

fn parse_edge_segments(value: &str) -> Result<Vec<Segment>, ImportError> {
    let bytes = value.as_bytes();
    let mut cursor = 0usize;
    let mut current = None::<Vec2>;
    let mut segments = Vec::new();
    while cursor < bytes.len() {
        skip_ws(bytes, &mut cursor);
        if cursor >= bytes.len() {
            break;
        }
        match bytes[cursor] {
            b'!' => {
                cursor += 1;
                current = Some(Vec2::new(
                    parse_edge_number(bytes, &mut cursor)?,
                    parse_edge_number(bytes, &mut cursor)?,
                ));
            }
            b'|' | b'/' => {
                cursor += 1;
                let end = Vec2::new(
                    parse_edge_number(bytes, &mut cursor)?,
                    parse_edge_number(bytes, &mut cursor)?,
                );
                if let Some(start) = current {
                    segments.push(Segment {
                        start,
                        end,
                        control: None,
                    });
                }
                current = Some(end);
            }
            b'[' => {
                cursor += 1;
                let control = Vec2::new(
                    parse_edge_number(bytes, &mut cursor)?,
                    parse_edge_number(bytes, &mut cursor)?,
                );
                let end = Vec2::new(
                    parse_edge_number(bytes, &mut cursor)?,
                    parse_edge_number(bytes, &mut cursor)?,
                );
                if let Some(start) = current {
                    segments.push(Segment {
                        start,
                        end,
                        control: Some(control),
                    });
                }
                current = Some(end);
            }
            b'S' => {
                cursor += 1;
                while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                    cursor += 1;
                }
            }
            _ => cursor += 1,
        }
    }
    Ok(segments)
}

fn parse_edge_number(bytes: &[u8], cursor: &mut usize) -> Result<f32, ImportError> {
    skip_ws(bytes, cursor);
    let start = *cursor;
    while *cursor < bytes.len() {
        let ch = bytes[*cursor];
        if ch.is_ascii_whitespace() || matches!(ch, b'!' | b'|' | b'/' | b'[' | b'S') {
            break;
        }
        *cursor += 1;
    }
    if start == *cursor {
        return Err(ImportError::Invalid(
            "missing xfl edge coordinate".to_string(),
        ));
    }
    let token = std::str::from_utf8(&bytes[start..*cursor])
        .map_err(|_| ImportError::Invalid("non-utf8 xfl edge coordinate".to_string()))?;
    let twips = if let Some(hex) = token.strip_prefix('#') {
        parse_hex_fixed(hex)?
    } else {
        token
            .parse::<f32>()
            .map_err(|_| ImportError::Invalid(format!("invalid xfl edge coordinate '{token}'")))?
    };
    if !twips.is_finite() {
        return Err(ImportError::Invalid(format!(
            "non-finite xfl edge coordinate '{token}'"
        )));
    }
    // Animate/XFL edge coordinates are twips: 20 units == 1 stage pixel.
    Ok(twips / 20.0)
}

fn parse_hex_fixed(value: &str) -> Result<f32, ImportError> {
    // Animate's compact edge coordinates are signed 24.8-style hexadecimal
    // fixed point. Positive values omit leading zeroes (for example #62.A2),
    // while negative values are sign-extended to six hexadecimal whole digits
    // (for example #FFFFF2.33 == -13.80078125). Treating that latter spelling
    // as an unsigned hexadecimal number creates control points hundreds of
    // thousands of stage units away and turns otherwise tiny curves into giant
    // wedges across the canvas.
    let explicit_negative = value.starts_with('-');
    let value = value.trim_start_matches('-');
    let mut parts = value.split('.');
    let whole_text = parts.next().unwrap_or("0");
    let frac = parts.next().unwrap_or("");
    if parts.next().is_some() || whole_text.len() > 6 {
        return Err(ImportError::Invalid(format!(
            "invalid xfl hex coordinate '#{value}'"
        )));
    }
    let whole_bits = u32::from_str_radix(
        if whole_text.is_empty() {
            "0"
        } else {
            whole_text
        },
        16,
    )
    .map_err(|_| ImportError::Invalid(format!("invalid xfl hex coordinate '#{value}'")))?;
    let mut whole = if whole_text.len() == 6 && (whole_bits & 0x80_0000) != 0 {
        i64::from(whole_bits) - (1_i64 << 24)
    } else {
        i64::from(whole_bits)
    } as f32;
    let mut fraction = 0.0f32;
    let mut scale = 16.0f32;
    for ch in frac.chars() {
        let digit = ch.to_digit(16).ok_or_else(|| {
            ImportError::Invalid(format!("invalid xfl hex coordinate '#{value}'"))
        })?;
        fraction += digit as f32 / scale;
        scale *= 16.0;
    }
    if explicit_negative {
        whole = -whole;
        fraction = -fraction;
    }
    Ok(whole + fraction)
}

fn skip_ws(bytes: &[u8], cursor: &mut usize) {
    while *cursor < bytes.len() && bytes[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PointKey(i64, i64);

fn point_key(point: Vec2) -> PointKey {
    const SCALE: f32 = 1_000.0;
    PointKey(
        (point.x * SCALE).round() as i64,
        (point.y * SCALE).round() as i64,
    )
}

fn take_index(
    lookup: &mut HashMap<PointKey, Vec<usize>>,
    key: PointKey,
    segments: &[Option<Segment>],
) -> Option<usize> {
    let indices = lookup.get_mut(&key)?;
    while let Some(index) = indices.pop() {
        if segments.get(index).is_some_and(Option::is_some) {
            return Some(index);
        }
    }
    None
}

fn take_unique_index(
    lookup: &mut HashMap<PointKey, Vec<usize>>,
    key: PointKey,
    segments: &[Option<Segment>],
) -> Option<usize> {
    let indices = lookup.get_mut(&key)?;
    indices.retain(|index| segments.get(*index).is_some_and(Option::is_some));
    if indices.len() == 1 {
        indices.pop()
    } else {
        None
    }
}

fn stitch_segments(segments: Vec<Segment>, fill: bool) -> Vec<VectorPath> {
    let mut slots = segments.into_iter().map(Some).collect::<Vec<_>>();
    let mut starts = HashMap::<PointKey, Vec<usize>>::new();
    let mut ends = HashMap::<PointKey, Vec<usize>>::new();
    for (index, segment) in slots.iter().enumerate() {
        let segment = segment.expect("new segment slot");
        starts
            .entry(point_key(segment.start))
            .or_default()
            .push(index);
        if !fill {
            ends.entry(point_key(segment.end)).or_default().push(index);
        }
    }

    let mut result = Vec::new();
    for seed in 0..slots.len() {
        let Some(first) = slots[seed].take() else {
            continue;
        };
        let start = first.start;
        let mut chain = vec![first];
        loop {
            let end = chain.last().map(|segment| segment.end).unwrap_or(start);
            if points_near(end, start) {
                break;
            }
            let key = point_key(end);
            let next = if fill {
                take_index(&mut starts, key, &slots)
            } else {
                // A Flash stroke can be a graph: outline and detail strokes often meet
                // at the same vertex. Never guess which branch should continue the
                // current path; stop at the junction and let each branch become its
                // own open path. Guessing creates hairpins and giant miter spikes.
                take_unique_index(&mut starts, key, &slots)
            };
            let reversed = if next.is_none() && !fill {
                take_unique_index(&mut ends, key, &slots)
            } else {
                None
            };
            let Some(index) = next.or(reversed) else {
                break;
            };
            let mut segment = slots[index].take().expect("live lookup segment");
            if reversed.is_some() {
                segment = segment.reversed();
            }
            chain.push(segment);
        }
        if let Some(path) = segments_to_path(&chain, fill) {
            result.push(path);
        }
    }
    result
}

fn segments_to_path(segments: &[Segment], fill: bool) -> Option<VectorPath> {
    let first = *segments.first()?;
    let closed = points_near(segments.last()?.end, first.start);
    if fill && !closed {
        return None;
    }
    let mut anchors = vec![Anchor {
        point: first.start,
        in_handle: None,
        out_handle: None,
    }];
    for (index, segment) in segments.iter().enumerate() {
        let handles = segment.control.map(|control| {
            (
                Vec2::new(
                    segment.start.x + (control.x - segment.start.x) * (2.0 / 3.0),
                    segment.start.y + (control.y - segment.start.y) * (2.0 / 3.0),
                ),
                Vec2::new(
                    segment.end.x + (control.x - segment.end.x) * (2.0 / 3.0),
                    segment.end.y + (control.y - segment.end.y) * (2.0 / 3.0),
                ),
            )
        });
        if let Some(anchor) = anchors.last_mut() {
            anchor.out_handle = handles.map(|(out, _)| out);
        }
        let closing = closed && index + 1 == segments.len();
        let in_handle = handles.map(|(_, input)| input);
        if closing {
            anchors[0].in_handle = in_handle;
        } else {
            anchors.push(Anchor {
                point: segment.end,
                in_handle,
                out_handle: None,
            });
        }
    }
    if anchors.len() < if closed { 3 } else { 2 } {
        return None;
    }
    Some(VectorPath { anchors, closed })
}

fn points_near(a: Vec2, b: Vec2) -> bool {
    (a.x - b.x).abs() <= 1.0e-3 && (a.y - b.y).abs() <= 1.0e-3
}

fn parse_matrix(reader: &Reader<&[u8]>, element: &BytesStart<'_>) -> Result<Affine, ImportError> {
    Ok(Affine {
        a11: parse_f32_attr(reader, element, b"a", 1.0)?,
        a12: parse_f32_attr(reader, element, b"c", 0.0)?,
        a21: parse_f32_attr(reader, element, b"b", 0.0)?,
        a22: parse_f32_attr(reader, element, b"d", 1.0)?,
        tx: parse_f32_attr(reader, element, b"tx", 0.0)?,
        ty: parse_f32_attr(reader, element, b"ty", 0.0)?,
    })
}

pub(crate) fn affine_to_transform(affine: Affine) -> Option<Transform2D> {
    let sx = affine.a11.hypot(affine.a21);
    let determinant = affine.a11 * affine.a22 - affine.a12 * affine.a21;
    if !sx.is_finite() || sx <= 1.0e-7 || !determinant.is_finite() || determinant.abs() <= 1.0e-9 {
        return None;
    }
    let rotation = affine.a21.atan2(affine.a11);
    let (sin, cos) = rotation.sin_cos();
    let sheared_x = cos * affine.a12 + sin * affine.a22;
    let sy = -sin * affine.a12 + cos * affine.a22;
    if !sy.is_finite() || sy.abs() <= 1.0e-7 {
        return None;
    }
    let transform = Transform2D {
        tx: affine.tx,
        ty: affine.ty,
        sx,
        sy,
        rotation,
        skew_x: (sheared_x / sy).atan(),
        skew_y: 0.0,
    };
    let rebuilt = Affine::from_transform(transform);
    let error = [
        (rebuilt.a11 - affine.a11).abs(),
        (rebuilt.a12 - affine.a12).abs(),
        (rebuilt.a21 - affine.a21).abs(),
        (rebuilt.a22 - affine.a22).abs(),
        (rebuilt.tx - affine.tx).abs(),
        (rebuilt.ty - affine.ty).abs(),
    ]
    .into_iter()
    .fold(0.0f32, f32::max);
    (error <= 2.0e-4).then_some(transform)
}

fn parse_hex_color(value: &str) -> Result<Rgba, ImportError> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 {
        return Err(ImportError::Invalid(format!(
            "invalid xfl colour '{value}'"
        )));
    }
    let parse = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&hex[range], 16)
            .map_err(|_| ImportError::Invalid(format!("invalid xfl colour '{value}'")))
    };
    Ok(Rgba {
        r: parse(0..2)?,
        g: parse(2..4)?,
        b: parse(4..6)?,
        a: 255,
    })
}

fn parse_u16_attr(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    key: &[u8],
    default: u16,
) -> Result<u16, ImportError> {
    attr(reader, element, key)?
        .map(|value| {
            value.parse::<u16>().map_err(|_| {
                ImportError::Invalid(format!(
                    "invalid {}='{value}'",
                    String::from_utf8_lossy(key)
                ))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn optional_u16_attr(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    key: &[u8],
) -> Result<Option<u16>, ImportError> {
    attr(reader, element, key)?
        .map(|value| {
            value.parse::<u16>().map_err(|_| {
                ImportError::Invalid(format!(
                    "invalid {}='{value}'",
                    String::from_utf8_lossy(key)
                ))
            })
        })
        .transpose()
}

fn parse_f32_attr(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    key: &[u8],
    default: f32,
) -> Result<f32, ImportError> {
    attr(reader, element, key)?
        .map(|value| {
            value.parse::<f32>().map_err(|_| {
                ImportError::Invalid(format!(
                    "invalid {}='{value}'",
                    String::from_utf8_lossy(key)
                ))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn attr(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    key: &[u8],
) -> Result<Option<String>, ImportError> {
    for attribute in element.attributes().with_checks(false) {
        let attribute = attribute.map_err(|error| ImportError::Xml(error.to_string()))?;
        if attribute.key.as_ref() == key {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| ImportError::Xml(error.to_string()))?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

fn dedup_warnings(warnings: &mut Vec<String>) {
    let mut seen = HashSet::new();
    warnings.retain(|warning| seen.insert(warning.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_hex_edge_coordinates_do_not_explode_negative_curve_controls() {
        let positive = 98.0_f32 + 162.0 / 256.0;
        let negative = -14.0_f32 + 51.0 / 256.0;
        assert!((parse_hex_fixed("62.A2").unwrap() - positive).abs() < 1.0e-6);
        assert!((parse_hex_fixed("FFFFF2.33").unwrap() - negative).abs() < 1.0e-6);
        assert!((parse_hex_fixed("FFFFFF.F7").unwrap() + 0.03515625).abs() < 1.0e-6);

        let segments = parse_edge_segments("!65 36|75 15!75 15[#62.A2 #FFFFF2.33 121.5 1.5")
            .expect("real Animate edge fixture");
        let curve = segments.last().expect("quadratic segment");
        let control = curve.control.expect("quadratic control");
        assert!((control.x - 4.9316406).abs() < 1.0e-5);
        assert!((control.y + 0.69003904).abs() < 1.0e-5);
        assert!(control.x.abs() < 10.0 && control.y.abs() < 10.0);
    }

    #[test]
    fn edge_parser_preserves_line_and_quadratic_geometry() {
        let segments = parse_edge_segments("!0 0|10 0[15 5 20 0/30 0").unwrap();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].start, Vec2::new(0.0, 0.0));
        assert_eq!(segments[0].end, Vec2::new(0.5, 0.0));
        assert_eq!(segments[1].control, Some(Vec2::new(0.75, 0.25)));
        assert_eq!(segments[2].end, Vec2::new(1.5, 0.0));
    }

    fn signed_area(path: &VectorPath) -> f32 {
        let points = path
            .anchors
            .iter()
            .map(|anchor| anchor.point)
            .collect::<Vec<_>>();
        points
            .iter()
            .enumerate()
            .map(|(index, point)| {
                let next = points[(index + 1) % points.len()];
                point.x * next.y - next.x * point.y
            })
            .sum::<f32>()
            * 0.5
    }

    fn line(a: (f32, f32), b: (f32, f32)) -> Segment {
        Segment {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            control: None,
        }
    }

    #[test]
    fn stroke_stitch_stops_at_branch_instead_of_guessing_a_hairpin_route() {
        let p = |x: f32, y: f32| Vec2::new(x, y);
        let segments = vec![
            Segment {
                start: p(0.0, 0.0),
                end: p(1.0, 0.0),
                control: None,
            },
            Segment {
                start: p(1.0, 0.0),
                end: p(2.0, 0.0),
                control: None,
            },
            Segment {
                start: p(1.0, 0.0),
                end: p(1.0, 1.0),
                control: None,
            },
        ];
        let paths = stitch_segments(segments, false);
        assert_eq!(paths.len(), 3);
        assert!(paths.iter().all(|path| path.anchors.len() == 2));
        assert!(paths.iter().all(|path| !path.closed));
    }

    #[test]
    fn fill_stitch_keeps_hole_winding_and_disconnected_contours() {
        let outer = [
            line((0.0, 0.0), (10.0, 0.0)),
            line((10.0, 0.0), (10.0, 10.0)),
            line((10.0, 10.0), (0.0, 10.0)),
            line((0.0, 10.0), (0.0, 0.0)),
        ];
        let hole = [
            line((3.0, 3.0), (3.0, 7.0)),
            line((3.0, 7.0), (7.0, 7.0)),
            line((7.0, 7.0), (7.0, 3.0)),
            line((7.0, 3.0), (3.0, 3.0)),
        ];
        let mut segments = Vec::new();
        // Interleave the two rings so the test exercises topology stitching rather
        // than accidentally relying on source order.
        for index in 0..4 {
            segments.push(outer[index]);
            segments.push(hole[index]);
        }
        let paths = stitch_segments(segments, true);
        assert_eq!(paths.len(), 2, "outer and hole must stay separate contours");
        assert!(paths.iter().all(|path| path.closed));
        let areas = paths.iter().map(signed_area).collect::<Vec<_>>();
        assert!(
            areas[0] * areas[1] < 0.0,
            "hole winding must remain opposite to exterior"
        );
    }

    #[test]
    fn fill_stitch_never_closes_a_gap_by_reversing_an_unrelated_edge() {
        let segments = vec![line((0.0, 0.0), (1.0, 0.0)), line((0.0, 1.0), (1.0, 1.0))];
        let paths = stitch_segments(segments, true);
        assert!(
            paths.is_empty(),
            "an open fill boundary must not grow a synthetic chord"
        );
    }

    #[test]
    fn static_text_is_preserved_as_vector_source_instead_of_disappearing() {
        let xml = br##"<DOMSymbolItem name="Credits"><timeline><DOMTimeline name="Credits"><layers><DOMLayer name="L"><frames><DOMFrame index="0"><elements><DOMStaticText><matrix><Matrix tx="2" ty="3"/></matrix><textRuns><DOMTextRun><characters>hello &amp; bye</characters><textAttrs><DOMTextAttrs face="Impact" size="26" fillColor="#CA0000"/></textAttrs></DOMTextRun></textRuns></DOMStaticText></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"##;
        let timeline = parse_timeline(xml, true).unwrap().unwrap();
        let RawElement::Text(text) = &timeline.layers[0].frames[0].elements[0] else {
            panic!("static text must remain an importable vector source")
        };
        assert_eq!(text.text, "hello & bye");
        assert_eq!(text.font, "Impact");
        assert!((text.size - 26.0).abs() <= f32::EPSILON);
        assert_eq!(
            text.color,
            Rgba {
                r: 202,
                g: 0,
                b: 0,
                a: 255
            }
        );
        assert!((text.affine.tx - 2.0).abs() <= f32::EPSILON);
        assert!((text.affine.ty - 3.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn unsupported_dynamic_text_is_reported_instead_of_silently_disappearing() {
        let xml = br#"<DOMSymbolItem name="Dynamic"><timeline><DOMTimeline name="Dynamic"><layers><DOMLayer name="L"><frames><DOMFrame index="0"><elements><DOMDynamicText/></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"#;
        let timeline = parse_timeline(xml, true).unwrap().unwrap();
        assert!(timeline
            .warnings
            .iter()
            .any(|warning| warning == "dynamic/input text elements are not imported yet"));
    }

    #[test]
    fn radial_gradient_preserves_matrix_stop_color_and_alpha() {
        let xml = br##"<DOMSymbolItem name="Gradient"><timeline><DOMTimeline name="Gradient"><layers><DOMLayer name="L"><frames><DOMFrame index="0"><elements><DOMShape><fills><FillStyle index="1"><fill><RadialGradient><matrix><Matrix a="0.01" d="0.02" tx="5" ty="6"/></matrix><GradientEntry color="#FF0000" alpha="0.25" ratio="0"/><GradientEntry color="#0000FF" alpha="0.75" ratio="1"/></RadialGradient></fill></FillStyle></fills><edges><Edge fillStyle1="1" edges="!0 0|200 0|200 200|0 200|0 0"/></edges></DOMShape></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"##;
        let timeline = parse_timeline(xml, true).unwrap().unwrap();
        let RawElement::Shape(shape) = &timeline.layers[0].frames[0].elements[0] else {
            panic!("gradient shape")
        };
        let Some(RawFillStyle::Gradient(gradient)) = shape.fills.get(&1) else {
            panic!("radial gradient style")
        };
        assert_eq!(gradient.kind, GradientKind::Radial);
        assert_eq!(gradient.stops.len(), 2);
        assert_eq!(
            gradient.stops[0].color,
            Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 64
            }
        );
        assert_eq!(
            gradient.stops[1].color,
            Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 191
            }
        );
        assert!((gradient.transform.a11 - 8.192).abs() < 0.001);
        assert!((gradient.transform.a22 - 16.384).abs() < 0.001);
        assert!((gradient.transform.tx - 5.0).abs() < f32::EPSILON);
        assert!((gradient.transform.ty - 6.0).abs() < f32::EPSILON);
    }

    #[test]
    fn real_muzzle_radial_gradient_uses_pixel_scaled_xfl_domain() {
        // Exact gradient matrix and shape bounds from Eastly's `Symbol 9.xml`.
        // The source shape is 1124x672 twips = 56.2x33.6 px and its gradient
        // basis is 0.034912109375. The imported radial radius must therefore be
        // ~28.6 px, not ~572 px.
        let xml = br##"<DOMSymbolItem name="Symbol 9"><timeline><DOMTimeline name="Symbol 9"><layers><DOMLayer name="L"><frames><DOMFrame index="0"><elements><DOMShape><fills><FillStyle index="1"><RadialGradient><matrix><Matrix a="0.034912109375" d="0.034912109375" tx="28.1" ty="16.8"/></matrix><GradientEntry color="#FFFFFF" alpha="0.701960784313725" ratio="0"/><GradientEntry color="#FFFFFF" alpha="0.219607843137255" ratio="0.411764705882353"/><GradientEntry color="#FFF08C" alpha="0" ratio="1"/></RadialGradient></FillStyle></fills><edges><Edge fillStyle1="1" edges="!1124 336[1124 475 959 573!959 573[795 672 562 672!562 672[329 672 164 573!164 573[0 475 0 336!0 336[0 197 164 98!164 98[329 0 562 0!562 0[795 0 959 98!959 98[1124 197 1124 336"/></edges></DOMShape></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"##;
        let timeline = parse_timeline(xml, true).unwrap().unwrap();
        let RawElement::Shape(shape) = &timeline.layers[0].frames[0].elements[0] else {
            panic!("gradient shape")
        };
        let Some(RawFillStyle::Gradient(gradient)) = shape.fills.get(&1) else {
            panic!("radial gradient style")
        };
        assert!((gradient.transform.a11 - 28.6).abs() < 0.01);
        assert!((gradient.transform.a22 - 28.6).abs() < 0.01);
        assert!((gradient.transform.tx - 28.1).abs() < f32::EPSILON);
        assert!((gradient.transform.ty - 16.8).abs() < f32::EPSILON);
        assert_eq!(gradient.stops[0].color.a, 179);
        assert_eq!(gradient.stops[1].color.a, 56);
        assert_eq!(gradient.stops[2].color.a, 0);
    }

    #[test]
    fn motion_tween_import_preserves_custom_cubic_ease() {
        let xml = br#"<DOMSymbolItem name="Tweened"><timeline><DOMTimeline name="Tweened"><layers><DOMLayer name="L"><frames><DOMFrame index="0" duration="10" tweenType="motion"><tweens><CustomEase target="all"><Point x="0" y="0"/><Point x="0.3333" y="0.3333"/><Point x="0.8272" y="1"/><Point x="1" y="1"/></CustomEase></tweens><elements><DOMSymbolInstance libraryItemName="Inner"/></elements></DOMFrame><DOMFrame index="10"><elements><DOMSymbolInstance libraryItemName="Inner"><matrix><Matrix tx="20"/></matrix></DOMSymbolInstance></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"#;
        let timeline = parse_timeline(xml, true).unwrap().unwrap();
        let source = &timeline.layers[0].frames[0];
        assert_eq!(source.tween_type, RawTweenType::Motion);
        assert_eq!(source.custom_ease_points.len(), 4);
        let mut warnings = Vec::new();
        let tween = import_frame_tween(source, Some(10), "Tweened", "L", &mut warnings);
        assert!(warnings.is_empty());
        let Tween::Eased { to_frame, easing } = tween else {
            panic!("Animate motion tween did not become a native eased tween");
        };
        assert_eq!(to_frame, 10);
        let Easing::CubicBezier { x1, y1, x2, y2 } = easing else {
            panic!("custom ease must remain cubic bezier");
        };
        assert!((x1 - 0.3333).abs() < 1.0e-6);
        assert!((y1 - 0.3333).abs() < 1.0e-6);
        assert!((x2 - 0.8272).abs() < 1.0e-6);
        assert!((y2 - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn motion_tween_without_custom_ease_imports_as_linear() {
        let xml = br#"<DOMSymbolItem name="Tweened"><timeline><DOMTimeline name="Tweened"><layers><DOMLayer name="L"><frames><DOMFrame index="3" duration="7" tweenType="motion"><elements><DOMSymbolInstance libraryItemName="Inner"/></elements></DOMFrame><DOMFrame index="10"><elements><DOMSymbolInstance libraryItemName="Inner"/></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"#;
        let timeline = parse_timeline(xml, true).unwrap().unwrap();
        let source = &timeline.layers[0].frames[0];
        let mut warnings = Vec::new();
        assert_eq!(
            import_frame_tween(source, Some(10), "Tweened", "L", &mut warnings),
            Tween::Linear { to_frame: 10 }
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn playback_import_maps_flash_graphic_modes() {
        let xml = br#"<DOMSymbolItem name="Outer"><timeline><DOMTimeline name="Outer"><layers><DOMLayer name="L"><frames><DOMFrame index="0"><elements><DOMSymbolInstance libraryItemName="Inner" symbolType="graphic" loop="single frame" firstFrame="3"><matrix><Matrix tx="4" ty="5"/></matrix></DOMSymbolInstance></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"#;
        let timeline = parse_timeline(xml, true).unwrap().unwrap();
        let RawElement::Symbol(instance) = &timeline.layers[0].frames[0].elements[0] else {
            panic!("symbol")
        };
        assert_eq!(instance.library_item_name, "Inner");
        assert_eq!(instance.playback.mode, Q0rgPlaybackMode::SingleFrame);
        assert_eq!(instance.playback.single_frame, 3);
        assert_eq!(instance.affine.tx, 4.0);
        assert_eq!(instance.affine.ty, 5.0);
    }

    #[test]
    fn symbol_add_blend_mode_is_preserved() {
        let xml = br#"<DOMSymbolItem name="Outer"><timeline><DOMTimeline name="Outer"><layers><DOMLayer name="L"><frames><DOMFrame index="0"><elements><DOMSymbolInstance libraryItemName="Inner" blendMode="add"/></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"#;
        let timeline = parse_timeline(xml, true).unwrap().unwrap();
        let RawElement::Symbol(instance) = &timeline.layers[0].frames[0].elements[0] else {
            panic!("symbol")
        };
        assert_eq!(instance.blend_mode, BlendMode::Add);
        assert!(instance.unsupported_blend_mode.is_none());
    }
    #[test]
    fn symbol_filters_preserve_anisotropy_quality_and_glow_color() {
        let xml = br##"<DOMSymbolItem name="Outer"><timeline><DOMTimeline name="Outer"><layers><DOMLayer name="L"><frames><DOMFrame index="0"><elements><DOMSymbolInstance libraryItemName="Inner"><filters><BlurFilter blurX="50" blurY="4" quality="1"/><GlowFilter blurX="2" blurY="11" color="#FFCC33" quality="3" strength="2.57"/></filters></DOMSymbolInstance></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"##;
        let timeline = parse_timeline(xml, true).unwrap().unwrap();
        let RawElement::Symbol(instance) = &timeline.layers[0].frames[0].elements[0] else {
            panic!("symbol")
        };
        assert_eq!(
            instance.blur,
            Some(BlurFx {
                radius_x: 50.0,
                radius_y: 4.0,
                quality: 1,
            })
        );
        let glow = instance.glow.expect("glow");
        assert_eq!(glow.radius_x, 2.0);
        assert_eq!(glow.radius_y, 11.0);
        assert_eq!(glow.quality, 3);
        assert!((glow.strength - 2.57).abs() < 1.0e-6);
        assert_eq!(
            glow.color,
            Rgba {
                r: 255,
                g: 204,
                b: 51,
                a: 255
            }
        );
    }
    #[test]
    fn lossless_bitmap_dat_decodes_chunked_argb_into_rgba() {
        use std::io::Write as _;

        let raw = [
            255u8, 10, 20, 30, // opaque ARGB
            1, 0, 0, 0, // Animate alpha=1 encodes fully transparent
        ];
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&raw).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut dat = Vec::new();
        dat.extend_from_slice(&[0x03, 0x05]);
        dat.extend_from_slice(&8u16.to_le_bytes());
        dat.extend_from_slice(&2u16.to_le_bytes());
        dat.extend_from_slice(&1u16.to_le_bytes());
        dat.extend_from_slice(&[0u8; 16]);
        dat.push(1); // alpha
        dat.push(1); // compressed
        dat.extend_from_slice(&(compressed.len() as u16).to_le_bytes());
        dat.extend_from_slice(&compressed);
        dat.extend_from_slice(&0u16.to_le_bytes());

        let (width, height, rgba) = decode_lossless_bitmap_dat(&dat).unwrap();
        assert_eq!((width, height), (2, 1));
        assert_eq!(rgba, vec![10, 20, 30, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn indexed_lossless_bitmap_dat_decodes_palette_and_padded_rows() {
        use std::io::Write as _;

        let raw_indices = [0u8, 1, 0, 0, 1, 0, 1, 0];
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&raw_indices).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut dat = Vec::new();
        dat.extend_from_slice(&[0x03, 0x03]);
        dat.extend_from_slice(&4u16.to_le_bytes()); // padded bytes per row
        dat.extend_from_slice(&3u16.to_le_bytes());
        dat.extend_from_slice(&2u16.to_le_bytes());
        dat.extend_from_slice(&[0u8; 16]);
        dat.push(1); // alpha is stored in palette entries
        dat.push(2); // two palette colors
        dat.extend_from_slice(&[0u8; 2]); // format prefix
        dat.extend_from_slice(&[10, 20, 30, 255, 200, 100, 50, 64]);
        dat.extend_from_slice(&(compressed.len() as u16).to_le_bytes());
        dat.extend_from_slice(&compressed);
        dat.extend_from_slice(&0u16.to_le_bytes());

        let (width, height, rgba) = decode_lossless_bitmap_dat(&dat).unwrap();
        assert_eq!((width, height), (3, 2));
        assert_eq!(
            rgba,
            vec![
                10, 20, 30, 255, 200, 100, 50, 64, 10, 20, 30, 255, 200, 100, 50, 64, 10, 20, 30,
                255, 200, 100, 50, 64,
            ]
        );
    }
}
