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

use egui::epaint::{PathShape, Vertex};
use egui::{
    pos2, Color32, ColorImage, Context, Mesh, Painter, Pos2, Rect, Shape, Stroke, TextureHandle,
    TextureOptions,
};
use lyon_path::math::point as lyon_point;
use lyon_tessellation::geometry_builder::{BuffersBuilder, Positions, VertexBuffers};
use lyon_tessellation::{FillOptions, FillRule as LyonFillRule, FillTessellator};
use q0s_format::geom::flatten_path;
use q0s_format::raster::active_placement_states_at;
use q0s_format::transform::Affine;
use q0s_format::v2::{Asset, BlendMode, Placement, PlacementFx, ProjectV2, Rgba, Target, Vec2};

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

#[derive(Default)]
pub struct TextureCache {
    by_asset_id: HashMap<u16, TextureHandle>,
    by_q0v_frame: HashMap<(u16, u32), TextureHandle>,
    q0v_media: HashMap<u16, q0video::q0v::Q0vFile>,
    fx_placement_textures: HashMap<FxPlacementKey, FxPlacementTexture>,
    fx_serial: u64,
}

impl TextureCache {
    pub fn invalidate(&mut self) {
        self.by_asset_id.clear();
        self.by_q0v_frame.clear();
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
    if white_stage_bg {
        let stage_rect = Rect::from_min_size(origin, drawn);
        painter.rect_filled(stage_rect, 0.0, Color32::WHITE);
    }
    let view = StageView { origin, scale };
    paint_q0rg(
        painter,
        ctx,
        project,
        q0rg_id,
        frame,
        Affine::IDENTITY,
        &view,
        0,
        cache,
        rig_overrides,
    );
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
    for layer in &q.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        for active in active_placement_states_at(layer, local_frame) {
            let Some(placement) = layer.placements.get(active.index) else {
                continue;
            };
            let local_affine = rig_pose
                .as_ref()
                .and_then(|pose| pose.binding_transform(placement.instance_id))
                .unwrap_or_else(|| Affine::from_transform(active.transform));
            // Affine matrix composition: parent skew / non-uniform scale
            // propagate cleanly into children, unlike the old SRSk
            // decomposition which dropped them.
            let composed = Affine::compose(parent, local_affine);
            if !active.fx.is_identity() {
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
                            composed,
                            view,
                            cache,
                            local_frame.saturating_sub(placement.frame),
                            project.meta.fps,
                        );
                    }
                }
                Target::Q0rg(child_id) if child_id != q0rg_id => {
                    paint_q0rg(
                        painter,
                        ctx,
                        project,
                        child_id,
                        local_frame,
                        composed,
                        view,
                        depth + 1,
                        cache,
                        rig_overrides,
                    );
                }
                _ => {}
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_asset(
    painter: &Painter,
    ctx: &Context,
    asset: &Asset,
    transform: Affine,
    view: &StageView,
    cache: &mut TextureCache,
    elapsed_host_frames: u16,
    host_fps: u16,
) {
    match asset {
        Asset::Bitmap(b) => {
            // Upload once, reuse forever. Bitmap data in v2 doesn't mutate
            // at playback time so the cache is safe across frames.
            let tex = cache.by_asset_id.entry(b.asset_id).or_insert_with(|| {
                let image = ColorImage::from_rgba_unmultiplied(
                    [usize::from(b.width), usize::from(b.height)],
                    &b.rgba,
                );
                ctx.load_texture(
                    format!("q0player_asset_{}", b.asset_id),
                    image,
                    TextureOptions::LINEAR,
                )
            });
            let w = f32::from(b.width);
            let h = f32::from(b.height);
            let local_corners = [
                Vec2::new(0.0, 0.0),
                Vec2::new(w, 0.0),
                Vec2::new(w, h),
                Vec2::new(0.0, h),
            ];
            let screen_corners: Vec<Pos2> = local_corners
                .iter()
                .map(|p| stage_to_screen(transform.apply(*p), view))
                .collect();
            let mut mesh = Mesh::with_texture(tex.id());
            let uv = [
                pos2(0.0, 0.0),
                pos2(1.0, 0.0),
                pos2(1.0, 1.0),
                pos2(0.0, 1.0),
            ];
            for (corner, uv) in screen_corners.iter().zip(uv.iter()) {
                mesh.vertices.push(Vertex {
                    pos: *corner,
                    uv: *uv,
                    color: Color32::WHITE,
                });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            painter.add(Shape::Mesh(mesh));
        }
        Asset::Q0v(v) => {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                cache.q0v_media.entry(v.asset_id)
            {
                let Ok(media) = q0video::q0v::Q0vFile::parse(v.bytes.clone()) else {
                    return;
                };
                entry.insert(media);
            }
            let Some(media) = cache.q0v_media.get(&v.asset_id) else {
                return;
            };
            let Some(frame_index) = media.spec.video_frame_for_host_frame(
                u32::from(elapsed_host_frames),
                u32::from(host_fps.max(1)),
            ) else {
                return;
            };
            let key = (v.asset_id, frame_index as u32);
            if let std::collections::hash_map::Entry::Vacant(entry) = cache.by_q0v_frame.entry(key)
            {
                let Ok(rgba) = media.decode_frame_rgba(frame_index) else {
                    return;
                };
                let image = ColorImage::from_rgba_unmultiplied(
                    [media.spec.width as usize, media.spec.height as usize],
                    &rgba,
                );
                let texture = ctx.load_texture(
                    format!("q0player_q0v_{}_{}", v.asset_id, frame_index),
                    image,
                    TextureOptions::LINEAR,
                );
                entry.insert(texture);
            }
            let Some(tex) = cache.by_q0v_frame.get(&key) else {
                return;
            };
            let w = media.spec.width as f32;
            let h = media.spec.height as f32;
            let local_corners = [
                Vec2::new(0.0, 0.0),
                Vec2::new(w, 0.0),
                Vec2::new(w, h),
                Vec2::new(0.0, h),
            ];
            let screen_corners: Vec<Pos2> = local_corners
                .iter()
                .map(|point| stage_to_screen(transform.apply(*point), view))
                .collect();
            let mut mesh = Mesh::with_texture(tex.id());
            let uv = [
                pos2(0.0, 0.0),
                pos2(1.0, 0.0),
                pos2(1.0, 1.0),
                pos2(0.0, 1.0),
            ];
            for (corner, uv) in screen_corners.iter().zip(uv.iter()) {
                mesh.vertices.push(Vertex {
                    pos: *corner,
                    uv: *uv,
                    color: Color32::WHITE,
                });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            painter.add(Shape::Mesh(mesh));
        }
        Asset::Rig(_) => {}
        Asset::Vector(v) => {
            // Fill every closed contour in one tessellation pass. A vector asset
            // can contain an exterior plus oppositely-wound hole contours; filling
            // each PathShape independently destroys those holes. egui 0.27 also
            // assumes PathShape fills are convex, which produces the old giant
            // diagonal "lasso" fans for concave brush graphics.
            let fill_color = v.fill.map(rgba_to_color32).unwrap_or(Color32::TRANSPARENT);
            if fill_color != Color32::TRANSPARENT {
                let contours: Vec<Vec<Pos2>> = v
                    .paths
                    .iter()
                    .filter(|path| path.closed)
                    .map(|path| {
                        flatten_path(path, BEZIER_SAMPLES)
                            .iter()
                            .map(|point| stage_to_screen(transform.apply(*point), view))
                            .collect()
                    })
                    .collect();
                paint_complex_fill(painter, &contours, fill_color);
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
                        s.width.max(0.5) * parent_scale * view.scale,
                        rgba_to_color32(s.color),
                    ),
                    None => Stroke::NONE,
                };
                if stroke != Stroke::NONE {
                    painter.add(Shape::Path(PathShape {
                        points: polyline_screen.clone(),
                        closed: path.closed,
                        fill: Color32::TRANSPARENT,
                        stroke,
                    }));
                }
                // Round caps via end-circles — egui's stroke is butt by
                // default. Mirrors the editor render so editor and
                // playback are pixel-identical.
                if let Some(s) = &v.stroke {
                    if !path.closed
                        && polyline_screen.len() >= 2
                        && matches!(s.cap, q0s_format::geom::CapShape::Round)
                    {
                        let r = s.width.max(0.5) * parent_scale * view.scale * 0.5;
                        let col = rgba_to_color32(s.color);
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
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "art".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
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
}
