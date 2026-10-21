//! GPU-side vector rendering for `.q0s` v2 movies.
//!
//! q0player's earlier path rasterised every frame to RGBA on the CPU and
//! shipped it as a texture. That's why the result looked pixelated and
//! ate a frame's worth of CPU per repaint: egui then *upscaled* the
//! bitmap to fill the canvas. The right answer for a "Flash-style"
//! player is the same one the editor uses on stage — emit
//! `egui::Shape::Path` and `Shape::Mesh` directly into the canvas
//! painter, letting egui's GPU tesselator hit native pixels at any zoom.
//!
//! We deliberately mirror q0editor's render module rather than depend on
//! it, so the player stays free of the editor crate. The shared bits
//! (path flattening, transform interpolation) live in `q0s_format::geom`
//! and `q0s_format::raster::active_placement_states_at`.

use std::collections::HashMap;

use egui::epaint::Vertex;
use egui::{
    pos2, Color32, ColorImage, Context, Mesh, Painter, Pos2, Rect, Shape, Stroke, TextureHandle,
    TextureOptions,
};
use lyon_path::math::point as lyon_point;
use lyon_tessellation::geometry_builder::{BuffersBuilder, Positions, VertexBuffers};
use lyon_tessellation::{
    FillOptions, FillRule as LyonFillRule, FillTessellator, LineCap, LineJoin, StrokeOptions,
    StrokeTessellator,
};
use q0s_format::geom::flatten_path;
use q0s_format::raster::active_placement_states_at;
use q0s_format::runtime_scene::RuntimeSceneState;
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Asset, BlendMode, GradientFill, InstanceKey, Placement, PlacementFx, ProjectV2, Rgba, Target,
    Vec2, VectorAppearance,
};

const Q0RG_RECURSION_LIMIT: u8 = 8;
const BEZIER_SAMPLES: usize = 16;

/// Bitmap texture cache keyed by `asset_id`. Lives on `PlayerApp` so we
/// only upload each bitmap to the GPU once per session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FxPlacementKey {
    host_q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    local_frame: u16,
    width: u32,
    height: u32,
    affine_bits: [u32; 6],
}

struct FxPlacementTexture {
    texture: TextureHandle,
    crop: [u32; 4],
    blend_mode: BlendMode,
}

struct VideoTexture {
    texture: TextureHandle,
    bytes: usize,
    last_used: u64,
}

#[derive(Default)]
pub struct TextureCache {
    by_asset_id: HashMap<u16, TextureHandle>,
    solid_vectors: HashMap<u16, Vec<std::sync::Arc<Mesh>>>,
    by_q0v_frame: HashMap<(usize, u32), VideoTexture>,
    masked_q0rg_frames: HashMap<(u16, u16, u32, u32), TextureHandle>,
    q0v_media: HashMap<usize, q0video::q0v::Q0vFile>,
    fx_placement_textures: HashMap<FxPlacementKey, FxPlacementTexture>,
    fx_serial: u64,
}

impl TextureCache {
    pub fn invalidate(&mut self) {
        self.by_asset_id.clear();
        self.solid_vectors.clear();
        self.by_q0v_frame.clear();
        self.masked_q0rg_frames.clear();
        self.q0v_media.clear();
        self.fx_placement_textures.clear();
    }
}

struct StageView {
    /// Top-left of the stage rectangle in screen pixels.
    origin: Pos2,
    /// pixels-per-stage-unit (square pixels).
    scale: f32,
}

/// Paint one frame of a v2 movie into `target_rect`. The stage is
/// content-aspect-fit inside the rect (letterboxed / pillarboxed), so
/// shapes never warp when the host window does.
///
/// `white_stage_bg` paints a solid white rect under the stage before any
/// movie shapes go down — useful on dark themes where transparent /
/// unfilled regions otherwise blend into the panel and disappear.
#[allow(clippy::too_many_arguments)]
pub fn paint_v2_frame(
    painter: &Painter,
    ctx: &Context,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    target_rect: Rect,
    cache: &mut TextureCache,
    white_stage_bg: bool,
) {
    paint_v2_frame_with_rig_overrides(
        painter,
        ctx,
        project,
        q0rg_id,
        frame,
        target_rect,
        cache,
        white_stage_bg,
        &q0s_format::rig::RigRuntimeOverrides::new(),
    );
}

#[allow(clippy::too_many_arguments)]
pub fn paint_v2_frame_with_rig_overrides(
    painter: &Painter,
    ctx: &Context,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    target_rect: Rect,
    cache: &mut TextureCache,
    white_stage_bg: bool,
    rig_overrides: &q0s_format::rig::RigRuntimeOverrides,
) {
    paint_v2_frame_with_runtime_overrides(
        painter,
        ctx,
        project,
        q0rg_id,
        frame,
        target_rect,
        cache,
        white_stage_bg,
        rig_overrides,
        &RuntimeSceneState::new(),
    );
}

#[allow(clippy::too_many_arguments)]
pub fn paint_v2_frame_with_runtime_overrides(
    painter: &Painter,
    ctx: &Context,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    target_rect: Rect,
    cache: &mut TextureCache,
    white_stage_bg: bool,
    rig_overrides: &q0s_format::rig::RigRuntimeOverrides,
    scene_state: &RuntimeSceneState,
) {
    let stage_w = project.meta.stage_width.max(1) as f32;
    let stage_h = project.meta.stage_height.max(1) as f32;
    let scale = (target_rect.width() / stage_w)
        .min(target_rect.height() / stage_h)
        .max(0.001);
    let drawn = egui::vec2(stage_w * scale, stage_h * scale);
    let origin = pos2(
        target_rect.center().x - drawn.x * 0.5,
        target_rect.center().y - drawn.y * 0.5,
    );
    // A player shows the stage only, as Flash does: artwork parked beside it
    // stays out of view.
    let clipped = painter.with_clip_rect(
        painter
            .clip_rect()
            .intersect(Rect::from_min_size(origin, drawn)),
    );
    let painter = &clipped;
    if white_stage_bg {
        let stage_rect = Rect::from_min_size(origin, drawn);
        let bg = project.meta.background_color;
        painter.rect_filled(
            stage_rect,
            0.0,
            Color32::from_rgba_unmultiplied(bg.r, bg.g, bg.b, 255),
        );
    }
    let view = StageView { origin, scale };
    // Masks are cut on the GPU like the editor does. Rasterizing the whole
    // frame on the CPU is the fallback for what that cannot draw: it costs
    // tens of milliseconds a frame and has no anti-aliasing.
    let cpu_masks = q0rg_contains_mask(project, q0rg_id, &mut std::collections::HashSet::new(), 0)
        && !gpu_masks_supported(
            project,
            q0rg_id,
            frame,
            authored_root_affine(project, q0rg_id, frame),
            &view,
            0,
            cache,
            scene_state,
        );
    if cpu_masks {
        let stage_rect = Rect::from_min_size(origin, drawn);
        let (width, height) = fx_raster_size(stage_rect, ctx);
        let key = (q0rg_id, frame, width, height);
        let can_reuse = rig_overrides.is_empty() && scene_state.is_empty();
        if !can_reuse || !cache.masked_q0rg_frames.contains_key(&key) {
            if !can_reuse {
                cache.masked_q0rg_frames.clear();
            } else if cache.masked_q0rg_frames.len() >= 3 {
                if let Some(oldest) = cache
                    .masked_q0rg_frames
                    .keys()
                    .copied()
                    .max_by_key(|cached| cached.1.abs_diff(frame))
                {
                    cache.masked_q0rg_frames.remove(&oldest);
                }
            }
            let rgba = q0s_format::raster::rasterize_q0rg_frame_with_runtime_state(
                project,
                q0rg_id,
                frame,
                width,
                height,
                1,
                [0, 0, 0, 0],
                rig_overrides,
                scene_state,
            );
            let image =
                ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
            let texture = ctx.load_texture(
                format!("masked-player-q0rg-{q0rg_id}-frame-{frame}"),
                image,
                TextureOptions::LINEAR,
            );
            cache.masked_q0rg_frames.insert(key, texture);
        }
        if let Some(texture) = cache.masked_q0rg_frames.get(&key) {
            painter.image(
                texture.id(),
                stage_rect,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    } else {
        paint_q0rg(
            painter,
            ctx,
            project,
            q0rg_id,
            frame,
            authored_root_affine(project, q0rg_id, frame),
            &view,
            0,
            cache,
            rig_overrides,
            scene_state,
            q0glblend::ColorTransform::default(),
        );
    }
}

fn q0rg_contains_mask(
    project: &ProjectV2,
    q0rg_id: u16,
    visited: &mut std::collections::HashSet<u16>,
    depth: u8,
) -> bool {
    if depth > Q0RG_RECURSION_LIMIT || !visited.insert(q0rg_id) {
        return false;
    }
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return false;
    };
    q0rg.layers.iter().any(|layer| {
        project.layer_metadata(q0rg_id, layer.layer_id).kind == q0s_format::v2::LayerKind::Mask
            || layer
                .placements
                .iter()
                .any(|placement| match placement.target {
                    Target::Q0rg(child_id) => {
                        q0rg_contains_mask(project, child_id, visited, depth + 1)
                    }
                    Target::Asset(_) => false,
                })
    })
}

fn authored_root_affine(project: &ProjectV2, q0rg_id: u16, frame: u16) -> Affine {
    if q0rg_id == project.meta.entry_q0rg_id && project.runtime.camera.enabled {
        project.camera_affine_at(frame)
    } else {
        Affine::IDENTITY
    }
}

fn fx_raster_size(stage_rect: Rect, ctx: &Context) -> (u32, u32) {
    let pixels_per_point = ctx.pixels_per_point().clamp(1.0, 2.0);
    let mut width = (stage_rect.width().max(1.0) * pixels_per_point).ceil();
    let mut height = (stage_rect.height().max(1.0) * pixels_per_point).ceil();
    let dimension_scale = (3072.0 / width.max(height)).min(1.0);
    width *= dimension_scale;
    height *= dimension_scale;
    let pixels = width * height;
    if pixels > 6_000_000.0 {
        let scale = (6_000_000.0 / pixels).sqrt();
        width *= scale;
        height *= scale;
    }
    (
        width.max(1.0).round() as u32,
        height.max(1.0).round() as u32,
    )
}

fn affine_bits(affine: Affine) -> [u32; 6] {
    [
        affine.a11.to_bits(),
        affine.a12.to_bits(),
        affine.a21.to_bits(),
        affine.a22.to_bits(),
        affine.tx.to_bits(),
        affine.ty.to_bits(),
    ]
}

fn solid_appearance(appearance: Option<&VectorAppearance>) -> bool {
    appearance.is_none_or(|appearance| {
        matches!(appearance.material, q0s_format::v2::VectorMaterial::Solid)
            && appearance.fill_gradient.is_none()
            && appearance.erase_mask.is_empty()
            && appearance.clip_mask.is_empty()
            && appearance.material_source.is_empty()
    })
}

fn similarity(affine: Affine) -> bool {
    let x = affine.a11 * affine.a11 + affine.a21 * affine.a21;
    let y = affine.a12 * affine.a12 + affine.a22 * affine.a22;
    let dot = affine.a11 * affine.a12 + affine.a21 * affine.a22;
    let tolerance = x.max(y).max(1e-12) * 1e-5;
    (x - y).abs() <= tolerance && dot.abs() <= tolerance
}

fn local_solid_meshes(vector: &q0s_format::v2::VectorAsset) -> Vec<std::sync::Arc<Mesh>> {
    let paths: Vec<_> = vector
        .paths
        .iter()
        .map(|path| {
            let mut points: Vec<_> = flatten_path(path, BEZIER_SAMPLES)
                .into_iter()
                .map(|point| pos2(point.x, point.y))
                .collect();
            if path.closed {
                drop_duplicate_closing_point_pos2(&mut points);
            }
            (points, path.closed)
        })
        .collect();
    let mut meshes = Vec::new();
    if let Some(color) = vector.fill {
        let contours = paths
            .iter()
            .filter(|(_, closed)| *closed)
            .map(|(points, _)| points.clone())
            .collect::<Vec<_>>();
        if let Some(buffers) = tessellate_complex_fill(&contours) {
            let mesh = Mesh {
                indices: buffers.indices,
                vertices: buffers
                    .vertices
                    .into_iter()
                    .map(|point| Vertex {
                        pos: pos2(point.x, point.y),
                        uv: Pos2::ZERO,
                        color: rgba_to_color32(color),
                    })
                    .collect(),
                ..Default::default()
            };
            meshes.push(std::sync::Arc::new(mesh));
        }
    }
    if let Some(stroke) = vector.stroke {
        for (points, closed) in &paths {
            if let Some(mesh) = styled_stroke_mesh(
                points,
                stroke.width,
                *closed,
                rgba_to_color32(stroke.color),
                stroke.cap,
                stroke.join,
            ) {
                meshes.push(std::sync::Arc::new(mesh));
            }
            if !closed
                && points.len() >= 2
                && matches!(stroke.cap, q0s_format::geom::CapShape::Round)
            {
                for center in [points[0], *points.last().unwrap()] {
                    let points = (0..32)
                        .map(|i| {
                            let angle = i as f32 * std::f32::consts::TAU / 32.;
                            pos2(
                                center.x + angle.cos() * stroke.width * 0.5,
                                center.y + angle.sin() * stroke.width * 0.5,
                            )
                        })
                        .collect::<Vec<_>>();
                    if let Some(buffers) = tessellate_complex_fill(&[points]) {
                        let mesh = Mesh {
                            indices: buffers.indices,
                            vertices: buffers
                                .vertices
                                .into_iter()
                                .map(|point| Vertex {
                                    pos: pos2(point.x, point.y),
                                    uv: Pos2::ZERO,
                                    color: rgba_to_color32(stroke.color),
                                })
                                .collect(),
                            ..Default::default()
                        };
                        meshes.push(std::sync::Arc::new(mesh));
                    }
                }
            }
        }
    }
    meshes
}

fn solid_meshes(
    vector: &q0s_format::v2::VectorAsset,
    transform: Affine,
    view: &StageView,
    cache: &mut TextureCache,
    color: q0glblend::ColorTransform,
) -> Vec<Mesh> {
    // Authored assets are immutable in the player. Scene changes invalidate
    // this cache; translation, tint, playback and zoom only transform vertices.
    cache
        .solid_vectors
        .entry(vector.asset_id)
        .or_insert_with(|| local_solid_meshes(vector))
        .iter()
        .map(|source| {
            let mut mesh = source.as_ref().clone();
            // These solid fill/stroke meshes have one authored color. Apply
            // tint once, rather than converting RGB for every glyph vertex.
            let paint = source
                .vertices
                .first()
                .map(|vertex| color.apply(vertex.color))
                .unwrap_or(Color32::TRANSPARENT);
            for vertex in &mut mesh.vertices {
                vertex.pos =
                    stage_to_screen(transform.apply(Vec2::new(vertex.pos.x, vertex.pos.y)), view);
                vertex.color = paint;
            }
            mesh
        })
        .collect()
}

/// Meshes for the in-between shapes of a shape tween. They change every
/// frame, so unlike authored vectors they are never cached by asset id.
fn morphed_meshes(
    shapes: &[q0s_format::v2::VectorAsset],
    transform: Affine,
    view: &StageView,
    color: q0glblend::ColorTransform,
    opacity: f32,
) -> Vec<Mesh> {
    shapes
        .iter()
        .flat_map(local_solid_meshes)
        .map(|source| {
            let mut mesh = source.as_ref().clone();
            for vertex in &mut mesh.vertices {
                vertex.pos =
                    stage_to_screen(transform.apply(Vec2::new(vertex.pos.x, vertex.pos.y)), view);
                vertex.color = color.apply(vertex.color).gamma_multiply(opacity);
            }
            mesh
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn collect_solid_q0rg(
    project: &ProjectV2,
    owner: u16,
    frame: u16,
    parent: Affine,
    view: &StageView,
    depth: u8,
    cache: &mut TextureCache,
    scene: &RuntimeSceneState,
    color: q0glblend::ColorTransform,
    meshes: &mut Vec<Mesh>,
) -> bool {
    if depth > Q0RG_RECURSION_LIMIT {
        return false;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == owner) else {
        return false;
    };
    // Nested masks, rig deformation and nested group effects keep their
    // existing renderer; this collector accepts only plain vector artwork.
    if q0s_format::rig::rig_for_q0rg(project, owner).is_some() {
        return false;
    }
    let frame = frame % q.frame_count.max(1);
    for layer in &q.layers {
        if !project.layer_is_visible(owner, layer.layer_id) {
            continue;
        }
        if project
            .layer_metadata
            .get(&q0s_format::v2::LayerKey {
                q0rg_id: owner,
                layer_id: layer.layer_id,
            })
            .is_some_and(|metadata| metadata.kind == q0s_format::v2::LayerKind::Mask)
        {
            return false;
        }
        let shape_morph = q0s_format::shape_tween::project_layer_morph(project, layer, frame);
        for active in active_placement_states_at(layer, frame) {
            let placement = &layer.placements[active.index];
            let mut visual = active.fx;
            visual.tint = None;
            if !visual.is_identity() {
                return false;
            }
            let key = InstanceKey::new(owner, placement.instance_id);
            let transform = Affine::compose(
                parent,
                scene.effective_affine(key, Affine::from_transform(active.transform)),
            );
            let color = active
                .fx
                .tint
                .map(q0glblend::ColorTransform::tint)
                .unwrap_or_default()
                .then(color);
            if let Some(shapes) = shape_morph
                .as_ref()
                .and_then(|morph| morph.for_placement(active.index))
            {
                meshes.extend(morphed_meshes(shapes, transform, view, color, 1.0));
                continue;
            }
            match placement.target {
                Target::Asset(id) => {
                    let Some(Asset::Vector(vector)) =
                        project.assets.iter().find(|asset| asset.id() == id)
                    else {
                        return false;
                    };
                    if !solid_appearance(project.asset_appearances.get(&id))
                        || (vector.stroke.is_some() && !similarity(transform))
                    {
                        return false;
                    }
                    meshes.extend(solid_meshes(vector, transform, view, cache, color));
                }
                Target::Q0rg(id) => {
                    let fallback = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        id,
                        frame,
                        active.playback_start_frame,
                        active.fx.q0rg_playback,
                    );
                    let (child_frame, child_scene) = scene.child_context(key, fallback);
                    if !collect_solid_q0rg(
                        project,
                        id,
                        child_frame,
                        transform,
                        view,
                        depth + 1,
                        cache,
                        child_scene,
                        color,
                        meshes,
                    ) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn paint_fx_placement(
    painter: &Painter,
    ctx: &Context,
    project: &ProjectV2,
    host_q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    placement: &Placement,
    local_frame: u16,
    composed: Affine,
    fx: PlacementFx,
    view: &StageView,
    cache: &mut TextureCache,
    inherited_color: q0glblend::ColorTransform,
) {
    let stage_w = project.meta.stage_width.max(1) as f32;
    let stage_h = project.meta.stage_height.max(1) as f32;
    let stage_rect = Rect::from_min_size(
        view.origin,
        egui::vec2(stage_w * view.scale, stage_h * view.scale),
    );
    let (width, height) = fx_raster_size(stage_rect, ctx);
    let key = FxPlacementKey {
        host_q0rg_id,
        layer_id,
        placement_idx,
        local_frame,
        width,
        height,
        affine_bits: affine_bits(composed),
    };
    if !cache.fx_placement_textures.contains_key(&key) {
        if cache.fx_placement_textures.len() >= 64 {
            cache.fx_placement_textures.clear();
        }
        let Some(crop) = q0s_format::raster::rasterize_placement_fx_scaled_cropped(
            project,
            host_q0rg_id,
            placement,
            local_frame,
            composed,
            fx,
            width,
            height,
        ) else {
            return;
        };
        let image = ColorImage::from_rgba_unmultiplied(
            [crop.width as usize, crop.height as usize],
            &crop.rgba,
        );
        cache.fx_serial = cache.fx_serial.wrapping_add(1);
        let texture = ctx.load_texture(
            format!("q0player_fx_placement_{}", cache.fx_serial),
            image,
            TextureOptions::LINEAR,
        );
        cache.fx_placement_textures.insert(
            key,
            FxPlacementTexture {
                texture,
                crop: [crop.x, crop.y, crop.width, crop.height],
                blend_mode: fx.blend_mode,
            },
        );
    }
    let Some(cached) = cache.fx_placement_textures.get(&key) else {
        return;
    };
    let [x, y, crop_width, crop_height] = cached.crop;
    let x0 = stage_rect.left() + x as f32 / width as f32 * stage_rect.width();
    let y0 = stage_rect.top() + y as f32 / height as f32 * stage_rect.height();
    let x1 = stage_rect.left() + (x + crop_width) as f32 / width as f32 * stage_rect.width();
    let y1 = stage_rect.top() + (y + crop_height) as f32 / height as f32 * stage_rect.height();
    if !inherited_color.is_identity() {
        let mut mesh = Mesh::with_texture(cached.texture.id());
        mesh.add_rect_with_uv(
            Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)),
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        q0glblend::paint_vector_group_on_opaque_backdrop(
            painter,
            vec![q0glblend::VectorGroupPrimitive::Textured {
                mesh: std::sync::Arc::new(mesh),
                color_transform: inherited_color,
            }],
            cached.blend_mode,
            1.0,
        );
        return;
    }
    q0glblend::paint_texture(
        painter,
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)),
        cached.texture.id(),
        cached.blend_mode,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_q0rg(
    painter: &Painter,
    ctx: &Context,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    view: &StageView,
    depth: u8,
    cache: &mut TextureCache,
    rig_overrides: &q0s_format::rig::RigRuntimeOverrides,
    scene_state: &RuntimeSceneState,
    inherited_color: q0glblend::ColorTransform,
) {
    if depth > Q0RG_RECURSION_LIMIT {
        return;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
        return;
    };
    let local_frame = if q.frame_count > 0 {
        frame % q.frame_count
    } else {
        0
    };
    let rig_pose = q0s_format::rig::rig_for_q0rg(project, q0rg_id).map(|rig| {
        let overrides = rig_overrides
            .get(&q0rg_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        q0s_format::rig::evaluate_rig(rig, f32::from(local_frame), overrides)
    });
    // Lights shine on the root timeline, after its light layer.
    let stage_rect = Rect::from_min_size(
        view.origin,
        egui::vec2(
            f32::from(project.meta.stage_width.max(1)) * view.scale,
            f32::from(project.meta.stage_height.max(1)) * view.scale,
        ),
    );
    let lighting = (depth == 0)
        .then(|| q0s_format::raster::frame_lighting(project, q0rg_id, local_frame, scene_state))
        .flatten()
        .and_then(|lighting| {
            let to_screen = Affine::compose(
                Affine {
                    a11: view.scale,
                    a12: 0.0,
                    a21: 0.0,
                    a22: view.scale,
                    tx: view.origin.x,
                    ty: view.origin.y,
                },
                parent,
            );
            let back = to_screen.inverse()?;
            Some(q0glblend::LightingPaint {
                screen_to_world: [[back.a11, back.a12, back.tx], [back.a21, back.a22, back.ty]],
                lighting,
            })
        });
    let mut lit = lighting.is_none();
    let mut previous_layer = None;
    for layer in &q.layers {
        if let Some(paint) = lighting.as_ref().filter(|_| !lit) {
            if previous_layer.is_some() && previous_layer == paint.lighting.layer_id {
                q0glblend::paint_lighting(painter, stage_rect, paint.clone());
                lit = true;
            }
        }
        previous_layer = Some(layer.layer_id);
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        // A mask draws nothing itself; it cuts the layers it masks.
        if is_mask_layer(project, q0rg_id, layer.layer_id) {
            continue;
        }
        if rig_pose.is_none()
            && !project
                .layer_mask_sources(q0rg_id, layer.layer_id)
                .is_empty()
        {
            if let Some((masks, placements)) = collect_masked_layer(
                project,
                Some(ctx),
                q,
                layer,
                local_frame,
                parent,
                view,
                depth,
                cache,
                scene_state,
                inherited_color,
            ) {
                q0glblend::paint_vector_masked_placements_on_opaque_backdrop(
                    painter, masks, placements,
                );
            }
            continue;
        }
        let shape_morph = q0s_format::shape_tween::project_layer_morph(project, layer, local_frame);
        for active in active_placement_states_at(layer, local_frame) {
            let Some(placement) = layer.placements.get(active.index) else {
                continue;
            };
            let authored_affine = rig_pose
                .as_ref()
                .and_then(|pose| pose.binding_transform(placement.instance_id))
                .unwrap_or_else(|| Affine::from_transform(active.transform));
            let key = InstanceKey::new(q0rg_id, placement.instance_id);
            let local_affine = scene_state.effective_affine(key, authored_affine);
            // Runtime scene state participates in the same affine composition
            // as authored/rig transforms, so the crisp GUI renderer and the
            // shared raster renderer cannot disagree about where an instance is.
            let composed = Affine::compose(parent, local_affine);
            let mut visual_fx = active.fx;
            visual_fx.tint = None;
            let color_transform = active
                .fx
                .tint
                .map(q0glblend::ColorTransform::tint)
                .unwrap_or_default()
                .then(inherited_color);
            if let Some(shapes) = shape_morph
                .as_ref()
                .and_then(|morph| morph.for_placement(active.index))
            {
                // In-between shapes carry tint and opacity; other placement
                // effects apply to the two authored keyframes only.
                for mesh in
                    morphed_meshes(shapes, composed, view, color_transform, active.fx.opacity)
                {
                    painter.add(Shape::Mesh(mesh));
                }
                continue;
            }
            if !visual_fx.is_identity() {
                let mut plain_group = visual_fx;
                plain_group.opacity = 1.0;
                plain_group.blend_mode = BlendMode::Normal;
                if plain_group.is_identity() {
                    let mut meshes = Vec::new();
                    let collected = match placement.target {
                        Target::Q0rg(id) => {
                            let fallback = q0s_format::v2::resolve_q0rg_playback_frame(
                                project,
                                id,
                                local_frame,
                                active.playback_start_frame,
                                active.fx.q0rg_playback,
                            );
                            let (child_frame, child_scene) =
                                scene_state.child_context(key, fallback);
                            collect_solid_q0rg(
                                project,
                                id,
                                child_frame,
                                composed,
                                view,
                                depth + 1,
                                cache,
                                child_scene,
                                color_transform,
                                &mut meshes,
                            )
                        }
                        Target::Asset(id) => {
                            if let Some(Asset::Vector(vector)) =
                                project.assets.iter().find(|asset| asset.id() == id)
                            {
                                if solid_appearance(project.asset_appearances.get(&id))
                                    && (vector.stroke.is_none() || similarity(composed))
                                {
                                    meshes = solid_meshes(
                                        vector,
                                        composed,
                                        view,
                                        cache,
                                        color_transform,
                                    );
                                    true
                                } else {
                                    false
                                }
                            } else {
                                false
                            }
                        }
                    };
                    if collected {
                        let rect = meshes
                            .iter()
                            .fold(Rect::NOTHING, |rect, mesh| rect.union(mesh.calc_bounds()));
                        q0glblend::paint_solid_mesh_group_with_opacity_on_opaque_backdrop(
                            painter,
                            rect,
                            meshes,
                            active.fx.blend_mode,
                            active.fx.opacity,
                        );
                        continue;
                    }
                }
                paint_fx_placement(
                    painter,
                    ctx,
                    project,
                    q0rg_id,
                    layer.layer_id,
                    active.index,
                    placement,
                    local_frame,
                    composed,
                    active.fx,
                    view,
                    cache,
                    inherited_color,
                );
                continue;
            }
            match placement.target {
                Target::Asset(asset_id) => {
                    if let Some(asset) = project.assets.iter().find(|a| a.id() == asset_id) {
                        paint_asset(
                            painter,
                            ctx,
                            asset,
                            project.asset_appearances.get(&asset_id),
                            composed,
                            view,
                            cache,
                            local_frame.saturating_sub(active.playback_start_frame),
                            project.meta.fps,
                            color_transform,
                        );
                    }
                }
                Target::Q0rg(child_id) if child_id != q0rg_id => {
                    let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child_id,
                        local_frame,
                        active.playback_start_frame,
                        active.fx.q0rg_playback,
                    );
                    let (child_frame, child_scene) = scene_state.child_context(key, child_frame);
                    let child_rig = if child_scene.rig_overrides.is_empty() {
                        rig_overrides
                    } else {
                        &child_scene.rig_overrides
                    };
                    paint_q0rg(
                        painter,
                        ctx,
                        project,
                        child_id,
                        child_frame,
                        composed,
                        view,
                        depth + 1,
                        cache,
                        child_rig,
                        child_scene,
                        color_transform,
                    );
                }
                _ => {}
            }
        }
    }
    if let Some(paint) = lighting.filter(|_| !lit) {
        q0glblend::paint_lighting(painter, stage_rect, paint);
    }
}

/// A bitmap's texture. Bitmaps never change during playback, so each is
/// uploaded once.
fn bitmap_texture(
    cache: &mut TextureCache,
    ctx: &Context,
    b: &q0s_format::v2::BitmapAsset,
) -> TextureHandle {
    cache
        .by_asset_id
        .entry(b.asset_id)
        .or_insert_with(|| {
            let image = ColorImage::from_rgba_unmultiplied(
                [usize::from(b.width), usize::from(b.height)],
                &b.rgba,
            );
            ctx.load_texture(
                format!("q0player_asset_{}", b.asset_id),
                image,
                TextureOptions::LINEAR,
            )
        })
        .clone()
}

/// The texture of the video frame `elapsed_host_frames` into the clip, with
/// the clip's size.
fn video_frame_texture(
    cache: &mut TextureCache,
    ctx: &Context,
    v: &q0s_format::v2::Q0vAsset,
    elapsed_host_frames: u16,
    host_fps: u16,
) -> Option<(TextureHandle, f32, f32)> {
    let source = std::sync::Arc::as_ptr(&v.bytes) as usize;
    if let std::collections::hash_map::Entry::Vacant(entry) = cache.q0v_media.entry(source) {
        let media = q0video::q0v::Q0vFile::parse_shared(std::sync::Arc::clone(&v.bytes)).ok()?;
        entry.insert(media);
    }
    let media = cache.q0v_media.get(&source)?;
    let frame_index = media
        .spec
        .video_frame_for_host_frame(u32::from(elapsed_host_frames), u32::from(host_fps.max(1)))?;
    let size = (media.spec.width as f32, media.spec.height as f32);
    let key = (source, frame_index as u32);
    if let std::collections::hash_map::Entry::Vacant(entry) = cache.by_q0v_frame.entry(key) {
        let rgba = media.decode_frame_rgba(frame_index).ok()?;
        let image = ColorImage::from_rgba_unmultiplied(
            [media.spec.width as usize, media.spec.height as usize],
            &rgba,
        );
        let bytes = image.pixels.len() * std::mem::size_of::<Color32>();
        let texture = ctx.load_texture(
            format!("q0player_q0v_{}_{}", source, frame_index),
            image,
            TextureOptions::LINEAR,
        );
        entry.insert(VideoTexture {
            texture,
            bytes,
            last_used: ctx.frame_nr(),
        });
    }
    let cached = cache.by_q0v_frame.get_mut(&key)?;
    cached.last_used = ctx.frame_nr();
    let texture = cached.texture.clone();
    let mut bytes: usize = cache.by_q0v_frame.values().map(|frame| frame.bytes).sum();
    while bytes > 128 * 1024 * 1024 || cache.by_q0v_frame.len() > 64 {
        let oldest = cache
            .by_q0v_frame
            .iter()
            .filter(|(_, frame)| frame.last_used != ctx.frame_nr())
            .min_by_key(|(_, frame)| frame.last_used)
            .map(|(key, _)| *key);
        let Some(oldest) = oldest else { break };
        if let Some(frame) = cache.by_q0v_frame.remove(&oldest) {
            bytes = bytes.saturating_sub(frame.bytes);
        }
    }
    Some((texture, size.0, size.1))
}

/// A `w` x `h` image placed on the stage by `transform`, as a screen mesh.
fn image_quad(
    texture: egui::TextureId,
    w: f32,
    h: f32,
    transform: Affine,
    view: &StageView,
) -> Mesh {
    let mut mesh = Mesh::with_texture(texture);
    for (corner, uv) in [
        (Vec2::new(0.0, 0.0), pos2(0.0, 0.0)),
        (Vec2::new(w, 0.0), pos2(1.0, 0.0)),
        (Vec2::new(w, h), pos2(1.0, 1.0)),
        (Vec2::new(0.0, h), pos2(0.0, 1.0)),
    ] {
        mesh.vertices.push(Vertex {
            pos: stage_to_screen(transform.apply(corner), view),
            uv,
            color: Color32::WHITE,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    mesh
}

fn is_mask_layer(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> bool {
    project.layer_metadata(q0rg_id, layer_id).kind == q0s_format::v2::LayerKind::Mask
}

/// Solid meshes of one placement: plain vector artwork, a shape tween's
/// in-between, or a symbol holding only those. False when it holds anything
/// else.
#[allow(clippy::too_many_arguments)]
fn collect_solid_placement(
    project: &ProjectV2,
    q0rg_id: u16,
    placement: &Placement,
    active: &q0s_format::raster::ActivePlacementState,
    morph: Option<&q0s_format::shape_tween::LayerMorph>,
    local_frame: u16,
    transform: Affine,
    view: &StageView,
    depth: u8,
    cache: &mut TextureCache,
    scene_state: &RuntimeSceneState,
    color: q0glblend::ColorTransform,
    meshes: &mut Vec<Mesh>,
) -> bool {
    if let Some(shapes) = morph.and_then(|morph| morph.for_placement(active.index)) {
        meshes.extend(morphed_meshes(shapes, transform, view, color, 1.0));
        return true;
    }
    match placement.target {
        Target::Asset(id) => {
            let Some(Asset::Vector(vector)) = project.assets.iter().find(|asset| asset.id() == id)
            else {
                return false;
            };
            if !solid_appearance(project.asset_appearances.get(&id))
                || (vector.stroke.is_some() && !similarity(transform))
            {
                return false;
            }
            meshes.extend(solid_meshes(vector, transform, view, cache, color));
            true
        }
        Target::Q0rg(id) => {
            let fallback = q0s_format::v2::resolve_q0rg_playback_frame(
                project,
                id,
                local_frame,
                active.playback_start_frame,
                active.fx.q0rg_playback,
            );
            let key = InstanceKey::new(q0rg_id, placement.instance_id);
            let (child_frame, child_scene) = scene_state.child_context(key, fallback);
            collect_solid_q0rg(
                project,
                id,
                child_frame,
                transform,
                view,
                depth + 1,
                cache,
                child_scene,
                color,
                meshes,
            )
        }
    }
}

type MaskedLayer = (
    Vec<Vec<q0glblend::VectorGroupPrimitive>>,
    Vec<q0glblend::VectorMaskedPlacement>,
);

/// `layer` and the masks over it, ready for the GPU to cut, or `None` when
/// something in them needs the CPU renderer. Without a `ctx` nothing is
/// decoded or uploaded: the call only checks that the layer can be drawn so.
#[allow(clippy::too_many_arguments)]
fn collect_masked_layer(
    project: &ProjectV2,
    ctx: Option<&Context>,
    q: &q0s_format::v2::Q0rg,
    layer: &q0s_format::v2::Layer,
    local_frame: u16,
    parent: Affine,
    view: &StageView,
    depth: u8,
    cache: &mut TextureCache,
    scene_state: &RuntimeSceneState,
    inherited_color: q0glblend::ColorTransform,
) -> Option<MaskedLayer> {
    let q0rg_id = q.q0rg_id;
    let placed = |placement: &Placement, active: &q0s_format::raster::ActivePlacementState| {
        let key = InstanceKey::new(q0rg_id, placement.instance_id);
        Affine::compose(
            parent,
            scene_state.effective_affine(key, Affine::from_transform(active.transform)),
        )
    };

    let mut masks = Vec::new();
    for mask_id in project.layer_mask_sources(q0rg_id, layer.layer_id) {
        let mask_layer = q.layers.iter().find(|layer| layer.layer_id == mask_id)?;
        if !project.layer_is_visible(q0rg_id, mask_id) || !is_mask_layer(project, q0rg_id, mask_id)
        {
            return None;
        }
        let morph = q0s_format::shape_tween::project_layer_morph(project, mask_layer, local_frame);
        let mut meshes = Vec::new();
        for active in active_placement_states_at(mask_layer, local_frame) {
            let placement = mask_layer.placements.get(active.index)?;
            let mut visual = active.fx;
            visual.tint = None;
            if !visual.is_identity()
                || !collect_solid_placement(
                    project,
                    q0rg_id,
                    placement,
                    &active,
                    morph.as_ref(),
                    local_frame,
                    placed(placement, &active),
                    view,
                    depth,
                    cache,
                    scene_state,
                    q0glblend::ColorTransform::default(),
                    &mut meshes,
                )
            {
                return None;
            }
        }
        masks.push(
            meshes
                .into_iter()
                .map(q0glblend::VectorGroupPrimitive::Solid)
                .collect(),
        );
    }

    let morph = q0s_format::shape_tween::project_layer_morph(project, layer, local_frame);
    let mut placements = Vec::new();
    for active in active_placement_states_at(layer, local_frame) {
        let placement = layer.placements.get(active.index)?;
        // Opacity and blending travel with the masked placement; filters
        // and the like need the CPU renderer.
        let mut plain = active.fx;
        plain.tint = None;
        plain.opacity = 1.0;
        plain.blend_mode = BlendMode::Normal;
        if !plain.is_identity() {
            return None;
        }
        let transform = placed(placement, &active);
        let color = active
            .fx
            .tint
            .map(q0glblend::ColorTransform::tint)
            .unwrap_or_default()
            .then(inherited_color);
        let mut primitives = Vec::new();
        let image = match placement.target {
            Target::Asset(id) => match project.assets.iter().find(|asset| asset.id() == id) {
                Some(Asset::Bitmap(b)) => Some(ctx.map(|ctx| {
                    let texture = bitmap_texture(cache, ctx, b);
                    Some((texture, f32::from(b.width), f32::from(b.height)))
                })),
                Some(Asset::Q0v(v)) => Some(ctx.map(|ctx| {
                    video_frame_texture(
                        cache,
                        ctx,
                        v,
                        local_frame.saturating_sub(active.playback_start_frame),
                        project.meta.fps,
                    )
                })),
                _ => None,
            },
            Target::Q0rg(_) => None,
        };
        match image {
            Some(texture) => {
                if let Some((texture, w, h)) = texture.flatten() {
                    primitives.push(q0glblend::VectorGroupPrimitive::Textured {
                        mesh: std::sync::Arc::new(image_quad(texture.id(), w, h, transform, view)),
                        color_transform: color,
                    });
                }
            }
            None => {
                let mut meshes = Vec::new();
                if !collect_solid_placement(
                    project,
                    q0rg_id,
                    placement,
                    &active,
                    morph.as_ref(),
                    local_frame,
                    transform,
                    view,
                    depth,
                    cache,
                    scene_state,
                    color,
                    &mut meshes,
                ) {
                    return None;
                }
                primitives.extend(
                    meshes
                        .into_iter()
                        .map(q0glblend::VectorGroupPrimitive::Solid),
                );
            }
        }
        placements.push(q0glblend::VectorMaskedPlacement {
            primitives,
            blend_mode: active.fx.blend_mode,
            opacity: active.fx.opacity.clamp(0.0, 1.0),
        });
    }
    Some((masks, placements))
}

/// Whether every masked layer this frame shows can be cut on the GPU.
#[allow(clippy::too_many_arguments)]
fn gpu_masks_supported(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    view: &StageView,
    depth: u8,
    cache: &mut TextureCache,
    scene_state: &RuntimeSceneState,
) -> bool {
    if depth > Q0RG_RECURSION_LIMIT {
        return true;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
        return true;
    };
    let local_frame = if q.frame_count > 0 {
        frame % q.frame_count
    } else {
        0
    };
    let rigged = q0s_format::rig::rig_for_q0rg(project, q0rg_id).is_some();
    for layer in &q.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || is_mask_layer(project, q0rg_id, layer.layer_id)
        {
            continue;
        }
        if !project
            .layer_mask_sources(q0rg_id, layer.layer_id)
            .is_empty()
        {
            if rigged
                || collect_masked_layer(
                    project,
                    None,
                    q,
                    layer,
                    local_frame,
                    parent,
                    view,
                    depth,
                    cache,
                    scene_state,
                    q0glblend::ColorTransform::default(),
                )
                .is_none()
            {
                return false;
            }
            continue;
        }
        for active in active_placement_states_at(layer, local_frame) {
            let Some(placement) = layer.placements.get(active.index) else {
                continue;
            };
            let Target::Q0rg(child_id) = placement.target else {
                continue;
            };
            let mut visual = active.fx;
            visual.tint = None;
            // Filtered symbols are drawn whole by their own renderer.
            if child_id == q0rg_id || !visual.is_identity() {
                continue;
            }
            let key = InstanceKey::new(q0rg_id, placement.instance_id);
            let fallback = q0s_format::v2::resolve_q0rg_playback_frame(
                project,
                child_id,
                local_frame,
                active.playback_start_frame,
                active.fx.q0rg_playback,
            );
            let (child_frame, child_scene) = scene_state.child_context(key, fallback);
            let transform = Affine::compose(
                parent,
                scene_state.effective_affine(key, Affine::from_transform(active.transform)),
            );
            if !gpu_masks_supported(
                project,
                child_id,
                child_frame,
                transform,
                view,
                depth + 1,
                cache,
                child_scene,
            ) {
                return false;
            }
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn paint_asset(
    painter: &Painter,
    ctx: &Context,
    asset: &Asset,
    appearance: Option<&VectorAppearance>,
    transform: Affine,
    view: &StageView,
    cache: &mut TextureCache,
    elapsed_host_frames: u16,
    host_fps: u16,
    color_transform: q0glblend::ColorTransform,
) {
    match asset {
        Asset::Bitmap(b) => {
            let texture = bitmap_texture(cache, ctx, b);
            let (w, h) = (f32::from(b.width), f32::from(b.height));
            let mesh = image_quad(texture.id(), w, h, transform, view);
            q0glblend::paint_mesh_color(painter, mesh, color_transform);
        }
        Asset::Q0v(v) => {
            let Some((texture, w, h)) =
                video_frame_texture(cache, ctx, v, elapsed_host_frames, host_fps)
            else {
                return;
            };
            let mesh = image_quad(texture.id(), w, h, transform, view);
            q0glblend::paint_mesh_color(painter, mesh, color_transform);
        }
        Asset::Rig(_) => {}
        Asset::Vector(v) => {
            if solid_appearance(appearance) && (v.stroke.is_none() || similarity(transform)) {
                for mesh in solid_meshes(v, transform, view, cache, color_transform) {
                    painter.add(Shape::Mesh(mesh));
                }
                return;
            }
            // Fill every closed contour in one tessellation pass. A vector asset
            // can contain an exterior plus oppositely-wound hole contours; filling
            // each PathShape independently destroys those holes. egui 0.27 also
            // assumes PathShape fills are convex, which produces the old giant
            // diagonal "lasso" fans for concave brush graphics.
            let local_contours: Vec<Vec<Pos2>> = v
                .paths
                .iter()
                .filter(|path| path.closed)
                .map(|path| {
                    flatten_path(path, BEZIER_SAMPLES)
                        .iter()
                        .map(|point| Pos2::new(point.x, point.y))
                        .collect()
                })
                .collect();
            if let Some(appearance) = appearance {
                if let Some(gradient) = appearance.fill_gradient.as_ref() {
                    paint_complex_gradient_color(
                        painter,
                        &local_contours,
                        gradient,
                        appearance.field_transform,
                        transform,
                        view,
                        color_transform,
                    );
                } else {
                    let fill_color = v
                        .fill
                        .map(|color| color_transform.apply(rgba_to_color32(color)))
                        .unwrap_or(Color32::TRANSPARENT);
                    if fill_color != Color32::TRANSPARENT {
                        let contours: Vec<Vec<Pos2>> = local_contours
                            .iter()
                            .map(|contour| {
                                contour
                                    .iter()
                                    .map(|point| {
                                        stage_to_screen(
                                            transform.apply(Vec2::new(point.x, point.y)),
                                            view,
                                        )
                                    })
                                    .collect()
                            })
                            .collect();
                        paint_complex_fill(painter, &contours, fill_color);
                    }
                }
            } else {
                let fill_color = v
                    .fill
                    .map(|color| color_transform.apply(rgba_to_color32(color)))
                    .unwrap_or(Color32::TRANSPARENT);
                if fill_color != Color32::TRANSPARENT {
                    let contours: Vec<Vec<Pos2>> = local_contours
                        .iter()
                        .map(|contour| {
                            contour
                                .iter()
                                .map(|point| {
                                    stage_to_screen(
                                        transform.apply(Vec2::new(point.x, point.y)),
                                        view,
                                    )
                                })
                                .collect()
                        })
                        .collect();
                    paint_complex_fill(painter, &contours, fill_color);
                }
            }

            // Stroke width is logical (stage units). Multiply by the
            // composed transform's area scale so a 2× scaled placement
            // shows 2×-thick outlines, then by view.scale for the
            // stage→screen mapping.
            let parent_scale = transform.uniform_scale();
            for path in &v.paths {
                let mut polyline_local = flatten_path(path, BEZIER_SAMPLES);
                if path.closed {
                    drop_duplicate_closing_point_vec2(&mut polyline_local);
                }
                if polyline_local.len() < 2 {
                    continue;
                }
                let polyline_screen: Vec<Pos2> = polyline_local
                    .iter()
                    .map(|p| stage_to_screen(transform.apply(*p), view))
                    .collect();
                let stroke = match &v.stroke {
                    Some(s) => Stroke::new(
                        s.width * parent_scale * view.scale,
                        rgba_to_color32(s.color),
                    ),
                    None => Stroke::NONE,
                };
                let shape = v.stroke.map_or(
                    (
                        q0s_format::geom::CapShape::Round,
                        q0s_format::geom::JoinShape::Bevel,
                    ),
                    |s| (s.cap, s.join),
                );
                if stroke != Stroke::NONE {
                    if let Some(mesh) = styled_stroke_mesh(
                        &polyline_screen,
                        stroke.width,
                        path.closed,
                        stroke.color,
                        shape.0,
                        shape.1,
                    ) {
                        q0glblend::paint_mesh_color(painter, mesh, color_transform);
                    }
                }
                // Round caps via end-circles — egui's stroke is butt by
                // default. Mirrors the editor render so editor and
                // playback are pixel-identical.
                if let Some(s) = &v.stroke {
                    if !path.closed
                        && polyline_screen.len() >= 2
                        && matches!(s.cap, q0s_format::geom::CapShape::Round)
                    {
                        let r = s.width * parent_scale * view.scale * 0.5;
                        let col = color_transform.apply(rgba_to_color32(s.color));
                        if let Some(p) = polyline_screen.first() {
                            painter.circle_filled(*p, r, col);
                        }
                        if let Some(p) = polyline_screen.last() {
                            painter.circle_filled(*p, r, col);
                        }
                    }
                }
            }
        }
    }
}

fn paint_complex_gradient_color(
    painter: &Painter,
    contours: &[Vec<Pos2>],
    gradient: &GradientFill,
    field_transform: Affine,
    transform: Affine,
    view: &StageView,
    color_transform: q0glblend::ColorTransform,
) {
    let mut effective = gradient.clone();
    effective.transform = Affine::compose(field_transform, gradient.transform);
    let gradient = &effective;
    let Some(buffers) = tessellate_complex_fill(contours) else {
        return;
    };
    let mut mesh = Mesh::default();
    for triangle in buffers.indices.chunks_exact(3) {
        let local = [
            Vec2::new(
                buffers.vertices[triangle[0] as usize].x,
                buffers.vertices[triangle[0] as usize].y,
            ),
            Vec2::new(
                buffers.vertices[triangle[1] as usize].x,
                buffers.vertices[triangle[1] as usize].y,
            ),
            Vec2::new(
                buffers.vertices[triangle[2] as usize].x,
                buffers.vertices[triangle[2] as usize].y,
            ),
        ];
        emit_gradient_triangle(&mut mesh, local, gradient, transform, view, 0);
    }
    if !mesh.indices.is_empty() {
        q0glblend::paint_mesh_color(painter, mesh, color_transform);
    }
}

fn gradient_color_error(actual: Rgba, expected: Rgba) -> u8 {
    [
        actual.r.abs_diff(expected.r),
        actual.g.abs_diff(expected.g),
        actual.b.abs_diff(expected.b),
        actual.a.abs_diff(expected.a),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

fn average_gradient_color(a: Rgba, b: Rgba) -> Rgba {
    let avg = |left: u8, right: u8| ((u16::from(left) + u16::from(right)) / 2) as u8;
    Rgba {
        r: avg(a.r, b.r),
        g: avg(a.g, b.g),
        b: avg(a.b, b.b),
        a: avg(a.a, b.a),
    }
}

fn emit_gradient_triangle(
    mesh: &mut Mesh,
    triangle: [Vec2; 3],
    gradient: &GradientFill,
    transform: Affine,
    view: &StageView,
    depth: u8,
) {
    let colors = triangle.map(|point| gradient.sample(point));
    let ab = Vec2::new(
        (triangle[0].x + triangle[1].x) * 0.5,
        (triangle[0].y + triangle[1].y) * 0.5,
    );
    let bc = Vec2::new(
        (triangle[1].x + triangle[2].x) * 0.5,
        (triangle[1].y + triangle[2].y) * 0.5,
    );
    let ca = Vec2::new(
        (triangle[2].x + triangle[0].x) * 0.5,
        (triangle[2].y + triangle[0].y) * 0.5,
    );
    let error = [
        gradient_color_error(
            gradient.sample(ab),
            average_gradient_color(colors[0], colors[1]),
        ),
        gradient_color_error(
            gradient.sample(bc),
            average_gradient_color(colors[1], colors[2]),
        ),
        gradient_color_error(
            gradient.sample(ca),
            average_gradient_color(colors[2], colors[0]),
        ),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    if error > 3 && depth < 5 {
        emit_gradient_triangle(
            mesh,
            [triangle[0], ab, ca],
            gradient,
            transform,
            view,
            depth + 1,
        );
        emit_gradient_triangle(
            mesh,
            [ab, triangle[1], bc],
            gradient,
            transform,
            view,
            depth + 1,
        );
        emit_gradient_triangle(
            mesh,
            [ca, bc, triangle[2]],
            gradient,
            transform,
            view,
            depth + 1,
        );
        emit_gradient_triangle(mesh, [ab, bc, ca], gradient, transform, view, depth + 1);
        return;
    }
    let Ok(base) = u32::try_from(mesh.vertices.len()) else {
        return;
    };
    for (point, color) in triangle.into_iter().zip(colors) {
        mesh.vertices.push(Vertex {
            pos: stage_to_screen(transform.apply(point), view),
            uv: Pos2::ZERO,
            color: rgba_to_color32(color),
        });
    }
    mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
}

/// Draws what live physics and particles add over the movie: liquid
/// surfaces and particles, in the same stage space the frame was painted in.
#[allow(clippy::too_many_arguments)]
pub fn paint_live_draws(
    painter: &Painter,
    ctx: &Context,
    project: &ProjectV2,
    target_rect: Rect,
    cache: &mut TextureCache,
    live: &crate::LiveDraws,
    rig_overrides: &q0s_format::rig::RigRuntimeOverrides,
    scene_state: &RuntimeSceneState,
) {
    use q0s_format::particles::ParticleLook;

    let stage_w = project.meta.stage_width.max(1) as f32;
    let stage_h = project.meta.stage_height.max(1) as f32;
    let scale = (target_rect.width() / stage_w)
        .min(target_rect.height() / stage_h)
        .max(0.001);
    let drawn = egui::vec2(stage_w * scale, stage_h * scale);
    let origin = pos2(
        target_rect.center().x - drawn.x * 0.5,
        target_rect.center().y - drawn.y * 0.5,
    );
    let clipped = painter.with_clip_rect(
        painter
            .clip_rect()
            .intersect(Rect::from_min_size(origin, drawn)),
    );
    let painter = &clipped;
    let view = StageView { origin, scale };
    let to_screen = |x: f32, y: f32| pos2(origin.x + x * scale, origin.y + y * scale);
    let color_of = |rgba: Rgba| Color32::from_rgba_unmultiplied(rgba.r, rgba.g, rgba.b, rgba.a);

    for (outlines, color) in &live.liquids {
        let contours = outlines
            .iter()
            .map(|outline| outline.iter().map(|p| to_screen(p[0], p[1])).collect())
            .collect::<Vec<Vec<Pos2>>>();
        paint_complex_fill(painter, &contours, color_of(*color));
    }

    for (strokes, color) in &live.links {
        for stroke in strokes {
            let points = stroke
                .points
                .iter()
                .map(|p| to_screen(p[0], p[1]))
                .collect::<Vec<_>>();
            if let Some(mesh) = styled_stroke_mesh(
                &points,
                stroke.width * scale,
                stroke.closed,
                color_of(*color),
                q0s_format::geom::CapShape::Round,
                q0s_format::geom::JoinShape::Round,
            ) {
                painter.add(Shape::mesh(mesh));
            }
            // The mesh leaves ends flat; round them like the baked drawings.
            if let (false, Some(first), Some(last)) = (stroke.closed, points.first(), points.last())
            {
                for end in [*first, *last] {
                    painter.circle_filled(end, stroke.width * scale * 0.5, color_of(*color));
                }
            }
        }
    }

    for particle in &live.particles {
        let style = particle.style;
        let [x, y] = particle.position;
        match particle.look {
            // Library items keep their own colours; at runtime they are
            // drawn at full opacity.
            ParticleLook::Item(target) => {
                let affine = Affine::from_transform(q0s_format::v2::Transform2D {
                    tx: x,
                    ty: y,
                    sx: (style.size * style.stretch).max(0.001),
                    sy: style.size.max(0.001),
                    rotation: style.rotation,
                    ..q0s_format::v2::Transform2D::IDENTITY
                });
                match target {
                    Target::Asset(id) => {
                        if let Some(asset) = project.assets.iter().find(|asset| asset.id() == id) {
                            paint_asset(
                                painter,
                                ctx,
                                asset,
                                project.asset_appearances.get(&id),
                                affine,
                                &view,
                                cache,
                                0,
                                project.meta.fps.max(1),
                                q0glblend::ColorTransform::default(),
                            );
                        }
                    }
                    Target::Q0rg(id) => paint_q0rg(
                        painter,
                        ctx,
                        project,
                        id,
                        0,
                        affine,
                        &view,
                        0,
                        cache,
                        rig_overrides,
                        scene_state,
                        q0glblend::ColorTransform::default(),
                    ),
                }
            }
            look => {
                painter.add(Shape::Mesh(particle_mesh(
                    look,
                    to_screen(x, y),
                    style,
                    scale,
                )));
            }
        }
    }
}

/// One built-in particle as a screen-space mesh. Additive particles carry
/// no alpha: under premultiplied blending that adds their light to what is
/// behind instead of covering it.
fn particle_mesh(
    look: q0s_format::particles::ParticleLook,
    centre: Pos2,
    style: q0physics::particles::ParticleStyle,
    scale: f32,
) -> Mesh {
    use q0s_format::particles::ParticleLook;

    let Rgba { r, g, b, a } = style.color;
    let paint = |strength: f32| -> Color32 {
        let alpha = (f32::from(a) * strength).round().clamp(0.0, 255.0) as u8;
        let color = Color32::from_rgba_unmultiplied(r, g, b, alpha);
        if style.additive {
            let [pr, pg, pb, _] = color.to_array();
            Color32::from_rgba_premultiplied(pr, pg, pb, 0)
        } else {
            color
        }
    };
    let half = style.size * 0.5 * scale;
    let stretch = style.stretch.max(0.01);
    let (sin, cos) = style.rotation.sin_cos();
    // Local units of half the particle's size, stretched along its x.
    let at = |x: f32, y: f32| {
        let (x, y) = (x * half * stretch, y * half);
        pos2(centre.x + cos * x - sin * y, centre.y + sin * x + cos * y)
    };
    let segments = ((half * stretch * 0.8) as usize).clamp(12, 48);
    let circle = |radius: f32| -> Vec<Pos2> {
        (0..segments)
            .map(|index| {
                let angle = index as f32 * std::f32::consts::TAU / segments as f32;
                at(angle.cos() * radius, angle.sin() * radius)
            })
            .collect()
    };
    let mut mesh = Mesh::default();
    match look {
        ParticleLook::Glow => {
            // The same soft falloff the editor bakes as a radial gradient.
            let rings = [(0.2, 0.9), (0.45, 0.55), (0.7, 0.2), (1.0, 0.0)];
            mesh.colored_vertex(centre, paint(1.0));
            let mut previous: Option<u32> = None;
            for (radius, strength) in rings {
                let first = mesh.vertices.len() as u32;
                for point in circle(radius) {
                    mesh.colored_vertex(point, paint(strength));
                }
                let n = segments as u32;
                for i in 0..n {
                    let j = (i + 1) % n;
                    match previous {
                        None => mesh.add_triangle(0, first + i, first + j),
                        Some(inner) => {
                            mesh.add_triangle(inner + i, first + i, first + j);
                            mesh.add_triangle(inner + i, first + j, inner + j);
                        }
                    }
                }
                previous = Some(first);
            }
        }
        ParticleLook::Ring => {
            let feather = 1.0 / half.max(1.0);
            let rings = [
                (0.72 - feather, 0.0),
                (0.72, 1.0),
                (1.0, 1.0),
                (1.0 + feather, 0.0),
            ];
            let n = segments as u32;
            for (index, (radius, strength)) in rings.into_iter().enumerate() {
                for point in circle(radius) {
                    mesh.colored_vertex(point, paint(strength));
                }
                if index > 0 {
                    let inner = (index as u32 - 1) * n;
                    let outer = index as u32 * n;
                    for i in 0..n {
                        let j = (i + 1) % n;
                        mesh.add_triangle(inner + i, outer + i, outer + j);
                        mesh.add_triangle(inner + i, outer + j, inner + j);
                    }
                }
            }
        }
        _ => {
            let outline = match look {
                ParticleLook::Square => {
                    vec![at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)]
                }
                ParticleLook::Star => (0..10)
                    .map(|index| {
                        let radius = if index % 2 == 0 { 1.0 } else { 0.4 };
                        let angle = -std::f32::consts::FRAC_PI_2
                            + index as f32 * std::f32::consts::PI / 5.0;
                        at(angle.cos() * radius, angle.sin() * radius)
                    })
                    .collect(),
                ParticleLook::Spark => {
                    vec![at(1.0, 0.0), at(0.0, 0.3), at(-1.0, 0.0), at(0.0, -0.3)]
                }
                _ => circle(1.0),
            };
            fan_with_feather(&mut mesh, centre, &outline, paint(1.0));
        }
    }
    mesh
}

/// Fills `outline`, which every ray from `centre` crosses once, with a
/// one-pixel fade around its edge for antialiasing.
fn fan_with_feather(mesh: &mut Mesh, centre: Pos2, outline: &[Pos2], color: Color32) {
    let n = outline.len() as u32;
    if n < 3 {
        return;
    }
    let base = mesh.vertices.len() as u32;
    mesh.colored_vertex(centre, color);
    for point in outline {
        mesh.colored_vertex(*point, color);
    }
    for point in outline {
        let away = *point - centre;
        let length = away.length().max(1.0e-3);
        mesh.colored_vertex(*point + away / length, Color32::TRANSPARENT);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let (edge_i, edge_j) = (base + 1 + i, base + 1 + j);
        let (soft_i, soft_j) = (base + 1 + n + i, base + 1 + n + j);
        mesh.add_triangle(base, edge_i, edge_j);
        mesh.add_triangle(edge_i, edge_j, soft_j);
        mesh.add_triangle(edge_i, soft_j, soft_i);
    }
}

fn paint_complex_fill(painter: &Painter, contours: &[Vec<Pos2>], color: Color32) {
    let Some(buffers) = tessellate_complex_fill(contours) else {
        return;
    };
    let mut mesh = Mesh::default();
    for point in buffers.vertices {
        mesh.vertices.push(Vertex {
            pos: Pos2::new(point.x, point.y),
            uv: Pos2::ZERO,
            color,
        });
    }
    mesh.indices = buffers.indices;
    painter.add(Shape::Mesh(mesh));
}

/// An artwork stroke: its corners as the stroke says, its ends flat or
/// square. Round ends are painted as circles by the callers, the same in the
/// editor and q0player.
fn styled_stroke_mesh(
    points: &[Pos2],
    width: f32,
    closed: bool,
    color: Color32,
    cap: q0s_format::geom::CapShape,
    join: q0s_format::geom::JoinShape,
) -> Option<Mesh> {
    if points.len() < 2 || !width.is_finite() || width <= 0.0 {
        return None;
    }
    let mut path_builder = lyon_path::Path::builder();
    path_builder.begin(lyon_point(points[0].x, points[0].y));
    for point in &points[1..] {
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        path_builder.line_to(lyon_point(point.x, point.y));
    }
    path_builder.end(closed);
    let path = path_builder.build();
    let line_join = match join {
        q0s_format::geom::JoinShape::Miter => LineJoin::Miter,
        q0s_format::geom::JoinShape::Round => LineJoin::Round,
        q0s_format::geom::JoinShape::Bevel => LineJoin::Bevel,
    };
    let line_cap = match cap {
        q0s_format::geom::CapShape::Square => LineCap::Square,
        _ => LineCap::Butt,
    };
    let options = StrokeOptions::default()
        .with_line_width(width)
        .with_line_join(line_join)
        .with_miter_limit(q0s_format::geom::MITER_LIMIT)
        .with_line_cap(line_cap)
        .with_tolerance(0.05);
    let mut buffers: VertexBuffers<lyon_path::math::Point, u32> = VertexBuffers::new();
    StrokeTessellator::new()
        .tessellate_path(
            &path,
            &options,
            &mut BuffersBuilder::new(&mut buffers, Positions),
        )
        .ok()?;
    let mut mesh = Mesh::default();
    for point in buffers.vertices {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: Pos2::new(point.x, point.y),
            uv: Pos2::ZERO,
            color,
        });
    }
    mesh.indices = buffers.indices;
    Some(mesh)
}

fn tessellate_complex_fill(
    contours: &[Vec<Pos2>],
) -> Option<VertexBuffers<lyon_path::math::Point, u32>> {
    if contours.is_empty() {
        return None;
    }
    let mut path_builder = lyon_path::Path::builder();
    let mut has_geometry = false;
    for points in contours {
        let mut points = points.clone();
        drop_duplicate_closing_point_pos2(&mut points);
        if points.len() < 3 {
            continue;
        }
        has_geometry = true;
        path_builder.begin(lyon_point(points[0].x, points[0].y));
        for point in &points[1..] {
            path_builder.line_to(lyon_point(point.x, point.y));
        }
        path_builder.end(true);
    }
    if !has_geometry {
        return None;
    }

    let path = path_builder.build();
    let mut buffers: VertexBuffers<lyon_path::math::Point, u32> = VertexBuffers::new();
    let mut tessellator = FillTessellator::new();
    let options = FillOptions::default().with_fill_rule(LyonFillRule::NonZero);
    tessellator
        .tessellate_path(
            &path,
            &options,
            &mut BuffersBuilder::new(&mut buffers, Positions),
        )
        .ok()?;
    Some(buffers)
}

fn drop_duplicate_closing_point_pos2(points: &mut Vec<Pos2>) {
    if points.len() > 1 && points[0].distance_sq(points[points.len() - 1]) <= 1.0e-8 {
        points.pop();
    }
}

fn drop_duplicate_closing_point_vec2(points: &mut Vec<Vec2>) {
    if points.len() > 1 {
        let first = points[0];
        let last = points[points.len() - 1];
        let dx = first.x - last.x;
        let dy = first.y - last.y;
        if dx * dx + dy * dy <= 1.0e-8 {
            points.pop();
        }
    }
}

fn stage_to_screen(p: Vec2, view: &StageView) -> Pos2 {
    pos2(
        view.origin.x + p.x * view.scale,
        view.origin.y + p.y * view.scale,
    )
}

fn rgba_to_color32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh_covers_point(
        buffers: &VertexBuffers<lyon_path::math::Point, u32>,
        point: Pos2,
    ) -> bool {
        buffers.indices.chunks_exact(3).any(|triangle| {
            let a = buffers.vertices[triangle[0] as usize];
            let b = buffers.vertices[triangle[1] as usize];
            let c = buffers.vertices[triangle[2] as usize];
            point_in_triangle(point, pos2(a.x, a.y), pos2(b.x, b.y), pos2(c.x, c.y))
        })
    }

    fn point_in_triangle(point: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
        fn cross(a: Pos2, b: Pos2, c: Pos2) -> f32 {
            (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
        }
        const EPSILON: f32 = 1.0e-4;
        let ab = cross(a, b, point);
        let bc = cross(b, c, point);
        let ca = cross(c, a, point);
        let has_negative = ab < -EPSILON || bc < -EPSILON || ca < -EPSILON;
        let has_positive = ab > EPSILON || bc > EPSILON || ca > EPSILON;
        !(has_negative && has_positive)
    }

    #[test]
    fn bounded_open_stroke_does_not_emit_unbounded_hairpin_vertices() {
        let points = vec![
            pos2(2.85, 16.10),
            pos2(2.85, 16.00),
            pos2(2.80, 15.95),
            pos2(2.80, 15.90),
            pos2(2.70, 15.55),
            pos2(4.55, 16.10),
        ];
        for join in [
            q0s_format::geom::JoinShape::Miter,
            q0s_format::geom::JoinShape::Round,
            q0s_format::geom::JoinShape::Bevel,
        ] {
            let mesh = styled_stroke_mesh(
                &points,
                1.0,
                false,
                Color32::WHITE,
                q0s_format::geom::CapShape::Butt,
                join,
            )
            .expect("bounded stroke");
            // A miter may reach its limit past a corner, then turns flat.
            let reach = if join == q0s_format::geom::JoinShape::Miter {
                q0s_format::geom::MITER_LIMIT * 0.5
            } else {
                0.7
            };
            assert!(
                mesh.vertices.iter().all(|v| v.pos.x > 2.7 - reach
                    && v.pos.x < 4.55 + reach
                    && v.pos.y > 15.55 - reach
                    && v.pos.y < 16.10 + reach),
                "{join:?} joins stay near the line"
            );
        }
    }

    #[test]
    fn radial_gradient_mesh_subdivides_and_samples_transparent_interior() {
        let gradient = GradientFill {
            kind: q0s_format::v2::GradientKind::Radial,
            stops: vec![
                q0s_format::v2::GradientStop {
                    position: 0.0,
                    color: Rgba {
                        r: 255,
                        g: 0,
                        b: 0,
                        a: 0,
                    },
                },
                q0s_format::v2::GradientStop {
                    position: 1.0,
                    color: Rgba {
                        r: 0,
                        g: 0,
                        b: 255,
                        a: 255,
                    },
                },
            ],
            transform: Affine::IDENTITY,
        };
        let view = StageView {
            origin: pos2(0.0, 0.0),
            scale: 1.0,
        };
        let mut mesh = Mesh::default();
        emit_gradient_triangle(
            &mut mesh,
            [
                Vec2::new(-1.0, -1.0),
                Vec2::new(1.0, -1.0),
                Vec2::new(0.0, 1.0),
            ],
            &gradient,
            Affine::IDENTITY,
            &view,
            0,
        );
        assert!(
            mesh.vertices.len() > 3,
            "player radial gradient must subdivide curved colour space instead of interpolating only triangle corners"
        );
        assert!(
            mesh.vertices.iter().any(|vertex| vertex.color.a() < 240),
            "player mesh must sample transparent interior gradient stops"
        );
        assert_eq!(mesh.indices.len() % 3, 0);
    }

    #[test]
    fn complex_fill_preserves_oppositely_wound_hole_contour() {
        let exterior = vec![
            pos2(0.0, 0.0),
            pos2(100.0, 0.0),
            pos2(100.0, 100.0),
            pos2(0.0, 100.0),
        ];
        let hole = vec![
            pos2(25.0, 25.0),
            pos2(25.0, 75.0),
            pos2(75.0, 75.0),
            pos2(75.0, 25.0),
        ];
        let buffers = tessellate_complex_fill(&[exterior, hole]).expect("hole tessellation");

        assert!(mesh_covers_point(&buffers, pos2(10.0, 10.0)));
        assert!(!mesh_covers_point(&buffers, pos2(50.0, 50.0)));
    }

    #[test]
    fn concave_fill_does_not_draw_lasso_triangle_across_cutout() {
        let u_shape = vec![
            pos2(0.0, 0.0),
            pos2(100.0, 0.0),
            pos2(100.0, 100.0),
            pos2(60.0, 100.0),
            pos2(60.0, 40.0),
            pos2(40.0, 40.0),
            pos2(40.0, 100.0),
            pos2(0.0, 100.0),
        ];
        let buffers = tessellate_complex_fill(&[u_shape]).expect("concave tessellation");

        assert!(mesh_covers_point(&buffers, pos2(20.0, 80.0)));
        assert!(!mesh_covers_point(&buffers, pos2(50.0, 80.0)));
    }

    #[test]
    fn closed_stroke_drops_duplicate_seam_point() {
        let mut points = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 0.0),
        ];
        drop_duplicate_closing_point_vec2(&mut points);
        assert_eq!(points.len(), 3);
        assert_ne!(points.first(), points.last());
    }
    #[test]
    fn interactive_player_renderer_applies_runtime_scene_overlay_to_nested_q0rg() {
        use q0s_format::runtime_scene::RuntimeSceneState;
        use q0s_format::v2::{
            BitmapAsset, InstanceKey, Layer, Placement, ProjectMeta, Q0rg, Transform2D, Tween,
        };

        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "runtime-gui-overlay".into(),
                fps: 24,
                stage_width: 16,
                stage_height: 16,
                background_color: q0s_format::v2::Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                },
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Bitmap(BitmapAsset {
                asset_id: 1,
                width: 2,
                height: 2,
                rgba: [255, 0, 0, 255].repeat(4),
            })],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![
                Q0rg {
                    q0rg_id: 1,
                    name: "Stage".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        timeline_frame_count: None,
                        name: "player".into(),
                        explicit_keyframes: vec![0],
                        placements: vec![Placement {
                            pivot: None,
                            instance_id: 7,
                            frame: 0,
                            target: Target::Q0rg(2),
                            transform: Transform2D {
                                tx: 2.0,
                                ty: 3.0,
                                ..Transform2D::IDENTITY
                            },
                            tween: Tween::None,
                            fx: Default::default(),
                        }],
                    }],
                },
                Q0rg {
                    q0rg_id: 2,
                    name: "PlayerSymbol".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 2,
                        timeline_frame_count: None,
                        name: "body".into(),
                        explicit_keyframes: vec![0],
                        placements: vec![Placement {
                            pivot: None,
                            instance_id: 8,
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
                        }],
                    }],
                },
            ],
        };

        let render_min_mesh_x = |scene: &RuntimeSceneState| {
            let context = Context::default();
            let target = Rect::from_min_size(Pos2::ZERO, egui::vec2(64.0, 64.0));
            let mut cache = TextureCache::default();
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(target),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new("runtime-overlay-render-test"),
                    ));
                    paint_v2_frame_with_runtime_overrides(
                        &painter,
                        ctx,
                        &project,
                        1,
                        0,
                        target,
                        &mut cache,
                        false,
                        &q0s_format::rig::RigRuntimeOverrides::new(),
                        scene,
                    );
                },
            );
            output
                .shapes
                .iter()
                .flat_map(|clipped| match &clipped.shape {
                    Shape::Mesh(mesh) => mesh.vertices.iter().map(|vertex| vertex.pos.x).collect(),
                    _ => Vec::new(),
                })
                .fold(f32::INFINITY, f32::min)
        };

        let authored = RuntimeSceneState::new();
        let authored_x = render_min_mesh_x(&authored);
        assert!(authored_x.is_finite());

        let mut moved = RuntimeSceneState::new();
        moved.set_position(InstanceKey::new(1, 7), 8.0, 3.0);
        let moved_x = render_min_mesh_x(&moved);

        // 16 stage units fit into 64 screen points, so the +6 stage-unit
        // runtime move must become exactly +24 screen points in the GUI mesh.
        assert!(
            ((moved_x - authored_x) - 24.0).abs() < 0.01,
            "authored_x={authored_x}, moved_x={moved_x}"
        );
    }

    #[test]
    fn interactive_player_renderer_omits_hidden_layers() {
        use q0s_format::v2::{
            BitmapAsset, Layer, LayerKey, LayerMetadata, Placement, ProjectMeta, Q0rg, Transform2D,
            Tween,
        };

        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "hidden-player-layer".into(),
                fps: 24,
                stage_width: 16,
                stage_height: 16,
                background_color: q0s_format::v2::Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                },
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Bitmap(BitmapAsset {
                asset_id: 1,
                width: 2,
                height: 2,
                rgba: [255, 0, 0, 255].repeat(4),
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    timeline_frame_count: None,
                    name: "art".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        pivot: None,
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        };

        let painted_shape_count = |project: &ProjectV2| {
            let context = Context::default();
            let target = Rect::from_min_size(Pos2::ZERO, egui::vec2(64.0, 64.0));
            let mut cache = TextureCache::default();
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(target),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new("hidden-layer-render-test"),
                    ));
                    paint_v2_frame(&painter, ctx, project, 1, 0, target, &mut cache, false);
                },
            );
            output.shapes.len()
        };

        assert!(
            painted_shape_count(&project) > 0,
            "visible layer must paint"
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 1),
            LayerMetadata {
                hidden: true,
                ..Default::default()
            },
        );
        assert_eq!(
            painted_shape_count(&project),
            0,
            "hidden layer must emit no interactive player shapes"
        );
    }

    #[test]
    fn interactive_player_uses_authored_camera_only_for_entry_q0rg() {
        use q0s_format::v2::{CameraKey, CameraState, ProjectMeta, Tween};

        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "camera-player".into(),
                fps: 24,
                stage_width: 100,
                stage_height: 80,
                background_color: q0s_format::v2::Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                },
                entry_q0rg_id: 1,
            },
            assets: Vec::new(),
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: Vec::new(),
        };
        project.runtime.camera.enabled = true;
        project.runtime.camera.keys.push(CameraKey {
            frame: 0,
            state: CameraState {
                center_x: 60.0,
                center_y: 40.0,
                zoom: 2.0,
                rotation: 0.25,
            },
            tween: Tween::None,
        });

        let expected = project.camera_affine_at(0);
        assert_eq!(authored_root_affine(&project, 1, 0), expected);
        assert_eq!(
            authored_root_affine(&project, 2, 0),
            Affine::IDENTITY,
            "nested/local q0rg previews must not receive the project camera twice"
        );
    }
}
