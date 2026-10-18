use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(feature = "appearance-mask-eraser")]
use std::sync::{
    mpsc::{self, Receiver, Sender, TryRecvError},
    Mutex, OnceLock,
};

use egui::epaint::Vertex;
use egui::{
    pos2, Color32, ColorImage, Context, Mesh, Painter, Pos2, Rect, Shape, Stroke, TextureHandle,
    TextureId, TextureOptions,
};
#[cfg(feature = "appearance-mask-eraser")]
use geo::{BooleanOps, MultiPolygon, Point, Polygon};
use geo::{Buffer, Coord, LineString};
use lyon_path::math::point as lyon_point;
use lyon_tessellation::geometry_builder::{BuffersBuilder, Positions, VertexBuffers};
use lyon_tessellation::{
    FillOptions, FillRule as LyonFillRule, FillTessellator, LineCap, LineJoin, StrokeOptions,
    StrokeTessellator,
};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Anchor, Asset, BlendMode, GradientKind, LayerKey, LayerMetadata, Path as VPath, Placement,
    PlacementFx, ProjectMeta, ProjectV2, Q0rg, Q0vAsset, Rgba, Target, Transform2D, Tween, Vec2,
    VectorAppearance, VectorAsset,
};

const Q0RG_RECURSION_LIMIT: u8 = 8;
const BEZIER_SAMPLES_PER_SEGMENT: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct StageView {
    /// Top-left corner of the stage rectangle in screen pixels.
    pub origin: Pos2,
    /// pixels-per-stage-unit; same on x and y (square pixels).
    pub scale: f32,
    /// Stage rect in screen pixels, used to clip if desired.
    pub stage_rect: Rect,
    /// Maps the active symbol's local stage coordinates into the document stage.
    /// Identity for ordinary editing; set to the selected instance chain for
    /// Edit In Place.
    pub edit_transform: Affine,
}

impl StageView {
    pub fn stage_to_screen(&self, point: Vec2) -> Pos2 {
        let point = self.edit_transform.apply(point);
        pos2(
            self.origin.x + point.x * self.scale,
            self.origin.y + point.y * self.scale,
        )
    }

    pub fn screen_to_stage(&self, point: Pos2) -> Vec2 {
        let document = Vec2::new(
            (point.x - self.origin.x) / self.scale,
            (point.y - self.origin.y) / self.scale,
        );
        self.edit_transform
            .inverse()
            .map(|inverse| inverse.apply(document))
            .unwrap_or(document)
    }

    pub fn effective_scale(&self) -> f32 {
        self.scale * self.edit_transform.uniform_scale().max(0.001)
    }

    pub fn stage_bounds_to_screen(&self, min: Vec2, max: Vec2) -> Rect {
        let corners = [
            self.stage_to_screen(min),
            self.stage_to_screen(Vec2::new(max.x, min.y)),
            self.stage_to_screen(max),
            self.stage_to_screen(Vec2::new(min.x, max.y)),
        ];
        let bounds = corners.iter().fold(
            (
                Pos2::new(f32::INFINITY, f32::INFINITY),
                Pos2::new(f32::NEG_INFINITY, f32::NEG_INFINITY),
            ),
            |(min, max), point| {
                (
                    Pos2::new(min.x.min(point.x), min.y.min(point.y)),
                    Pos2::new(max.x.max(point.x), max.y.max(point.y)),
                )
            },
        );
        Rect::from_min_max(bounds.0, bounds.1)
    }

    pub fn screen_transform(&self) -> Affine {
        Affine::compose(
            Affine {
                a11: self.scale,
                a12: 0.0,
                a21: 0.0,
                a22: self.scale,
                tx: self.origin.x,
                ty: self.origin.y,
            },
            self.edit_transform,
        )
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StageTraversalStats {
    pub q0rg_visits: u64,
    pub active_placements: u64,
    pub vector_instances_batched: u64,
    pub vector_batches: u64,
    pub vector_vertices: u64,
    pub vector_indices: u64,
    pub vector_fallbacks: u64,
}

#[derive(Default)]
struct StageWorkTimings {
    video_index: std::time::Duration,
    video_decode: std::time::Duration,
    video_pixels: std::time::Duration,
    video_upload: std::time::Duration,
    mask_raster: std::time::Duration,
    group_strokes: std::time::Duration,
    prefetch_hits: u64,
    prefetch_misses: u64,
}

impl StageWorkTimings {
    fn record(self, perf: &mut crate::perf::PerfLog) {
        use crate::perf::{PerfCounter, PerfSpan};
        for (span, duration) in [
            (PerfSpan::VideoIndex, self.video_index),
            (PerfSpan::VideoDecode, self.video_decode),
            (PerfSpan::VideoPixels, self.video_pixels),
            (PerfSpan::VideoUpload, self.video_upload),
            (PerfSpan::MaskRaster, self.mask_raster),
            (PerfSpan::MaskGroupStrokes, self.group_strokes),
        ] {
            if !duration.is_zero() {
                perf.record(span, duration);
            }
        }
        perf.add_counter(PerfCounter::VideoPrefetchHits, self.prefetch_hits);
        perf.add_counter(PerfCounter::VideoPrefetchMisses, self.prefetch_misses);
    }
}

const VECTOR_MESH_BATCH_VERTEX_LIMIT: usize = 65_536;

/// Collects ordinary vector triangles in paint order and submits fewer egui shapes.
/// Callers flush before drawing anything that needs a separate rendering path.
#[derive(Default)]
struct VectorMeshBatch {
    mesh: Mesh,
    instances: u64,
    batches: u64,
    vertices: u64,
    indices: u64,
    fallbacks: u64,
}

impl VectorMeshBatch {
    #[allow(clippy::too_many_arguments)]
    fn append(
        &mut self,
        painter: &Painter,
        geometry: &CachedVectorRenderGeometry,
        vector: &VectorAsset,
        transform: Affine,
        view: &StageView,
        tint: Color32,
        stroke_scale_compensation: f32,
        color_transform: q0glblend::ColorTransform,
    ) -> bool {
        let fill_color = vector
            .fill
            .map(|color| color_transform.apply(modulate(rgba_to_color32(color), tint)))
            .unwrap_or(Color32::TRANSPARENT);
        let has_fill = fill_color != Color32::TRANSPARENT
            && !geometry.fill_vertices.is_empty()
            && !geometry.fill_indices.is_empty();
        let stroke_color = vector
            .stroke
            .map(|stroke| color_transform.apply(modulate(rgba_to_color32(stroke.color), tint)));
        let has_stroke = stroke_color.is_some();

        if !has_fill && !has_stroke {
            return false;
        }
        if has_stroke
            && ((stroke_scale_compensation - 1.0).abs() > 1.0e-6
                || !affine_is_similarity(transform)
                || geometry.stroke_vertices.is_empty()
                || geometry.stroke_indices.is_empty())
        {
            return false;
        }

        let fill_vertex_count = if has_fill {
            geometry.fill_vertices.len()
        } else {
            0
        };
        let stroke_vertex_count = if has_stroke {
            geometry.stroke_vertices.len()
        } else {
            0
        };
        let vertex_count = fill_vertex_count.saturating_add(stroke_vertex_count);
        let fill_index_count = if has_fill {
            geometry.fill_indices.len()
        } else {
            0
        };
        let stroke_index_count = if has_stroke {
            geometry.stroke_indices.len()
        } else {
            0
        };
        let index_count = fill_index_count.saturating_add(stroke_index_count);
        if self.mesh.vertices.len().saturating_add(vertex_count) > VECTOR_MESH_BATCH_VERTEX_LIMIT
            && !self.mesh.is_empty()
        {
            self.flush(painter);
        }
        let Ok(base) = u32::try_from(self.mesh.vertices.len()) else {
            return false;
        };
        let Ok(fill_vertex_count_u32) = u32::try_from(fill_vertex_count) else {
            return false;
        };
        if u32::try_from(self.mesh.vertices.len().saturating_add(vertex_count)).is_err() {
            return false;
        }
        let Some(stroke_base) = base.checked_add(fill_vertex_count_u32) else {
            return false;
        };
        self.mesh.vertices.reserve(vertex_count);
        self.mesh.indices.reserve(index_count);

        if has_fill {
            append_transformed_vector_part(
                &mut self.mesh,
                &geometry.fill_vertices,
                &geometry.fill_indices,
                base,
                transform,
                view,
                fill_color,
            );
        }
        if let Some(color) = stroke_color {
            append_transformed_vector_part(
                &mut self.mesh,
                &geometry.stroke_vertices,
                &geometry.stroke_indices,
                stroke_base,
                transform,
                view,
                color,
            );
        }
        self.instances = self.instances.saturating_add(1);
        self.vertices = self.vertices.saturating_add(vertex_count as u64);
        self.indices = self.indices.saturating_add(index_count as u64);
        true
    }

    fn flush(&mut self, painter: &Painter) {
        if self.mesh.is_empty() {
            return;
        }
        painter.add(Shape::Mesh(std::mem::take(&mut self.mesh)));
        self.batches = self.batches.saturating_add(1);
    }

    fn record_fallback(&mut self) {
        self.fallbacks = self.fallbacks.saturating_add(1);
    }
}

fn append_transformed_vector_part(
    mesh: &mut Mesh,
    vertices: &[Vec2],
    indices: &[u32],
    base: u32,
    transform: Affine,
    view: &StageView,
    color: Color32,
) {
    mesh.vertices.extend(vertices.iter().map(|point| Vertex {
        pos: stage_to_screen(transform.apply(*point), view),
        uv: Pos2::ZERO,
        color,
    }));
    mesh.indices
        .extend(indices.iter().map(|index| base + *index));
}

struct ClassicBrushPreviewTexture {
    texture: TextureHandle,
    pixels: Vec<Color32>,
    size: [usize; 2],
    screen_rect: Rect,
    pixels_per_point: f32,
    view_origin: Pos2,
    view_scale: f32,
    view_transform: Affine,
}

fn preview_polygon_contains(point: Pos2, polygon: &[Pos2]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = polygon[polygon.len() - 1];
    for &current in polygon {
        let crosses = (current.y > point.y) != (previous.y > point.y);
        if crosses {
            let dy = previous.y - current.y;
            if dy.abs() > 1.0e-8 {
                let x = current.x + (point.y - current.y) * (previous.x - current.x) / dy;
                if point.x < x {
                    inside = !inside;
                }
            }
        }
        previous = current;
    }
    inside
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone)]
struct CachedAppearanceTexture {
    texture: TextureHandle,
    local_min_offset: Vec2,
    width: u32,
    height: u32,
    pixels_per_unit: f32,
}

#[cfg(feature = "appearance-mask-eraser")]
type AppearanceRasterResult = Option<q0s_format::raster::RasterizedVectorAppearance>;

#[cfg(feature = "appearance-mask-eraser")]
struct AppearanceRasterRequest {
    vector: VectorAsset,
    appearance: VectorAppearance,
    pixels_per_unit: f32,
    reply: Sender<AppearanceRasterResult>,
}

#[cfg(feature = "appearance-mask-eraser")]
fn appearance_raster_worker() -> &'static Sender<AppearanceRasterRequest> {
    static WORKER: OnceLock<Sender<AppearanceRasterRequest>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<AppearanceRasterRequest>();
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..2 {
            let receiver = Arc::clone(&receiver);
            std::thread::spawn(move || loop {
                let request = {
                    let Ok(receiver) = receiver.lock() else {
                        return;
                    };
                    receiver.recv()
                };
                let Ok(request) = request else {
                    return;
                };
                let tile = q0s_format::raster::rasterize_vector_halo_local(
                    &request.vector,
                    &request.appearance,
                    request.pixels_per_unit,
                );
                let _ = request.reply.send(tile);
            });
        }
        sender
    })
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone)]
pub(crate) struct CachedInteractivePath {
    pub points: Vec<Vec2>,
    pub signed_area: f64,
    pub bounds: Option<(f32, f32, f32, f32)>,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone, Default)]
pub(crate) struct CachedAppearanceHitGeometry {
    pub source: Vec<CachedInteractivePath>,
    pub clip: Vec<CachedInteractivePath>,
    pub erase: Vec<CachedInteractivePath>,
    /// Open paths and stroke-only closed paths flattened once per immutable asset.
    /// Selection hover used to re-flatten these Beziers on every pointer frame.
    pub strokes: Vec<CachedInteractivePath>,
    /// All authored paths used when testing the visible body of nested symbols.
    pub body_paths: Vec<CachedInteractivePath>,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone)]
pub(crate) struct CachedRawSelectionComponent {
    pub canonical_surface: MultiPolygon<f64>,
    pub path_indices: Vec<usize>,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone, Default)]
pub(crate) struct CachedRawSelectionComponents {
    pub components: Vec<CachedRawSelectionComponent>,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone, Default)]
pub(crate) struct CachedSelectionGeometry {
    pub body: Vec<Vec<Vec2>>,
    pub fallback_material_support: Vec<Vec<Vec2>>,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone)]
pub(crate) struct CachedParametricStroke {
    base_vertices: Vec<Pos2>,
    width_vectors: Vec<egui::Vec2>,
    indices: Vec<u32>,
    min_width: f32,
    max_width: f32,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone)]
struct CachedSelectionStroke {
    contour_stage: Vec<Pos2>,
    parametric: Vec<CachedParametricStroke>,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone, Default)]
pub(crate) struct CachedSelectionStageMesh {
    fill_vertices: Vec<Pos2>,
    fill_indices: Vec<u32>,
    strokes: Vec<CachedSelectionStroke>,
    /// Whether the exact screen outline can be represented as a reusable
    /// centerline + fixed-pixel extrusion mesh. None means not attempted yet.
    pub(crate) affine_reprojection_supported: Option<bool>,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SelectionStageMeshKey {
    asset_id: u16,
    path_indices: Vec<usize>,
    field_transform_bits: [u32; 6],
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SelectionPaintKey {
    pub asset_id: u16,
    pub path_indices: Vec<usize>,
    pub field_transform_bits: [u32; 6],
    pub placement_transform_bits: [u32; 6],
    pub view_origin_bits: [u32; 2],
    pub view_scale_bits: u32,
    pub view_transform_bits: [u32; 6],
    pub accent_rgba: [u8; 4],
    pub texture_id: TextureId,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone, Default)]
pub(crate) struct CachedSelectionPaint {
    /// Textured selection meshes (currently the stipple fill) still painted by egui.
    pub meshes: Vec<Arc<Mesh>>,
    /// Solid outline triangles are identical to the old egui meshes, but kept
    /// behind Arc so q0glblend can upload them once and reuse the GPU buffers.
    pub solid_overlay: Option<Arc<Mesh>>,
    /// True when the solid mesh UV stores a fixed screen-space stroke offset
    /// and the whole paint can follow an arbitrary affine transform on the GPU.
    pub affine_reprojectable: bool,
    pub view_origin: Pos2,
    pub view_scale: f32,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RawAreaSelectionKey {
    pub assets: Vec<(u16, [u32; 6])>,
    pub clip_rings: Vec<Vec<[u64; 2]>>,
}

#[cfg(feature = "appearance-mask-eraser")]
pub(crate) struct CachedRawAreaSelection {
    pub fill: CachedSelectionStageMesh,
    pub strokes: Vec<Vec<Pos2>>,
    pub paint: Option<CachedSelectionPaint>,
    pub paint_key: Option<(Color32, TextureId, [u32; 7])>,
}

#[cfg(feature = "appearance-mask-eraser")]
impl CachedRawAreaSelection {
    pub(crate) fn new(contours: &[Vec<Pos2>], strokes: Vec<Vec<Pos2>>) -> Self {
        Self {
            fill: build_selection_stage_mesh(contours).unwrap_or_default(),
            strokes,
            paint: None,
            paint_key: None,
        }
    }
}

#[cfg(feature = "appearance-mask-eraser")]
pub(crate) fn split_selection_paint_meshes(
    meshes: Vec<Mesh>,
) -> (Vec<Arc<Mesh>>, Option<Arc<Mesh>>) {
    let mut textured = Vec::with_capacity(meshes.len());
    let mut solid = Mesh::default();
    for mesh in meshes {
        if mesh.texture_id == TextureId::default() {
            solid.append(mesh);
        } else {
            textured.push(Arc::new(mesh));
        }
    }
    let solid = (!solid.is_empty()).then(|| Arc::new(solid));
    (textured, solid)
}

#[cfg(feature = "appearance-mask-eraser")]
pub(crate) struct PlainSelectionSplitCacheSeed {
    render: CachedVectorRenderGeometry,
    remainder_render: Option<CachedVectorRenderGeometry>,
    stage: CachedSelectionStageMesh,
    paints: Vec<(SelectionPaintKey, CachedSelectionPaint)>,
}

#[cfg(feature = "appearance-mask-eraser")]
fn cache_interactive_paths(paths: &[VPath]) -> Vec<CachedInteractivePath> {
    paths
        .iter()
        .filter(|path| path.closed)
        .filter_map(|path| {
            let points = flatten_path(path);
            if points.len() < 3 {
                return None;
            }
            let signed_area = points
                .iter()
                .zip(points.iter().cycle().skip(1))
                .take(points.len())
                .map(|(a, b)| f64::from(a.x) * f64::from(b.y) - f64::from(b.x) * f64::from(a.y))
                .sum::<f64>()
                * 0.5;
            let mut bounds = (
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            );
            for point in &points {
                bounds.0 = bounds.0.min(point.x);
                bounds.1 = bounds.1.min(point.y);
                bounds.2 = bounds.2.max(point.x);
                bounds.3 = bounds.3.max(point.y);
            }
            Some(CachedInteractivePath {
                points,
                signed_area,
                bounds: (bounds.0.is_finite()
                    && bounds.1.is_finite()
                    && bounds.2.is_finite()
                    && bounds.3.is_finite())
                .then_some(bounds),
            })
        })
        .collect()
}

#[cfg(feature = "appearance-mask-eraser")]
fn cache_interactive_strokes(vector: &VectorAsset) -> Vec<CachedInteractivePath> {
    vector
        .paths
        .iter()
        .filter(|path| !path.closed || vector.fill.is_none())
        .filter_map(|path| {
            let points = flatten_path(path);
            if points.len() < 2 {
                return None;
            }
            let mut bounds = (
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            );
            for point in &points {
                bounds.0 = bounds.0.min(point.x);
                bounds.1 = bounds.1.min(point.y);
                bounds.2 = bounds.2.max(point.x);
                bounds.3 = bounds.3.max(point.y);
            }
            Some(CachedInteractivePath {
                points,
                signed_area: 0.0,
                bounds: (bounds.0.is_finite()
                    && bounds.1.is_finite()
                    && bounds.2.is_finite()
                    && bounds.3.is_finite())
                .then_some(bounds),
            })
        })
        .collect()
}

#[cfg(feature = "appearance-mask-eraser")]
fn cache_interactive_body_paths(paths: &[VPath]) -> Vec<CachedInteractivePath> {
    paths
        .iter()
        .filter_map(|path| {
            let points = flatten_path(path);
            if points.len() < 2 {
                return None;
            }
            let mut bounds = (
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            );
            for point in &points {
                bounds.0 = bounds.0.min(point.x);
                bounds.1 = bounds.1.min(point.y);
                bounds.2 = bounds.2.max(point.x);
                bounds.3 = bounds.3.max(point.y);
            }
            Some(CachedInteractivePath {
                points,
                signed_area: 0.0,
                bounds: (bounds.0.is_finite()
                    && bounds.1.is_finite()
                    && bounds.2.is_finite()
                    && bounds.3.is_finite())
                .then_some(bounds),
            })
        })
        .collect()
}

#[cfg(feature = "appearance-mask-eraser")]
fn geo_segment_distance_sq(point: Point<f64>, start: Coord<f64>, end: Coord<f64>) -> f64 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_sq = dx * dx + dy * dy;
    if length_sq <= 1.0e-20 {
        let px = point.x() - start.x;
        let py = point.y() - start.y;
        return px * px + py * py;
    }
    let t = (((point.x() - start.x) * dx + (point.y() - start.y) * dy) / length_sq).clamp(0.0, 1.0);
    let nearest_x = start.x + dx * t;
    let nearest_y = start.y + dy * t;
    let px = point.x() - nearest_x;
    let py = point.y() - nearest_y;
    px * px + py * py
}

#[cfg(feature = "appearance-mask-eraser")]
fn polygon_boundary_near_point(polygon: &Polygon<f64>, point: Vec2, radius: f32) -> bool {
    let point = Point::new(f64::from(point.x), f64::from(point.y));
    let limit_sq = f64::from(radius.max(0.0)).powi(2);
    let ring_near = |ring: &LineString<f64>| {
        ring.0
            .windows(2)
            .any(|segment| geo_segment_distance_sq(point, segment[0], segment[1]) <= limit_sq)
    };
    ring_near(polygon.exterior()) || polygon.interiors().iter().any(ring_near)
}

#[derive(Clone, Default)]
struct CachedVectorRenderGeometry {
    fill_vertices: Vec<Vec2>,
    fill_indices: Vec<u32>,
    fill_bounds: Option<Rect>,
    fill_contours: Vec<Vec<Pos2>>,
    stroke_paths: Vec<(Vec<Vec2>, bool)>,
    /// Local-space stroke triangles tessellated once per immutable vector asset.
    /// Bevel joins bound pathological Flash hairpins without paying lyon's stroke
    /// tessellator on every repaint. Used when the placement affine is a similarity.
    stroke_vertices: Vec<Vec2>,
    stroke_indices: Vec<u32>,
}

#[derive(Clone)]
struct CachedGradientRenderGeometry {
    signature: u64,
    mesh: Option<Arc<Mesh>>,
    source_bounds: Option<Rect>,
    inverse: Option<Affine>,
    kind: GradientKind,
    stops: Arc<Vec<q0s_format::v2::GradientStop>>,
}

impl Default for CachedGradientRenderGeometry {
    fn default() -> Self {
        Self {
            signature: 0,
            mesh: None,
            source_bounds: None,
            inverse: None,
            kind: GradientKind::Linear,
            stops: Arc::new(Vec::new()),
        }
    }
}

struct RenderIndex {
    asset_ptr: usize,
    asset_len: usize,
    asset_capacity: usize,
    q0rg_ptr: usize,
    q0rg_len: usize,
    q0rg_capacity: usize,
    stale: AtomicBool,
    assets: HashMap<u16, usize>,
    q0rgs: HashMap<u16, usize>,
    rigs: HashMap<u16, usize>,
}

impl RenderIndex {
    fn new(project: &ProjectV2) -> Self {
        let mut assets = HashMap::with_capacity(project.assets.len());
        let mut rigs = HashMap::new();
        for (index, asset) in project.assets.iter().enumerate() {
            let id = asset.id();
            if let Asset::Rig(rig) = asset {
                rigs.insert(rig.owner_q0rg_id, index);
            }
            assets.insert(id, index);
        }
        let q0rgs = project
            .q0rgs
            .iter()
            .map(|q0rg| q0rg.q0rg_id)
            .enumerate()
            .map(|(index, id)| (id, index))
            .collect();
        Self {
            asset_ptr: project.assets.as_ptr() as usize,
            asset_len: project.assets.len(),
            asset_capacity: project.assets.capacity(),
            q0rg_ptr: project.q0rgs.as_ptr() as usize,
            q0rg_len: project.q0rgs.len(),
            q0rg_capacity: project.q0rgs.capacity(),
            stale: AtomicBool::new(false),
            assets,
            q0rgs,
            rigs,
        }
    }

    fn matches(&self, project: &ProjectV2) -> bool {
        !self.stale.load(Ordering::Relaxed)
            && self.asset_ptr == project.assets.as_ptr() as usize
            && self.asset_len == project.assets.len()
            && self.asset_capacity == project.assets.capacity()
            && self.q0rg_ptr == project.q0rgs.as_ptr() as usize
            && self.q0rg_len == project.q0rgs.len()
            && self.q0rg_capacity == project.q0rgs.capacity()
    }
}

struct RenderLookup<'a> {
    project: &'a ProjectV2,
    index: Arc<RenderIndex>,
    linear_fallback: bool,
    rig_poses: RefCell<HashMap<(u16, u16), Rc<q0s_format::rig::RigPose>>>,
}

impl<'a> RenderLookup<'a> {
    fn new(project: &'a ProjectV2, index: Arc<RenderIndex>, linear_fallback: bool) -> Self {
        Self {
            project,
            index,
            linear_fallback,
            rig_poses: RefCell::new(HashMap::new()),
        }
    }

    fn asset(&self, asset_id: u16) -> Option<&'a Asset> {
        let indexed = self
            .index
            .assets
            .get(&asset_id)
            .and_then(|index| self.project.assets.get(*index));
        match indexed {
            Some(asset) if asset.id() == asset_id => Some(asset),
            None if !self.linear_fallback => None,
            _ => {
                self.index.stale.store(true, Ordering::Relaxed);
                self.project
                    .assets
                    .iter()
                    .rfind(|asset| asset.id() == asset_id)
            }
        }
    }

    fn q0rg(&self, q0rg_id: u16) -> Option<&'a Q0rg> {
        let indexed = self
            .index
            .q0rgs
            .get(&q0rg_id)
            .and_then(|index| self.project.q0rgs.get(*index));
        match indexed {
            Some(q0rg) if q0rg.q0rg_id == q0rg_id => Some(q0rg),
            None if !self.linear_fallback => None,
            _ => {
                self.index.stale.store(true, Ordering::Relaxed);
                self.project
                    .q0rgs
                    .iter()
                    .rfind(|q0rg| q0rg.q0rg_id == q0rg_id)
            }
        }
    }

    fn has_rig(&self, q0rg_id: u16) -> bool {
        self.rig(q0rg_id).is_some()
    }

    fn rig(&self, q0rg_id: u16) -> Option<&'a q0s_format::v2::RigAsset> {
        let indexed = self
            .index
            .rigs
            .get(&q0rg_id)
            .and_then(|index| self.project.assets.get(*index))
            .and_then(|asset| match asset {
                Asset::Rig(rig) if rig.owner_q0rg_id == q0rg_id => Some(rig),
                _ => None,
            });
        if indexed.is_some() {
            return indexed;
        }
        if !self.linear_fallback && !self.index.rigs.contains_key(&q0rg_id) {
            return None;
        }
        self.index.stale.store(true, Ordering::Relaxed);
        self.project
            .assets
            .iter()
            .rev()
            .find_map(|asset| match asset {
                Asset::Rig(rig) if rig.owner_q0rg_id == q0rg_id => Some(rig),
                _ => None,
            })
    }

    fn rig_pose(&self, q0rg_id: u16, local_frame: u16) -> Option<Rc<q0s_format::rig::RigPose>> {
        let key = (q0rg_id, local_frame);
        if let Some(pose) = self.rig_poses.borrow().get(&key).cloned() {
            return Some(pose);
        }
        let rig = self.rig(q0rg_id)?;
        let pose = Rc::new(q0s_format::rig::evaluate_rig(
            rig,
            f32::from(local_frame),
            &[],
        ));
        self.rig_poses.borrow_mut().insert(key, Rc::clone(&pose));
        Some(pose)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FxPlacementKey {
    host_q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    local_frame: u16,
    width: u32,
    height: u32,
    linear_affine_bits: [u32; 4],
    tint: [u8; 4],
    outer_only: bool,
}

struct FxPlacementTexture {
    texture: TextureHandle,
    crop: [u32; 4],
    blend_mode: BlendMode,
    raster_translation: Vec2,
    raster_transform: Affine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MaskFrameKey {
    q0rg_id: u16,
    frame: u16,
    width: u32,
    height: u32,
    scale_x_bits: u32,
    scale_y_bits: u32,
    shear_x_bits: u32,
    shear_y_bits: u32,
    translate_x_bits: u32,
    translate_y_bits: u32,
    editing_q0rg_id: Option<u16>,
    mask_visibility_revision: u64,
    layer_filter_hash: u64,
}

struct CachedMaskFrame {
    texture: TextureHandle,
    stage_origin: Pos2,
    screen_rect: Rect,
}

struct ScopedPlacementBoundsCache {
    revision: (u64, u64),
    bounds: PlacementBoundsCache,
}

#[derive(Clone)]
struct CachedVectorGroupFill {
    mesh: Arc<Mesh>,
    bounds: Rect,
    stops: Arc<Vec<q0s_format::v2::GradientStop>>,
}

#[derive(Default)]
pub struct TextureCache {
    pub(crate) hidden_text_placement: Option<(u16, u16, usize)>,
    /// Mask source artwork hidden for the active project. Mask alpha continues
    /// to feed the GPU clipping pass when its source artwork is hidden here.
    pub(crate) hidden_mask_art_layers: HashSet<(u16, u16)>,
    /// Only the actively edited timeline shows mask-source artwork. Symbols
    /// and library previews retain their final composited appearance.
    pub(crate) editing_mask_q0rg_id: Option<u16>,
    pub(crate) mask_visibility_revision: u64,
    /// Reuse a transformed soft-effect tile while the pointer is held down.
    pub(crate) interactive_transform: bool,
    /// Reuse exact filtered vector outputs while the stage is idle.
    pub(crate) gpu_filter_cache_enabled: bool,
    /// Raw graphics bake temporary Select transforms into their path geometry.
    /// Compensate only that live local scale so authored stroke widths stay
    /// constant during the gesture just as they will after the bake.
    live_raw_stroke_scale_compensation: HashMap<u16, f32>,
    by_asset_id: HashMap<u16, TextureHandle>,
    canvas_background: Option<CachedCanvasBackground>,
    vector_render_by_asset: HashMap<u16, CachedVectorRenderGeometry>,
    vector_group_fill_by_asset: HashMap<u16, CachedVectorGroupFill>,
    gradient_render_by_asset: HashMap<u16, CachedGradientRenderGeometry>,
    vector_bounds_by_asset: HashMap<u16, Option<(f32, f32, f32, f32)>>,
    vector_bounds_revision: Option<(u64, u64)>,
    pub(crate) library_frame_previews:
        HashMap<(u16, u16), crate::panels::library::LibraryFramePreview>,
    pub(crate) library_frame_preview_revision: Option<(u64, u64)>,
    pub(crate) library_frame_preview_budget: Option<(u64, std::time::Instant)>,
    render_index: Option<Arc<RenderIndex>>,
    render_index_revision: Option<(u64, u64)>,
    active_placement_indexes: HashMap<(u16, u16), LayerFrameIndex>,
    active_placement_indexes_revision: Option<(u64, u64)>,
    placement_bounds_cache: Option<ScopedPlacementBoundsCache>,
    rig_pose_cache: HashMap<(u16, u16), Option<Rc<q0s_format::rig::RigPose>>>,
    rig_pose_cache_revision: Option<(u64, u64)>,
    stage_traversal_stats: StageTraversalStats,
    stage_work_timings: StageWorkTimings,
    pub(crate) video_playback_active: bool,
    video_prefetch: crate::video_preview::VideoPrefetch,
    by_q0v_frame: HashMap<(usize, u32), CachedVideoTexture>,
    q0v_media: HashMap<usize, CachedVideoMedia>,
    audio_waveforms: HashMap<u16, crate::audio::AudioWaveformCache>,
    fx_project_snapshot: Option<FxProjectSnapshot>,
    fx_project_revision: Option<(u64, u64)>,
    fx_placement_textures: HashMap<FxPlacementKey, FxPlacementTexture>,
    mask_frame_revision: Option<(u64, u64)>,
    mask_frame_textures: HashMap<MaskFrameKey, CachedMaskFrame>,
    fx_serial: u64,
    classic_brush_preview: Option<ClassicBrushPreviewTexture>,
    #[cfg(test)]
    classic_preview_candidate_pixels: usize,
    #[cfg(test)]
    classic_preview_uploaded_pixels: usize,
    #[cfg(feature = "appearance-mask-eraser")]
    appearance_by_asset: HashMap<(u16, u16, u64), CachedAppearanceTexture>,
    #[cfg(feature = "appearance-mask-eraser")]
    dynamic_appearance_by_instance: HashMap<u32, CachedAppearanceTexture>,
    #[cfg(feature = "appearance-mask-eraser")]
    appearance_raster_jobs: HashMap<(u16, u16, u64), Receiver<AppearanceRasterResult>>,
    #[cfg(feature = "appearance-mask-eraser")]
    appearance_signature_by_asset: HashMap<u16, (u64, Vec2)>,
    #[cfg(feature = "appearance-mask-eraser")]
    visible_body_by_asset: HashMap<u16, Vec<Vec<Vec2>>>,
    #[cfg(feature = "appearance-mask-eraser")]
    visible_body_mesh_by_asset: HashMap<u16, CachedVectorRenderGeometry>,
    #[cfg(feature = "appearance-mask-eraser")]
    visible_selection_body_by_asset: HashMap<u16, Vec<Vec<Vec2>>>,
    #[cfg(feature = "appearance-mask-eraser")]
    appearance_hit_by_asset: HashMap<u16, CachedAppearanceHitGeometry>,
    #[cfg(feature = "appearance-mask-eraser")]
    raw_selection_components_by_key: HashMap<(u16, [u32; 6]), CachedRawSelectionComponents>,
    #[cfg(feature = "appearance-mask-eraser")]
    selection_source_surface_by_key: HashMap<(u16, Vec<usize>), MultiPolygon<f64>>,
    #[cfg(feature = "appearance-mask-eraser")]
    selection_geometry_by_key: HashMap<(u16, Vec<usize>), CachedSelectionGeometry>,
    #[cfg(feature = "appearance-mask-eraser")]
    selection_stage_mesh_by_key: HashMap<SelectionStageMeshKey, CachedSelectionStageMesh>,
    #[cfg(feature = "appearance-mask-eraser")]
    selection_paint_by_key: HashMap<SelectionPaintKey, CachedSelectionPaint>,
    // Keep only the current marquee. Its stage geometry survives zoom/pan, but
    // changed selection clips cannot accumulate large meshes indefinitely.
    #[cfg(feature = "appearance-mask-eraser")]
    raw_area_selection: Option<(RawAreaSelectionKey, CachedRawAreaSelection)>,
    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    appearance_signature_build_count: usize,
    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    visible_body_build_count: usize,
    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    appearance_hit_build_count: usize,
    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    raw_selection_components_build_count: usize,
    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    selection_geometry_build_count: usize,
    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    selection_stage_mesh_build_count: usize,
    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    selection_paint_build_count: usize,
    #[cfg(test)]
    vector_render_geometry_build_count: usize,
    #[cfg(test)]
    gradient_render_geometry_build_count: usize,
}

struct CachedVideoMedia {
    source: Arc<Vec<u8>>,
    media: Option<Arc<q0video::q0v::Q0vFile>>,
}

struct CachedVideoTexture {
    texture: TextureHandle,
    last_used: u64,
    bytes: usize,
}

struct CachedCanvasBackground {
    path: PathBuf,
    texture: Option<TextureHandle>,
    size: [usize; 2],
}

impl TextureCache {
    fn sync_video_sources(&mut self, project: &ProjectV2) {
        let sources: HashSet<_> = project
            .assets
            .iter()
            .filter_map(|asset| match asset {
                Asset::Q0v(video) => Some(Arc::as_ptr(&video.bytes) as usize),
                _ => None,
            })
            .collect();
        self.q0v_media.retain(|source, _| sources.contains(source));
        self.by_q0v_frame
            .retain(|(source, _), _| sources.contains(source));
        self.video_prefetch.retain_sources(&sources);
    }

    fn video_texture(
        &mut self,
        video: &Q0vAsset,
        elapsed_host_frames: u16,
        host_fps: u16,
        ctx: &Context,
    ) -> Option<(TextureHandle, [f32; 2])> {
        // The source Arc is kept alive by CachedVideoMedia, so its pointer is
        // stable even when independent library items share this immutable clip.
        let source = Arc::as_ptr(&video.bytes) as usize;
        if !self.q0v_media.contains_key(&source) {
            let started = std::time::Instant::now();
            let media = q0video::q0v::Q0vFile::parse_shared(video.bytes.clone())
                .ok()
                .map(Arc::new);
            self.stage_work_timings.video_index += started.elapsed();
            self.q0v_media.insert(
                source,
                CachedVideoMedia {
                    source: video.bytes.clone(),
                    media,
                },
            );
        }
        self.video_prefetch.poll();
        let cached_media = self.q0v_media.get(&source)?;
        debug_assert_eq!(Arc::as_ptr(&cached_media.source) as usize, source);
        let media = cached_media.media.as_ref()?;
        let frame_index = media.spec.video_frame_for_host_frame(
            u32::from(elapsed_host_frames),
            u32::from(host_fps.max(1)),
        )?;
        let size = [media.spec.width as f32, media.spec.height as f32];
        let key = (source, frame_index as u32);
        let paint_frame = ctx.frame_nr();
        if let std::collections::hash_map::Entry::Vacant(entry) = self.by_q0v_frame.entry(key) {
            let image = match self.video_prefetch.take(source, frame_index) {
                Some(image) => {
                    self.stage_work_timings.prefetch_hits += 1;
                    image?
                }
                None => {
                    if self.video_playback_active {
                        self.stage_work_timings.prefetch_misses += 1;
                    }
                    // A seek or first frame must remain exact. Never substitute
                    // stale pixels from a previous video position on a cache miss.
                    let started = std::time::Instant::now();
                    let rgba = media.decode_frame_rgba(frame_index).ok();
                    self.stage_work_timings.video_decode += started.elapsed();
                    let rgba = rgba?;
                    let started = std::time::Instant::now();
                    let image = ColorImage::from_rgba_unmultiplied(
                        [media.spec.width as usize, media.spec.height as usize],
                        &rgba,
                    );
                    self.stage_work_timings.video_pixels += started.elapsed();
                    image
                }
            };
            let bytes = image.pixels.len() * std::mem::size_of::<Color32>();
            let started = std::time::Instant::now();
            let texture = ctx.load_texture(
                format!("q0s_q0v_{}_{}", source, frame_index),
                image,
                TextureOptions::LINEAR,
            );
            self.stage_work_timings.video_upload += started.elapsed();
            entry.insert(CachedVideoTexture {
                texture,
                last_used: paint_frame,
                bytes,
            });
        }
        if self.video_playback_active {
            let ahead = crate::video_preview::VideoPrefetch::look_ahead(media);
            for offset in 1..=ahead {
                let Some(next) = media.spec.video_frame_for_host_frame(
                    u32::from(elapsed_host_frames).saturating_add(offset as u32),
                    u32::from(host_fps.max(1)),
                ) else {
                    continue;
                };
                if next < media.frames.len()
                    && !self.by_q0v_frame.contains_key(&(source, next as u32))
                {
                    self.video_prefetch.request(source, next, media);
                }
            }
        }
        let cached = self.by_q0v_frame.get_mut(&key)?;
        cached.last_used = paint_frame;
        let texture = cached.texture.clone();
        // Keep this paint's textures alive for mask callbacks and onion frames.
        // Older decoded frames have a bounded LRU budget, not the whole clip.
        let mut bytes: usize = self.by_q0v_frame.values().map(|frame| frame.bytes).sum();
        while bytes > 128 * 1024 * 1024 || self.by_q0v_frame.len() > 64 {
            let oldest = self
                .by_q0v_frame
                .iter()
                .filter(|(_, frame)| frame.last_used != paint_frame)
                .min_by_key(|(_, frame)| frame.last_used)
                .map(|(key, _)| *key);
            let Some(oldest) = oldest else { break };
            if let Some(removed) = self.by_q0v_frame.remove(&oldest) {
                bytes = bytes.saturating_sub(removed.bytes);
            }
        }
        Some((texture, size))
    }

    fn mask_clips(&self, q0rg_id: u16, mask_layer_id: u16) -> bool {
        self.editing_mask_q0rg_id != Some(q0rg_id)
            || self
                .hidden_mask_art_layers
                .contains(&(q0rg_id, mask_layer_id))
    }

    fn mask_art_visible(&self, q0rg_id: u16, mask_layer_id: u16) -> bool {
        self.editing_mask_q0rg_id == Some(q0rg_id)
            && !self
                .hidden_mask_art_layers
                .contains(&(q0rg_id, mask_layer_id))
    }

    pub(crate) fn canvas_background_texture(
        &mut self,
        ctx: &Context,
        path: &Path,
    ) -> Option<(TextureId, [usize; 2])> {
        if self
            .canvas_background
            .as_ref()
            .is_none_or(|cached| cached.path != path)
        {
            let decoded = crate::bitmap_import::decode_bitmap_path(path, u16::MAX)
                .ok()
                .map(|bitmap| {
                    let size = [usize::from(bitmap.width), usize::from(bitmap.height)];
                    let image = ColorImage::from_rgba_unmultiplied(size, &bitmap.rgba);
                    let texture = ctx.load_texture(
                        format!("canvas-background:{}", path.display()),
                        image,
                        TextureOptions::LINEAR,
                    );
                    (texture, size)
                });
            self.canvas_background = Some(match decoded {
                Some((texture, size)) => CachedCanvasBackground {
                    path: path.to_path_buf(),
                    texture: Some(texture),
                    size,
                },
                None => CachedCanvasBackground {
                    path: path.to_path_buf(),
                    texture: None,
                    size: [0, 0],
                },
            });
        }
        self.canvas_background.as_ref().and_then(|cached| {
            cached
                .texture
                .as_ref()
                .map(|texture| (texture.id(), cached.size))
        })
    }

    pub(crate) fn reset_stage_traversal_stats(&mut self) {
        self.stage_traversal_stats = StageTraversalStats::default();
        self.stage_work_timings = StageWorkTimings::default();
    }

    pub(crate) fn record_stage_work_timings(&mut self, perf: &mut crate::perf::PerfLog) {
        std::mem::take(&mut self.stage_work_timings).record(perf);
    }

    pub(crate) fn record_stage_q0rg_visit(&mut self) {
        self.stage_traversal_stats.q0rg_visits =
            self.stage_traversal_stats.q0rg_visits.saturating_add(1);
    }

    pub(crate) fn record_stage_active_placements(&mut self, amount: usize) {
        self.stage_traversal_stats.active_placements = self
            .stage_traversal_stats
            .active_placements
            .saturating_add(amount as u64);
    }

    pub(crate) fn stage_traversal_stats(&self) -> StageTraversalStats {
        self.stage_traversal_stats
    }

    fn record_vector_mesh_batch(&mut self, batch: &VectorMeshBatch) {
        self.stage_traversal_stats.vector_instances_batched = self
            .stage_traversal_stats
            .vector_instances_batched
            .saturating_add(batch.instances);
        self.stage_traversal_stats.vector_batches = self
            .stage_traversal_stats
            .vector_batches
            .saturating_add(batch.batches);
        self.stage_traversal_stats.vector_vertices = self
            .stage_traversal_stats
            .vector_vertices
            .saturating_add(batch.vertices);
        self.stage_traversal_stats.vector_indices = self
            .stage_traversal_stats
            .vector_indices
            .saturating_add(batch.indices);
        self.stage_traversal_stats.vector_fallbacks = self
            .stage_traversal_stats
            .vector_fallbacks
            .saturating_add(batch.fallbacks);
    }

    fn render_index(
        &mut self,
        project: &ProjectV2,
        revision: Option<(u64, u64)>,
    ) -> Arc<RenderIndex> {
        if revision.is_some() && self.vector_bounds_revision != revision {
            self.vector_bounds_by_asset.clear();
            self.vector_bounds_revision = revision;
        }
        if revision.is_some() && self.active_placement_indexes_revision != revision {
            self.active_placement_indexes.clear();
            self.active_placement_indexes_revision = revision;
        }
        let rebuild = self
            .render_index
            .as_ref()
            .is_none_or(|index| !index.matches(project))
            || (revision.is_some() && self.render_index_revision != revision);
        if rebuild {
            self.render_index = Some(Arc::new(RenderIndex::new(project)));
        }
        self.render_index_revision = revision;
        Arc::clone(
            self.render_index
                .as_ref()
                .expect("render index initialized"),
        )
    }

    pub(crate) fn with_placement_bounds_cache<R>(
        &mut self,
        project: &ProjectV2,
        revision: (u64, u64),
        render: impl FnOnce(&mut PlacementBoundsCache, &mut TextureCache) -> R,
    ) -> R {
        let mut bounds = self.take_placement_bounds_cache(project, revision);
        let result = render(&mut bounds, self);
        self.store_placement_bounds_cache(bounds, revision);
        result
    }

    pub(crate) fn take_placement_bounds_cache(
        &mut self,
        project: &ProjectV2,
        revision: (u64, u64),
    ) -> PlacementBoundsCache {
        match self.placement_bounds_cache.take() {
            Some(cached) if cached.revision == revision => cached.bounds,
            _ => PlacementBoundsCache::new(project),
        }
    }

    pub(crate) fn store_placement_bounds_cache(
        &mut self,
        bounds: PlacementBoundsCache,
        revision: (u64, u64),
    ) {
        self.placement_bounds_cache = Some(ScopedPlacementBoundsCache { revision, bounds });
    }

    pub(crate) fn scoped_active_placement_states_at(
        &mut self,
        project: &ProjectV2,
        revision: (u64, u64),
        q0rg_id: u16,
        layer: &q0s_format::v2::Layer,
        frame: u16,
    ) -> Vec<q0s_format::raster::ActivePlacementState> {
        if layer
            .timeline_frame_count
            .is_some_and(|frame_count| frame >= frame_count)
        {
            return Vec::new();
        }
        self.render_index(project, Some(revision));
        if self.active_placement_indexes_revision != Some(revision) {
            self.active_placement_indexes.clear();
            self.active_placement_indexes_revision = Some(revision);
        }
        self.refresh_layer_structure(q0rg_id, layer);
        self.active_placement_indexes
            .entry((q0rg_id, layer.layer_id))
            .or_insert_with(|| LayerFrameIndex::new(layer))
            .resolve_shared(layer, frame)
            .as_ref()
            .clone()
    }

    pub(crate) fn scoped_rig_pose(
        &mut self,
        project: &ProjectV2,
        revision: (u64, u64),
        q0rg_id: u16,
        local_frame: u16,
    ) -> Option<Rc<q0s_format::rig::RigPose>> {
        self.render_index(project, Some(revision));
        if self.rig_pose_cache_revision != Some(revision) {
            self.rig_pose_cache.clear();
            self.rig_pose_cache_revision = Some(revision);
        }
        let key = (q0rg_id, local_frame);
        if let Some(pose) = self.rig_pose_cache.get(&key) {
            return pose.clone();
        }
        let pose = RenderLookup::new(
            project,
            Arc::clone(
                self.render_index
                    .as_ref()
                    .expect("render index initialized"),
            ),
            false,
        )
        .rig_pose(q0rg_id, local_frame);
        self.rig_pose_cache.insert(key, pose.clone());
        pose
    }

    pub(crate) fn placement_transforms_changed(
        &mut self,
        project: &ProjectV2,
        revision: (u64, u64),
        locators: &[(u16, u16, usize)],
    ) {
        if locators.is_empty() {
            return;
        }
        // Live transforms may keep render_revision stable while changing the
        // bounds of library symbols that contain the edited placement.
        self.library_frame_previews.clear();
        // A software mask tile includes placement positions. It cannot survive
        // live transform edits whose project render revision stays unchanged.
        self.mask_frame_textures.clear();
        if let Some(cache) = self.placement_bounds_cache.as_mut() {
            cache.bounds.invalidate_placement_transforms();
        }
        // A bound placement move edits its rig asset, not placement.transform.
        // Refresh every pose of the touched rig, including its descendants.
        let index = self.render_index(project, Some(revision));
        let rig_owners: HashSet<u16> = locators
            .iter()
            .filter_map(|(q0rg_id, _, _)| index.rigs.contains_key(q0rg_id).then_some(*q0rg_id))
            .collect();
        if !rig_owners.is_empty() {
            self.rig_pose_cache
                .retain(|(q0rg_id, _), _| !rig_owners.contains(q0rg_id));
            // Cached filtered parent symbols contain the rig's old pose even
            // though the authored placements themselves have not changed.
            self.fx_placement_textures.retain(|key, _| {
                let target = index
                    .q0rgs
                    .get(&key.host_q0rg_id)
                    .and_then(|q0rg_index| project.q0rgs.get(*q0rg_index))
                    .and_then(|q0rg| {
                        q0rg.layers
                            .iter()
                            .find(|layer| layer.layer_id == key.layer_id)
                    })
                    .and_then(|layer| layer.placements.get(key.placement_idx))
                    .map(|placement| placement.target);
                !rig_owners.contains(&key.host_q0rg_id)
                    && !matches!(target, Some(Target::Q0rg(root)) if rig_owners.iter().any(|owner|
                        q0rg_reaches_q0rg(project, root, *owner, Q0RG_RECURSION_LIMIT)))
            });
        }
        // Live drags keep render_revision stable, but LayerFrameIndex also memoizes
        // the resolved transform values for its last frame. Invalidate only the
        // touched layers so artwork and selection geometry refresh together.
        for &(q0rg_id, layer_id, _) in locators {
            if let Some(index) = self.active_placement_indexes.get_mut(&(q0rg_id, layer_id)) {
                index.invalidate_resolved_frame();
            }
        }
        if self.fx_project_revision != Some(revision) {
            return;
        }
        let updates = (|| {
            let snapshot = self.fx_project_snapshot.as_ref()?;
            let mut updates = Vec::with_capacity(locators.len());
            let mut seen = std::collections::HashSet::with_capacity(locators.len());
            for &(q0rg_id, layer_id, placement_idx) in locators {
                if !seen.insert((q0rg_id, layer_id, placement_idx)) {
                    continue;
                }
                let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
                let snapshot_q0rg_index = snapshot
                    .q0rgs
                    .iter()
                    .position(|candidate| candidate.q0rg_id == q0rg_id)?;
                let layer = q0rg
                    .layers
                    .iter()
                    .find(|layer| layer.layer_id == layer_id)?;
                let snapshot_layer_index = snapshot.q0rgs[snapshot_q0rg_index]
                    .layers
                    .iter()
                    .position(|candidate| candidate.layer_id == layer_id)?;
                let placement = layer.placements.get(placement_idx)?;
                let snapshot_q0rg = &snapshot.q0rgs[snapshot_q0rg_index];
                let snapshot_layer = &snapshot_q0rg.layers[snapshot_layer_index];
                let snapshot_placement = snapshot_layer.placements.get(placement_idx)?;
                if snapshot_q0rg.name != q0rg.name
                    || snapshot_q0rg.frame_count != q0rg.frame_count
                    || snapshot_q0rg.script != q0rg.script
                    || snapshot_layer.name != layer.name
                    || snapshot_layer.explicit_keyframes != layer.explicit_keyframes
                    || snapshot_placement.frame != placement.frame
                    || snapshot_placement.target != placement.target
                    || snapshot_placement.tween != placement.tween
                    || snapshot_placement.fx != placement.fx
                {
                    return None;
                }
                if snapshot_placement.transform != placement.transform {
                    updates.push((
                        snapshot_q0rg_index,
                        snapshot_layer_index,
                        placement_idx,
                        FxProjectChange {
                            q0rg_id,
                            layer_id,
                            placement_idx,
                            blend_mode_changed: false,
                        },
                        placement.transform,
                    ));
                }
            }
            Some(updates)
        })();
        let Some(updates) = updates else {
            self.fx_project_revision = None;
            return;
        };
        if updates.is_empty() {
            return;
        }
        self.fx_placement_textures.retain(|key, _| {
            updates.iter().all(|(_, _, _, changed, _)| {
                !fx_cache_depends_on_project_change(project, *key, *changed)
            })
        });
        if let Some(snapshot) = self.fx_project_snapshot.as_mut() {
            for (q0rg_index, layer_index, placement_idx, _, transform) in updates {
                snapshot.q0rgs[q0rg_index].layers[layer_index].placements[placement_idx]
                    .transform = transform;
            }
        }
    }

    fn asset_intersects_clip(
        &mut self,
        project: &ProjectV2,
        asset_id: u16,
        asset: &Asset,
        transform: Affine,
        view: &StageView,
        clip_rect: Rect,
    ) -> bool {
        if project.asset_appearances.contains_key(&asset_id) {
            return true;
        }
        let bounds = match asset {
            Asset::Bitmap(bitmap) => {
                Some((0.0, 0.0, f32::from(bitmap.width), f32::from(bitmap.height)))
            }
            Asset::Vector(vector) if self.render_index_revision.is_some() => *self
                .vector_bounds_by_asset
                .entry(asset_id)
                .or_insert_with(|| vector_conservative_local_bounds(vector)),
            Asset::Vector(vector) => vector_conservative_local_bounds(vector),
            Asset::Q0v(_) | Asset::Rig(_) => return true,
        };
        let Some(bounds) = bounds else {
            return false;
        };
        conservative_screen_bounds(bounds, transform, view)
            .is_none_or(|bounds| bounds.expand(2.0).intersects(clip_rect))
    }

    fn refresh_layer_structure(&mut self, q0rg_id: u16, layer: &q0s_format::v2::Layer) {
        // Auto-keying can change topology while live drags keep render_revision
        // stable. Refresh the touched layer before resolving artwork or hits.
        if self
            .active_placement_indexes
            .get(&(q0rg_id, layer.layer_id))
            .is_some_and(|index| !index.matches_structure(layer))
        {
            self.active_placement_indexes
                .remove(&(q0rg_id, layer.layer_id));
            if let Some(cache) = self.placement_bounds_cache.as_mut() {
                cache.bounds.invalidate_placement_transforms();
            }
            self.fx_project_revision = None;
            self.mask_frame_textures.clear();
        }
    }
    fn active_placement_states_at(
        &mut self,
        q0rg_id: u16,
        layer: &q0s_format::v2::Layer,
        frame: u16,
    ) -> Arc<Vec<q0s_format::raster::ActivePlacementState>> {
        if layer
            .timeline_frame_count
            .is_some_and(|frame_count| frame >= frame_count)
        {
            return Arc::new(Vec::new());
        }
        let Some(revision) = self.render_index_revision else {
            return Arc::new(q0s_format::raster::active_placement_states_at(layer, frame));
        };
        if self.active_placement_indexes_revision != Some(revision) {
            self.active_placement_indexes.clear();
            self.active_placement_indexes_revision = Some(revision);
        }
        self.refresh_layer_structure(q0rg_id, layer);
        self.active_placement_indexes
            .entry((q0rg_id, layer.layer_id))
            .or_insert_with(|| LayerFrameIndex::new(layer))
            .resolve_shared(layer, frame)
    }

    pub(crate) fn set_live_raw_stroke_scale_compensation(
        &mut self,
        asset_id: u16,
        compensation: f32,
    ) {
        if compensation.is_finite() && compensation > 0.0 {
            self.live_raw_stroke_scale_compensation
                .insert(asset_id, compensation);
        }
    }

    pub(crate) fn clear_live_raw_stroke_scale_compensation(&mut self, asset_id: u16) {
        self.live_raw_stroke_scale_compensation.remove(&asset_id);
    }

    pub(crate) fn live_raw_stroke_scale_compensation(&self, asset_id: u16) -> f32 {
        self.live_raw_stroke_scale_compensation
            .get(&asset_id)
            .copied()
            .unwrap_or(1.0)
    }

    pub fn begin_classic_brush_preview(
        &mut self,
        ctx: &Context,
        screen_rect: Rect,
        view: &StageView,
    ) {
        let pixels_per_point = ctx.pixels_per_point().clamp(1.0, 2.0);
        let width = (screen_rect.width().max(1.0) * pixels_per_point)
            .ceil()
            .max(1.0) as usize;
        let height = (screen_rect.height().max(1.0) * pixels_per_point)
            .ceil()
            .max(1.0) as usize;
        let size = [width, height];
        let image = ColorImage::new(size, Color32::TRANSPARENT);
        let texture = ctx.load_texture("classic_brush_prerender", image, TextureOptions::LINEAR);
        self.classic_brush_preview = Some(ClassicBrushPreviewTexture {
            texture,
            pixels: vec![Color32::TRANSPARENT; width * height],
            size,
            screen_rect,
            pixels_per_point,
            view_origin: view.origin,
            view_scale: view.effective_scale(),
            view_transform: view.screen_transform(),
        });
    }

    pub fn classic_brush_preview_matches_view(&self, screen_rect: Rect, view: &StageView) -> bool {
        let Some(preview) = self.classic_brush_preview.as_ref() else {
            return false;
        };
        preview.screen_rect == screen_rect
            && (preview.view_origin - view.origin).length_sq() <= 1.0e-6
            && (preview.view_scale - view.effective_scale()).abs() <= 1.0e-6
            && preview.view_transform == view.screen_transform()
    }

    pub fn clear_classic_brush_preview(&mut self) {
        self.classic_brush_preview = None;
    }

    pub fn raster_classic_brush_preview(&mut self, polygons: &[Vec<Pos2>], color: Color32) {
        self.raster_classic_brush_preview_mode(polygons, color, false);
    }

    pub fn raster_classic_brush_preview_outline(&mut self, polygons: &[Vec<Pos2>], color: Color32) {
        // The eraser body and outline share one preview texture. New outline
        // pixels may extend into body pixels painted by an older segment; those
        // are interior to the unioned sweep and must stay body-coloured. The new
        // body can still overwrite an old outline when that edge becomes interior.
        self.raster_classic_brush_preview_mode(polygons, color, true);
    }

    fn raster_classic_brush_preview_mode(
        &mut self,
        polygons: &[Vec<Pos2>],
        color: Color32,
        transparent_only: bool,
    ) {
        let Some(preview) = self.classic_brush_preview.as_mut() else {
            return;
        };
        let width = preview.size[0] as isize;
        let height = preview.size[1] as isize;
        let ppp = preview.pixels_per_point;
        let mut dirty: Option<(usize, usize, usize, usize)> = None;

        for polygon in polygons.iter().filter(|polygon| polygon.len() >= 3) {
            let min_x_screen = polygon.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
            let min_y_screen = polygon.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
            let max_x_screen = polygon
                .iter()
                .map(|p| p.x)
                .fold(f32::NEG_INFINITY, f32::max);
            let max_y_screen = polygon
                .iter()
                .map(|p| p.y)
                .fold(f32::NEG_INFINITY, f32::max);
            if !min_x_screen.is_finite()
                || !min_y_screen.is_finite()
                || !max_x_screen.is_finite()
                || !max_y_screen.is_finite()
            {
                continue;
            }

            let min_x = (((min_x_screen - preview.screen_rect.min.x) * ppp).floor() as isize - 1)
                .clamp(0, width.saturating_sub(1));
            let min_y = (((min_y_screen - preview.screen_rect.min.y) * ppp).floor() as isize - 1)
                .clamp(0, height.saturating_sub(1));
            let max_x = (((max_x_screen - preview.screen_rect.min.x) * ppp).ceil() as isize + 1)
                .clamp(0, width.saturating_sub(1));
            let max_y = (((max_y_screen - preview.screen_rect.min.y) * ppp).ceil() as isize + 1)
                .clamp(0, height.saturating_sub(1));
            if min_x > max_x || min_y > max_y {
                continue;
            }

            #[cfg(test)]
            {
                self.classic_preview_candidate_pixels +=
                    (max_x - min_x + 1) as usize * (max_y - min_y + 1) as usize;
            }
            for y in min_y..=max_y {
                let screen_y = preview.screen_rect.min.y + (y as f32 + 0.5) / ppp;
                let row = y as usize * preview.size[0];
                for x in min_x..=max_x {
                    let screen_x = preview.screen_rect.min.x + (x as f32 + 0.5) / ppp;
                    if preview_polygon_contains(pos2(screen_x, screen_y), polygon) {
                        let pixel = &mut preview.pixels[row + x as usize];
                        if !transparent_only || *pixel == Color32::TRANSPARENT {
                            *pixel = color;
                        }
                    }
                }
            }

            let bounds = (
                min_x as usize,
                min_y as usize,
                max_x as usize,
                max_y as usize,
            );
            dirty = Some(match dirty {
                None => bounds,
                Some((x0, y0, x1, y1)) => (
                    x0.min(bounds.0),
                    y0.min(bounds.1),
                    x1.max(bounds.2),
                    y1.max(bounds.3),
                ),
            });
        }

        let Some((x0, y0, x1, y1)) = dirty else {
            return;
        };
        let sub_width = x1 - x0 + 1;
        let sub_height = y1 - y0 + 1;
        let mut sub_pixels = Vec::with_capacity(sub_width * sub_height);
        for y in y0..=y1 {
            let row = y * preview.size[0];
            sub_pixels.extend_from_slice(&preview.pixels[row + x0..=row + x1]);
        }
        #[cfg(test)]
        {
            self.classic_preview_uploaded_pixels += sub_width * sub_height;
        }
        preview.texture.set_partial(
            [x0, y0],
            ColorImage {
                size: [sub_width, sub_height],
                pixels: sub_pixels,
            },
            TextureOptions::LINEAR,
        );
    }

    pub fn paint_classic_brush_preview(&self, painter: &Painter) -> bool {
        let Some(preview) = self.classic_brush_preview.as_ref() else {
            return false;
        };
        painter.image(
            preview.texture.id(),
            preview.screen_rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        true
    }

    pub fn invalidate(&mut self) {
        self.library_frame_previews.clear();
        self.library_frame_preview_revision = None;
        self.live_raw_stroke_scale_compensation.clear();
        self.by_asset_id.clear();
        self.vector_render_by_asset.clear();
        self.vector_group_fill_by_asset.clear();
        self.vector_bounds_by_asset.clear();
        // Video sources are immutable. Transform/undo invalidation must not
        // decode and upload them again; source identity is checked on access.
        // Artwork edits do not change immutable source audio. Keep completed
        // peaks and pending jobs; identify replaced sources by their Arc below.
        self.audio_waveforms.retain(|_, cache| cache.source_alive());
        self.fx_placement_textures.clear();
        self.mask_frame_textures.clear();
        self.mask_frame_revision = None;
        self.placement_bounds_cache = None;
        self.rig_pose_cache.clear();
        self.rig_pose_cache_revision = None;
        self.classic_brush_preview = None;
        #[cfg(test)]
        {
            self.classic_preview_candidate_pixels = 0;
            self.classic_preview_uploaded_pixels = 0;
        }
        #[cfg(feature = "appearance-mask-eraser")]
        {
            self.appearance_by_asset.clear();
            self.appearance_raster_jobs.clear();
            self.appearance_signature_by_asset.clear();
            self.visible_body_by_asset.clear();
            self.visible_body_mesh_by_asset.clear();
            self.visible_selection_body_by_asset.clear();
            self.appearance_hit_by_asset.clear();
            self.raw_selection_components_by_key.clear();
            self.selection_source_surface_by_key.clear();
            self.selection_geometry_by_key.clear();
            self.selection_stage_mesh_by_key.clear();
            self.selection_paint_by_key.clear();
            self.raw_area_selection = None;
        }
    }

    /// Drop cached render data only for assets whose model data actually changed.
    /// Brush commits used to clear every halo/body texture in the project, so the
    /// Nth Advanced stroke forced all previous strokes through raster/boolean work
    /// again. Unrelated assets are immutable and can safely keep their caches.
    pub(crate) fn invalidate_asset(&mut self, asset_id: u16) {
        // A library preview may reach the edited asset through nested symbols.
        self.library_frame_previews.clear();
        if let Some(cache) = self.placement_bounds_cache.as_mut() {
            cache.bounds.invalidate_asset(asset_id);
        }
        self.live_raw_stroke_scale_compensation.remove(&asset_id);
        self.by_asset_id.remove(&asset_id);
        self.vector_render_by_asset.remove(&asset_id);
        self.vector_group_fill_by_asset.remove(&asset_id);
        self.vector_bounds_by_asset.remove(&asset_id);
        // Media pixels are keyed by immutable source, not editable item ID.
        // Source synchronization drops unused payloads after a model change.
        self.audio_waveforms.remove(&asset_id);
        // Placement FX can contain this asset through an arbitrarily nested q0rg.
        // Asset edits are comparatively cold; clear the soft-effect cache here so
        // the hot placement-translation path never has to deep-compare vector/
        // bitmap payloads merely to prove that only tx/ty changed.
        self.fx_placement_textures.clear();
        #[cfg(feature = "appearance-mask-eraser")]
        {
            self.appearance_by_asset
                .retain(|(id, _, _), _| *id != asset_id);
            self.appearance_raster_jobs
                .retain(|(id, _, _), _| *id != asset_id);
            self.appearance_signature_by_asset.remove(&asset_id);
            self.visible_body_by_asset.remove(&asset_id);
            self.visible_body_mesh_by_asset.remove(&asset_id);
            self.visible_selection_body_by_asset.remove(&asset_id);
            self.appearance_hit_by_asset.remove(&asset_id);
            self.raw_selection_components_by_key
                .retain(|(id, _), _| *id != asset_id);
            self.selection_source_surface_by_key
                .retain(|(id, _), _| *id != asset_id);
            self.selection_geometry_by_key
                .retain(|(id, _), _| *id != asset_id);
            self.selection_stage_mesh_by_key
                .retain(|key, _| key.asset_id != asset_id);
            self.selection_paint_by_key
                .retain(|key, _| key.asset_id != asset_id);
            if self
                .raw_area_selection
                .as_ref()
                .is_some_and(|(key, _)| key.assets.iter().any(|(id, _)| *id == asset_id))
            {
                self.raw_area_selection = None;
            }
        }
    }

    pub(crate) fn invalidate_assets(&mut self, asset_ids: impl IntoIterator<Item = u16>) {
        for asset_id in asset_ids {
            self.invalidate_asset(asset_id);
        }
    }

    pub(crate) fn audio_waveform(
        &mut self,
        project: &ProjectV2,
        asset_id: u16,
    ) -> Option<(&crate::audio::AudioWaveform, bool)> {
        let bytes = project.assets.iter().find_map(|asset| match asset {
            Asset::Q0v(media) if media.asset_id == asset_id => Some(&media.bytes),
            _ => None,
        })?;
        if !self
            .audio_waveforms
            .get(&asset_id)
            .is_some_and(|cache| cache.matches_source(bytes))
        {
            self.audio_waveforms.insert(
                asset_id,
                crate::audio::AudioWaveformCache::new(std::sync::Arc::clone(bytes))?,
            );
        }
        Some(self.audio_waveforms.get_mut(&asset_id)?.poll())
    }

    fn vector_render_geometry(&mut self, vector: &VectorAsset) -> &CachedVectorRenderGeometry {
        let asset_id = vector.asset_id;
        if let std::collections::hash_map::Entry::Vacant(entry) =
            self.vector_render_by_asset.entry(asset_id)
        {
            entry.insert(build_cached_vector_render_geometry(vector));
            #[cfg(test)]
            {
                self.vector_render_geometry_build_count += 1;
            }
        }
        self.vector_render_by_asset
            .get(&asset_id)
            .expect("vector render geometry inserted above")
    }

    fn vector_group_fill(&mut self, vector: &VectorAsset) -> Option<CachedVectorGroupFill> {
        if let Some(cached) = self.vector_group_fill_by_asset.get(&vector.asset_id) {
            return Some(cached.clone());
        }
        let fill = vector.fill?;
        let geometry = self.vector_render_geometry(vector);
        let bounds = geometry.fill_bounds?;
        if geometry.fill_vertices.is_empty() || geometry.fill_indices.is_empty() {
            return None;
        }
        let mut mesh = Mesh::default();
        mesh.vertices.reserve(geometry.fill_vertices.len());
        mesh.vertices
            .extend(geometry.fill_vertices.iter().map(|point| Vertex {
                pos: pos2(point.x, point.y),
                uv: Pos2::ZERO,
                color: Color32::WHITE,
            }));
        mesh.indices.clone_from(&geometry.fill_indices);
        let stop_color = fill;
        let cached = CachedVectorGroupFill {
            mesh: Arc::new(mesh),
            bounds,
            stops: Arc::new(vec![
                q0s_format::v2::GradientStop {
                    position: 0.0,
                    color: stop_color,
                },
                q0s_format::v2::GradientStop {
                    position: 1.0,
                    color: stop_color,
                },
            ]),
        };
        self.vector_group_fill_by_asset
            .insert(vector.asset_id, cached.clone());
        Some(cached)
    }

    #[cfg(test)]
    pub(crate) fn vector_render_geometry_build_count(&self) -> usize {
        self.vector_render_geometry_build_count
    }

    fn gradient_render_geometry(
        &mut self,
        vector: &VectorAsset,
        appearance: &VectorAppearance,
    ) -> &CachedGradientRenderGeometry {
        let signature = gradient_render_signature(appearance);
        let rebuild = self
            .gradient_render_by_asset
            .get(&vector.asset_id)
            .is_none_or(|cached| cached.signature != signature);
        if rebuild {
            let built = build_cached_gradient_render_geometry(vector, appearance, signature);
            self.gradient_render_by_asset.insert(vector.asset_id, built);
            #[cfg(test)]
            {
                self.gradient_render_geometry_build_count += 1;
            }
        }
        self.gradient_render_by_asset
            .get(&vector.asset_id)
            .expect("gradient render geometry inserted above")
    }

    #[cfg(test)]
    fn gradient_render_geometry_build_count(&self) -> usize {
        self.gradient_render_geometry_build_count
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn appearance_hit_geometry(
        &mut self,
        vector: &q0s_format::v2::VectorAsset,
        appearance: Option<&VectorAppearance>,
    ) -> &CachedAppearanceHitGeometry {
        let asset_id = vector.asset_id;
        if let std::collections::hash_map::Entry::Vacant(entry) =
            self.appearance_hit_by_asset.entry(asset_id)
        {
            let source_paths = appearance
                .filter(|appearance| !appearance.material_source.is_empty())
                .map(|appearance| appearance.material_source.as_slice())
                .unwrap_or(vector.paths.as_slice());
            let clip_paths = appearance
                .map(|appearance| appearance.clip_mask.as_slice())
                .unwrap_or(&[]);
            let erase_paths = appearance
                .map(|appearance| appearance.erase_mask.as_slice())
                .unwrap_or(&[]);
            entry.insert(CachedAppearanceHitGeometry {
                source: cache_interactive_paths(source_paths),
                clip: cache_interactive_paths(clip_paths),
                erase: cache_interactive_paths(erase_paths),
                strokes: cache_interactive_strokes(vector),
                body_paths: cache_interactive_body_paths(&vector.paths),
            });
            #[cfg(all(test, feature = "appearance-mask-eraser"))]
            {
                self.appearance_hit_build_count += 1;
            }
        }
        self.appearance_hit_by_asset
            .get(&asset_id)
            .expect("appearance hit geometry inserted above")
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn raw_selection_components(
        &mut self,
        vector: &q0s_format::v2::VectorAsset,
        appearance: Option<&VectorAppearance>,
    ) -> Option<&CachedRawSelectionComponents> {
        let field = appearance
            .map(|appearance| appearance.field_transform)
            .unwrap_or(Affine::IDENTITY);
        let inverse_field = field.inverse()?;
        let field_bits = [
            field.a11.to_bits(),
            field.a12.to_bits(),
            field.a21.to_bits(),
            field.a22.to_bits(),
            field.tx.to_bits(),
            field.ty.to_bits(),
        ];
        let key = (vector.asset_id, field_bits);
        if !self.raw_selection_components_by_key.contains_key(&key) {
            let topology = crate::brush::vector_fill_topology(vector);
            let direct_component_paths = topology.component_path_indices;
            let source = topology.surface;
            let mut components = Vec::with_capacity(source.0.len());
            let mut remembered = Vec::new();
            for (component_index, polygon) in source.0.into_iter().enumerate() {
                let mut path_indices: Vec<usize> = direct_component_paths
                    .as_ref()
                    .and_then(|components| components.get(component_index).cloned())
                    .unwrap_or_else(|| {
                        // Crossing/imported contours use the exact boolean fallback.
                        // Only that rare path needs the old boundary ownership scan;
                        // canonical editor-generated raw fills already know their
                        // exterior/hole path ids from the nesting topology pass.
                        vector
                            .paths
                            .iter()
                            .enumerate()
                            .filter(|(_, path)| path.closed)
                            .filter_map(|(index, path)| {
                                path.anchors
                                    .iter()
                                    .any(|anchor| {
                                        polygon_boundary_near_point(&polygon, anchor.point, 0.5)
                                    })
                                    .then_some(index)
                            })
                            .collect()
                    });
                path_indices.sort_unstable();
                path_indices.dedup();
                if path_indices.is_empty() {
                    continue;
                }
                let stage_surface = MultiPolygon(vec![polygon]);
                let canonical_surface = if appearance.is_some() {
                    crate::appearance::transform_surface(&stage_surface, inverse_field)
                } else {
                    stage_surface.clone()
                };
                remembered.push(((vector.asset_id, path_indices.clone()), stage_surface));
                components.push(CachedRawSelectionComponent {
                    canonical_surface,
                    path_indices,
                });
            }
            for (surface_key, surface) in remembered {
                self.selection_source_surface_by_key
                    .entry(surface_key)
                    .or_insert(surface);
            }
            self.raw_selection_components_by_key
                .insert(key, CachedRawSelectionComponents { components });
            #[cfg(all(test, feature = "appearance-mask-eraser"))]
            {
                self.raw_selection_components_build_count += 1;
            }
        }
        self.raw_selection_components_by_key.get(&key)
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn selection_geometry(
        &mut self,
        vector: &q0s_format::v2::VectorAsset,
        appearance: &VectorAppearance,
        selected_indices: &[usize],
    ) -> Option<&CachedSelectionGeometry> {
        let inverse_field = appearance.field_transform.inverse()?;
        let mut key_indices = selected_indices.to_vec();
        key_indices.sort_unstable();
        key_indices.dedup();
        let key = (vector.asset_id, key_indices);
        if !self.selection_geometry_by_key.contains_key(&key) {
            // A normal click already resolved the exact connected source component.
            // Reuse that surface instead of reconstructing the same dense VectorAsset
            // from anchors again when the selection overlay appears one frame later.
            let all_closed_selected = vector
                .paths
                .iter()
                .enumerate()
                .filter(|(_, path)| path.closed)
                .all(|(index, _)| key.1.binary_search(&index).is_ok());
            let cached_full_body = all_closed_selected
                .then(|| {
                    self.visible_selection_body_by_asset
                        .get(&vector.asset_id)
                        .cloned()
                })
                .flatten();
            let (canonical_body, body) = if let Some(body) = cached_full_body {
                // The stage renderer runs before tool interaction and has already
                // resolved this exact full-asset body for masked vectors. Reuse its
                // canonical contours instead of repeating clip/erase booleans on
                // the first selection repaint.
                (None, body)
            } else {
                let cached_source = self.selection_source_surface_by_key.get(&key).cloned();
                let canonical_body = if let Some(source_stage) = cached_source {
                    let source = crate::appearance::transform_surface(&source_stage, inverse_field);
                    let clipped = if appearance.clip_mask.is_empty() {
                        source
                    } else {
                        source.intersection(&crate::appearance::mask_paths_to_coverage(
                            &appearance.clip_mask,
                        ))
                    };
                    let erase = crate::appearance::mask_paths_to_coverage(&appearance.erase_mask);
                    if erase.0.is_empty() {
                        clipped
                    } else {
                        clipped.difference(&erase)
                    }
                } else {
                    let subset = q0s_format::v2::VectorAsset {
                        asset_id: vector.asset_id,
                        paths: key
                            .1
                            .iter()
                            .filter_map(|index| vector.paths.get(*index).cloned())
                            .collect(),
                        fill: vector.fill,
                        stroke: None,
                    };
                    crate::appearance::canonical_visible_source_surface_for_vector(
                        &subset, appearance,
                    )?
                };
                let body = crate::brush::coverage_to_linear_paths(&canonical_body)
                    .iter()
                    .map(flatten_path)
                    .collect();
                (Some(canonical_body), body)
            };
            let canonical_body_is_empty = canonical_body
                .as_ref()
                .map(|surface| surface.0.is_empty())
                .unwrap_or_else(|| body.is_empty());
            let fallback_material_support =
                if canonical_body_is_empty && !appearance.clip_mask.is_empty() {
                    let clip = crate::appearance::mask_paths_to_coverage(&appearance.clip_mask);
                    let erase = crate::appearance::mask_paths_to_coverage(&appearance.erase_mask);
                    let visible = if erase.0.is_empty() {
                        clip
                    } else {
                        clip.difference(&erase)
                    };
                    crate::brush::coverage_to_linear_paths(&visible)
                        .iter()
                        .map(flatten_path)
                        .collect()
                } else {
                    Vec::new()
                };
            self.selection_geometry_by_key.insert(
                key.clone(),
                CachedSelectionGeometry {
                    body,
                    fallback_material_support,
                },
            );
            #[cfg(all(test, feature = "appearance-mask-eraser"))]
            {
                self.selection_geometry_build_count += 1;
            }
        }
        self.selection_geometry_by_key.get(&key)
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn selection_stage_mesh(
        &mut self,
        vector: &q0s_format::v2::VectorAsset,
        appearance: &VectorAppearance,
        selected_indices: &[usize],
    ) -> Option<&mut CachedSelectionStageMesh> {
        let mut path_indices: Vec<usize> = selected_indices
            .iter()
            .copied()
            .filter(|index| vector.paths.get(*index).is_some_and(|path| path.closed))
            .collect();
        path_indices.sort_unstable();
        path_indices.dedup();
        if path_indices.is_empty() {
            return None;
        }
        let field = appearance.field_transform;
        let field_bits = [
            field.a11.to_bits(),
            field.a12.to_bits(),
            field.a21.to_bits(),
            field.a22.to_bits(),
            field.tx.to_bits(),
            field.ty.to_bits(),
        ];
        let key = SelectionStageMeshKey {
            asset_id: vector.asset_id,
            path_indices: path_indices.clone(),
            field_transform_bits: field_bits,
        };
        if !self.selection_stage_mesh_by_key.contains_key(&key) {
            let stage_mesh = if appearance.erase_mask.is_empty()
                && appearance.clip_mask.is_empty()
                && field == Affine::IDENTITY
            {
                // The stage has already flattened+tessellated ordinary vector
                // artwork for painting. Selection must reuse that immutable cache,
                // not independently chew through the same 10k+ anchors again.
                let all_closed: Vec<usize> = vector
                    .paths
                    .iter()
                    .enumerate()
                    .filter_map(|(index, path)| path.closed.then_some(index))
                    .collect();
                let render_geometry = self.vector_render_geometry(vector);
                if path_indices == all_closed {
                    cached_vector_fill_as_selection_stage_mesh(render_geometry)?
                } else {
                    let mut closed_contour = 0usize;
                    let selected: std::collections::BTreeSet<usize> =
                        path_indices.iter().copied().collect();
                    let mut stage_contours = Vec::new();
                    for (path_index, path) in vector.paths.iter().enumerate() {
                        if !path.closed {
                            continue;
                        }
                        if selected.contains(&path_index) {
                            if let Some(contour) = render_geometry.fill_contours.get(closed_contour)
                            {
                                stage_contours.push(contour.clone());
                            }
                        }
                        closed_contour += 1;
                    }
                    build_selection_stage_mesh(&stage_contours)?
                }
            } else {
                let canonical = {
                    let geometry = self.selection_geometry(vector, appearance, &path_indices)?;
                    if geometry.body.is_empty() {
                        geometry.fallback_material_support.clone()
                    } else {
                        geometry.body.clone()
                    }
                };
                let stage_contours: Vec<Vec<Pos2>> = canonical
                    .into_iter()
                    .map(|contour| {
                        contour
                            .into_iter()
                            .map(|point| {
                                let point = field.apply(point);
                                pos2(point.x, point.y)
                            })
                            .collect()
                    })
                    .collect();
                build_selection_stage_mesh(&stage_contours)?
            };
            self.selection_stage_mesh_by_key
                .insert(key.clone(), stage_mesh);
            #[cfg(all(test, feature = "appearance-mask-eraser"))]
            {
                self.selection_stage_mesh_build_count += 1;
            }
        }
        self.selection_stage_mesh_by_key.get_mut(&key)
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn plain_selection_split_cache_seed(
        &self,
        vector: &VectorAsset,
        selected_indices: &[usize],
    ) -> Option<PlainSelectionSplitCacheSeed> {
        let mut path_indices: Vec<usize> = selected_indices
            .iter()
            .copied()
            .filter(|index| vector.paths.get(*index).is_some_and(|path| path.closed))
            .collect();
        path_indices.sort_unstable();
        path_indices.dedup();
        if path_indices.is_empty() {
            return None;
        }
        let identity_bits = [
            1.0_f32.to_bits(),
            0.0_f32.to_bits(),
            0.0_f32.to_bits(),
            1.0_f32.to_bits(),
            0.0_f32.to_bits(),
            0.0_f32.to_bits(),
        ];
        let stage_key = SelectionStageMeshKey {
            asset_id: vector.asset_id,
            path_indices: path_indices.clone(),
            field_transform_bits: identity_bits,
        };
        let stage = self.selection_stage_mesh_by_key.get(&stage_key)?.clone();
        let source_render = self.vector_render_by_asset.get(&vector.asset_id)?;

        let selected: std::collections::BTreeSet<usize> = path_indices.iter().copied().collect();
        let mut closed_contour = 0usize;
        let mut fill_contours = Vec::new();
        let mut remainder_contours = Vec::new();
        for (path_index, path) in vector.paths.iter().enumerate() {
            if !path.closed {
                continue;
            }
            if let Some(contour) = source_render.fill_contours.get(closed_contour) {
                if selected.contains(&path_index) {
                    fill_contours.push(contour.clone());
                } else {
                    remainder_contours.push(contour.clone());
                }
            }
            closed_contour += 1;
        }
        let remainder_render = (!remainder_contours.is_empty()).then(|| {
            split_cached_fill_remainder(source_render, &fill_contours, remainder_contours.clone())
                .unwrap_or_else(|| build_cached_fill_geometry_from_pos2(remainder_contours))
        });
        let fill_vertices: Vec<Vec2> = stage
            .fill_vertices
            .iter()
            .map(|point| Vec2::new(point.x, point.y))
            .collect();
        let render = CachedVectorRenderGeometry {
            fill_bounds: vector_geometry_bounds(&fill_vertices),
            fill_vertices,
            fill_indices: stage.fill_indices.clone(),
            fill_contours,
            stroke_paths: Vec::new(),
            stroke_vertices: Vec::new(),
            stroke_indices: Vec::new(),
        };
        let paints = self
            .selection_paint_by_key
            .iter()
            .filter(|(key, _)| {
                key.asset_id == vector.asset_id
                    && key.path_indices == path_indices
                    && key.field_transform_bits == identity_bits
            })
            .map(|(key, paint)| (key.clone(), paint.clone()))
            .collect();
        Some(PlainSelectionSplitCacheSeed {
            render,
            remainder_render,
            stage,
            paints,
        })
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn install_plain_selection_split_cache_seed(
        &mut self,
        asset_id: u16,
        path_count: usize,
        remainder_asset_id: u16,
        seed: PlainSelectionSplitCacheSeed,
    ) {
        let path_indices: Vec<usize> = (0..path_count).collect();
        let identity_bits = [
            1.0_f32.to_bits(),
            0.0_f32.to_bits(),
            0.0_f32.to_bits(),
            1.0_f32.to_bits(),
            0.0_f32.to_bits(),
            0.0_f32.to_bits(),
        ];
        if let Some(remainder) = seed.remainder_render {
            self.vector_render_by_asset
                .insert(remainder_asset_id, remainder);
            self.vector_group_fill_by_asset.remove(&remainder_asset_id);
        }
        self.vector_render_by_asset.insert(asset_id, seed.render);
        self.vector_group_fill_by_asset.remove(&asset_id);
        self.selection_stage_mesh_by_key.insert(
            SelectionStageMeshKey {
                asset_id,
                path_indices: path_indices.clone(),
                field_transform_bits: identity_bits,
            },
            seed.stage,
        );
        for (mut key, paint) in seed.paints {
            key.asset_id = asset_id;
            key.path_indices = path_indices.clone();
            self.selection_paint_by_key.insert(key, paint);
        }
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn selection_reference_meshes(
        source: &mut CachedSelectionStageMesh,
        reference_scale: f32,
        texture_id: TextureId,
        tile_size_points: f32,
        accent: Color32,
    ) -> Option<Vec<Mesh>> {
        let result = build_selection_reference_meshes(
            source,
            reference_scale,
            texture_id,
            tile_size_points,
            accent,
        );
        source.affine_reprojection_supported = Some(result.is_some());
        result
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn selection_screen_meshes(
        source: &mut CachedSelectionStageMesh,
        placement_transform: Affine,
        view: &StageView,
        texture_id: TextureId,
        tile_size_points: f32,
        accent: Color32,
    ) -> Vec<Mesh> {
        build_selection_screen_meshes(
            source,
            placement_transform,
            view,
            texture_id,
            tile_size_points,
            accent,
            true,
        )
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn selection_screen_meshes_direct(
        source: &mut CachedSelectionStageMesh,
        placement_transform: Affine,
        view: &StageView,
        texture_id: TextureId,
        tile_size_points: f32,
        accent: Color32,
    ) -> Vec<Mesh> {
        build_selection_screen_meshes(
            source,
            placement_transform,
            view,
            texture_id,
            tile_size_points,
            accent,
            false,
        )
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn raw_area_selection(
        &mut self,
        key: &RawAreaSelectionKey,
    ) -> Option<&mut CachedRawAreaSelection> {
        self.raw_area_selection
            .as_mut()
            .filter(|(cached_key, _)| cached_key == key)
            .map(|(_, cached)| cached)
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn insert_raw_area_selection(
        &mut self,
        key: RawAreaSelectionKey,
        geometry: CachedRawAreaSelection,
    ) {
        self.raw_area_selection = Some((key, geometry));
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn selection_paint(&self, key: &SelectionPaintKey) -> Option<&CachedSelectionPaint> {
        self.selection_paint_by_key.get(key)
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn insert_selection_paint(
        &mut self,
        key: SelectionPaintKey,
        paint: CachedSelectionPaint,
    ) {
        self.selection_paint_by_key.retain(|existing, _| {
            existing.asset_id != key.asset_id
                || existing.path_indices != key.path_indices
                || existing.accent_rgba != key.accent_rgba
                || existing.texture_id != key.texture_id
                || (existing.field_transform_bits == key.field_transform_bits
                    && existing.placement_transform_bits == key.placement_transform_bits
                    && existing.view_origin_bits == key.view_origin_bits
                    && existing.view_scale_bits == key.view_scale_bits)
        });
        self.selection_paint_by_key.insert(key, paint);
        #[cfg(all(test, feature = "appearance-mask-eraser"))]
        {
            self.selection_paint_build_count += 1;
        }
    }

    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    pub(crate) fn raw_selection_components_build_count(&self) -> usize {
        self.raw_selection_components_build_count
    }

    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    pub(crate) fn appearance_hit_build_count(&self) -> usize {
        self.appearance_hit_build_count
    }

    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    pub(crate) fn selection_stage_mesh_build_count(&self) -> usize {
        self.selection_stage_mesh_build_count
    }

    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    pub(crate) fn selection_paint_build_count(&self) -> usize {
        self.selection_paint_build_count
    }

    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    fn selection_paint_cache_len(&self) -> usize {
        self.selection_paint_by_key.len()
    }

    #[cfg(feature = "appearance-mask-eraser")]
    fn appearance_signature(
        &mut self,
        vector: &q0s_format::v2::VectorAsset,
        appearance: &VectorAppearance,
    ) -> (u64, Vec2) {
        if let Some(signature) = self.appearance_signature_by_asset.get(&vector.asset_id) {
            return *signature;
        }
        let signature = appearance_cache_signature(vector, appearance);
        self.appearance_signature_by_asset
            .insert(vector.asset_id, signature);
        #[cfg(all(test, feature = "appearance-mask-eraser"))]
        {
            self.appearance_signature_build_count += 1;
        }
        signature
    }
}

fn cached_fill_nonzero_contains(contours: &[Vec<Pos2>], point: Pos2) -> bool {
    let mut winding = 0_i32;
    for contour in contours {
        if contour.len() < 3 {
            continue;
        }
        for index in 0..contour.len() {
            let a = contour[index];
            let b = contour[(index + 1) % contour.len()];
            let side = (b.x - a.x) * (point.y - a.y) - (point.x - a.x) * (b.y - a.y);
            if a.y <= point.y {
                if b.y > point.y && side > 0.0 {
                    winding += 1;
                }
            } else if b.y <= point.y && side < 0.0 {
                winding -= 1;
            }
        }
    }
    winding != 0
}

fn fill_mesh_find(parent: &mut [usize], index: usize) -> usize {
    let mut root = index;
    while parent[root] != root {
        root = parent[root];
    }
    let mut cursor = index;
    while parent[cursor] != cursor {
        let next = parent[cursor];
        parent[cursor] = root;
        cursor = next;
    }
    root
}

fn fill_mesh_union(parent: &mut [usize], rank: &mut [u8], left: usize, right: usize) {
    let mut left = fill_mesh_find(parent, left);
    let mut right = fill_mesh_find(parent, right);
    if left == right {
        return;
    }
    if rank[left] < rank[right] {
        std::mem::swap(&mut left, &mut right);
    }
    parent[right] = left;
    if rank[left] == rank[right] {
        rank[left] = rank[left].saturating_add(1);
    }
}

/// Split a warm tessellated fill by disconnected mesh components instead of
/// tessellating the huge stationary remainder again on mouse-down. Lyon never
/// emits triangles that bridge disconnected NonZero fill components, so shared
/// vertex connectivity gives an exact partition of the rendered mesh.
fn split_cached_fill_remainder(
    source: &CachedVectorRenderGeometry,
    selected_contours: &[Vec<Pos2>],
    remainder_contours: Vec<Vec<Pos2>>,
) -> Option<CachedVectorRenderGeometry> {
    if source.fill_vertices.is_empty()
        || source.fill_indices.is_empty()
        || !source.fill_indices.len().is_multiple_of(3)
        || selected_contours.is_empty()
        || remainder_contours.is_empty()
    {
        return None;
    }

    let vertex_count = source.fill_vertices.len();
    let mut parent: Vec<usize> = (0..vertex_count).collect();
    let mut rank = vec![0_u8; vertex_count];
    for triangle in source.fill_indices.chunks_exact(3) {
        let [a, b, c] = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        if a >= vertex_count || b >= vertex_count || c >= vertex_count {
            return None;
        }
        fill_mesh_union(&mut parent, &mut rank, a, b);
        fill_mesh_union(&mut parent, &mut rank, b, c);
    }

    let mut selected_root = vec![None; vertex_count];
    for triangle in source.fill_indices.chunks_exact(3) {
        let a = triangle[0] as usize;
        let root = fill_mesh_find(&mut parent, a);
        if selected_root[root].is_some() {
            continue;
        }
        let pa = source.fill_vertices[a];
        let pb = source.fill_vertices[triangle[1] as usize];
        let pc = source.fill_vertices[triangle[2] as usize];
        let centroid = pos2((pa.x + pb.x + pc.x) / 3.0, (pa.y + pb.y + pc.y) / 3.0);
        selected_root[root] = Some(cached_fill_nonzero_contains(selected_contours, centroid));
    }

    let mut remap = vec![u32::MAX; vertex_count];
    let mut fill_vertices = Vec::new();
    let mut fill_indices = Vec::new();
    for triangle in source.fill_indices.chunks_exact(3) {
        let root = fill_mesh_find(&mut parent, triangle[0] as usize);
        if selected_root[root].unwrap_or(false) {
            continue;
        }
        for &old in triangle {
            let old = old as usize;
            let mapped = if remap[old] == u32::MAX {
                let mapped = u32::try_from(fill_vertices.len()).ok()?;
                remap[old] = mapped;
                fill_vertices.push(source.fill_vertices[old]);
                mapped
            } else {
                remap[old]
            };
            fill_indices.push(mapped);
        }
    }
    if fill_indices.is_empty() {
        return None;
    }
    let fill_bounds = vector_geometry_bounds(&fill_vertices);
    Some(CachedVectorRenderGeometry {
        fill_vertices,
        fill_indices,
        fill_bounds,
        fill_contours: remainder_contours,
        stroke_paths: Vec::new(),
        stroke_vertices: Vec::new(),
        stroke_indices: Vec::new(),
    })
}

fn build_cached_fill_geometry_from_pos2(
    fill_contours: Vec<Vec<Pos2>>,
) -> CachedVectorRenderGeometry {
    let (fill_vertices, fill_indices): (Vec<Vec2>, Vec<u32>) =
        tessellate_complex_fill(&fill_contours)
            .map(|buffers| {
                (
                    buffers
                        .vertices
                        .into_iter()
                        .map(|point| Vec2::new(point.x, point.y))
                        .collect(),
                    buffers.indices,
                )
            })
            .unwrap_or_default();
    let fill_bounds = vector_geometry_bounds(&fill_vertices);
    CachedVectorRenderGeometry {
        fill_vertices,
        fill_indices,
        fill_bounds,
        fill_contours,
        stroke_paths: Vec::new(),
        stroke_vertices: Vec::new(),
        stroke_indices: Vec::new(),
    }
}

fn build_cached_fill_geometry(contours: &[Vec<Vec2>]) -> CachedVectorRenderGeometry {
    let fill_contours: Vec<Vec<Pos2>> = contours
        .iter()
        .map(|contour| contour.iter().map(|point| pos2(point.x, point.y)).collect())
        .collect();
    let (fill_vertices, fill_indices): (Vec<Vec2>, Vec<u32>) =
        tessellate_complex_fill(&fill_contours)
            .map(|buffers| {
                (
                    buffers
                        .vertices
                        .into_iter()
                        .map(|point| Vec2::new(point.x, point.y))
                        .collect(),
                    buffers.indices,
                )
            })
            .unwrap_or_default();
    let fill_bounds = vector_geometry_bounds(&fill_vertices);
    CachedVectorRenderGeometry {
        fill_vertices,
        fill_indices,
        fill_bounds,
        fill_contours,
        stroke_paths: Vec::new(),
        stroke_vertices: Vec::new(),
        stroke_indices: Vec::new(),
    }
}

fn tessellate_cached_asset_strokes(
    vector: &VectorAsset,
    stroke_paths: &[(Vec<Vec2>, bool)],
) -> (Vec<Vec2>, Vec<u32>) {
    let Some(stroke) = vector.stroke else {
        return (Vec::new(), Vec::new());
    };
    let width = stroke.width;
    if !width.is_finite() || width <= 0.0 {
        return (Vec::new(), Vec::new());
    }
    let line_cap = match stroke.cap {
        q0s_format::geom::CapShape::Round => LineCap::Round,
        q0s_format::geom::CapShape::Butt => LineCap::Butt,
    };
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (points, closed) in stroke_paths {
        if points.len() < 2 {
            continue;
        }
        let mut builder = lyon_path::Path::builder();
        builder.begin(lyon_point(points[0].x, points[0].y));
        let mut valid = true;
        for point in &points[1..] {
            if !point.x.is_finite() || !point.y.is_finite() {
                valid = false;
                break;
            }
            builder.line_to(lyon_point(point.x, point.y));
        }
        if !valid {
            continue;
        }
        builder.end(*closed);
        let path = builder.build();
        let mut buffers: VertexBuffers<lyon_path::math::Point, u32> = VertexBuffers::new();
        let options = StrokeOptions::default()
            .with_line_width(width)
            .with_line_join(LineJoin::Bevel)
            .with_line_cap(line_cap)
            .with_tolerance(0.05);
        let mut tessellator = StrokeTessellator::new();
        if tessellator
            .tessellate_path(
                &path,
                &options,
                &mut BuffersBuilder::new(&mut buffers, Positions),
            )
            .is_err()
        {
            continue;
        }
        let Ok(base) = u32::try_from(vertices.len()) else {
            break;
        };
        vertices.extend(
            buffers
                .vertices
                .into_iter()
                .map(|point| Vec2::new(point.x, point.y)),
        );
        indices.extend(
            buffers
                .indices
                .into_iter()
                .filter_map(|index| index.checked_add(base)),
        );
    }
    (vertices, indices)
}

fn affine_is_similarity(affine: Affine) -> bool {
    let x_len_sq = affine.a11 * affine.a11 + affine.a21 * affine.a21;
    let y_len_sq = affine.a12 * affine.a12 + affine.a22 * affine.a22;
    let dot = affine.a11 * affine.a12 + affine.a21 * affine.a22;
    let scale_sq = x_len_sq.max(y_len_sq).max(1.0e-12);
    (x_len_sq - y_len_sq).abs() <= scale_sq * 1.0e-4 && dot.abs() <= scale_sq * 1.0e-4
}

fn cached_vector_stroke_mesh(
    geometry: &CachedVectorRenderGeometry,
    transform: Affine,
    view: &StageView,
    color: Color32,
) -> Option<Mesh> {
    if geometry.stroke_vertices.is_empty()
        || geometry.stroke_indices.is_empty()
        || !affine_is_similarity(transform)
    {
        return None;
    }
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(geometry.stroke_vertices.len());
    for point in &geometry.stroke_vertices {
        mesh.vertices.push(Vertex {
            pos: stage_to_screen(transform.apply(*point), view),
            uv: Pos2::ZERO,
            color,
        });
    }
    mesh.indices = geometry.stroke_indices.clone();
    Some(mesh)
}

fn build_cached_vector_render_geometry(vector: &VectorAsset) -> CachedVectorRenderGeometry {
    let fill_contours: Vec<Vec<Pos2>> = if vector.fill.is_some() {
        vector
            .paths
            .iter()
            .filter(|path| path.closed)
            .map(|path| {
                flatten_path(path)
                    .into_iter()
                    .map(|point| pos2(point.x, point.y))
                    .collect()
            })
            .collect()
    } else {
        Vec::new()
    };
    let (fill_vertices, fill_indices): (Vec<Vec2>, Vec<u32>) =
        tessellate_complex_fill(&fill_contours)
            .map(|buffers| {
                (
                    buffers
                        .vertices
                        .into_iter()
                        .map(|point| Vec2::new(point.x, point.y))
                        .collect(),
                    buffers.indices,
                )
            })
            .unwrap_or_default();
    let fill_bounds = vector_geometry_bounds(&fill_vertices);
    let stroke_paths = if vector.stroke.is_some() {
        vector
            .paths
            .iter()
            .map(|path| (flatten_path_for_stroke(path), path.closed))
            .collect()
    } else {
        Vec::new()
    };
    let (stroke_vertices, stroke_indices) = tessellate_cached_asset_strokes(vector, &stroke_paths);
    CachedVectorRenderGeometry {
        fill_vertices,
        fill_indices,
        fill_bounds,
        fill_contours,
        stroke_paths,
        stroke_vertices,
        stroke_indices,
    }
}

fn vector_geometry_bounds(vertices: &[Vec2]) -> Option<Rect> {
    let mut min = pos2(f32::INFINITY, f32::INFINITY);
    let mut max = pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut any = false;
    for point in vertices {
        if point.x.is_finite() && point.y.is_finite() {
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
            any = true;
        }
    }
    (any && min.x < max.x && min.y < max.y).then(|| Rect::from_min_max(min, max))
}

fn vector_conservative_local_bounds(vector: &VectorAsset) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for anchor in vector.paths.iter().flat_map(|path| &path.anchors) {
        for point in [Some(anchor.point), anchor.in_handle, anchor.out_handle]
            .into_iter()
            .flatten()
        {
            if !point.x.is_finite() || !point.y.is_finite() {
                continue;
            }
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }
    }
    if !min_x.is_finite() {
        return None;
    }
    let stroke_pad = vector
        .stroke
        .map(|stroke| stroke.width.abs().max(0.5) * 12.0)
        .unwrap_or(0.0);
    Some((
        min_x - stroke_pad,
        min_y - stroke_pad,
        max_x + stroke_pad,
        max_y + stroke_pad,
    ))
}

fn conservative_screen_bounds(
    bounds: (f32, f32, f32, f32),
    transform: Affine,
    view: &StageView,
) -> Option<Rect> {
    let (min_x, min_y, max_x, max_y) = bounds;
    let points = [
        Vec2::new(min_x, min_y),
        Vec2::new(max_x, min_y),
        Vec2::new(max_x, max_y),
        Vec2::new(min_x, max_y),
    ];
    let mut min = pos2(f32::INFINITY, f32::INFINITY);
    let mut max = pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for point in points {
        let screen = stage_to_screen(transform.apply(point), view);
        if !screen.x.is_finite() || !screen.y.is_finite() {
            return None;
        }
        min.x = min.x.min(screen.x);
        min.y = min.y.min(screen.y);
        max.x = max.x.max(screen.x);
        max.y = max.y.max(screen.y);
    }
    Some(Rect::from_min_max(min, max))
}

pub(crate) fn text_preview_mesh(paths: Vec<VPath>) -> Option<Mesh> {
    let vector = VectorAsset {
        asset_id: 0,
        paths,
        fill: Some(Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        }),
        stroke: None,
    };
    cached_vector_fill_mesh(
        &build_cached_vector_render_geometry(&vector),
        Affine::IDENTITY,
        &StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: Rect::EVERYTHING,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        },
        Color32::WHITE,
    )
}

fn cached_vector_fill_mesh(
    geometry: &CachedVectorRenderGeometry,
    transform: Affine,
    view: &StageView,
    color: Color32,
) -> Option<Mesh> {
    if geometry.fill_vertices.is_empty() || geometry.fill_indices.is_empty() {
        return None;
    }
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(geometry.fill_vertices.len());
    for point in &geometry.fill_vertices {
        mesh.vertices.push(Vertex {
            pos: stage_to_screen(transform.apply(*point), view),
            uv: Pos2::ZERO,
            color,
        });
    }
    mesh.indices = geometry.fill_indices.clone();
    Some(mesh)
}

fn gradient_render_signature(appearance: &VectorAppearance) -> u64 {
    // Small deterministic FNV-1a hash. This stays independent of Hash impls on
    // float-heavy format structs and invalidates only when the authored field changes.
    let mut hash = 0xcbf29ce484222325u64;
    let mut feed = |value: u32| {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    let field = appearance.field_transform;
    for value in [
        field.a11, field.a12, field.a21, field.a22, field.tx, field.ty,
    ] {
        feed(value.to_bits());
    }
    if let Some(gradient) = appearance.fill_gradient.as_ref() {
        feed(match gradient.kind {
            GradientKind::Linear => 1,
            GradientKind::Radial => 2,
        });
        let t = gradient.transform;
        for value in [t.a11, t.a12, t.a21, t.a22, t.tx, t.ty] {
            feed(value.to_bits());
        }
        feed(gradient.stops.len() as u32);
        for stop in &gradient.stops {
            feed(stop.position.to_bits());
            feed(u32::from_le_bytes([
                stop.color.r,
                stop.color.g,
                stop.color.b,
                stop.color.a,
            ]));
        }
    }
    hash
}

fn build_cached_gradient_render_geometry(
    vector: &VectorAsset,
    appearance: &VectorAppearance,
    signature: u64,
) -> CachedGradientRenderGeometry {
    let Some(gradient) = appearance.fill_gradient.as_ref() else {
        return CachedGradientRenderGeometry {
            signature,
            ..Default::default()
        };
    };
    let effective = Affine::compose(appearance.field_transform, gradient.transform);
    let inverse = effective.inverse();
    let geometry = build_dynamic_vector_body_geometry(vector, Some(appearance));
    let source_bounds = vector_geometry_bounds(&geometry.fill_vertices);
    let mesh = if geometry.fill_vertices.is_empty() || geometry.fill_indices.is_empty() {
        None
    } else {
        let mut mesh = Mesh::default();
        mesh.vertices.reserve(geometry.fill_vertices.len());
        mesh.indices = geometry.fill_indices.clone();
        mesh.vertices
            .extend(geometry.fill_vertices.iter().map(|point| Vertex {
                // Keep local vector coordinates. q0glblend reprojects them on the GPU
                // and evaluates the gradient analytically per fragment.
                pos: pos2(point.x, point.y),
                uv: Pos2::ZERO,
                color: Color32::WHITE,
            }));
        Some(Arc::new(mesh))
    };
    CachedGradientRenderGeometry {
        signature,
        mesh,
        source_bounds,
        inverse,
        kind: gradient.kind,
        stops: Arc::new(gradient.stops.clone()),
    }
}

fn gradient_mesh_paint(
    geometry: &CachedGradientRenderGeometry,
    transform: Affine,
    view: &StageView,
    tint: Color32,
) -> Option<q0glblend::GradientMeshPaint> {
    let inverse = geometry.inverse?;
    let source_bounds = geometry.source_bounds?;
    Some(q0glblend::GradientMeshPaint {
        transform: cached_vector_screen_transform(transform, view),
        source_bounds,
        gradient_inverse: [
            inverse.a11,
            inverse.a12,
            inverse.a21,
            inverse.a22,
            inverse.tx,
            inverse.ty,
        ],
        kind: geometry.kind,
        stops: Arc::clone(&geometry.stops),
        tint,
        color_transform: q0glblend::ColorTransform::default(),
    })
}

fn cached_vector_screen_transform(
    transform: Affine,
    view: &StageView,
) -> q0glblend::CachedMeshTransform {
    q0glblend::CachedMeshTransform {
        source_origin: Pos2::ZERO,
        target_origin: {
            let screen = Affine::compose(view.screen_transform(), transform);
            pos2(screen.tx, screen.ty)
        },
        linear: {
            let screen = Affine::compose(view.screen_transform(), transform);
            [screen.a11, screen.a12, screen.a21, screen.a22]
        },
        fixed_screen_offset: false,
    }
}

fn solid_vector_group_paint(
    geometry: &CachedVectorGroupFill,
    transform: Affine,
    view: &StageView,
) -> q0glblend::GradientMeshPaint {
    q0glblend::GradientMeshPaint {
        transform: cached_vector_screen_transform(transform, view),
        source_bounds: geometry.bounds,
        // Identical endpoints make the existing analytic gradient shader an
        // exact constant-color vector fill while retaining its cached GPU mesh.
        gradient_inverse: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        kind: GradientKind::Linear,
        stops: Arc::clone(&geometry.stops),
        tint: Color32::WHITE,
        color_transform: q0glblend::ColorTransform::default(),
    }
}

fn paint_cached_vector_gradient_color(
    painter: &Painter,
    geometry: &CachedGradientRenderGeometry,
    transform: Affine,
    view: &StageView,
    tint: Color32,
    color_transform: q0glblend::ColorTransform,
) {
    let (Some(mesh), Some(mut paint)) = (
        geometry.mesh.as_ref(),
        gradient_mesh_paint(geometry, transform, view, tint),
    ) else {
        return;
    };
    paint.color_transform = color_transform;
    q0glblend::paint_gradient_mesh(painter, view.stage_rect, Arc::clone(mesh), paint);
}

fn paint_cached_vector_fill(
    painter: &Painter,
    geometry: &CachedVectorRenderGeometry,
    transform: Affine,
    view: &StageView,
    color: Color32,
) {
    if let Some(mesh) = cached_vector_fill_mesh(geometry, transform, view, color) {
        painter.add(Shape::Mesh(mesh));
    }
}

fn fx_stage_render_size(view: &StageView, ctx: &Context) -> (u32, u32) {
    let pixels_per_point = ctx.pixels_per_point().clamp(1.0, 2.0);
    let mut width = (view.stage_rect.width().max(1.0) * pixels_per_point).ceil();
    let mut height = (view.stage_rect.height().max(1.0) * pixels_per_point).ceil();
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

const CANONICAL_FX_CACHE_PPU: f32 = 2.0;

fn canonical_fx_stage_render_size(project: &ProjectV2) -> (u32, u32) {
    let mut width = f32::from(project.meta.stage_width.max(1)) * CANONICAL_FX_CACHE_PPU;
    let mut height = f32::from(project.meta.stage_height.max(1)) * CANONICAL_FX_CACHE_PPU;
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

fn placement_fx_has_active_blur(fx: PlacementFx) -> bool {
    fx.blur
        .is_some_and(|blur| blur.radius_x > 1.0e-6 || blur.radius_y > 1.0e-6)
}

fn fx_cache_render_size(
    project: &ProjectV2,
    view: &StageView,
    ctx: &Context,
    outer_only: bool,
    fx: PlacementFx,
) -> (u32, u32) {
    // Blur is inherently raster, but camera zoom must not change its source
    // resolution or invalidate the texture. Rasterize once in stage space and
    // let the GPU resample that tile while the camera moves.
    if outer_only || placement_fx_has_active_blur(fx) {
        canonical_fx_stage_render_size(project)
    } else {
        fx_stage_render_size(view, ctx)
    }
}
fn placement_fx_keeps_vector_body(fx: PlacementFx) -> bool {
    let blur_is_identity = fx
        .blur
        .is_none_or(|blur| blur.radius_x <= 1.0e-6 && blur.radius_y <= 1.0e-6);
    let glow_active = fx
        .glow
        .is_some_and(|glow| glow.strength > 1.0e-6 && glow.color.a > 0);
    let shadow_active = fx
        .shadow
        .is_some_and(|shadow| shadow.strength > 1.0e-6 && shadow.color.a > 0);
    fx.tint.is_none_or(|tint| tint.amount <= 1.0e-6)
        && fx.opacity >= 0.999_999
        && fx.blend_mode == BlendMode::Normal
        && blur_is_identity
        && (glow_active || shadow_active)
}
fn placement_fx_is_pure_blend(fx: PlacementFx) -> bool {
    let blur_is_identity = fx
        .blur
        .is_none_or(|blur| blur.radius_x <= 1.0e-6 && blur.radius_y <= 1.0e-6);
    let glow_is_identity = fx
        .glow
        .is_none_or(|glow| glow.strength <= 1.0e-6 || glow.color.a == 0);
    let shadow_is_identity = fx
        .shadow
        .is_none_or(|shadow| shadow.strength <= 1.0e-6 || shadow.color.a == 0);
    fx.tint.is_none_or(|tint| tint.amount <= 1.0e-6)
        && fx.opacity >= 0.999_999
        && fx.blend_mode != BlendMode::Normal
        && blur_is_identity
        && glow_is_identity
        && shadow_is_identity
}

fn placement_fx_is_pure_opacity(fx: PlacementFx) -> bool {
    let blur_is_identity = fx
        .blur
        .is_none_or(|blur| blur.radius_x <= 1.0e-6 && blur.radius_y <= 1.0e-6);
    let glow_is_identity = fx
        .glow
        .is_none_or(|glow| glow.strength <= 1.0e-6 || glow.color.a == 0);
    let shadow_is_identity = fx
        .shadow
        .is_none_or(|shadow| shadow.strength <= 1.0e-6 || shadow.color.a == 0);
    fx.tint.is_none_or(|tint| tint.amount <= 1.0e-6)
        && fx.opacity < 0.999_999
        && fx.blend_mode == BlendMode::Normal
        && blur_is_identity
        && glow_is_identity
        && shadow_is_identity
}

fn placement_fx_is_visual_identity(fx: PlacementFx) -> bool {
    let blur_is_identity = fx
        .blur
        .is_none_or(|blur| blur.radius_x <= 1.0e-6 && blur.radius_y <= 1.0e-6);
    let glow_is_identity = fx
        .glow
        .is_none_or(|glow| glow.strength <= 1.0e-6 || glow.color.a == 0);
    let shadow_is_identity = fx
        .shadow
        .is_none_or(|shadow| shadow.strength <= 1.0e-6 || shadow.color.a == 0);
    fx.tint.is_none_or(|tint| tint.amount <= 1.0e-6)
        && fx.opacity >= 0.999_999
        && fx.blend_mode == BlendMode::Normal
        && blur_is_identity
        && glow_is_identity
        && shadow_is_identity
}

fn placement_fx_has_gpu_blur_or_glow(fx: PlacementFx) -> bool {
    placement_fx_has_active_blur(fx)
        || fx.glow.is_some_and(|glow| {
            glow.strength > 1.0e-6
                && glow.color.a > 0
                && (glow.radius_x > 1.0e-6 || glow.radius_y > 1.0e-6)
        })
}

fn placement_fx_without_tint(mut fx: PlacementFx) -> PlacementFx {
    fx.tint = None;
    fx
}
fn linear_affine_bits(affine: Affine) -> [u32; 4] {
    [
        affine.a11.to_bits(),
        affine.a12.to_bits(),
        affine.a21.to_bits(),
        affine.a22.to_bits(),
    ]
}
#[derive(Clone)]
struct FxPlacementSnapshot {
    frame: u16,
    target: Target,
    tween: Tween,
    transform: Transform2D,
    fx: PlacementFx,
}

#[derive(Clone)]
struct FxLayerSnapshot {
    layer_id: u16,
    name: String,
    explicit_keyframes: Vec<u16>,
    placements: Vec<FxPlacementSnapshot>,
}

#[derive(Clone)]
struct FxQ0rgSnapshot {
    q0rg_id: u16,
    name: String,
    frame_count: u16,
    script: String,
    layers: Vec<FxLayerSnapshot>,
}

#[derive(Clone)]
struct FxProjectSnapshot {
    meta: ProjectMeta,
    layer_metadata: HashMap<LayerKey, LayerMetadata>,
    q0rgs: Vec<FxQ0rgSnapshot>,
}

impl FxProjectSnapshot {
    fn from_project(project: &ProjectV2) -> Self {
        Self {
            meta: project.meta.clone(),
            layer_metadata: project.layer_metadata.clone(),
            q0rgs: project
                .q0rgs
                .iter()
                .map(|q0rg| FxQ0rgSnapshot {
                    q0rg_id: q0rg.q0rg_id,
                    name: q0rg.name.clone(),
                    frame_count: q0rg.frame_count,
                    script: q0rg.script.clone(),
                    layers: q0rg
                        .layers
                        .iter()
                        .map(|layer| FxLayerSnapshot {
                            layer_id: layer.layer_id,
                            name: layer.name.clone(),
                            explicit_keyframes: layer.explicit_keyframes.clone(),
                            placements: layer
                                .placements
                                .iter()
                                .map(|placement| FxPlacementSnapshot {
                                    frame: placement.frame,
                                    target: placement.target,
                                    tween: placement.tween,
                                    transform: placement.transform,
                                    fx: placement.fx,
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct FxProjectChange {
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    blend_mode_changed: bool,
}

impl FxProjectChange {
    fn locator(self) -> (u16, u16, usize) {
        (self.q0rg_id, self.layer_id, self.placement_idx)
    }
}

fn placement_fx_same_except_blend(left: PlacementFx, right: PlacementFx) -> bool {
    left.opacity == right.opacity
        && left.blur == right.blur
        && left.glow == right.glow
        && left.shadow == right.shadow
}

fn project_fx_cache_compatible_changes(
    left: &FxProjectSnapshot,
    right: &FxProjectSnapshot,
) -> Option<Vec<FxProjectChange>> {
    // Asset/appearance edits explicitly invalidate TextureCache. Keep this check
    // deliberately lightweight because it runs on every live placement move.
    if left.meta != right.meta
        || left.layer_metadata != right.layer_metadata
        || left.q0rgs.len() != right.q0rgs.len()
    {
        return None;
    }
    let mut changes = Vec::new();
    for (left_q0rg, right_q0rg) in left.q0rgs.iter().zip(&right.q0rgs) {
        if left_q0rg.q0rg_id != right_q0rg.q0rg_id
            || left_q0rg.name != right_q0rg.name
            || left_q0rg.frame_count != right_q0rg.frame_count
            || left_q0rg.script != right_q0rg.script
            || left_q0rg.layers.len() != right_q0rg.layers.len()
        {
            return None;
        }
        for (left_layer, right_layer) in left_q0rg.layers.iter().zip(&right_q0rg.layers) {
            if left_layer.layer_id != right_layer.layer_id
                || left_layer.name != right_layer.name
                || left_layer.explicit_keyframes != right_layer.explicit_keyframes
                || left_layer.placements.len() != right_layer.placements.len()
            {
                return None;
            }
            for (placement_idx, (left_placement, right_placement)) in left_layer
                .placements
                .iter()
                .zip(&right_layer.placements)
                .enumerate()
            {
                let blend_mode_changed =
                    left_placement.fx.blend_mode != right_placement.fx.blend_mode;
                if left_placement.frame != right_placement.frame
                    || left_placement.target != right_placement.target
                    || left_placement.tween != right_placement.tween
                    || !placement_fx_same_except_blend(left_placement.fx, right_placement.fx)
                {
                    return None;
                }
                let transform_changed = left_placement.transform != right_placement.transform;
                if transform_changed || blend_mode_changed {
                    changes.push(FxProjectChange {
                        q0rg_id: left_q0rg.q0rg_id,
                        layer_id: left_layer.layer_id,
                        placement_idx,
                        blend_mode_changed,
                    });
                }
            }
        }
    }
    Some(changes)
}
fn q0rg_reaches_q0rg(project: &ProjectV2, root: u16, sought: u16, depth: u8) -> bool {
    if root == sought {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == root) else {
        return false;
    };
    q0rg.layers
        .iter()
        .flat_map(|layer| &layer.placements)
        .filter_map(|placement| match placement.target {
            Target::Q0rg(child) => Some(child),
            Target::Asset(_) => None,
        })
        .any(|child| q0rg_reaches_q0rg(project, child, sought, depth - 1))
}

fn fx_cache_depends_on_project_change(
    project: &ProjectV2,
    key: FxPlacementKey,
    changed: FxProjectChange,
) -> bool {
    if (key.host_q0rg_id, key.layer_id, key.placement_idx) == changed.locator() {
        // Moving the carrier or changing only its blend equation does not alter
        // the already-rasterized source pixels for that same placement.
        return false;
    }
    let Some(placement) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == key.host_q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .find(|layer| layer.layer_id == key.layer_id)
        })
        .and_then(|layer| layer.placements.get(key.placement_idx))
    else {
        return true;
    };
    match placement.target {
        Target::Asset(_) => false,
        Target::Q0rg(root) => {
            q0rg_reaches_q0rg(project, root, changed.q0rg_id, Q0RG_RECURSION_LIMIT)
        }
    }
}

fn resolved_blend_mode_for_fx_key(project: &ProjectV2, key: FxPlacementKey) -> Option<BlendMode> {
    let layer = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == key.host_q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == key.layer_id)?;
    q0s_format::raster::active_placement_states_at(layer, key.local_frame)
        .into_iter()
        .find(|active| active.index == key.placement_idx)
        .map(|active| active.fx.blend_mode)
}

fn sync_fx_project_snapshot(
    project: &ProjectV2,
    textures: &mut TextureCache,
    revision: Option<(u64, u64)>,
) {
    if revision.is_some() && textures.fx_project_revision == revision {
        return;
    }
    if !project
        .q0rgs
        .iter()
        .flat_map(|q| &q.layers)
        .flat_map(|l| &l.placements)
        .any(|p| !p.fx.is_identity())
    {
        textures.fx_project_snapshot = None;
        textures.fx_placement_textures.clear();
        textures.fx_project_revision = revision;
        return;
    }
    let current = FxProjectSnapshot::from_project(project);
    let compatible_changes = textures
        .fx_project_snapshot
        .as_ref()
        .and_then(|snapshot| project_fx_cache_compatible_changes(snapshot, &current));
    if let Some(changes) = compatible_changes {
        if !changes.is_empty() {
            textures.fx_placement_textures.retain(|key, _| {
                !changes
                    .iter()
                    .copied()
                    .any(|changed| fx_cache_depends_on_project_change(project, *key, changed))
            });
            for (key, cached) in &mut textures.fx_placement_textures {
                let blend_changed_here = changes.iter().copied().any(|changed| {
                    changed.blend_mode_changed
                        && changed.locator() == (key.host_q0rg_id, key.layer_id, key.placement_idx)
                });
                if blend_changed_here {
                    if let Some(mode) = resolved_blend_mode_for_fx_key(project, *key) {
                        cached.blend_mode = if key.outer_only {
                            BlendMode::Normal
                        } else {
                            mode
                        };
                    }
                }
            }
        }
    } else {
        textures.fx_placement_textures.clear();
    }
    textures.fx_project_snapshot = Some(current);
    textures.fx_project_revision = revision;
}
fn fx_crop_screen_rect(
    project: &ProjectV2,
    width: u32,
    height: u32,
    crop: [u32; 4],
    raster_translation: Vec2,
    current_translation: Vec2,
    view: &StageView,
) -> Rect {
    let stage_width = f32::from(project.meta.stage_width.max(1));
    let stage_height = f32::from(project.meta.stage_height.max(1));
    let [x, y, crop_width, crop_height] = crop;
    let delta = Vec2::new(
        current_translation.x - raster_translation.x,
        current_translation.y - raster_translation.y,
    );
    let to_stage = |px: u32, py: u32| {
        Vec2::new(
            px as f32 / width.max(1) as f32 * stage_width + delta.x,
            py as f32 / height.max(1) as f32 * stage_height + delta.y,
        )
    };
    Rect::from_min_max(
        stage_to_screen(to_stage(x, y), view),
        stage_to_screen(to_stage(x + crop_width, y + crop_height), view),
    )
}

#[allow(clippy::too_many_arguments)]
fn collect_plain_vector_asset_mesh(
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    asset_id: u16,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    tint: Color32,
    meshes: &mut Vec<Mesh>,
) -> bool {
    let Some(asset) = lookup.asset(asset_id) else {
        return false;
    };
    let Asset::Vector(vector) = asset else {
        return false;
    };
    // Soft/material appearances, strokes and media keep the established
    // fallback until their GPU group primitives can preserve the same pixels.
    // Plain vector fills are the common q0rg case and need no CPU raster at all.
    if project.asset_appearances.contains_key(&asset_id) || vector.stroke.is_some() {
        return false;
    }
    let Some(fill) = vector.fill else {
        return true;
    };
    let color = modulate(rgba_to_color32(fill), tint);
    if color == Color32::TRANSPARENT {
        return true;
    }
    let geometry = textures.vector_render_geometry(vector);
    if let Some(mesh) = cached_vector_fill_mesh(geometry, transform, view, color) {
        meshes.push(mesh);
    }
    true
}

fn video_frame_mesh(
    texture: TextureId,
    [width, height]: [f32; 2],
    transform: Affine,
    view: &StageView,
    tint: Color32,
) -> Mesh {
    let corners = [
        Vec2::new(0.0, 0.0),
        Vec2::new(width, 0.0),
        Vec2::new(width, height),
        Vec2::new(0.0, height),
    ];
    let uv = [
        pos2(0.0, 0.0),
        pos2(1.0, 0.0),
        pos2(1.0, 1.0),
        pos2(0.0, 1.0),
    ];
    let mut mesh = Mesh::with_texture(texture);
    for (point, uv) in corners.into_iter().zip(uv) {
        mesh.vertices.push(Vertex {
            pos: stage_to_screen(transform.apply(point), view),
            uv,
            color: tint,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    mesh
}

#[allow(clippy::too_many_arguments)]
fn collect_vector_asset_group_primitives(
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    asset_id: u16,
    elapsed_host_frames: u16,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
    primitives: &mut Vec<q0glblend::VectorGroupPrimitive>,
) -> bool {
    let Some(asset) = lookup.asset(asset_id) else {
        return false;
    };
    let vector = match asset {
        Asset::Bitmap(bitmap) => {
            let texture = textures
                .by_asset_id
                .entry(bitmap.asset_id)
                .or_insert_with(|| {
                    let image = ColorImage::from_rgba_unmultiplied(
                        [usize::from(bitmap.width), usize::from(bitmap.height)],
                        &bitmap.rgba,
                    );
                    ctx.load_texture(
                        format!("q0s_asset_{}", bitmap.asset_id),
                        image,
                        TextureOptions::NEAREST,
                    )
                });
            let local_corners = [
                Vec2::new(0.0, 0.0),
                Vec2::new(f32::from(bitmap.width), 0.0),
                Vec2::new(f32::from(bitmap.width), f32::from(bitmap.height)),
                Vec2::new(0.0, f32::from(bitmap.height)),
            ];
            let uv = [
                pos2(0.0, 0.0),
                pos2(1.0, 0.0),
                pos2(1.0, 1.0),
                pos2(0.0, 1.0),
            ];
            let mut mesh = Mesh::with_texture(texture.id());
            for (point, uv) in local_corners.into_iter().zip(uv) {
                mesh.vertices.push(Vertex {
                    pos: stage_to_screen(transform.apply(point), view),
                    uv,
                    color: tint,
                });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            primitives.push(q0glblend::VectorGroupPrimitive::Textured {
                mesh: Arc::new(mesh),
                color_transform: q0glblend::ColorTransform::default(),
            });
            return true;
        }
        Asset::Vector(vector) => vector,
        Asset::Q0v(video) => {
            if let Some((texture, size)) =
                textures.video_texture(video, elapsed_host_frames, project.meta.fps, ctx)
            {
                primitives.push(q0glblend::VectorGroupPrimitive::Textured {
                    mesh: Arc::new(video_frame_mesh(texture.id(), size, transform, view, tint)),
                    color_transform: q0glblend::ColorTransform::default(),
                });
            }
            return true;
        }
        Asset::Rig(_) => return false,
    };
    let mut fill_transform = transform;
    match project.asset_appearances.get(&asset_id) {
        None => {
            if let Some(group_fill) = textures.vector_group_fill(vector) {
                let mut paint = solid_vector_group_paint(&group_fill, fill_transform, view);
                paint.tint = tint;
                primitives.push(q0glblend::VectorGroupPrimitive::Gradient {
                    mesh: group_fill.mesh,
                    paint,
                });
            }
        }
        Some(appearance) => {
            // Solid fills and Animate gradients remain vector primitives. Soft
            // materials and vector-local erase/clip paths need separate GPU
            // material passes and are left to the established renderer.
            if !matches!(appearance.material, q0s_format::v2::VectorMaterial::Solid) {
                return false;
            }
            fill_transform = Affine::compose(transform, appearance.field_transform);
            if appearance.fill_gradient.is_some() {
                let geometry = textures.gradient_render_geometry(vector, appearance);
                if let (Some(mesh), Some(paint)) = (
                    geometry.mesh.as_ref(),
                    gradient_mesh_paint(geometry, transform, view, tint),
                ) {
                    primitives.push(q0glblend::VectorGroupPrimitive::Gradient {
                        mesh: Arc::clone(mesh),
                        paint,
                    });
                }
            } else if !appearance.erase_mask.is_empty() || !appearance.clip_mask.is_empty() {
                #[cfg(feature = "appearance-mask-eraser")]
                {
                    let Some(geometry) =
                        cached_vector_appearance_body_geometry(textures, vector, appearance)
                    else {
                        return false;
                    };
                    if let Some(fill) = vector.fill {
                        if let Some(mesh) = cached_vector_fill_mesh(
                            geometry,
                            fill_transform,
                            view,
                            modulate(rgba_to_color32(fill), tint),
                        ) {
                            primitives.push(q0glblend::VectorGroupPrimitive::Solid(mesh));
                        }
                    }
                }
                #[cfg(not(feature = "appearance-mask-eraser"))]
                return false;
            } else if let Some(group_fill) = textures.vector_group_fill(vector) {
                let mut paint = solid_vector_group_paint(&group_fill, fill_transform, view);
                paint.tint = tint;
                primitives.push(q0glblend::VectorGroupPrimitive::Gradient {
                    mesh: group_fill.mesh,
                    paint,
                });
            }
        }
    }
    if vector.stroke.is_some()
        && !collect_vector_group_stroke(vector, transform, view, textures, tint, primitives)
    {
        return false;
    }
    true
}

#[cfg(feature = "appearance-mask-eraser")]
fn cached_vector_appearance_body_geometry<'a>(
    textures: &'a mut TextureCache,
    vector: &VectorAsset,
    appearance: &VectorAppearance,
) -> Option<&'a CachedVectorRenderGeometry> {
    if !textures
        .visible_body_mesh_by_asset
        .contains_key(&vector.asset_id)
    {
        if let std::collections::hash_map::Entry::Vacant(e) =
            textures.visible_body_by_asset.entry(vector.asset_id)
        {
            let surface =
                crate::appearance::canonical_visible_source_surface_for_vector(vector, appearance)
                    .unwrap_or_else(|| {
                        crate::appearance::visible_source_surface_for_vector(
                            vector,
                            Some(appearance),
                        )
                    });
            let contours: Vec<Vec<Vec2>> = crate::brush::coverage_to_linear_paths(&surface)
                .iter()
                .map(flatten_path)
                .collect();
            e.insert(contours.clone());
            textures
                .visible_selection_body_by_asset
                .insert(vector.asset_id, contours);
        }
        let geometry = build_cached_fill_geometry(
            textures
                .visible_body_by_asset
                .get(&vector.asset_id)
                .map(Vec::as_slice)?,
        );
        textures
            .visible_body_mesh_by_asset
            .insert(vector.asset_id, geometry);
    }
    textures.visible_body_mesh_by_asset.get(&vector.asset_id)
}

fn collect_vector_group_stroke(
    vector: &VectorAsset,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    tint: Color32,
    primitives: &mut Vec<q0glblend::VectorGroupPrimitive>,
) -> bool {
    let started = std::time::Instant::now();
    let result =
        collect_vector_group_stroke_impl(vector, transform, view, textures, tint, primitives);
    textures.stage_work_timings.group_strokes += started.elapsed();
    result
}

fn collect_vector_group_stroke_impl(
    vector: &VectorAsset,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    tint: Color32,
    primitives: &mut Vec<q0glblend::VectorGroupPrimitive>,
) -> bool {
    let Some(stroke) = vector.stroke else {
        return true;
    };
    let geometry = textures.vector_render_geometry(vector);
    if geometry.stroke_paths.iter().all(|(path, _)| path.len() < 2) {
        return true;
    }
    // The authored stroke triangles already exist in the vector cache. For
    // uniform transforms use them just as the normal scene renderer does.
    if let Some(mesh) = cached_vector_stroke_mesh(
        geometry,
        transform,
        view,
        modulate(rgba_to_color32(stroke.color), tint),
    ) {
        primitives.push(q0glblend::VectorGroupPrimitive::Solid(mesh));
        return true;
    }
    let scale = transform.uniform_scale().abs() * view.effective_scale().abs();
    let width = stroke.width * scale;
    if !width.is_finite() || width <= 0.0 {
        return false;
    }
    let screen_paths: Vec<_> = geometry
        .stroke_paths
        .iter()
        .map(|(path, closed)| {
            (
                path.iter()
                    .map(|point| {
                        let screen = stage_to_screen(transform.apply(*point), view);
                        Vec2::new(screen.x, screen.y)
                    })
                    .collect(),
                *closed,
            )
        })
        .collect();
    let mut screen_vector = vector.clone();
    screen_vector.stroke = Some(q0s_format::v2::Stroke { width, ..stroke });
    let (vertices, indices) = tessellate_cached_asset_strokes(&screen_vector, &screen_paths);
    if vertices.is_empty() || indices.is_empty() {
        return true;
    }
    let color = modulate(rgba_to_color32(stroke.color), tint);
    let mut mesh = Mesh::default();
    mesh.vertices
        .extend(vertices.into_iter().map(|point| Vertex {
            pos: pos2(point.x, point.y),
            uv: Pos2::ZERO,
            color,
        }));
    mesh.indices = indices;
    primitives.push(q0glblend::VectorGroupPrimitive::Solid(mesh));
    true
}

#[allow(clippy::too_many_arguments)]
fn collect_vector_q0rg_group_primitives(
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    depth: u8,
    tint: Color32,
    primitives: &mut Vec<q0glblend::VectorGroupPrimitive>,
) -> bool {
    if depth > Q0RG_RECURSION_LIMIT {
        return false;
    }
    let Some(q0rg) = lookup.q0rg(q0rg_id) else {
        return false;
    };
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    for layer in &q0rg.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        if project.layer_metadata(q0rg_id, layer.layer_id).kind == q0s_format::v2::LayerKind::Mask {
            continue;
        }
        let resolved_layer = resolve_layer_at_frame(layer, local_frame);
        if layer_has_mask_assignment(project, q0rg_id, layer.layer_id) && !resolved_layer.is_empty()
        {
            return false;
        }
        for resolved in resolved_layer {
            if !placement_fx_is_visual_identity(placement_fx_without_tint(resolved.fx)) {
                return false;
            }
            let primitive_start = primitives.len();
            let placement = resolved.placement;
            let composed = Affine::compose(parent, Affine::from_transform(resolved.interp));
            match placement.target {
                Target::Asset(asset_id) => {
                    if !collect_vector_asset_group_primitives(
                        project,
                        lookup,
                        asset_id,
                        local_frame.saturating_sub(resolved.playback_start_frame),
                        composed,
                        view,
                        textures,
                        ctx,
                        tint,
                        primitives,
                    ) {
                        return false;
                    }
                }
                Target::Q0rg(child_id) if child_id != q0rg_id => {
                    let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child_id,
                        local_frame,
                        resolved.playback_start_frame,
                        resolved.fx.q0rg_playback,
                    );
                    if !collect_vector_q0rg_group_primitives(
                        project,
                        lookup,
                        child_id,
                        child_frame,
                        composed,
                        view,
                        textures,
                        ctx,
                        depth + 1,
                        tint,
                        primitives,
                    ) {
                        return false;
                    }
                }
                Target::Q0rg(_) => return false,
            }
            if let Some(effect) = resolved.fx.tint {
                q0glblend::transform_primitive_colors(
                    &mut primitives[primitive_start..],
                    q0glblend::ColorTransform::tint(effect),
                );
            }
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn try_render_gpu_vector_filter_placement(
    painter: &Painter,
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    host_q0rg_id: u16,
    placement: &Placement,
    local_frame: u16,
    playback_start_frame: u16,
    composed: Affine,
    fx: PlacementFx,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    depth: u8,
    tint: Color32,
    inherited_color: q0glblend::ColorTransform,
) -> bool {
    let tint_active = fx.tint.is_some_and(|effect| effect.amount > 1.0e-6);
    if (!placement_fx_has_gpu_blur_or_glow(fx) && !tint_active) || lookup.has_rig(host_q0rg_id) {
        return false;
    }
    if fx.glow.is_some_and(|glow| {
        glow.strength > 1.0e-6
            && glow.color.a > 0
            && glow.radius_x <= 1.0e-6
            && glow.radius_y <= 1.0e-6
    }) {
        return false;
    }
    if fx
        .shadow
        .is_some_and(|shadow| shadow.strength > 1.0e-6 && shadow.color.a > 0)
    {
        return false;
    }

    // Apply the editor's preview modulation after authored tint, not before it.
    let group_tint = if tint_active {
        Color32::WHITE
    } else {
        Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 255)
    };
    let mut primitives = Vec::new();
    let supported = match placement.target {
        Target::Asset(asset_id) => collect_vector_asset_group_primitives(
            project,
            lookup,
            asset_id,
            local_frame.saturating_sub(playback_start_frame),
            composed,
            view,
            textures,
            ctx,
            group_tint,
            &mut primitives,
        ),
        Target::Q0rg(child_id) if child_id != host_q0rg_id => {
            let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                project,
                child_id,
                local_frame,
                playback_start_frame,
                fx.q0rg_playback,
            );
            collect_vector_q0rg_group_primitives(
                project,
                lookup,
                child_id,
                child_frame,
                composed,
                view,
                textures,
                ctx,
                depth + 1,
                group_tint,
                &mut primitives,
            )
        }
        Target::Q0rg(_) => false,
    };
    if !supported {
        return false;
    }

    if tint_active {
        let effect = q0glblend::ColorTransform::tint(fx.tint.unwrap());
        let modulation = q0glblend::ColorTransform {
            scale: [tint.r(), tint.g(), tint.b()].map(|c| f32::from(c) / 255.0),
            offset: [0.0; 3],
        };
        q0glblend::transform_primitive_colors(&mut primitives, effect.then(modulation));
        if placement_fx_is_visual_identity(placement_fx_without_tint(fx)) && tint.a() == 255 {
            q0glblend::transform_primitive_colors(&mut primitives, inherited_color);
            q0glblend::paint_vector_primitives(painter, primitives);
            return true;
        }
    }

    let filter_scale = composed.uniform_scale().abs() * view.effective_scale().abs();
    let blur = fx
        .blur
        .filter(|blur| blur.radius_x > 1.0e-6 || blur.radius_y > 1.0e-6);
    let glow = fx.glow.filter(|glow| {
        glow.strength > 1.0e-6
            && glow.color.a > 0
            && (glow.radius_x > 1.0e-6 || glow.radius_y > 1.0e-6)
    });
    let gpu_fx = q0glblend::VectorGroupFx {
        blur_radius_points: blur
            .map(|blur| [blur.radius_x * filter_scale, blur.radius_y * filter_scale])
            .unwrap_or([0.0, 0.0]),
        blur_quality: blur.map_or(1, |blur| blur.quality),
        glow_radius_points: glow
            .map(|glow| [glow.radius_x * filter_scale, glow.radius_y * filter_scale])
            .unwrap_or([0.0, 0.0]),
        glow_quality: glow.map_or(1, |glow| glow.quality),
        glow_color: glow
            .map(|glow| inherited_color.apply(modulate(rgba_to_color32(glow.color), group_tint)))
            .unwrap_or(Color32::TRANSPARENT),
        glow_strength: glow.map_or(0.0, |glow| glow.strength),
    };
    q0glblend::transform_primitive_colors(&mut primitives, inherited_color);
    let opacity = fx.opacity.clamp(0.0, 1.0) * (f32::from(tint.a()) / 255.0);
    if textures.gpu_filter_cache_enabled {
        q0glblend::paint_vector_group_fx_cached_on_opaque_backdrop(
            painter,
            primitives,
            fx.blend_mode,
            opacity,
            gpu_fx,
        );
    } else {
        q0glblend::paint_vector_group_fx_on_opaque_backdrop(
            painter,
            primitives,
            fx.blend_mode,
            opacity,
            gpu_fx,
        );
    }
    true
}
#[allow(clippy::too_many_arguments)]
fn try_render_direct_vector_opacity_placement(
    painter: &Painter,
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    host_q0rg_id: u16,
    placement: &Placement,
    local_frame: u16,
    playback_start_frame: u16,
    composed: Affine,
    fx: PlacementFx,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    depth: u8,
    tint: Color32,
    inherited_color: q0glblend::ColorTransform,
) -> bool {
    if fx.tint.is_some_and(|tint| tint.amount > 1.0e-6) {
        return false;
    }
    if !placement_fx_is_pure_opacity(fx) || lookup.has_rig(host_q0rg_id) {
        return false;
    }
    let mut meshes = Vec::new();
    match placement.target {
        Target::Asset(asset_id) => {
            let alpha = (f32::from(tint.a()) * fx.opacity.clamp(0.0, 1.0)).round() as u8;
            let opacity_tint = Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), alpha);
            if !collect_plain_vector_asset_mesh(
                project,
                lookup,
                asset_id,
                composed,
                view,
                textures,
                opacity_tint,
                &mut meshes,
            ) {
                return false;
            }
            for mut mesh in meshes {
                for vertex in &mut mesh.vertices {
                    vertex.color = inherited_color.apply(vertex.color);
                }
                painter.add(Shape::Mesh(mesh));
            }
            true
        }
        Target::Q0rg(child_id) if child_id != host_q0rg_id => {
            let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                project,
                child_id,
                local_frame,
                playback_start_frame,
                fx.q0rg_playback,
            );
            // Group opacity is applied once after children overlap. Solids and
            // analytic gradients are emitted as ordered GPU primitives into the
            // same transient offscreen target, so the q0rg never becomes a cached
            // bitmap merely because its instance has alpha.
            let group_tint = Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 255);
            let mut primitives = Vec::new();
            if !collect_vector_q0rg_group_primitives(
                project,
                lookup,
                child_id,
                child_frame,
                composed,
                view,
                textures,
                ctx,
                depth + 1,
                group_tint,
                &mut primitives,
            ) {
                return false;
            }
            q0glblend::transform_primitive_colors(&mut primitives, inherited_color);
            let opacity = fx.opacity.clamp(0.0, 1.0) * (f32::from(tint.a()) / 255.0);
            q0glblend::paint_vector_group_on_opaque_backdrop(
                painter,
                primitives,
                BlendMode::Normal,
                opacity,
            );
            true
        }
        Target::Q0rg(_) => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn try_render_gpu_vector_blend_placement(
    painter: &Painter,
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    host_q0rg_id: u16,
    placement: &Placement,
    local_frame: u16,
    playback_start_frame: u16,
    composed: Affine,
    fx: PlacementFx,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    depth: u8,
    tint: Color32,
    inherited_color: q0glblend::ColorTransform,
) -> bool {
    if fx.tint.is_some_and(|tint| tint.amount > 1.0e-6) {
        return false;
    }
    if !placement_fx_is_pure_blend(fx) || lookup.has_rig(host_q0rg_id) {
        return false;
    }
    let group_tint = Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 255);
    let mut primitives = Vec::new();
    let supported = match placement.target {
        Target::Asset(asset_id) => collect_vector_asset_group_primitives(
            project,
            lookup,
            asset_id,
            local_frame.saturating_sub(playback_start_frame),
            composed,
            view,
            textures,
            ctx,
            group_tint,
            &mut primitives,
        ),
        Target::Q0rg(child_id) if child_id != host_q0rg_id => {
            let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                project,
                child_id,
                local_frame,
                playback_start_frame,
                fx.q0rg_playback,
            );
            collect_vector_q0rg_group_primitives(
                project,
                lookup,
                child_id,
                child_frame,
                composed,
                view,
                textures,
                ctx,
                depth + 1,
                group_tint,
                &mut primitives,
            )
        }
        Target::Q0rg(_) => false,
    };
    if !supported {
        return false;
    }
    q0glblend::transform_primitive_colors(&mut primitives, inherited_color);
    let opacity = f32::from(tint.a()) / 255.0;
    q0glblend::paint_vector_group_on_opaque_backdrop(painter, primitives, fx.blend_mode, opacity);
    true
}
#[allow(clippy::too_many_arguments)]
fn render_fx_placement(
    painter: &Painter,
    project: &ProjectV2,
    host_q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    placement: &Placement,
    local_frame: u16,
    composed: Affine,
    fx: PlacementFx,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
    outer_only: bool,
    inherited_color: q0glblend::ColorTransform,
) {
    let (width, height) = fx_cache_render_size(project, view, ctx, outer_only, fx);

    let key = FxPlacementKey {
        host_q0rg_id,
        layer_id,
        placement_idx,
        local_frame,
        width,
        height,
        linear_affine_bits: linear_affine_bits(composed),
        tint: tint.to_array(),
        outer_only,
    };
    if !textures.fx_placement_textures.contains_key(&key) {
        if textures.interactive_transform {
            if let Some((old_key, cached)) =
                textures.fx_placement_textures.iter().find(|(old, tile)| {
                    old.host_q0rg_id == key.host_q0rg_id
                        && old.layer_id == key.layer_id
                        && old.placement_idx == key.placement_idx
                        && old.local_frame == key.local_frame
                        && old.tint == key.tint
                        && old.outer_only == key.outer_only
                        && tile.blend_mode == BlendMode::Normal
                        && tile.raster_transform.inverse().is_some()
                })
            {
                paint_transformed_fx_tile(painter, project, *old_key, cached, composed, view);
                return;
            }
        }
        // Keep other objects warm, and bound historical affine variants of this tile.
        textures.fx_placement_textures.retain(|old, _| {
            old.host_q0rg_id != key.host_q0rg_id
                || old.layer_id != key.layer_id
                || old.placement_idx != key.placement_idx
                || old.local_frame != key.local_frame
                || old.tint != key.tint
                || old.outer_only != key.outer_only
        });
        if textures.fx_placement_textures.len() >= 512 {
            textures.fx_placement_textures.clear();
        }
        let crop = if outer_only {
            q0s_format::raster::rasterize_placement_outer_fx_scaled_cropped(
                project,
                host_q0rg_id,
                placement,
                local_frame,
                composed,
                fx,
                width,
                height,
            )
        } else {
            q0s_format::raster::rasterize_placement_fx_scaled_cropped(
                project,
                host_q0rg_id,
                placement,
                local_frame,
                composed,
                fx,
                width,
                height,
            )
        };
        let Some(mut crop) = crop else {
            return;
        };
        let tint_rgba = tint.to_array();
        if tint_rgba != [255, 255, 255, 255] {
            for pixel in crop.rgba.chunks_exact_mut(4) {
                for channel in 0..3 {
                    pixel[channel] = ((u16::from(pixel[channel]) * u16::from(tint_rgba[channel])
                        + 127)
                        / 255) as u8;
                }
                pixel[3] = ((u16::from(pixel[3]) * u16::from(tint_rgba[3]) + 127) / 255) as u8;
            }
        }
        let image = ColorImage::from_rgba_unmultiplied(
            [crop.width as usize, crop.height as usize],
            &crop.rgba,
        );
        textures.fx_serial = textures.fx_serial.wrapping_add(1);
        let texture = ctx.load_texture(
            format!("q0s_fx_placement_{}", textures.fx_serial),
            image,
            TextureOptions::LINEAR,
        );
        textures.fx_placement_textures.insert(
            key,
            FxPlacementTexture {
                texture,
                crop: [crop.x, crop.y, crop.width, crop.height],
                blend_mode: if outer_only {
                    BlendMode::Normal
                } else {
                    fx.blend_mode
                },
                raster_translation: Vec2::new(composed.tx, composed.ty),
                raster_transform: composed,
            },
        );
    }
    let Some(cached) = textures.fx_placement_textures.get(&key) else {
        return;
    };
    let screen_rect = fx_crop_screen_rect(
        project,
        width,
        height,
        cached.crop,
        cached.raster_translation,
        Vec2::new(composed.tx, composed.ty),
        view,
    );
    if !inherited_color.is_identity() {
        let mut mesh = Mesh::with_texture(cached.texture.id());
        mesh.add_rect_with_uv(
            screen_rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        q0glblend::paint_vector_group_on_opaque_backdrop(
            painter,
            vec![q0glblend::VectorGroupPrimitive::Textured {
                mesh: Arc::new(mesh),
                color_transform: inherited_color,
            }],
            cached.blend_mode,
            1.0,
        );
        return;
    }
    q0glblend::paint_texture_on_opaque_backdrop(
        painter,
        screen_rect,
        cached.texture.id(),
        cached.blend_mode,
    );
}

fn paint_transformed_fx_tile(
    painter: &Painter,
    project: &ProjectV2,
    key: FxPlacementKey,
    cached: &FxPlacementTexture,
    current: Affine,
    view: &StageView,
) {
    let Some(inverse) = cached.raster_transform.inverse() else {
        return;
    };
    let delta = Affine::compose(current, inverse);
    let rect = fx_crop_screen_rect(
        project,
        key.width,
        key.height,
        cached.crop,
        cached.raster_translation,
        cached.raster_translation,
        &StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: Rect::EVERYTHING,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        },
    );
    let mut mesh = Mesh::with_texture(cached.texture.id());
    for (point, uv) in [
        (rect.left_top(), pos2(0.0, 0.0)),
        (rect.right_top(), pos2(1.0, 0.0)),
        (rect.right_bottom(), pos2(1.0, 1.0)),
        (rect.left_bottom(), pos2(0.0, 1.0)),
    ] {
        mesh.vertices.push(Vertex {
            pos: stage_to_screen(delta.apply(Vec2::new(point.x, point.y)), view),
            uv,
            color: Color32::WHITE,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(Shape::Mesh(mesh));
}

pub(crate) struct StageRenderPass<'a> {
    project: &'a ProjectV2,
    lookup: RenderLookup<'a>,
}

#[derive(Clone, Copy)]
enum RootLayerFilter<'a> {
    Excluding(&'a HashSet<u16>),
    ExcludingPlacement(u16, usize),
}

impl RootLayerFilter<'_> {
    fn includes(self, layer_id: u16) -> bool {
        match self {
            Self::Excluding(excluded) => !excluded.contains(&layer_id),
            Self::ExcludingPlacement(_, _) => true,
        }
    }

    fn includes_placement(self, layer_id: u16, placement_idx: usize) -> bool {
        match self {
            Self::ExcludingPlacement(excluded_layer, excluded_idx) => {
                layer_id != excluded_layer || placement_idx != excluded_idx
            }
            Self::Excluding(_) => true,
        }
    }
}

impl<'a> StageRenderPass<'a> {
    pub(crate) fn new(project: &'a ProjectV2, textures: &mut TextureCache) -> Self {
        Self::new_with_revision(project, textures, None)
    }

    pub(crate) fn new_with_revision(
        project: &'a ProjectV2,
        textures: &mut TextureCache,
        revision: Option<(u64, u64)>,
    ) -> Self {
        // One editor paint can draw many onion frames plus the live frame. FX cache
        // compatibility and id lookup tables depend on the project, not on which
        // of those frames is being drawn, so build/sync them exactly once.
        let __profile = std::env::var_os("Q0_RENDER_PROFILE").is_some();
        let __t0 = std::time::Instant::now();
        if textures.render_index_revision != revision || revision.is_none() {
            textures.sync_video_sources(project);
        }
        sync_fx_project_snapshot(project, textures, revision);
        if textures.mask_frame_revision != revision || revision.is_none() {
            textures.mask_frame_textures.clear();
            textures.mask_frame_revision = revision;
        }
        let __t1 = std::time::Instant::now();
        let render_index = textures.render_index(project, revision);
        let __t2 = std::time::Instant::now();
        if __profile {
            eprintln!(
                "render_profile new sync_ms={:.3} index_ms={:.3}",
                (__t1 - __t0).as_secs_f64() * 1000.0,
                (__t2 - __t1).as_secs_f64() * 1000.0
            );
        }
        Self {
            project,
            lookup: RenderLookup::new(project, render_index, revision.is_none()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_tinted(
        &self,
        painter: &Painter,
        q0rg_id: u16,
        frame: u16,
        view: &StageView,
        textures: &mut TextureCache,
        ctx: &Context,
        tint: Color32,
    ) {
        self.render_tinted_filtered(painter, q0rg_id, frame, view, textures, ctx, tint, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_tinted_filtered(
        &self,
        painter: &Painter,
        q0rg_id: u16,
        frame: u16,
        view: &StageView,
        textures: &mut TextureCache,
        ctx: &Context,
        tint: Color32,
        excluded_layers: Option<&HashSet<u16>>,
    ) {
        let layer_filter_hash = excluded_layers.map_or(0, |excluded| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            if let Some(q0rg) = self.lookup.q0rg(q0rg_id) {
                for layer in &q0rg.layers {
                    if excluded.contains(&layer.layer_id) {
                        layer.layer_id.hash(&mut hasher);
                    }
                }
            }
            hasher.finish()
        });
        let __t = std::time::Instant::now();
        let contains_mask = q0rg_contains_mask(
            self.project,
            q0rg_id,
            frame,
            &mut std::collections::HashSet::new(),
            0,
            textures,
        );
        if contains_mask
            && !q0rg_masks_supported_by_gpu(
                self.project,
                &self.lookup,
                q0rg_id,
                frame,
                0,
                &mut HashSet::new(),
                textures,
            )
        {
            let canvas_rect = painter.clip_rect();
            let visible_rect = view.stage_rect.intersect(canvas_rect);
            if visible_rect.width() > 0.0 && visible_rect.height() > 0.0 {
                // Keep the established exact renderer for artwork the GPU
                // primitive collector cannot represent yet. Supported vector,
                // gradient, and bitmap masks use the native GPU compositor.
                let overscan = canvas_rect.size() * 0.25;
                let raster_rect =
                    Rect::from_min_max(canvas_rect.min - overscan, canvas_rect.max + overscan);
                let raster_view = StageView {
                    origin: view.origin,
                    scale: view.scale,
                    stage_rect: raster_rect,
                    edit_transform: view.edit_transform,
                };
                let (width, height) = fx_stage_render_size(&raster_view, ctx);
                let raster_scale_x = width as f32 / raster_rect.width().max(1.0);
                let raster_scale_y = height as f32 / raster_rect.height().max(1.0);
                let screen_to_raster = Affine {
                    a11: view.scale * raster_scale_x,
                    a12: 0.0,
                    a21: 0.0,
                    a22: view.scale * raster_scale_y,
                    tx: (view.origin.x - raster_rect.min.x) * raster_scale_x,
                    ty: (view.origin.y - raster_rect.min.y) * raster_scale_y,
                };
                let root = Affine::compose(screen_to_raster, view.edit_transform);
                let key = MaskFrameKey {
                    q0rg_id,
                    frame,
                    width,
                    height,
                    scale_x_bits: root.a11.to_bits(),
                    scale_y_bits: root.a22.to_bits(),
                    shear_x_bits: root.a12.to_bits(),
                    shear_y_bits: root.a21.to_bits(),
                    translate_x_bits: root.tx.to_bits(),
                    translate_y_bits: root.ty.to_bits(),
                    editing_q0rg_id: textures.editing_mask_q0rg_id,
                    mask_visibility_revision: textures.mask_visibility_revision,
                    layer_filter_hash,
                };
                let translation = view.origin
                    - textures
                        .mask_frame_textures
                        .get(&key)
                        .map(|cached| cached.stage_origin)
                        .unwrap_or(view.origin);
                let tile_is_usable = textures
                    .mask_frame_textures
                    .get(&key)
                    .is_some_and(|cached| {
                        let shifted = cached.screen_rect.translate(translation);
                        shifted.min.x <= visible_rect.min.x
                            && shifted.min.y <= visible_rect.min.y
                            && shifted.max.x >= visible_rect.max.x
                            && shifted.max.y >= visible_rect.max.y
                    });

                if !tile_is_usable {
                    if textures.mask_frame_textures.len() >= 13 {
                        if let Some(oldest) = textures
                            .mask_frame_textures
                            .keys()
                            .copied()
                            .max_by_key(|cached| cached.frame.abs_diff(frame))
                        {
                            textures.mask_frame_textures.remove(&oldest);
                        }
                    }
                    let mask_started = std::time::Instant::now();
                    let rgba = q0s_format::raster::rasterize_q0rg_frame_with_mask_preview(
                        self.project,
                        q0rg_id,
                        frame,
                        width,
                        height,
                        1,
                        [0, 0, 0, 0],
                        root,
                        &q0s_format::rig::RigRuntimeOverrides::new(),
                        &q0s_format::runtime_scene::RuntimeSceneState::new(),
                        (excluded_layers.is_some()
                            || textures.editing_mask_q0rg_id == Some(q0rg_id))
                        .then_some(q0s_format::raster::MaskEditingPreview {
                            q0rg_id,
                            hidden_artwork: (textures.editing_mask_q0rg_id == Some(q0rg_id))
                                .then_some(&textures.hidden_mask_art_layers),
                            excluded_layers,
                        }),
                    );
                    textures.stage_work_timings.mask_raster += mask_started.elapsed();
                    let image = ColorImage::from_rgba_unmultiplied(
                        [width as usize, height as usize],
                        &rgba,
                    );
                    let texture = ctx.load_texture(
                        format!("masked-q0rg-{q0rg_id}-frame-{frame}"),
                        image,
                        TextureOptions::LINEAR,
                    );
                    textures.mask_frame_textures.insert(
                        key,
                        CachedMaskFrame {
                            texture,
                            stage_origin: view.origin,
                            screen_rect: raster_rect,
                        },
                    );
                }
                if let Some(cached) = textures.mask_frame_textures.get(&key) {
                    let translation = view.origin - cached.stage_origin;
                    let screen_rect = cached.screen_rect.translate(translation);
                    painter.with_clip_rect(visible_rect).image(
                        cached.texture.id(),
                        screen_rect,
                        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                        tint,
                    );
                }
            }
        } else {
            render_q0rg(
                painter,
                self.project,
                &self.lookup,
                q0rg_id,
                frame,
                Affine::IDENTITY,
                view,
                textures,
                ctx,
                0,
                tint,
                excluded_layers.map(RootLayerFilter::Excluding),
            );
        }
        if std::env::var_os("Q0_RENDER_PROFILE").is_some() {
            eprintln!(
                "render_profile body frame={} ms={:.3}",
                frame,
                __t.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    pub(crate) fn render(
        &self,
        painter: &Painter,
        q0rg_id: u16,
        frame: u16,
        view: &StageView,
        textures: &mut TextureCache,
        ctx: &Context,
    ) {
        self.render_tinted(painter, q0rg_id, frame, view, textures, ctx, Color32::WHITE);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_tinted_excluding_layers(
        &self,
        painter: &Painter,
        q0rg_id: u16,
        frame: u16,
        view: &StageView,
        textures: &mut TextureCache,
        ctx: &Context,
        tint: Color32,
        excluded_layers: &HashSet<u16>,
    ) {
        self.render_tinted_filtered(
            painter,
            q0rg_id,
            frame,
            view,
            textures,
            ctx,
            tint,
            Some(excluded_layers),
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_tinted_excluding_placement(
        &self,
        painter: &Painter,
        q0rg_id: u16,
        frame: u16,
        view: &StageView,
        textures: &mut TextureCache,
        ctx: &Context,
        tint: Color32,
        layer_id: u16,
        placement_idx: usize,
    ) {
        let contains_mask = q0rg_contains_mask(
            self.project,
            q0rg_id,
            frame,
            &mut HashSet::new(),
            0,
            textures,
        );
        if contains_mask
            && !q0rg_masks_supported_by_gpu(
                self.project,
                &self.lookup,
                q0rg_id,
                frame,
                0,
                &mut HashSet::new(),
                textures,
            )
        {
            self.render_tinted(painter, q0rg_id, frame, view, textures, ctx, tint);
            return;
        }
        render_q0rg(
            painter,
            self.project,
            &self.lookup,
            q0rg_id,
            frame,
            Affine::IDENTITY,
            view,
            textures,
            ctx,
            0,
            tint,
            Some(RootLayerFilter::ExcludingPlacement(layer_id, placement_idx)),
        );
    }
}

fn q0rg_contains_mask(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    visited: &mut std::collections::HashSet<u16>,
    depth: u8,
    textures: &mut TextureCache,
) -> bool {
    if depth > Q0RG_RECURSION_LIMIT || !visited.insert(q0rg_id) {
        return false;
    }
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        visited.remove(&q0rg_id);
        return false;
    };
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };

    // a mask layer by itself changes nothing. only use the software fallback
    // when visible, active artwork on this frame references a populated mask.
    let mut contains_mask = false;
    for layer in &q0rg.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        let metadata = project.layer_metadata(q0rg_id, layer.layer_id);
        if metadata.kind == q0s_format::v2::LayerKind::Mask
            && !textures.mask_art_visible(q0rg_id, layer.layer_id)
        {
            continue;
        }
        let active = textures.active_placement_states_at(q0rg_id, layer, local_frame);
        if active.is_empty() {
            continue;
        }
        let mask_sources = if layer_has_mask_assignment(project, q0rg_id, layer.layer_id) {
            project.layer_mask_sources(q0rg_id, layer.layer_id)
        } else {
            Default::default()
        }
        .into_iter()
        .filter(|mask_id| {
            textures.mask_clips(q0rg_id, *mask_id)
                && !mask_layer_is_blank(q0rg, *mask_id, local_frame, textures)
        })
        .collect::<Vec<_>>();
        if !mask_sources.is_empty()
            && mask_sources.iter().all(|mask_layer_id| {
                mask_layer_has_active_art(project, q0rg, *mask_layer_id, local_frame, textures)
            })
        {
            contains_mask = true;
            break;
        }
        for state in active.iter().copied() {
            let Some(placement) = layer.placements.get(state.index) else {
                continue;
            };
            let Target::Q0rg(child_id) = placement.target else {
                continue;
            };
            let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                project,
                child_id,
                local_frame,
                state.playback_start_frame,
                state.fx.q0rg_playback,
            );
            if q0rg_contains_mask(project, child_id, child_frame, visited, depth + 1, textures) {
                contains_mask = true;
                break;
            }
        }
        if contains_mask {
            break;
        }
    }
    visited.remove(&q0rg_id);
    contains_mask
}

fn mask_layer_is_blank(
    q0rg: &Q0rg,
    mask_layer_id: u16,
    frame: u16,
    textures: &mut TextureCache,
) -> bool {
    q0rg.layers
        .iter()
        .find(|layer| layer.layer_id == mask_layer_id)
        .is_some_and(|layer| {
            textures
                .active_placement_states_at(q0rg.q0rg_id, layer, frame)
                .is_empty()
        })
}

fn mask_layer_has_active_art(
    project: &ProjectV2,
    q0rg: &Q0rg,
    mask_layer_id: u16,
    frame: u16,
    textures: &mut TextureCache,
) -> bool {
    let Some(mask_layer) = q0rg
        .layers
        .iter()
        .find(|layer| layer.layer_id == mask_layer_id)
    else {
        return false;
    };
    project.layer_metadata(q0rg.q0rg_id, mask_layer_id).kind == q0s_format::v2::LayerKind::Mask
        && project.layer_is_visible(q0rg.q0rg_id, mask_layer_id)
        && !textures
            .active_placement_states_at(q0rg.q0rg_id, mask_layer, frame)
            .is_empty()
}

fn layer_has_mask_assignment(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> bool {
    let mut current = Some(layer_id);
    for _ in 0..=q0s_format::v2::MAX_LAYER_FOLDER_NESTING_DEPTH {
        let Some(id) = current else {
            return false;
        };
        let metadata = project.layer_metadata(q0rg_id, id);
        if metadata.mask_layer_id.is_some() {
            return true;
        }
        current = metadata.parent_folder_id;
    }
    false
}

fn group_asset_supported(project: &ProjectV2, lookup: &RenderLookup<'_>, asset_id: u16) -> bool {
    match lookup.asset(asset_id) {
        Some(Asset::Bitmap(_) | Asset::Q0v(_)) => true,
        Some(Asset::Vector(_)) => match project.asset_appearances.get(&asset_id) {
            None => true,
            Some(appearance) => {
                matches!(appearance.material, q0s_format::v2::VectorMaterial::Solid)
            }
        },
        Some(Asset::Rig(_)) | None => false,
    }
}

fn q0rg_group_supported_for_gpu_masks(
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    q0rg_id: u16,
    frame: u16,
    depth: u8,
    visited: &mut HashSet<u16>,
) -> bool {
    if depth > Q0RG_RECURSION_LIMIT || lookup.rig(q0rg_id).is_some() {
        return false;
    }
    if !visited.insert(q0rg_id) {
        return false;
    }
    let Some(q0rg) = lookup.q0rg(q0rg_id) else {
        visited.remove(&q0rg_id);
        return false;
    };
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    let mut supported = true;
    for layer in &q0rg.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_metadata(q0rg_id, layer.layer_id).kind
                == q0s_format::v2::LayerKind::Mask
        {
            continue;
        }
        let active = q0s_format::raster::active_placement_states_at(layer, local_frame);
        if active.is_empty() {
            continue;
        }
        if layer_has_mask_assignment(project, q0rg_id, layer.layer_id) {
            supported = false;
            break;
        }
        for state in active {
            if !placement_fx_is_visual_identity(placement_fx_without_tint(state.fx)) {
                supported = false;
                break;
            }
            let Some(placement) = layer.placements.get(state.index) else {
                continue;
            };
            match placement.target {
                Target::Asset(asset_id) => {
                    if !group_asset_supported(project, lookup, asset_id) {
                        supported = false;
                        break;
                    }
                }
                Target::Q0rg(child_id) if child_id != q0rg_id => {
                    let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child_id,
                        local_frame,
                        state.playback_start_frame,
                        state.fx.q0rg_playback,
                    );
                    if !q0rg_group_supported_for_gpu_masks(
                        project,
                        lookup,
                        child_id,
                        child_frame,
                        depth + 1,
                        visited,
                    ) {
                        supported = false;
                        break;
                    }
                }
                Target::Q0rg(_) => {
                    supported = false;
                    break;
                }
            }
        }
        if !supported {
            break;
        }
    }
    visited.remove(&q0rg_id);
    supported
}

fn q0rg_masks_supported_by_gpu(
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    q0rg_id: u16,
    frame: u16,
    depth: u8,
    visited: &mut HashSet<u16>,
    textures: &mut TextureCache,
) -> bool {
    if depth > Q0RG_RECURSION_LIMIT || lookup.rig(q0rg_id).is_some() {
        return false;
    }
    if !visited.insert(q0rg_id) {
        return false;
    }
    let Some(q0rg) = lookup.q0rg(q0rg_id) else {
        visited.remove(&q0rg_id);
        return false;
    };
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    let mut supported = true;
    for layer in &q0rg.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || (project.layer_metadata(q0rg_id, layer.layer_id).kind
                == q0s_format::v2::LayerKind::Mask
                && !textures.mask_art_visible(q0rg_id, layer.layer_id))
        {
            continue;
        }
        let active = textures.active_placement_states_at(q0rg_id, layer, local_frame);
        if active.is_empty() {
            continue;
        }
        let mask_sources = if layer_has_mask_assignment(project, q0rg_id, layer.layer_id) {
            project.layer_mask_sources(q0rg_id, layer.layer_id)
        } else {
            Default::default()
        }
        .into_iter()
        .filter(|mask_id| {
            textures.mask_clips(q0rg_id, *mask_id)
                && !mask_layer_is_blank(q0rg, *mask_id, local_frame, textures)
        })
        .collect::<Vec<_>>();
        let is_masked = !mask_sources.is_empty();
        if is_masked {
            // The in-between outlines of a shape tween exist for one frame
            // only; the raster path clips with them, the cached GPU mask
            // primitives would use the first drawing.
            let morphing = |candidate: &q0s_format::v2::Layer| {
                q0s_format::shape_tween::span_at(candidate, local_frame).is_some()
            };
            if morphing(layer)
                || mask_sources.iter().any(|mask_id| {
                    q0rg.layers
                        .iter()
                        .find(|candidate| candidate.layer_id == *mask_id)
                        .is_some_and(morphing)
                })
            {
                supported = false;
                break;
            }
            let masks_supported = mask_sources.iter().all(|mask_id| {
                let Some(mask_layer) = q0rg
                    .layers
                    .iter()
                    .find(|candidate| candidate.layer_id == *mask_id)
                else {
                    return false;
                };
                if !project.layer_is_visible(q0rg_id, *mask_id) {
                    return false;
                }
                textures
                    .active_placement_states_at(q0rg_id, mask_layer, local_frame)
                    .iter()
                    .all(|state| {
                        placement_fx_is_visual_identity(placement_fx_without_tint(state.fx))
                            && mask_layer
                                .placements
                                .get(state.index)
                                .is_some_and(|placement| match placement.target {
                                    Target::Asset(asset_id) => {
                                        group_asset_supported(project, lookup, asset_id)
                                    }
                                    Target::Q0rg(child_id) if child_id != q0rg_id => {
                                        let child_frame =
                                            q0s_format::v2::resolve_q0rg_playback_frame(
                                                project,
                                                child_id,
                                                local_frame,
                                                state.playback_start_frame,
                                                state.fx.q0rg_playback,
                                            );
                                        q0rg_group_supported_for_gpu_masks(
                                            project,
                                            lookup,
                                            child_id,
                                            child_frame,
                                            depth + 1,
                                            &mut HashSet::new(),
                                        )
                                    }
                                    Target::Q0rg(_) => false,
                                })
                    })
            });
            let targets_supported = active.iter().all(|state| {
                let Some(placement) = layer.placements.get(state.index) else {
                    return true;
                };
                let fx_supported =
                    placement_fx_is_visual_identity(placement_fx_without_tint(state.fx))
                        || placement_fx_is_pure_opacity(placement_fx_without_tint(state.fx))
                        || placement_fx_is_pure_blend(placement_fx_without_tint(state.fx));
                fx_supported
                    && match placement.target {
                        Target::Asset(asset_id) => group_asset_supported(project, lookup, asset_id),
                        Target::Q0rg(child_id) if child_id != q0rg_id => {
                            let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                                project,
                                child_id,
                                local_frame,
                                state.playback_start_frame,
                                state.fx.q0rg_playback,
                            );
                            q0rg_group_supported_for_gpu_masks(
                                project,
                                lookup,
                                child_id,
                                child_frame,
                                depth + 1,
                                &mut HashSet::new(),
                            )
                        }
                        Target::Q0rg(_) => false,
                    }
            });
            if !masks_supported || !targets_supported {
                supported = false;
                break;
            }
        }
        for state in active.iter() {
            let Some(placement) = layer.placements.get(state.index) else {
                continue;
            };
            if let Target::Q0rg(child_id) = placement.target {
                if child_id == q0rg_id {
                    supported = false;
                    break;
                }
                let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                    project,
                    child_id,
                    local_frame,
                    state.playback_start_frame,
                    state.fx.q0rg_playback,
                );
                if q0rg_contains_mask(
                    project,
                    child_id,
                    child_frame,
                    &mut HashSet::new(),
                    depth + 1,
                    textures,
                ) && (!placement_fx_is_visual_identity(placement_fx_without_tint(state.fx))
                    || !q0rg_masks_supported_by_gpu(
                        project,
                        lookup,
                        child_id,
                        child_frame,
                        depth + 1,
                        visited,
                        textures,
                    ))
                {
                    supported = false;
                    break;
                }
            }
        }
        if !supported {
            break;
        }
    }
    visited.remove(&q0rg_id);
    supported
}

#[allow(clippy::too_many_arguments)]
fn collect_gpu_mask_placement_primitives(
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    q0rg_id: u16,
    frame: u16,
    placement: &Placement,
    active: &q0s_format::raster::ActivePlacementState,
    parent: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    depth: u8,
    tint: Color32,
    primitives: &mut Vec<q0glblend::VectorGroupPrimitive>,
) -> bool {
    if depth > Q0RG_RECURSION_LIMIT || lookup.rig(q0rg_id).is_some() {
        return false;
    }
    let composed = Affine::compose(parent, Affine::from_transform(active.transform));
    let start = primitives.len();
    let supported = match placement.target {
        Target::Asset(asset_id) => collect_vector_asset_group_primitives(
            project,
            lookup,
            asset_id,
            frame.saturating_sub(active.playback_start_frame),
            composed,
            view,
            textures,
            ctx,
            tint,
            primitives,
        ),
        Target::Q0rg(child_id) if child_id != q0rg_id => {
            let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                project,
                child_id,
                frame,
                active.playback_start_frame,
                active.fx.q0rg_playback,
            );
            collect_vector_q0rg_group_primitives(
                project,
                lookup,
                child_id,
                child_frame,
                composed,
                view,
                textures,
                ctx,
                depth + 1,
                tint,
                primitives,
            )
        }
        Target::Q0rg(_) => false,
    };
    if supported {
        if let Some(effect) = active.fx.tint {
            q0glblend::transform_primitive_colors(
                &mut primitives[start..],
                q0glblend::ColorTransform::tint(effect),
            );
        }
    }
    supported
}

#[allow(clippy::too_many_arguments)]
fn collect_gpu_masked_layer(
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    q0rg: &Q0rg,
    layer: &q0s_format::v2::Layer,
    frame: u16,
    parent: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    depth: u8,
    tint: Color32,
    inherited_color: q0glblend::ColorTransform,
) -> Option<(
    Vec<Vec<q0glblend::VectorGroupPrimitive>>,
    Vec<q0glblend::VectorMaskedPlacement>,
)> {
    if lookup.rig(q0rg.q0rg_id).is_some() {
        return None;
    }
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    let mut masks = Vec::new();
    for mask_id in project
        .layer_mask_sources(q0rg.q0rg_id, layer.layer_id)
        .into_iter()
        .filter(|mask_id| {
            textures.mask_clips(q0rg.q0rg_id, *mask_id)
                && !mask_layer_is_blank(q0rg, *mask_id, local_frame, textures)
        })
        .collect::<Vec<_>>()
    {
        let mask_layer = q0rg
            .layers
            .iter()
            .find(|candidate| candidate.layer_id == mask_id)?;
        if !project.layer_is_visible(q0rg.q0rg_id, mask_id)
            || project.layer_metadata(q0rg.q0rg_id, mask_id).kind != q0s_format::v2::LayerKind::Mask
        {
            return None;
        }
        let active = textures.active_placement_states_at(q0rg.q0rg_id, mask_layer, local_frame);
        let mut primitives = Vec::new();
        for state in active.iter() {
            if textures.hidden_text_placement == Some((q0rg.q0rg_id, mask_id, state.index)) {
                continue;
            }
            let Some(placement) = mask_layer.placements.get(state.index) else {
                continue;
            };
            if !placement_fx_is_visual_identity(placement_fx_without_tint(state.fx))
                || !collect_gpu_mask_placement_primitives(
                    project,
                    lookup,
                    q0rg.q0rg_id,
                    local_frame,
                    placement,
                    state,
                    parent,
                    view,
                    textures,
                    ctx,
                    depth,
                    Color32::WHITE,
                    &mut primitives,
                )
            {
                return None;
            }
        }
        masks.push(primitives);
    }
    let mut placements = Vec::new();
    let active = textures.active_placement_states_at(q0rg.q0rg_id, layer, local_frame);
    let group_tint = Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 255);
    for state in active.iter() {
        if textures.hidden_text_placement == Some((q0rg.q0rg_id, layer.layer_id, state.index)) {
            continue;
        }
        if !placement_fx_is_visual_identity(placement_fx_without_tint(state.fx))
            && !placement_fx_is_pure_opacity(placement_fx_without_tint(state.fx))
            && !placement_fx_is_pure_blend(placement_fx_without_tint(state.fx))
        {
            return None;
        }
        let Some(placement) = layer.placements.get(state.index) else {
            continue;
        };
        let mut primitives = Vec::new();
        if !collect_gpu_mask_placement_primitives(
            project,
            lookup,
            q0rg.q0rg_id,
            local_frame,
            placement,
            state,
            parent,
            view,
            textures,
            ctx,
            depth,
            group_tint,
            &mut primitives,
        ) {
            return None;
        }
        q0glblend::transform_primitive_colors(&mut primitives, inherited_color);
        placements.push(q0glblend::VectorMaskedPlacement {
            primitives,
            blend_mode: state.fx.blend_mode,
            opacity: state.fx.opacity.clamp(0.0, 1.0) * (f32::from(tint.a()) / 255.0),
        });
    }
    Some((masks, placements))
}

pub fn render_stage(
    painter: &Painter,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
) {
    StageRenderPass::new(project, textures).render(painter, q0rg_id, frame, view, textures, ctx);
}

pub(crate) fn render_stage_outline(
    painter: &Painter,
    render_pass: &StageRenderPass<'_>,
    q0rg_id: u16,
    frame: u16,
    view: &StageView,
    hidden_mask_art_layers: &HashSet<(u16, u16)>,
) {
    render_q0rg_outline(
        painter,
        render_pass.project,
        &render_pass.lookup,
        q0rg_id,
        frame,
        Affine::IDENTITY,
        view,
        0,
        None,
        hidden_mask_art_layers,
    );
}

pub(crate) fn render_stage_outline_layers(
    painter: &Painter,
    render_pass: &StageRenderPass<'_>,
    q0rg_id: u16,
    frame: u16,
    view: &StageView,
    layer_ids: &HashSet<u16>,
    hidden_mask_art_layers: &HashSet<(u16, u16)>,
) {
    render_q0rg_outline(
        painter,
        render_pass.project,
        &render_pass.lookup,
        q0rg_id,
        frame,
        Affine::IDENTITY,
        view,
        0,
        Some(layer_ids),
        hidden_mask_art_layers,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_q0rg_outline(
    painter: &Painter,
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    view: &StageView,
    depth: u8,
    root_layer_filter: Option<&HashSet<u16>>,
    hidden_mask_art_layers: &HashSet<(u16, u16)>,
) {
    if depth > Q0RG_RECURSION_LIMIT {
        return;
    }
    let Some(q0rg) = lookup.q0rg(q0rg_id) else {
        return;
    };
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    let rig = lookup.rig(q0rg_id);
    let rig_pose = lookup.rig_pose(q0rg_id, local_frame);
    for layer in &q0rg.layers {
        if depth == 0 && root_layer_filter.is_some_and(|only| !only.contains(&layer.layer_id)) {
            continue;
        }
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        let metadata = project.layer_metadata(q0rg_id, layer.layer_id);
        if metadata.kind == q0s_format::v2::LayerKind::Mask
            && hidden_mask_art_layers.contains(&(q0rg_id, layer.layer_id))
        {
            continue;
        }
        let color = outline_color_for_layer(layer.layer_id);
        let shape_morph = layer_shape_morph(lookup, layer, local_frame);
        for resolved in resolve_layer_at_frame(layer, local_frame) {
            let placement = resolved.placement;
            let local_affine = rig_pose
                .as_ref()
                .and_then(|pose| pose.binding_transform(placement.instance_id))
                .unwrap_or_else(|| Affine::from_transform(resolved.interp));
            let composed = Affine::compose(parent, local_affine);
            if let Some(shapes) = shape_morph
                .as_ref()
                .and_then(|morph| morph.for_placement(resolved.index))
            {
                for path in shapes.iter().flat_map(|shape| &shape.paths) {
                    let points: Vec<Pos2> = flatten_path(path)
                        .into_iter()
                        .map(|point| stage_to_screen(composed.apply(point), view))
                        .collect();
                    if points.len() >= 2 {
                        painter.add(Shape::line(points, Stroke::new(1.15_f32, color)));
                    }
                }
                continue;
            }
            let target = match (rig, rig_pose.as_ref()) {
                (Some(rig), Some(pose)) => q0s_format::rig::resolved_variant_target(
                    rig,
                    pose,
                    placement.instance_id,
                    placement.target,
                ),
                _ => placement.target,
            };
            match target {
                Target::Asset(asset_id) => {
                    let Some(asset) = lookup.asset(asset_id) else {
                        continue;
                    };
                    match asset {
                        Asset::Vector(vector) => {
                            let deformed = rig.zip(rig_pose.as_ref()).and_then(|(rig, pose)| {
                                q0s_format::rig::deform_vector_for_instance(
                                    rig,
                                    pose,
                                    placement.instance_id,
                                    vector,
                                )
                            });
                            let vector = deformed.as_ref().unwrap_or(vector);
                            for path in &vector.paths {
                                let points: Vec<Pos2> = flatten_path(path)
                                    .into_iter()
                                    .map(|point| stage_to_screen(composed.apply(point), view))
                                    .collect();
                                if points.len() >= 2 {
                                    painter.add(Shape::line(points, Stroke::new(1.15_f32, color)));
                                }
                            }
                        }
                        Asset::Bitmap(bitmap) => {
                            let corners = [
                                Vec2::new(0.0, 0.0),
                                Vec2::new(f32::from(bitmap.width), 0.0),
                                Vec2::new(f32::from(bitmap.width), f32::from(bitmap.height)),
                                Vec2::new(0.0, f32::from(bitmap.height)),
                                Vec2::new(0.0, 0.0),
                            ];
                            let points = corners
                                .into_iter()
                                .map(|point| stage_to_screen(composed.apply(point), view))
                                .collect();
                            painter.add(Shape::line(points, Stroke::new(1.15_f32, color)));
                        }
                        Asset::Q0v(_) | Asset::Rig(_) => {}
                    }
                }
                Target::Q0rg(child_id) if child_id != q0rg_id => {
                    let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                        project,
                        child_id,
                        local_frame,
                        resolved.playback_start_frame,
                        resolved.fx.q0rg_playback,
                    );
                    render_q0rg_outline(
                        painter,
                        project,
                        lookup,
                        child_id,
                        child_frame,
                        composed,
                        view,
                        depth + 1,
                        None,
                        hidden_mask_art_layers,
                    );
                }
                Target::Q0rg(_) => {}
            }
        }
    }
}

fn outline_color_for_layer(layer_id: u16) -> Color32 {
    const COLORS: [Color32; 8] = [
        Color32::from_rgb(255, 86, 102),
        Color32::from_rgb(51, 211, 182),
        Color32::from_rgb(255, 190, 66),
        Color32::from_rgb(108, 137, 255),
        Color32::from_rgb(221, 102, 255),
        Color32::from_rgb(56, 196, 239),
        Color32::from_rgb(245, 131, 63),
        Color32::from_rgb(155, 218, 87),
    ];
    COLORS[usize::from(layer_id) % COLORS.len()]
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_stage_with_revision(
    painter: &Painter,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    revision: (u64, u64),
) {
    StageRenderPass::new_with_revision(project, textures, Some(revision))
        .render(painter, q0rg_id, frame, view, textures, ctx);
}

/// Same as `render_stage` but every emitted shape's RGBA is multiplied by
/// `tint` (per-channel × tint / 255). Used for onion-skin renders: pass a
/// low-alpha bluish/orangeish Color32 to dim and colour-cast historical /
/// upcoming frames.
#[allow(clippy::too_many_arguments)]
pub fn render_stage_tinted(
    painter: &Painter,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
) {
    StageRenderPass::new(project, textures)
        .render_tinted(painter, q0rg_id, frame, view, textures, ctx, tint);
}

#[allow(clippy::too_many_arguments)]
fn render_q0rg(
    painter: &Painter,
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    depth: u8,
    tint: Color32,
    root_layer_filter: Option<RootLayerFilter<'_>>,
) {
    let mut vector_batch = VectorMeshBatch::default();
    render_q0rg_with_vector_batch(
        painter,
        project,
        lookup,
        q0rg_id,
        frame,
        parent,
        view,
        textures,
        ctx,
        depth,
        tint,
        root_layer_filter,
        &mut vector_batch,
        q0glblend::ColorTransform::default(),
    );
    vector_batch.flush(painter);
    textures.record_vector_mesh_batch(&vector_batch);
}

#[allow(clippy::too_many_arguments)]
fn render_q0rg_with_vector_batch(
    painter: &Painter,
    project: &ProjectV2,
    lookup: &RenderLookup<'_>,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    depth: u8,
    tint: Color32,
    root_layer_filter: Option<RootLayerFilter<'_>>,
    vector_batch: &mut VectorMeshBatch,
    inherited_color: q0glblend::ColorTransform,
) {
    if depth > Q0RG_RECURSION_LIMIT {
        return;
    }
    let Some(q0rg) = lookup.q0rg(q0rg_id) else {
        return;
    };
    textures.record_stage_q0rg_visit();
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    // Resolve the optional rig once per q0rg render. This must never call
    // q0s_format::rig::rig_for_q0rg inside the placement loop: that helper
    // linearly scans every project asset and turns large FLA libraries into
    // O(active placements * all assets) work per frame.
    let rig = lookup.rig(q0rg_id);
    let rig_pose = lookup.rig_pose(q0rg_id, local_frame);

    for layer in &q0rg.layers {
        if depth == 0 && root_layer_filter.is_some_and(|filter| !filter.includes(layer.layer_id)) {
            continue;
        }
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        let is_mask =
            project.layer_metadata(q0rg_id, layer.layer_id).kind == q0s_format::v2::LayerKind::Mask;
        if is_mask && !textures.mask_art_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        // Blank mask frames disable that mask; populated masks keep clipping.
        let mask_sources = if layer_has_mask_assignment(project, q0rg_id, layer.layer_id) {
            project.layer_mask_sources(q0rg_id, layer.layer_id)
        } else {
            Default::default()
        }
        .into_iter()
        .filter(|mask_id| {
            textures.mask_clips(q0rg_id, *mask_id)
                && !mask_layer_is_blank(q0rg, *mask_id, local_frame, textures)
        })
        .collect::<Vec<_>>();
        if !mask_sources.is_empty()
            && mask_sources.iter().any(|mask_layer_id| {
                !mask_layer_has_active_art(project, q0rg, *mask_layer_id, local_frame, textures)
            })
        {
            continue;
        }
        if !mask_sources.is_empty() {
            vector_batch.flush(painter);
            if let Some((masks, placements)) = collect_gpu_masked_layer(
                project,
                lookup,
                q0rg,
                layer,
                local_frame,
                parent,
                view,
                textures,
                ctx,
                depth,
                tint,
                inherited_color,
            ) {
                q0glblend::paint_vector_masked_placements_on_opaque_backdrop(
                    painter, masks, placements,
                );
            }
            continue;
        }
        let active_placements = textures.active_placement_states_at(q0rg_id, layer, local_frame);
        textures.record_stage_active_placements(active_placements.len());
        // Only layers whose held keyframe carries a shape tween pay for this.
        let shape_morph = active_placements
            .iter()
            .any(|active| {
                layer
                    .placements
                    .get(active.index)
                    .is_some_and(|placement| placement.tween.is_shape())
            })
            .then(|| layer_shape_morph(lookup, layer, local_frame))
            .flatten();
        for active in active_placements.iter().copied() {
            if depth == 0
                && root_layer_filter
                    .is_some_and(|filter| !filter.includes_placement(layer.layer_id, active.index))
            {
                continue;
            }
            let Some(placement) = layer.placements.get(active.index) else {
                continue;
            };
            let resolved = Resolved {
                placement,
                playback_start_frame: active.playback_start_frame,
                interp: active.transform,
                fx: active.fx,
                index: active.index,
            };
            if textures.hidden_text_placement == Some((q0rg_id, layer.layer_id, resolved.index)) {
                continue;
            }
            let placement = resolved.placement;
            let local_affine = rig_pose
                .as_ref()
                .and_then(|pose| pose.binding_transform(placement.instance_id))
                .unwrap_or_else(|| Affine::from_transform(resolved.interp));
            // Affine matrix composition — propagates parent skew and
            // non-uniform scale through to children, which the old
            // `compose(Transform2D, Transform2D)` discarded.
            let composed = Affine::compose(parent, local_affine);
            let color_only =
                placement_fx_is_visual_identity(placement_fx_without_tint(resolved.fx));
            let color_transform = if color_only {
                resolved
                    .fx
                    .tint
                    .map(q0glblend::ColorTransform::tint)
                    .unwrap_or_default()
                    .then(inherited_color)
            } else {
                inherited_color
            };
            if let Some(shapes) = shape_morph
                .as_ref()
                .and_then(|morph| morph.for_placement(resolved.index))
            {
                // In-between shapes are new geometry on every frame: paint
                // them through the uncached dynamic-vector path, in order.
                vector_batch.flush(painter);
                let opacity = resolved.fx.opacity.clamp(0.0, 1.0);
                let tint = if opacity < 1.0 {
                    tint.gamma_multiply(opacity)
                } else {
                    tint
                };
                let morph_color = resolved
                    .fx
                    .tint
                    .map(q0glblend::ColorTransform::tint)
                    .unwrap_or_default()
                    .then(inherited_color);
                for shape in shapes {
                    render_asset_color(
                        painter,
                        &Asset::Vector(shape.clone()),
                        None,
                        composed,
                        view,
                        textures,
                        ctx,
                        tint,
                        morph_color,
                        Some(placement.instance_id),
                        0,
                        project.meta.fps,
                    );
                }
                continue;
            }
            if !color_only {
                vector_batch.flush(painter);
                if try_render_gpu_vector_filter_placement(
                    painter,
                    project,
                    lookup,
                    q0rg_id,
                    placement,
                    local_frame,
                    resolved.playback_start_frame,
                    composed,
                    resolved.fx,
                    view,
                    textures,
                    ctx,
                    depth,
                    tint,
                    inherited_color,
                ) {
                    continue;
                }
                if try_render_direct_vector_opacity_placement(
                    painter,
                    project,
                    lookup,
                    q0rg_id,
                    placement,
                    local_frame,
                    resolved.playback_start_frame,
                    composed,
                    resolved.fx,
                    view,
                    textures,
                    ctx,
                    depth,
                    tint,
                    inherited_color,
                ) {
                    continue;
                }
                if try_render_gpu_vector_blend_placement(
                    painter,
                    project,
                    lookup,
                    q0rg_id,
                    placement,
                    local_frame,
                    resolved.playback_start_frame,
                    composed,
                    resolved.fx,
                    view,
                    textures,
                    ctx,
                    depth,
                    tint,
                    inherited_color,
                ) {
                    continue;
                }
                let outer_only = placement_fx_keeps_vector_body(resolved.fx);
                render_fx_placement(
                    painter,
                    project,
                    q0rg_id,
                    layer.layer_id,
                    resolved.index,
                    placement,
                    local_frame,
                    composed,
                    resolved.fx,
                    view,
                    textures,
                    ctx,
                    tint,
                    outer_only,
                    inherited_color,
                );
                if !outer_only {
                    continue;
                }
            }
            let resolved_target = match (rig, rig_pose.as_ref()) {
                (Some(rig), Some(pose)) => q0s_format::rig::resolved_variant_target(
                    rig,
                    pose,
                    placement.instance_id,
                    placement.target,
                ),
                _ => placement.target,
            };
            if resolved.fx.is_identity() && rig.is_none() {
                if let Target::Asset(asset_id) = resolved_target {
                    let Some(asset) = lookup.asset(asset_id) else {
                        continue;
                    };
                    if !textures.asset_intersects_clip(
                        project,
                        asset_id,
                        asset,
                        composed,
                        view,
                        painter.clip_rect(),
                    ) {
                        continue;
                    }
                }
            }
            match resolved_target {
                Target::Asset(asset_id) => {
                    if let Some(asset) = lookup.asset(asset_id) {
                        let deformed = match (asset, rig_pose.as_ref()) {
                            (Asset::Vector(vector), Some(pose)) => rig
                                .and_then(|rig| {
                                    q0s_format::rig::deform_vector_for_instance(
                                        rig,
                                        pose,
                                        placement.instance_id,
                                        vector,
                                    )
                                })
                                .map(Asset::Vector),
                            _ => None,
                        };
                        let dynamic_instance_id = deformed.as_ref().map(|_| placement.instance_id);
                        let appearance = project.asset_appearances.get(&asset_id);
                        if color_only && deformed.is_none() && appearance.is_none() {
                            if let Asset::Vector(vector) = asset {
                                let compensation =
                                    textures.live_raw_stroke_scale_compensation(vector.asset_id);
                                let geometry = textures.vector_render_geometry(vector);
                                if vector_batch.append(
                                    painter,
                                    geometry,
                                    vector,
                                    composed,
                                    view,
                                    tint,
                                    compensation,
                                    color_transform,
                                ) {
                                    continue;
                                }
                            }
                        }
                        if matches!(asset, Asset::Vector(_)) {
                            vector_batch.record_fallback();
                        }
                        vector_batch.flush(painter);
                        render_asset_color(
                            painter,
                            deformed.as_ref().unwrap_or(asset),
                            appearance,
                            composed,
                            view,
                            textures,
                            ctx,
                            tint,
                            color_transform,
                            dynamic_instance_id,
                            local_frame.saturating_sub(resolved.playback_start_frame),
                            project.meta.fps,
                        );
                    }
                }
                Target::Q0rg(child_id) => {
                    if child_id != q0rg_id {
                        let child_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                            project,
                            child_id,
                            local_frame,
                            resolved.playback_start_frame,
                            resolved.fx.q0rg_playback,
                        );
                        render_q0rg_with_vector_batch(
                            painter,
                            project,
                            lookup,
                            child_id,
                            child_frame,
                            composed,
                            view,
                            textures,
                            ctx,
                            depth + 1,
                            tint,
                            None,
                            vector_batch,
                            color_transform,
                        );
                    }
                }
            }
        }
    }
}

/// In-between artwork of a shape tween on this layer frame, if there is one.
fn layer_shape_morph(
    lookup: &RenderLookup<'_>,
    layer: &q0s_format::v2::Layer,
    frame: u16,
) -> Option<q0s_format::shape_tween::LayerMorph> {
    q0s_format::shape_tween::layer_morph(layer, frame, |asset_id| match lookup.asset(asset_id) {
        Some(Asset::Vector(vector)) => Some(vector),
        _ => None,
    })
}

/// Public snapshot of which placements are active at `frame` and what their
/// interpolated transform is. Used by Insert-Keyframe to capture the current
/// visual state and bake it into a new keyframe placement.
pub fn active_placements_at(
    layer: &q0s_format::v2::Layer,
    frame: u16,
) -> Vec<(usize, Transform2D)> {
    resolve_layer_at_frame(layer, frame)
        .into_iter()
        .map(|r| (r.index, r.interp))
        .collect()
}

/// Return the visual transform of one placement when it is the active
/// keyframe/span for `frame`. A held or tweened placement therefore remains
/// selectable even though its source keyframe lives earlier on the timeline.
pub fn active_transform_for_placement(
    layer: &q0s_format::v2::Layer,
    placement_idx: usize,
    frame: u16,
) -> Option<Transform2D> {
    resolve_layer_at_frame(layer, frame)
        .into_iter()
        .find(|resolved| resolved.index == placement_idx)
        .map(|resolved| resolved.interp)
}

pub fn placement_is_active_at(
    layer: &q0s_format::v2::Layer,
    placement_idx: usize,
    frame: u16,
) -> bool {
    if layer
        .timeline_frame_count
        .is_some_and(|frame_count| frame >= frame_count)
    {
        return false;
    }
    let Some(placement) = layer.placements.get(placement_idx) else {
        return false;
    };
    let Some(keyframe) = layer
        .explicit_keyframes
        .iter()
        .copied()
        .chain(layer.placements.iter().map(|candidate| candidate.frame))
        .filter(|candidate| *candidate <= frame)
        .max()
    else {
        return false;
    };
    // A complete layer keyframe replaces the previous one as a whole. Activity
    // therefore depends only on membership in the latest keyframe; resolving
    // tween transforms/FX for every sibling is unnecessary for this yes/no test.
    placement.frame == keyframe
}

struct Resolved<'a> {
    placement: &'a Placement,
    playback_start_frame: u16,
    interp: Transform2D,
    fx: PlacementFx,
    /// Index of the active placement within `layer.placements` — exposed so
    /// `active_placements_at` can hand callers a stable reference.
    index: usize,
}

/// Resolve one complete layer keyframe at `frame`.
///
/// Every placement stored at the same frame belongs to the same layer keyframe.
/// The latest keyframe at or before the playhead replaces the previous keyframe
/// as a whole. This is deliberately not keyed by asset id: raw graphics produce
/// new assets as they are edited, and multiple instances may legitimately share
/// one asset inside the same keyframe.
fn resolve_layer_at_frame(layer: &q0s_format::v2::Layer, frame: u16) -> Vec<Resolved<'_>> {
    q0s_format::raster::active_placement_states_at(layer, frame)
        .into_iter()
        .filter_map(|active| {
            layer
                .placements
                .get(active.index)
                .map(|placement| Resolved {
                    placement,
                    playback_start_frame: active.playback_start_frame,
                    interp: active.transform,
                    fx: active.fx,
                    index: active.index,
                })
        })
        .collect()
}

struct LayerFrameIndex {
    placement_count: usize,
    playback_starts: Vec<u16>,
    explicit_keyframe_count: usize,
    keyframes: Vec<u16>,
    placements_by_frame: HashMap<u16, Vec<usize>>,
    tween_destinations: HashMap<(u16, u8, u16), Vec<usize>>,
    resolved_frame: Option<(u16, Arc<Vec<q0s_format::raster::ActivePlacementState>>)>,
}

fn lerp_render_transform(left: Transform2D, right: Transform2D, t: f32) -> Transform2D {
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    Transform2D {
        tx: lerp(left.tx, right.tx),
        ty: lerp(left.ty, right.ty),
        sx: lerp(left.sx, right.sx),
        sy: lerp(left.sy, right.sy),
        rotation: lerp(left.rotation, right.rotation),
        skew_x: lerp(left.skew_x, right.skew_x),
        skew_y: lerp(left.skew_y, right.skew_y),
    }
}

impl LayerFrameIndex {
    fn new(layer: &q0s_format::v2::Layer) -> Self {
        let mut keyframes = layer.explicit_keyframes.clone();
        let mut placements_by_frame: HashMap<u16, Vec<usize>> = HashMap::new();
        let mut tween_destinations: HashMap<(u16, u8, u16), Vec<usize>> = HashMap::new();
        for (index, placement) in layer.placements.iter().enumerate() {
            keyframes.push(placement.frame);
            placements_by_frame
                .entry(placement.frame)
                .or_default()
                .push(index);
            let (kind, id) = Self::target_key(placement.target);
            tween_destinations
                .entry((placement.frame, kind, id))
                .or_default()
                .push(index);
        }
        keyframes.sort_unstable();
        keyframes.dedup();
        Self {
            placement_count: layer.placements.len(),
            playback_starts: q0s_format::v2::placement_playback_start_frames(layer),
            explicit_keyframe_count: layer.explicit_keyframes.len(),
            keyframes,
            placements_by_frame,
            tween_destinations,
            resolved_frame: None,
        }
    }

    fn matches_structure(&self, layer: &q0s_format::v2::Layer) -> bool {
        self.placement_count == layer.placements.len()
            && self.explicit_keyframe_count == layer.explicit_keyframes.len()
    }

    fn target_key(target: Target) -> (u8, u16) {
        match target {
            Target::Asset(id) => (0, id),
            Target::Q0rg(id) => (1, id),
        }
    }

    fn resolve_shared(
        &mut self,
        layer: &q0s_format::v2::Layer,
        frame: u16,
    ) -> Arc<Vec<q0s_format::raster::ActivePlacementState>> {
        if !self.matches_structure(layer) {
            *self = Self::new(layer);
        }
        if let Some((cached_frame, states)) = &self.resolved_frame {
            if *cached_frame == frame {
                return Arc::clone(states);
            }
        }
        let states = Arc::new(self.resolve_uncached(layer, frame));
        self.resolved_frame = Some((frame, Arc::clone(&states)));
        states
    }

    fn invalidate_resolved_frame(&mut self) {
        self.resolved_frame = None;
    }

    fn resolve_uncached(
        &self,
        layer: &q0s_format::v2::Layer,
        frame: u16,
    ) -> Vec<q0s_format::raster::ActivePlacementState> {
        let keyframe_count = self
            .keyframes
            .partition_point(|candidate| *candidate <= frame);
        let Some(&keyframe) = keyframe_count
            .checked_sub(1)
            .and_then(|index| self.keyframes.get(index))
        else {
            return Vec::new();
        };
        let Some(source_indices) = self.placements_by_frame.get(&keyframe) else {
            return Vec::new();
        };
        let has_forward_tween = source_indices.iter().any(|index| {
            layer.placements[*index]
                .tween
                .to_frame()
                .is_some_and(|to_frame| to_frame > keyframe)
        });
        if !has_forward_tween {
            return source_indices
                .iter()
                .map(|&index| {
                    let placement = &layer.placements[index];
                    q0s_format::raster::ActivePlacementState {
                        index,
                        playback_start_frame: self.playback_starts[index],
                        transform: placement.transform,
                        fx: placement.fx,
                    }
                })
                .collect();
        }

        let mut occurrences: HashMap<(u8, u16), usize> = HashMap::new();
        source_indices
            .iter()
            .map(|&index| {
                let placement = &layer.placements[index];
                let target = Self::target_key(placement.target);
                let occurrence = occurrences.entry(target).or_insert(0);
                let source_occurrence = *occurrence;
                *occurrence += 1;
                let (transform, fx) = match placement.tween.to_frame() {
                    Some(to_frame)
                        if to_frame > keyframe
                            && frame >= keyframe
                            && !placement.tween.is_shape() =>
                    {
                        let destination = self
                            .tween_destinations
                            .get(&(to_frame, target.0, target.1))
                            .and_then(|indices| indices.get(source_occurrence))
                            .and_then(|target_index| layer.placements.get(*target_index));
                        let denominator = f32::from(to_frame - keyframe);
                        let raw_t = (f32::from(frame - keyframe) / denominator).clamp(0.0, 1.0);
                        let t = placement.tween.easing().sample(raw_t);
                        let target_transform = destination
                            .map(|candidate| candidate.transform)
                            .unwrap_or(placement.transform);
                        let target_fx = destination
                            .map(|candidate| candidate.fx)
                            .unwrap_or(placement.fx);
                        (
                            lerp_render_transform(placement.transform, target_transform, t),
                            placement.fx.lerp_to(target_fx, t),
                        )
                    }
                    Some(_) | None => (placement.transform, placement.fx),
                };
                q0s_format::raster::ActivePlacementState {
                    index,
                    playback_start_frame: self.playback_starts[index],
                    transform,
                    fx,
                }
            })
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_asset(
    painter: &Painter,
    asset: &Asset,
    appearance: Option<&VectorAppearance>,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
    dynamic_instance_id: Option<u32>,
    elapsed_host_frames: u16,
    host_fps: u16,
) {
    render_asset_color(
        painter,
        asset,
        appearance,
        transform,
        view,
        textures,
        ctx,
        tint,
        q0glblend::ColorTransform::default(),
        dynamic_instance_id,
        elapsed_host_frames,
        host_fps,
    );
}
#[allow(clippy::too_many_arguments)]
fn render_asset_color(
    painter: &Painter,
    asset: &Asset,
    appearance: Option<&VectorAppearance>,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
    color_transform: q0glblend::ColorTransform,
    dynamic_instance_id: Option<u32>,
    elapsed_host_frames: u16,
    host_fps: u16,
) {
    match asset {
        Asset::Bitmap(b) => {
            let tex = textures.by_asset_id.entry(b.asset_id).or_insert_with(|| {
                let image = ColorImage::from_rgba_unmultiplied(
                    [usize::from(b.width), usize::from(b.height)],
                    &b.rgba,
                );
                ctx.load_texture(
                    format!("q0s_asset_{}", b.asset_id),
                    image,
                    TextureOptions::NEAREST,
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
                    color: tint,
                });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            q0glblend::paint_mesh_color(painter, mesh, color_transform);
        }
        Asset::Q0v(video) => {
            if let Some((texture, size)) =
                textures.video_texture(video, elapsed_host_frames, host_fps, ctx)
            {
                q0glblend::paint_mesh_color(
                    painter,
                    video_frame_mesh(texture.id(), size, transform, view, tint),
                    color_transform,
                );
            }
        }
        Asset::Rig(_) => {}
        Asset::Vector(v) => {
            if let Some(appearance) = appearance {
                if appearance.fill_gradient.is_some() {
                    #[cfg(feature = "appearance-mask-eraser")]
                    paint_vector_appearance_halo_color(
                        painter,
                        v,
                        appearance,
                        transform,
                        view,
                        textures,
                        ctx,
                        tint,
                        color_transform,
                    );
                    {
                        let gradient_geometry = textures.gradient_render_geometry(v, appearance);
                        paint_cached_vector_gradient_color(
                            painter,
                            gradient_geometry,
                            transform,
                            view,
                            tint,
                            color_transform,
                        );
                    }
                    if let Some(stroke_style) = &v.stroke {
                        let geometry = textures.vector_render_geometry(v);
                        let parent_scale = transform.uniform_scale();
                        let stroke = Stroke::new(
                            stroke_style.width * parent_scale * view.effective_scale(),
                            modulate(rgba_to_color32(stroke_style.color), tint),
                        );
                        for (polyline_local, closed) in &geometry.stroke_paths {
                            if polyline_local.len() < 2 {
                                continue;
                            }
                            let polyline_screen: Vec<Pos2> = polyline_local
                                .iter()
                                .map(|point| stage_to_screen(transform.apply(*point), view))
                                .collect();
                            if let Some(mesh) = bounded_stroke_mesh(
                                &polyline_screen,
                                stroke.width,
                                *closed,
                                stroke.color,
                            ) {
                                q0glblend::paint_mesh_color(painter, mesh, color_transform);
                            }
                            if !*closed
                                && matches!(stroke_style.cap, q0s_format::geom::CapShape::Round)
                            {
                                let radius = stroke_style.width
                                    * parent_scale
                                    * view.effective_scale()
                                    * 0.5;
                                let color = modulate(rgba_to_color32(stroke_style.color), tint);
                                if let Some(point) = polyline_screen.first() {
                                    painter.circle_filled(
                                        *point,
                                        radius,
                                        color_transform.apply(color),
                                    );
                                }
                                if let Some(point) = polyline_screen.last() {
                                    painter.circle_filled(
                                        *point,
                                        radius,
                                        color_transform.apply(color),
                                    );
                                }
                            }
                        }
                    }
                    return;
                }
            }
            if let Some(instance_id) = dynamic_instance_id {
                #[cfg(feature = "appearance-mask-eraser")]
                if let Some(appearance) = appearance {
                    paint_dynamic_vector_appearance_halo_color(
                        painter,
                        v,
                        appearance,
                        transform,
                        view,
                        textures,
                        ctx,
                        tint,
                        color_transform,
                        instance_id,
                    );
                }
                let parent_scale = transform.uniform_scale();
                let fill_color = v
                    .fill
                    .map(|color| color_transform.apply(modulate(rgba_to_color32(color), tint)))
                    .unwrap_or(Color32::TRANSPARENT);
                let geometry = build_dynamic_vector_body_geometry(v, appearance);
                if fill_color != Color32::TRANSPARENT {
                    paint_cached_vector_fill(painter, &geometry, transform, view, fill_color);
                }
                if let Some(stroke_style) = &v.stroke {
                    let stroke = Stroke::new(
                        stroke_style.width * parent_scale * view.effective_scale(),
                        modulate(rgba_to_color32(stroke_style.color), tint),
                    );
                    for (polyline_local, closed) in &geometry.stroke_paths {
                        if polyline_local.len() < 2 {
                            continue;
                        }
                        let polyline_screen: Vec<Pos2> = polyline_local
                            .iter()
                            .map(|point| stage_to_screen(transform.apply(*point), view))
                            .collect();
                        if let Some(mesh) = bounded_stroke_mesh(
                            &polyline_screen,
                            stroke.width,
                            *closed,
                            stroke.color,
                        ) {
                            q0glblend::paint_mesh_color(painter, mesh, color_transform);
                        }
                        if !*closed && matches!(stroke_style.cap, q0s_format::geom::CapShape::Round)
                        {
                            let radius =
                                stroke_style.width * parent_scale * view.effective_scale() * 0.5;
                            let color = modulate(rgba_to_color32(stroke_style.color), tint);
                            if let Some(point) = polyline_screen.first() {
                                painter.circle_filled(*point, radius, color_transform.apply(color));
                            }
                            if let Some(point) = polyline_screen.last() {
                                painter.circle_filled(*point, radius, color_transform.apply(color));
                            }
                        }
                    }
                }
                return;
            }
            #[cfg(feature = "appearance-mask-eraser")]
            if let Some(appearance) = appearance {
                paint_vector_appearance_halo_color(
                    painter,
                    v,
                    appearance,
                    transform,
                    view,
                    textures,
                    ctx,
                    tint,
                    color_transform,
                );
                if let Some(fill) = v.fill {
                    let fill_color = color_transform.apply(modulate(rgba_to_color32(fill), tint));
                    if appearance.erase_mask.is_empty() && appearance.clip_mask.is_empty() {
                        let geometry = textures.vector_render_geometry(v);
                        paint_cached_vector_fill(painter, geometry, transform, view, fill_color);
                    } else {
                        paint_masked_vector_body_cached(
                            painter, v, appearance, transform, view, textures, fill_color,
                        );
                    }
                }
                // Appearance metadata is validated only for fill-only vectors.
                // The body above stays real tessellated vector geometry; only
                // the soft halo is a filtered texture.
                return;
            }
            #[cfg(not(feature = "appearance-mask-eraser"))]
            let _ = appearance;
            // Multiply the on-disk stroke width by the composed area
            // scale so a 2× scaled q0rg actually renders 2×-thick
            // outlines. Otherwise stroked vectors look comically thin
            // when shrunk and thin-as-paper when blown up.
            let parent_scale =
                transform.uniform_scale() * textures.live_raw_stroke_scale_compensation(v.asset_id);
            let fill_color = v
                .fill
                .map(|color| color_transform.apply(modulate(rgba_to_color32(color), tint)))
                .unwrap_or(Color32::TRANSPARENT);
            let compensation = textures.live_raw_stroke_scale_compensation(v.asset_id);
            let geometry = textures.vector_render_geometry(v);
            let cached_stroke = v.stroke.and_then(|stroke| {
                if (compensation - 1.0).abs() > 1.0e-6 {
                    return None;
                }
                cached_vector_stroke_mesh(
                    geometry,
                    transform,
                    view,
                    color_transform.apply(modulate(rgba_to_color32(stroke.color), tint)),
                )
            });

            // Fill and cached stroke use the same untextured mesh pipeline. Merge
            // them into one egui shape so dense FLA sprite-sheet frames do not
            // spend thousands of draw-list entries on a fill/stroke pair.
            if cached_stroke.is_some() {
                let mut body = Mesh::default();
                if fill_color != Color32::TRANSPARENT {
                    if let Some(fill) =
                        cached_vector_fill_mesh(geometry, transform, view, fill_color)
                    {
                        body.append(fill);
                    }
                }
                if let Some(stroke) = cached_stroke {
                    body.append(stroke);
                }
                if !body.is_empty() {
                    painter.add(Shape::Mesh(body));
                }
            } else {
                if fill_color != Color32::TRANSPARENT {
                    paint_cached_vector_fill(painter, geometry, transform, view, fill_color);
                }
                if let Some(s) = &v.stroke {
                    let color = modulate(rgba_to_color32(s.color), tint);
                    let stroke =
                        Stroke::new(s.width * parent_scale * view.effective_scale(), color);
                    for (polyline_local, closed) in &geometry.stroke_paths {
                        if polyline_local.len() < 2 {
                            continue;
                        }
                        let polyline_screen: Vec<Pos2> = polyline_local
                            .iter()
                            .map(|point| stage_to_screen(transform.apply(*point), view))
                            .collect();
                        if let Some(mesh) = bounded_stroke_mesh(
                            &polyline_screen,
                            stroke.width,
                            *closed,
                            stroke.color,
                        ) {
                            q0glblend::paint_mesh_color(painter, mesh, color_transform);
                        }
                        if !*closed && matches!(s.cap, q0s_format::geom::CapShape::Round) {
                            let radius = s.width * parent_scale * view.effective_scale() * 0.5;
                            if let Some(point) = polyline_screen.first() {
                                painter.circle_filled(*point, radius, color_transform.apply(color));
                            }
                            if let Some(point) = polyline_screen.last() {
                                painter.circle_filled(*point, radius, color_transform.apply(color));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Fill a simple concave polygon through ear-clipping triangulation.
/// Returns without painting if the contour is degenerate. Brush contours are
/// simple in normal use; a defensive fan fallback is intentionally avoided
/// because that is precisely the visual corruption this function replaces.
pub fn paint_concave_fill(painter: &Painter, points: &[Pos2], color: Color32) {
    paint_complex_fill(painter, &[points.to_vec()], color);
}

pub fn paint_complex_fill(painter: &Painter, contours: &[Vec<Pos2>], color: Color32) {
    let Some(buffers) = tessellate_complex_fill(contours) else {
        return;
    };
    // The native framebuffer is 4x multisampled. Keeping the fill as one mesh
    // avoids double-blending translucent edges and lets MSAA resolve coverage.
    paint_lyon_buffers(painter, buffers, color);
}

fn tessellate_bounded_stroke(
    points: &[Pos2],
    width: f32,
    closed: bool,
) -> Option<VertexBuffers<lyon_path::math::Point, u32>> {
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
    let mut buffers: VertexBuffers<lyon_path::math::Point, u32> = VertexBuffers::new();
    let mut tessellator = StrokeTessellator::new();
    let options = StrokeOptions::default()
        .with_line_width(width)
        .with_line_join(LineJoin::Bevel)
        .with_tolerance(0.05);
    if tessellator
        .tessellate_path(
            &path,
            &options,
            &mut BuffersBuilder::new(&mut buffers, Positions),
        )
        .is_err()
    {
        return None;
    }
    Some(buffers)
}

fn tessellate_closed_bevel_stroke(
    points: &[Pos2],
    width: f32,
) -> Option<VertexBuffers<lyon_path::math::Point, u32>> {
    tessellate_bounded_stroke(points, width, true)
}

fn bounded_stroke_mesh(points: &[Pos2], width: f32, closed: bool, color: Color32) -> Option<Mesh> {
    let buffers = tessellate_bounded_stroke(points, width, closed)?;
    Some(lyon_buffers_mesh(buffers, color))
}

/// Closed display stroke with bounded bevel joins. egui 0.27 uses an unlimited
/// miter for closed PathShape strokes, which can produce giant spikes when a
/// thin selection contour nearly folds back on itself at low zoom.
pub(crate) fn closed_bevel_stroke_mesh(
    points: &[Pos2],
    width: f32,
    color: Color32,
) -> Option<Mesh> {
    let buffers = tessellate_closed_bevel_stroke(points, width)?;
    Some(lyon_buffers_mesh(buffers, color))
}

pub fn paint_closed_bevel_stroke(painter: &Painter, points: &[Pos2], width: f32, color: Color32) {
    if let Some(mesh) = closed_bevel_stroke_mesh(points, width, color) {
        painter.add(Shape::Mesh(mesh));
    }
}

#[cfg(feature = "appearance-mask-eraser")]
fn exact_closed_contour(points: &[Pos2]) -> Vec<Pos2> {
    let mut result = Vec::with_capacity(points.len());
    for point in points
        .iter()
        .copied()
        .filter(|point| point.x.is_finite() && point.y.is_finite())
    {
        if result.last().is_some_and(|previous| *previous == point) {
            continue;
        }
        result.push(point);
    }
    if result.len() > 1 && result.first() == result.last() {
        result.pop();
    }
    result
}

#[cfg(feature = "appearance-mask-eraser")]
const SELECTION_PARAMETRIC_MIN_WIDTH: f32 = 1.0 / 64.0;
#[cfg(feature = "appearance-mask-eraser")]
const SELECTION_PARAMETRIC_MAX_WIDTH: f32 = 64.0;
#[cfg(feature = "appearance-mask-eraser")]
const SELECTION_PARAMETRIC_NEIGHBOR_FACTOR: f32 = 2.0;
#[cfg(feature = "appearance-mask-eraser")]
const SELECTION_PARAMETRIC_BOUNDARY_STEPS: usize = 12;

#[cfg(feature = "appearance-mask-eraser")]
fn same_stroke_topology(
    left: &VertexBuffers<lyon_path::math::Point, u32>,
    right: &VertexBuffers<lyon_path::math::Point, u32>,
) -> bool {
    left.indices == right.indices && left.vertices.len() == right.vertices.len()
}

#[cfg(feature = "appearance-mask-eraser")]
fn parametric_stroke_from_samples(
    a_width: f32,
    a: &VertexBuffers<lyon_path::math::Point, u32>,
    b_width: f32,
    b: &VertexBuffers<lyon_path::math::Point, u32>,
) -> Option<CachedParametricStroke> {
    if !same_stroke_topology(a, b) {
        return None;
    }
    let width_delta = b_width - a_width;
    if !width_delta.is_finite() || width_delta.abs() <= 1.0e-8 {
        return None;
    }
    let mut base_vertices = Vec::with_capacity(a.vertices.len());
    let mut width_vectors = Vec::with_capacity(a.vertices.len());
    for (a, b) in a.vertices.iter().zip(&b.vertices) {
        let a = pos2(a.x, a.y);
        let b = pos2(b.x, b.y);
        let width_vector = (b - a) / width_delta;
        base_vertices.push(a - width_vector * a_width);
        width_vectors.push(width_vector);
    }
    Some(CachedParametricStroke {
        base_vertices,
        width_vectors,
        indices: a.indices.clone(),
        min_width: a_width.min(b_width),
        max_width: a_width.max(b_width),
    })
}

#[cfg(feature = "appearance-mask-eraser")]
fn parametric_range_toward(
    points: &[Pos2],
    target_width: f32,
    target: &VertexBuffers<lyon_path::math::Point, u32>,
    neighbor_width: f32,
    neighbor: VertexBuffers<lyon_path::math::Point, u32>,
) -> Option<CachedParametricStroke> {
    parametric_range_toward_bounded(points, true, target_width, target, neighbor_width, neighbor)
}

#[cfg(feature = "appearance-mask-eraser")]
fn parametric_range_toward_bounded(
    points: &[Pos2],
    closed: bool,
    target_width: f32,
    target: &VertexBuffers<lyon_path::math::Point, u32>,
    neighbor_width: f32,
    neighbor: VertexBuffers<lyon_path::math::Point, u32>,
) -> Option<CachedParametricStroke> {
    if same_stroke_topology(target, &neighbor) {
        return parametric_stroke_from_samples(target_width, target, neighbor_width, &neighbor);
    }

    // Lyon changes topology at a few width thresholds for near-folding contours.
    // Search only the side of the current width that actually crossed a threshold.
    // This keeps cache creation bounded while still extending the fast range right
    // up to the transition instead of re-tessellating on every wheel tick.
    let mut same_width = target_width;
    let mut other_width = neighbor_width;
    let mut nearest_same = None;
    for _ in 0..SELECTION_PARAMETRIC_BOUNDARY_STEPS {
        let midpoint_width = (same_width * other_width).sqrt();
        let midpoint = tessellate_bounded_stroke(points, midpoint_width, closed)?;
        if same_stroke_topology(target, &midpoint) {
            same_width = midpoint_width;
            nearest_same = Some((midpoint_width, midpoint));
        } else {
            other_width = midpoint_width;
        }
    }
    let (same_width, same) = nearest_same?;
    parametric_stroke_from_samples(target_width, target, same_width, &same)
}

#[cfg(feature = "appearance-mask-eraser")]
fn cache_parametric_range_for_width(stroke: &mut CachedSelectionStroke, stage_width: f32) -> bool {
    if stroke
        .parametric
        .iter()
        .any(|range| stage_width >= range.min_width && stage_width <= range.max_width)
    {
        return true;
    }
    if !stage_width.is_finite()
        || !(SELECTION_PARAMETRIC_MIN_WIDTH..=SELECTION_PARAMETRIC_MAX_WIDTH).contains(&stage_width)
    {
        return false;
    }

    let target = match tessellate_closed_bevel_stroke(&stroke.contour_stage, stage_width) {
        Some(target) => target,
        None => return false,
    };
    let lower_width =
        (stage_width / SELECTION_PARAMETRIC_NEIGHBOR_FACTOR).max(SELECTION_PARAMETRIC_MIN_WIDTH);
    let upper_width =
        (stage_width * SELECTION_PARAMETRIC_NEIGHBOR_FACTOR).min(SELECTION_PARAMETRIC_MAX_WIDTH);

    if lower_width < stage_width {
        if let Some(lower) = tessellate_closed_bevel_stroke(&stroke.contour_stage, lower_width) {
            if let Some(range) = parametric_range_toward(
                &stroke.contour_stage,
                stage_width,
                &target,
                lower_width,
                lower,
            ) {
                stroke.parametric.push(range);
            }
        }
    }
    if upper_width > stage_width {
        if let Some(upper) = tessellate_closed_bevel_stroke(&stroke.contour_stage, upper_width) {
            if let Some(range) = parametric_range_toward(
                &stroke.contour_stage,
                stage_width,
                &target,
                upper_width,
                upper,
            ) {
                stroke.parametric.push(range);
            }
        }
    }
    stroke
        .parametric
        .sort_by(|left, right| left.min_width.total_cmp(&right.min_width));
    stroke
        .parametric
        .iter()
        .any(|range| stage_width >= range.min_width && stage_width <= range.max_width)
}

#[cfg(feature = "appearance-mask-eraser")]
fn cached_vector_fill_as_selection_stage_mesh(
    geometry: &CachedVectorRenderGeometry,
) -> Option<CachedSelectionStageMesh> {
    if geometry.fill_vertices.is_empty() && geometry.fill_contours.is_empty() {
        return None;
    }
    let strokes = geometry
        .fill_contours
        .iter()
        .map(|contour| exact_closed_contour(contour))
        .filter(|contour| contour.len() >= 3)
        .map(|contour_stage| CachedSelectionStroke {
            parametric: Vec::new(),
            contour_stage,
        })
        .collect();
    Some(CachedSelectionStageMesh {
        fill_vertices: geometry
            .fill_vertices
            .iter()
            .map(|point| pos2(point.x, point.y))
            .collect(),
        fill_indices: geometry.fill_indices.clone(),
        strokes,
        affine_reprojection_supported: None,
    })
}

#[cfg(feature = "appearance-mask-eraser")]
fn build_selection_stage_mesh(contours: &[Vec<Pos2>]) -> Option<CachedSelectionStageMesh> {
    if contours.is_empty() {
        return None;
    }
    let (fill_vertices, fill_indices) = tessellate_complex_fill(contours)
        .map(|buffers| {
            (
                buffers
                    .vertices
                    .into_iter()
                    .map(|point| pos2(point.x, point.y))
                    .collect::<Vec<_>>(),
                buffers.indices,
            )
        })
        .unwrap_or_default();
    let strokes: Vec<CachedSelectionStroke> = contours
        .iter()
        .map(|contour| exact_closed_contour(contour))
        .filter(|contour| contour.len() >= 3)
        .map(|contour_stage| CachedSelectionStroke {
            parametric: Vec::new(),
            contour_stage,
        })
        .collect();
    if fill_vertices.is_empty() && strokes.is_empty() {
        return None;
    }
    Some(CachedSelectionStageMesh {
        fill_vertices,
        fill_indices,
        strokes,
        affine_reprojection_supported: None,
    })
}

#[cfg(feature = "appearance-mask-eraser")]
fn stage_pos_to_screen(point: Pos2, view: &StageView) -> Pos2 {
    view.stage_to_screen(Vec2::new(point.x, point.y))
}

#[cfg(feature = "appearance-mask-eraser")]
fn parametric_stroke_screen_mesh(
    stroke: &CachedParametricStroke,
    width_points: f32,
    color: Color32,
    view: &StageView,
) -> Option<Mesh> {
    let scale = view.effective_scale().abs();
    if !scale.is_finite() || scale <= 1.0e-6 || !width_points.is_finite() || width_points <= 0.0 {
        return None;
    }
    let stage_width = width_points / scale;
    if stage_width < stroke.min_width || stage_width > stroke.max_width {
        return None;
    }
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(stroke.base_vertices.len());
    for (base, width_vector) in stroke.base_vertices.iter().zip(&stroke.width_vectors) {
        let stage = *base + *width_vector * stage_width;
        mesh.vertices.push(Vertex {
            pos: stage_pos_to_screen(stage, view),
            uv: Pos2::ZERO,
            color,
        });
    }
    mesh.indices = stroke.indices.clone();
    Some(mesh)
}

#[cfg(feature = "appearance-mask-eraser")]
fn cached_selection_stroke_screen_mesh(
    stroke: &mut CachedSelectionStroke,
    width_points: f32,
    color: Color32,
    view: &StageView,
) -> Option<Mesh> {
    if let Some(mesh) = stroke
        .parametric
        .iter()
        .find_map(|parametric| parametric_stroke_screen_mesh(parametric, width_points, color, view))
    {
        return Some(mesh);
    }
    let scale = view.effective_scale().abs();
    if scale.is_finite() && scale > 1.0e-6 && width_points.is_finite() && width_points > 0.0 {
        let stage_width = width_points / scale;
        if cache_parametric_range_for_width(stroke, stage_width) {
            if let Some(mesh) = stroke.parametric.iter().find_map(|parametric| {
                parametric_stroke_screen_mesh(parametric, width_points, color, view)
            }) {
                return Some(mesh);
            }
        }
    }
    direct_stroke_screen_mesh(&stroke.contour_stage, width_points, color, view)
}
#[cfg(feature = "appearance-mask-eraser")]
fn direct_stroke_screen_mesh(
    contour_stage: &[Pos2],
    width_points: f32,
    color: Color32,
    view: &StageView,
) -> Option<Mesh> {
    let screen: Vec<Pos2> = contour_stage
        .iter()
        .copied()
        .map(|point| stage_pos_to_screen(point, view))
        .collect();
    closed_bevel_stroke_mesh(&screen, width_points, color)
}

#[cfg(feature = "appearance-mask-eraser")]
fn parametric_stroke_reference_mesh(
    stroke: &mut CachedSelectionStroke,
    width_points: f32,
    color: Color32,
    reference_scale: f32,
) -> Option<Mesh> {
    let scale = reference_scale.abs();
    if !scale.is_finite() || scale <= 1.0e-6 || !width_points.is_finite() || width_points <= 0.0 {
        return None;
    }
    let stage_width = width_points / scale;
    if !stroke
        .parametric
        .iter()
        .any(|range| stage_width >= range.min_width && stage_width <= range.max_width)
        && !cache_parametric_range_for_width(stroke, stage_width)
    {
        return None;
    }
    let range = stroke
        .parametric
        .iter()
        .find(|range| stage_width >= range.min_width && stage_width <= range.max_width)?;
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(range.base_vertices.len());
    for (base, width_vector) in range.base_vertices.iter().zip(&range.width_vectors) {
        let center = pos2(base.x * reference_scale, base.y * reference_scale);
        let offset = *width_vector * width_points;
        mesh.vertices.push(Vertex {
            pos: center + offset,
            // Native cached selection rendering interprets this UV as the
            // fixed-pixel extrusion from the zero-width centerline.
            uv: pos2(offset.x, offset.y),
            color,
        });
    }
    mesh.indices = range.indices.clone();
    Some(mesh)
}

#[cfg(feature = "appearance-mask-eraser")]
fn fixed_screen_stroke_mesh(points: &[Pos2], width: f32, color: Color32) -> Option<Mesh> {
    fixed_screen_bounded_stroke_mesh(points, width, true, color)
}

#[cfg(feature = "appearance-mask-eraser")]
fn fixed_screen_bounded_stroke_mesh(
    points: &[Pos2],
    width: f32,
    closed: bool,
    color: Color32,
) -> Option<Mesh> {
    let target = tessellate_bounded_stroke(points, width, closed)?;
    let mut range = None;
    for neighbor_width in [width * 2.0, width * 0.5, width * 1.125, width * 0.875] {
        if neighbor_width <= 0.0 || !neighbor_width.is_finite() {
            continue;
        }
        let Some(neighbor) = tessellate_bounded_stroke(points, neighbor_width, closed) else {
            continue;
        };
        if let Some(candidate) = parametric_range_toward_bounded(
            points,
            closed,
            width,
            &target,
            neighbor_width,
            neighbor,
        ) {
            range = Some(candidate);
            break;
        }
    }
    let range = range?;
    if range.base_vertices.len() != target.vertices.len() || range.indices != target.indices {
        return None;
    }
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(target.vertices.len());
    for (vertex, base) in target.vertices.iter().zip(&range.base_vertices) {
        let pos = pos2(vertex.x, vertex.y);
        let offset = pos - *base;
        mesh.vertices.push(Vertex {
            // Keep Lyon's exact target-width position byte-for-byte; the base
            // is used only to recover the fixed-pixel extrusion in the shader.
            pos,
            uv: pos2(offset.x, offset.y),
            color,
        });
    }
    mesh.indices = target.indices;
    Some(mesh)
}

#[cfg(feature = "appearance-mask-eraser")]
fn build_selection_reference_meshes(
    source: &mut CachedSelectionStageMesh,
    reference_scale: f32,
    texture_id: TextureId,
    tile_size_points: f32,
    accent: Color32,
) -> Option<Vec<Mesh>> {
    if !reference_scale.is_finite() || reference_scale <= 1.0e-6 {
        return None;
    }
    let reference_view = StageView {
        origin: Pos2::ZERO,
        scale: reference_scale,
        stage_rect: Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
        edit_transform: q0s_format::transform::Affine::IDENTITY,
    };
    let mut meshes = Vec::with_capacity(3);
    if !source.fill_vertices.is_empty() && !source.fill_indices.is_empty() {
        let tile = tile_size_points.max(1.0);
        let mut fill = Mesh::with_texture(texture_id);
        fill.vertices.reserve(source.fill_vertices.len());
        for stage in &source.fill_vertices {
            let pos = stage_pos_to_screen(*stage, &reference_view);
            fill.vertices.push(Vertex {
                pos,
                uv: pos2(pos.x / tile, pos.y / tile),
                color: Color32::WHITE,
            });
        }
        fill.indices = source.fill_indices.clone();
        meshes.push(fill);
    }

    let mut black_outline = Mesh::default();
    let mut accent_outline = Mesh::default();
    for stroke in &mut source.strokes {
        let black = parametric_stroke_reference_mesh(
            stroke,
            2.0,
            Color32::from_black_alpha(180),
            reference_scale,
        )
        .or_else(|| {
            let screen = stroke
                .contour_stage
                .iter()
                .copied()
                .map(|point| stage_pos_to_screen(point, &reference_view))
                .collect::<Vec<_>>();
            fixed_screen_stroke_mesh(&screen, 2.0, Color32::from_black_alpha(180))
        })?;
        let accent_mesh = parametric_stroke_reference_mesh(stroke, 1.0, accent, reference_scale)
            .or_else(|| {
                let screen = stroke
                    .contour_stage
                    .iter()
                    .copied()
                    .map(|point| stage_pos_to_screen(point, &reference_view))
                    .collect::<Vec<_>>();
                fixed_screen_stroke_mesh(&screen, 1.0, accent)
            })?;
        black_outline.append(black);
        accent_outline.append(accent_mesh);
    }
    if !black_outline.is_empty() {
        meshes.push(black_outline);
    }
    if !accent_outline.is_empty() {
        meshes.push(accent_outline);
    }
    Some(meshes)
}

#[cfg(feature = "appearance-mask-eraser")]
pub(crate) fn raw_area_selection_reference_meshes(
    source: &mut CachedRawAreaSelection,
    texture_id: TextureId,
    tile_size: f32,
    accent: Color32,
) -> Option<Vec<Mesh>> {
    let mut meshes =
        build_selection_reference_meshes(&mut source.fill, 1.0, texture_id, tile_size, accent)?;
    // Stroke selections are open polylines. Do not close their endpoints or
    // turn their enclosed area into a selected fill.
    let mut black = Mesh::default();
    let mut colored = Mesh::default();
    for points in &source.strokes {
        black.append(fixed_screen_bounded_stroke_mesh(
            points,
            3.0,
            false,
            Color32::from_black_alpha(210),
        )?);
        colored.append(fixed_screen_bounded_stroke_mesh(
            points, 1.5, false, accent,
        )?);
    }
    if !black.is_empty() {
        meshes.push(black);
    }
    if !colored.is_empty() {
        meshes.push(colored);
    }
    Some(meshes)
}

#[cfg(feature = "appearance-mask-eraser")]
pub(crate) fn raw_area_selection_screen_meshes(
    source: &mut CachedRawAreaSelection,
    view: &StageView,
    texture_id: TextureId,
    tile_size: f32,
    accent: Color32,
) -> Vec<Mesh> {
    let mut meshes = build_selection_screen_meshes(
        &mut source.fill,
        Affine::IDENTITY,
        view,
        texture_id,
        tile_size,
        accent,
        true,
    );
    let mut black = Mesh::default();
    let mut colored = Mesh::default();
    for points in &source.strokes {
        let screen: Vec<_> = points
            .iter()
            .map(|point| stage_pos_to_screen(*point, view))
            .collect();
        if let Some(mesh) = bounded_stroke_mesh(&screen, 3.0, false, Color32::from_black_alpha(210))
        {
            black.append(mesh);
        }
        if let Some(mesh) = bounded_stroke_mesh(&screen, 1.5, false, accent) {
            colored.append(mesh);
        }
    }
    if !black.is_empty() {
        meshes.push(black);
    }
    if !colored.is_empty() {
        meshes.push(colored);
    }
    meshes
}

#[cfg(all(test, feature = "appearance-mask-eraser"))]
fn screen_cubic_flatness(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2) -> f32 {
    fn distance_to_chord(point: Pos2, start: Pos2, end: Pos2) -> f32 {
        let segment = end - start;
        let len_sq = segment.length_sq();
        if len_sq <= 1.0e-12 {
            return point.distance(start);
        }
        let t = ((point - start).dot(segment) / len_sq).clamp(0.0, 1.0);
        point.distance(start + segment * t)
    }
    distance_to_chord(p1, p0, p3).max(distance_to_chord(p2, p0, p3))
}

#[cfg(all(test, feature = "appearance-mask-eraser"))]
fn flatten_cubic_screen_adaptive(
    p0: Pos2,
    p1: Pos2,
    p2: Pos2,
    p3: Pos2,
    tolerance: f32,
    depth: u8,
    out: &mut Vec<Pos2>,
) {
    if depth >= 10 || screen_cubic_flatness(p0, p1, p2, p3) <= tolerance {
        out.push(p3);
        return;
    }
    let p01 = p0.lerp(p1, 0.5);
    let p12 = p1.lerp(p2, 0.5);
    let p23 = p2.lerp(p3, 0.5);
    let p012 = p01.lerp(p12, 0.5);
    let p123 = p12.lerp(p23, 0.5);
    let mid = p012.lerp(p123, 0.5);
    flatten_cubic_screen_adaptive(p0, p01, p012, mid, tolerance, depth + 1, out);
    flatten_cubic_screen_adaptive(mid, p123, p23, p3, tolerance, depth + 1, out);
}

#[cfg(all(test, feature = "appearance-mask-eraser"))]
fn adaptive_selection_bezier_screen_path(
    path: &VPath,
    placement_transform: Affine,
    view: &StageView,
) -> Vec<Pos2> {
    let Some(first) = path.anchors.first() else {
        return Vec::new();
    };
    if path.anchors.len() == 1 {
        let point = placement_transform.apply(first.point);
        return vec![stage_pos_to_screen(pos2(point.x, point.y), view)];
    }
    let to_screen = |point: Vec2| {
        let point = placement_transform.apply(point);
        stage_pos_to_screen(pos2(point.x, point.y), view)
    };
    let mut out = Vec::with_capacity(path.anchors.len() * 2);
    out.push(to_screen(first.point));
    let segment_count = if path.closed {
        path.anchors.len()
    } else {
        path.anchors.len() - 1
    };
    // Quarter-pixel geometric error is below the selection stroke width while
    // still preserving every source anchor and every segment boundary.
    const TOLERANCE_PX: f32 = 0.25;
    for index in 0..segment_count {
        let a = &path.anchors[index];
        let b = &path.anchors[(index + 1) % path.anchors.len()];
        let p0 = to_screen(a.point);
        let p3 = to_screen(b.point);
        if a.out_handle.is_none() && b.in_handle.is_none() {
            out.push(p3);
            continue;
        }
        let p1 = to_screen(a.out_handle.unwrap_or(a.point));
        let p2 = to_screen(b.in_handle.unwrap_or(b.point));
        flatten_cubic_screen_adaptive(p0, p1, p2, p3, TOLERANCE_PX, 0, &mut out);
    }
    if path.closed && out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

#[cfg(all(test, feature = "appearance-mask-eraser"))]
pub(crate) fn adaptive_selection_bezier_screen_path_for_test(
    path: &VPath,
    placement_transform: Affine,
    view: &StageView,
) -> Vec<Pos2> {
    adaptive_selection_bezier_screen_path(path, placement_transform, view)
}

#[cfg(feature = "appearance-mask-eraser")]
fn build_selection_screen_meshes(
    source: &mut CachedSelectionStageMesh,
    placement_transform: Affine,
    view: &StageView,
    texture_id: TextureId,
    tile_size_points: f32,
    accent: Color32,
    use_parametric_for_translation: bool,
) -> Vec<Mesh> {
    let stage_to_screen_transformed = |point: Pos2| {
        let transformed = placement_transform.apply(Vec2::new(point.x, point.y));
        stage_pos_to_screen(pos2(transformed.x, transformed.y), view)
    };
    let mut meshes = Vec::with_capacity(1 + source.strokes.len() * 2);
    if !source.fill_vertices.is_empty() && !source.fill_indices.is_empty() {
        let tile = tile_size_points.max(1.0);
        let mut fill = Mesh::with_texture(texture_id);
        fill.vertices.reserve(source.fill_vertices.len());
        for stage in &source.fill_vertices {
            let pos = stage_to_screen_transformed(*stage);
            fill.vertices.push(Vertex {
                pos,
                uv: pos2(pos.x / tile, pos.y / tile),
                color: Color32::WHITE,
            });
        }
        fill.indices = source.fill_indices.clone();
        meshes.push(fill);
    }
    let pure_translation = (placement_transform.a11 - 1.0).abs() <= 1.0e-6
        && placement_transform.a12.abs() <= 1.0e-6
        && placement_transform.a21.abs() <= 1.0e-6
        && (placement_transform.a22 - 1.0).abs() <= 1.0e-6;
    let origin = view.stage_to_screen(Vec2::new(0.0, 0.0));
    let translation_px =
        view.stage_to_screen(Vec2::new(placement_transform.tx, placement_transform.ty)) - origin;
    let mut black_outline = Mesh::default();
    let mut accent_outline = Mesh::default();
    for stroke in &mut source.strokes {
        if pure_translation && use_parametric_for_translation {
            if let Some(mut mesh) = cached_selection_stroke_screen_mesh(
                stroke,
                2.0,
                Color32::from_black_alpha(180),
                view,
            ) {
                if translation_px != egui::Vec2::ZERO {
                    for vertex in &mut mesh.vertices {
                        vertex.pos += translation_px;
                    }
                }
                black_outline.append(mesh);
            }
            if let Some(mut mesh) = cached_selection_stroke_screen_mesh(stroke, 1.0, accent, view) {
                if translation_px != egui::Vec2::ZERO {
                    for vertex in &mut mesh.vertices {
                        vertex.pos += translation_px;
                    }
                }
                accent_outline.append(mesh);
            }
        } else {
            let screen: Vec<Pos2> = stroke
                .contour_stage
                .iter()
                .copied()
                .map(stage_to_screen_transformed)
                .collect();
            if let Some(mesh) =
                closed_bevel_stroke_mesh(&screen, 2.0, Color32::from_black_alpha(180))
            {
                black_outline.append(mesh);
            }
            if let Some(mesh) = closed_bevel_stroke_mesh(&screen, 1.0, accent) {
                accent_outline.append(mesh);
            }
        }
    }
    if !black_outline.is_empty() {
        meshes.push(black_outline);
    }
    if !accent_outline.is_empty() {
        meshes.push(accent_outline);
    }
    meshes
}

/// Paint one tessellated fill with a repeating screen-space texture. UVs are
/// derived from absolute screen coordinates, so the pattern density is stable
/// through smooth zoom and does not require one CPU shape per visual dot.
pub(crate) fn complex_fill_pattern_mesh(
    contours: &[Vec<Pos2>],
    texture_id: TextureId,
    tile_size_points: f32,
) -> Option<Mesh> {
    let buffers = tessellate_complex_fill(contours)?;
    let tile = tile_size_points.max(1.0);
    let mut mesh = Mesh::with_texture(texture_id);
    mesh.vertices.reserve(buffers.vertices.len());
    for point in buffers.vertices {
        let pos = Pos2::new(point.x, point.y);
        mesh.vertices.push(Vertex {
            pos,
            uv: Pos2::new(pos.x / tile, pos.y / tile),
            color: Color32::WHITE,
        });
    }
    mesh.indices = buffers.indices;
    Some(mesh)
}

pub fn paint_complex_fill_pattern(
    painter: &Painter,
    contours: &[Vec<Pos2>],
    texture_id: TextureId,
    tile_size_points: f32,
) {
    if let Some(mesh) = complex_fill_pattern_mesh(contours, texture_id, tile_size_points) {
        painter.add(Shape::Mesh(mesh));
    }
}

/// Live preview for the classic circular nib. The centreline is buffered
/// into one planar surface before tessellation, so translucent preview pixels
/// are blended once instead of once per overlapping stroke triangle.
pub fn paint_round_stroke_preview(painter: &Painter, points: &[Pos2], width: f32, color: Color32) {
    let Some(contours) = round_stroke_preview_contours(points, width) else {
        return;
    };
    paint_complex_fill(painter, &contours, color);
}

fn lyon_buffers_mesh(buffers: VertexBuffers<lyon_path::math::Point, u32>, color: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(buffers.vertices.len());
    for point in buffers.vertices {
        mesh.vertices.push(Vertex {
            pos: Pos2::new(point.x, point.y),
            uv: Pos2::ZERO,
            color,
        });
    }
    mesh.indices = buffers.indices;
    mesh
}

fn paint_lyon_buffers(
    painter: &Painter,
    buffers: VertexBuffers<lyon_path::math::Point, u32>,
    color: Color32,
) {
    painter.add(Shape::Mesh(lyon_buffers_mesh(buffers, color)));
}

fn round_stroke_preview_contours(points: &[Pos2], width: f32) -> Option<Vec<Vec<Pos2>>> {
    let radius = width.max(0.1) * 0.5;
    let unique = simplify_preview_polyline(&sanitize_preview_polyline(points, width), width);
    let first = *unique.first()?;
    if unique.len() == 1 {
        const SEGMENTS: usize = 48;
        return Some(vec![(0..SEGMENTS)
            .map(|index| {
                let angle = std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
                Pos2::new(
                    first.x + angle.cos() * radius,
                    first.y + angle.sin() * radius,
                )
            })
            .collect()]);
    }

    let line = LineString::new(
        unique
            .iter()
            .map(|point| Coord {
                x: f64::from(point.x),
                y: f64::from(point.y),
            })
            .collect(),
    );
    let coverage = line.buffer(f64::from(radius));
    let mut contours = Vec::new();
    for polygon in coverage.0 {
        let mut exterior: Vec<Pos2> = polygon
            .exterior()
            .0
            .iter()
            .map(|coord| Pos2::new(coord.x as f32, coord.y as f32))
            .collect();
        drop_duplicate_closing_point(&mut exterior);
        if exterior.len() >= 3 {
            contours.push(exterior);
        }
        for hole in polygon.interiors() {
            let mut interior: Vec<Pos2> = hole
                .0
                .iter()
                .map(|coord| Pos2::new(coord.x as f32, coord.y as f32))
                .collect();
            drop_duplicate_closing_point(&mut interior);
            if interior.len() >= 3 {
                contours.push(interior);
            }
        }
    }
    (!contours.is_empty()).then_some(contours)
}

#[cfg(test)]
fn tessellate_round_stroke(
    points: &[Pos2],
    width: f32,
) -> Option<VertexBuffers<lyon_path::math::Point, u32>> {
    let contours = round_stroke_preview_contours(points, width)?;
    tessellate_complex_fill(&contours)
}

fn drop_duplicate_closing_point(points: &mut Vec<Pos2>) {
    if points.len() > 1 && points[0].distance_sq(points[points.len() - 1]) <= 1.0e-8 {
        points.pop();
    }
}

pub fn sanitize_display_polyline(points: &[Pos2], width: f32, closed: bool) -> Vec<Pos2> {
    let mut cleaned = simplify_preview_polyline(&sanitize_preview_polyline(points, width), width);
    if closed {
        drop_duplicate_closing_point(&mut cleaned);
    }
    cleaned
}

fn sanitize_preview_polyline(points: &[Pos2], width: f32) -> Vec<Pos2> {
    // This is display-only subpixel cleanup, not fixed-step input sampling.
    // The committed gesture still receives every pointer event.
    let min_distance = (width.abs() * 0.001).clamp(0.05, 0.25);
    let min_distance_sq = min_distance * min_distance;
    let mut result = Vec::with_capacity(points.len());
    for point in points
        .iter()
        .copied()
        .filter(|point| point.x.is_finite() && point.y.is_finite())
    {
        if result
            .last()
            .is_some_and(|previous: &Pos2| previous.distance_sq(point) < min_distance_sq)
        {
            continue;
        }
        if result.len() >= 2 && result[result.len() - 2].distance_sq(point) < min_distance_sq {
            result.pop();
            continue;
        }
        result.push(point);
    }
    result
}

fn simplify_preview_polyline(points: &[Pos2], width: f32) -> Vec<Pos2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let tolerance = (width.abs() * 0.004).clamp(0.08, 0.35);
    let mut current = points.to_vec();
    for _ in 0..2 {
        if current.len() < 3 {
            break;
        }
        let mut simplified = Vec::with_capacity(current.len());
        simplified.push(current[0]);
        for index in 1..current.len() - 1 {
            let previous = *simplified.last().expect("preview start");
            let point = current[index];
            let next = current[index + 1];
            let forward = (point - previous).dot(next - point) >= 0.0;
            if forward && pos_segment_distance(point, previous, next) <= tolerance {
                continue;
            }
            simplified.push(point);
        }
        simplified.push(*current.last().expect("preview end"));
        current = simplified;
    }
    current
}

fn pos_segment_distance(point: Pos2, start: Pos2, end: Pos2) -> f32 {
    let segment = end - start;
    let length_sq = segment.length_sq();
    if length_sq <= 1.0e-12 {
        return point.distance(start);
    }
    let t = ((point - start).dot(segment) / length_sq).clamp(0.0, 1.0);
    point.distance(start + segment * t)
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
    if tessellator
        .tessellate_path(
            &path,
            &options,
            &mut BuffersBuilder::new(&mut buffers, Positions),
        )
        .is_err()
    {
        return None;
    }
    Some(buffers)
}

#[cfg(test)]
fn triangulate_polygon(points: &[Pos2]) -> Option<Vec<u32>> {
    const EPSILON: f32 = 1.0e-5;
    if points.len() < 3 {
        return None;
    }

    // Flattened Béziers contain many collinear samples, and a closed path can
    // repeat its first point at the end. Strip both forms of zero-area vertex
    // before ear clipping.
    let mut remaining: Vec<usize> = Vec::with_capacity(points.len());
    for (index, point) in points.iter().enumerate() {
        if remaining
            .last()
            .map(|last| points[*last].distance_sq(*point) > EPSILON)
            .unwrap_or(true)
        {
            remaining.push(index);
        }
    }
    if remaining.len() > 3
        && points[remaining[0]].distance_sq(points[*remaining.last()?]) <= EPSILON
    {
        remaining.pop();
    }
    while remaining.len() > 3 {
        let Some(index) = (0..remaining.len()).find(|index| {
            let prev = remaining[(*index + remaining.len() - 1) % remaining.len()];
            let current = remaining[*index];
            let next = remaining[(*index + 1) % remaining.len()];
            cross_2d(points[prev], points[current], points[next]).abs() <= EPSILON
        }) else {
            break;
        };
        remaining.remove(index);
    }
    if remaining.len() < 3 {
        return None;
    }

    let area = polygon_signed_area_indexed(points, &remaining);
    if area.abs() <= EPSILON {
        return None;
    }
    let ccw = area > 0.0;
    let mut triangles = Vec::with_capacity((remaining.len() - 2) * 3);
    let mut guard = remaining.len() * remaining.len();

    while remaining.len() > 3 && guard > 0 {
        guard -= 1;
        let mut clipped = false;
        for i in 0..remaining.len() {
            let prev = remaining[(i + remaining.len() - 1) % remaining.len()];
            let current = remaining[i];
            let next = remaining[(i + 1) % remaining.len()];
            let a = points[prev];
            let b = points[current];
            let c = points[next];
            let cross = cross_2d(a, b, c);
            if (ccw && cross <= EPSILON) || (!ccw && cross >= -EPSILON) {
                continue;
            }
            if remaining.iter().copied().any(|candidate| {
                candidate != prev
                    && candidate != current
                    && candidate != next
                    && point_in_triangle(points[candidate], a, b, c, ccw)
            }) {
                continue;
            }

            if ccw {
                triangles.extend_from_slice(&[prev as u32, current as u32, next as u32]);
            } else {
                triangles.extend_from_slice(&[next as u32, current as u32, prev as u32]);
            }
            remaining.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            return None;
        }
    }

    if remaining.len() == 3 {
        if ccw {
            triangles.extend_from_slice(&[
                remaining[0] as u32,
                remaining[1] as u32,
                remaining[2] as u32,
            ]);
        } else {
            triangles.extend_from_slice(&[
                remaining[2] as u32,
                remaining[1] as u32,
                remaining[0] as u32,
            ]);
        }
    }
    Some(triangles)
}

#[cfg(test)]
fn polygon_signed_area_indexed(points: &[Pos2], indices: &[usize]) -> f32 {
    let mut area = 0.0;
    for i in 0..indices.len() {
        let a = points[indices[i]];
        let b = points[indices[(i + 1) % indices.len()]];
        area += a.x * b.y - b.x * a.y;
    }
    area * 0.5
}

#[cfg(test)]
fn cross_2d(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

#[cfg(test)]
fn point_in_triangle(p: Pos2, a: Pos2, b: Pos2, c: Pos2, ccw: bool) -> bool {
    const EPSILON: f32 = 1.0e-5;
    let ab = cross_2d(a, b, p);
    let bc = cross_2d(b, c, p);
    let ca = cross_2d(c, a, p);
    if ccw {
        ab >= -EPSILON && bc >= -EPSILON && ca >= -EPSILON
    } else {
        ab <= EPSILON && bc <= EPSILON && ca <= EPSILON
    }
}

/// Per-channel × tint / 255. Result keeps tint==WHITE as identity (so the
/// non-onion render path is byte-identical to before).
fn modulate(c: Color32, tint: Color32) -> Color32 {
    if tint == Color32::WHITE {
        return c;
    }
    let r = (c.r() as u16 * tint.r() as u16 / 255) as u8;
    let g = (c.g() as u16 * tint.g() as u16 / 255) as u8;
    let b = (c.b() as u16 * tint.b() as u16 / 255) as u8;
    let a = (c.a() as u16 * tint.a() as u16 / 255) as u8;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Local-space AABB of a placement's *content* — i.e. the bounds of the
/// asset/q0rg before the placement's own transform is applied.  Used by the
/// transform tool to compute resize handles relative to the untransformed
/// shape.
pub fn target_local_bbox(
    project: &ProjectV2,
    target: Target,
    vector_override: Option<&VectorAsset>,
) -> Option<(f32, f32, f32, f32)> {
    match target {
        Target::Asset(id) => {
            if let Some(vector) = vector_override.filter(|vector| vector.asset_id == id) {
                let temporary = Asset::Vector(vector.clone());
                let pts = asset_local_visual_outline(project, id, &temporary);
                return aabb_of(&pts);
            }
            let asset = project.assets.iter().find(|a| a.id() == id)?;
            let pts = asset_local_visual_outline(project, id, asset);
            aabb_of(&pts)
        }
        Target::Q0rg(child_id) => q0rg_local_bbox(project, child_id, Q0RG_RECURSION_LIMIT),
    }
}

pub fn placement_local_bbox(
    project: &ProjectV2,
    placement: &Placement,
) -> Option<(f32, f32, f32, f32)> {
    target_local_bbox(project, placement.target, None)
}

fn aabb_of(pts: &[Vec2]) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for p in pts {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    if min_x.is_finite() {
        Some((min_x, min_y, max_x, max_y))
    } else {
        None
    }
}

/// Frame-aware local bounds of a q0rg. Unlike the legacy all-frame helper,
/// this resolves hold/tween state exactly as the renderer does for `frame`.
fn preview_bbox_with_placement_fx(
    bounds: (f32, f32, f32, f32),
    fx: PlacementFx,
    transform_scale: f32,
) -> (f32, f32, f32, f32) {
    let scale = transform_scale.abs().max(1.0e-4);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = bounds;
    let union = |base: &mut (f32, f32, f32, f32), other: (f32, f32, f32, f32)| {
        base.0 = base.0.min(other.0);
        base.1 = base.1.min(other.1);
        base.2 = base.2.max(other.2);
        base.3 = base.3.max(other.3);
    };
    let mut result = bounds;
    if let Some(blur) = fx.blur {
        let pad_x = blur.radius_x.max(0.0) * scale;
        let pad_y = blur.radius_y.max(0.0) * scale;
        union(
            &mut result,
            (min_x - pad_x, min_y - pad_y, max_x + pad_x, max_y + pad_y),
        );
    }
    if let Some(glow) = fx.glow {
        if glow.strength > 1.0e-6 && glow.color.a > 0 {
            let pad_x = glow.radius_x.max(0.0) * scale;
            let pad_y = glow.radius_y.max(0.0) * scale;
            union(
                &mut result,
                (min_x - pad_x, min_y - pad_y, max_x + pad_x, max_y + pad_y),
            );
        }
    }
    if let Some(shadow) = fx.shadow {
        if shadow.strength > 1.0e-6 && shadow.color.a > 0 {
            let pad = shadow.blur_radius.max(0.0) * scale;
            let dx = shadow.offset_x * scale;
            let dy = shadow.offset_y * scale;
            union(
                &mut result,
                (
                    min_x - pad + dx,
                    min_y - pad + dy,
                    max_x + pad + dx,
                    max_y + pad + dy,
                ),
            );
        }
    }
    (min_x, min_y, max_x, max_y) = result;
    (min_x, min_y, max_x, max_y)
}
pub fn q0rg_frame_bbox(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
) -> Option<(f32, f32, f32, f32)> {
    let q0rg = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for layer in &q0rg.layers {
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        for resolved in resolve_layer_at_frame(layer, local_frame) {
            let mut placement = resolved.placement.clone();
            placement.transform = resolved.interp;
            if let Some(bounds) = placement_bbox_at_depth(project, &placement, Q0RG_RECURSION_LIMIT)
            {
                // Library/scene preview fits the complete painted placement, not
                // just its vector body. This is preview-only and deliberately does
                // not change selection/transform bounds.
                let (x0, y0, x1, y1) = preview_bbox_with_placement_fx(
                    bounds,
                    resolved.fx,
                    Affine::from_transform(resolved.interp).uniform_scale(),
                );
                min_x = min_x.min(x0);
                min_y = min_y.min(y0);
                max_x = max_x.max(x1);
                max_y = max_y.max(y1);
            }
        }
    }
    min_x.is_finite().then_some((min_x, min_y, max_x, max_y))
}

/// Render one Library target with a temporary transform. Used for drag-and-drop
/// previews so the ghost is produced by the exact same vector/bitmap/q0rg
/// renderer as the committed placement.
#[allow(clippy::too_many_arguments)]
pub fn render_target_preview(
    painter: &Painter,
    project: &ProjectV2,
    target: Target,
    frame: u16,
    transform: Transform2D,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
) {
    render_target_preview_with_revision(
        painter, project, target, frame, transform, view, textures, ctx, tint, None,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_target_preview_with_revision(
    painter: &Painter,
    project: &ProjectV2,
    target: Target,
    frame: u16,
    transform: Transform2D,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
    revision: Option<(u64, u64)>,
) {
    let affine = Affine::from_transform(transform);
    // Library q0rg previews can contain filtered children. Keep the same cache
    // semantics as a top-level stage render now that recursive render_q0rg no
    // longer performs redundant per-frame synchronization itself. Plain Asset
    // previews have no placement FX tree and do not need this project-wide scan.
    if matches!(target, Target::Q0rg(_)) {
        sync_fx_project_snapshot(project, textures, revision);
    }
    let render_index = textures.render_index(project, revision);
    let lookup = RenderLookup::new(project, render_index, revision.is_none());
    match target {
        Target::Asset(asset_id) => {
            if let Some(asset) = lookup.asset(asset_id) {
                render_asset(
                    painter,
                    asset,
                    project.asset_appearances.get(&asset_id),
                    affine,
                    view,
                    textures,
                    ctx,
                    tint,
                    None,
                    frame,
                    project.meta.fps,
                );
            }
        }
        Target::Q0rg(q0rg_id) => render_q0rg(
            painter, project, &lookup, q0rg_id, frame, affine, view, textures, ctx, 0, tint, None,
        ),
    }
}

/// Place the local bounds centre of a target under a stage-space position.
/// Empty symbols fall back to using their origin directly.
pub fn centered_target_transform(
    project: &ProjectV2,
    target: Target,
    position: Vec2,
) -> Transform2D {
    let provisional = Placement {
        instance_id: 0,
        frame: 0,
        target,
        transform: Transform2D::IDENTITY,
        tween: Tween::None,
        fx: Default::default(),
    };
    placement_bbox(project, &provisional)
        .map(|bounds| Transform2D {
            tx: position.x - (bounds.0 + bounds.2) * 0.5,
            ty: position.y - (bounds.1 + bounds.3) * 0.5,
            ..Transform2D::IDENTITY
        })
        .unwrap_or(Transform2D {
            tx: position.x,
            ty: position.y,
            ..Transform2D::IDENTITY
        })
}

/// Render one asset at identity. Used by the Library preview so thumbnails
/// share the exact same bitmap/vector path as the stage.
pub fn render_asset_preview(
    painter: &Painter,
    project: &ProjectV2,
    asset_id: u16,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
) {
    render_asset_preview_with_revision(painter, project, asset_id, view, textures, ctx, None);
}

pub(crate) fn render_asset_preview_with_revision(
    painter: &Painter,
    project: &ProjectV2,
    asset_id: u16,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    revision: Option<(u64, u64)>,
) {
    let render_index = textures.render_index(project, revision);
    let lookup = RenderLookup::new(project, render_index, revision.is_none());
    let Some(asset) = lookup.asset(asset_id) else {
        return;
    };
    render_asset(
        painter,
        asset,
        project.asset_appearances.get(&asset_id),
        Affine::IDENTITY,
        view,
        textures,
        ctx,
        Color32::WHITE,
        None,
        0,
        project.meta.fps,
    );
}

/// World-space AABB of a placement, computed by sampling the placement's local
/// outline points and applying its transform.  Recurses into Q0rg targets up
/// to `Q0RG_RECURSION_LIMIT` levels (matches the renderer) so nested symbols
/// produce a non-empty bbox even when they don't directly host an asset.
pub fn placement_bbox(project: &ProjectV2, placement: &Placement) -> Option<(f32, f32, f32, f32)> {
    placement_bbox_at_depth(project, placement, Q0RG_RECURSION_LIMIT)
}

fn placement_bbox_at_depth(
    project: &ProjectV2,
    placement: &Placement,
    depth: u8,
) -> Option<(f32, f32, f32, f32)> {
    if depth == 0 {
        return None;
    }
    // Stroke half-width pads the bbox so the draggable rect actually covers the
    // painted pixels, not just the centerline. For Q0rg targets the padding is
    // already baked in by the recursive call, so we only add it here for Asset
    // placements. Approximate: scale-space conversion is skipped (cheap & good
    // enough at typical sx/sy near 1.0).
    let stroke_pad = match placement.target {
        Target::Asset(id) => project
            .assets
            .iter()
            .find(|a| a.id() == id)
            .map(asset_stroke_padding)
            .unwrap_or(0.0),
        Target::Q0rg(_) => 0.0,
    };
    let pts_local: Vec<Vec2> = match placement.target {
        Target::Asset(id) => {
            let asset = project.assets.iter().find(|a| a.id() == id)?;
            asset_local_visual_outline(project, id, asset)
        }
        Target::Q0rg(child_id) => {
            let (lmin_x, lmin_y, lmax_x, lmax_y) = q0rg_local_bbox(project, child_id, depth - 1)?;
            vec![
                Vec2::new(lmin_x, lmin_y),
                Vec2::new(lmax_x, lmin_y),
                Vec2::new(lmax_x, lmax_y),
                Vec2::new(lmin_x, lmax_y),
            ]
        }
    };
    if pts_local.is_empty() {
        return None;
    }
    let aff = Affine::from_transform(placement.transform);
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for p in &pts_local {
        let w = aff.apply(*p);
        min_x = min_x.min(w.x);
        min_y = min_y.min(w.y);
        max_x = max_x.max(w.x);
        max_y = max_y.max(w.y);
    }
    if stroke_pad > 0.0 {
        // Inflate by half the stroke width × placement's area scale so
        // the draggable rect tracks the rendered stroke under any
        // affine transform.
        let pad = stroke_pad * aff.uniform_scale();
        min_x -= pad;
        min_y -= pad;
        max_x += pad;
        max_y += pad;
    }
    Some((min_x, min_y, max_x, max_y))
}

fn asset_stroke_padding(asset: &Asset) -> f32 {
    match asset {
        Asset::Vector(v) => v.stroke.map(|s| s.width * 0.5).unwrap_or(0.0),
        Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => 0.0,
    }
}

/// Mask source artwork controls clipping, not the body of an enclosing symbol.
/// Direct selection on the mask's own timeline still uses ordinary layer rules.
/// Masked content contributes its full bounds regardless of the clipping shape.
pub(crate) fn symbol_content_layer_is_visible(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
) -> bool {
    project.layer_is_visible(q0rg_id, layer_id)
        && project
            .layer_metadata
            .get(&LayerKey { q0rg_id, layer_id })
            .is_none_or(|metadata| metadata.kind != q0s_format::v2::LayerKind::Mask)
}

fn q0rg_local_bbox(project: &ProjectV2, q0rg_id: u16, depth: u8) -> Option<(f32, f32, f32, f32)> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for layer in &q.layers {
        if !symbol_content_layer_is_visible(project, q0rg_id, layer.layer_id) {
            continue;
        }
        for placement in &layer.placements {
            if let Some(b) = placement_bbox_at_depth(project, placement, depth) {
                min_x = min_x.min(b.0);
                min_y = min_y.min(b.1);
                max_x = max_x.max(b.2);
                max_y = max_y.max(b.3);
            }
        }
    }
    if min_x.is_finite() {
        Some((min_x, min_y, max_x, max_y))
    } else {
        None
    }
}

/// Reusable project indexes and recursive bounds for drawing a large
/// multi-selection. Selection outlines often contain many instances of the
/// same nested symbol; resolving each one independently used to rescan the
/// whole symbol tree for every outline on every frame.
pub(crate) struct PlacementBoundsCache {
    q0rg_indices: HashMap<u16, usize>,
    asset_indices: HashMap<u16, usize>,
    layer_indices: HashMap<(u16, u16), (usize, usize)>,
    #[allow(clippy::type_complexity)]
    q0rg_local_bounds: HashMap<(u16, u8), Option<(f32, f32, f32, f32)>>,
    q0rg_static_body_bounds: HashMap<(u16, u8), bool>,
    asset_outlines: HashMap<u16, Vec<Vec2>>,
}

impl PlacementBoundsCache {
    pub(crate) fn new(project: &ProjectV2) -> Self {
        let mut q0rg_indices = HashMap::with_capacity(project.q0rgs.len());
        let mut asset_indices = HashMap::with_capacity(project.assets.len());
        let layer_indices = HashMap::new();
        for (q0rg_index, q0rg) in project.q0rgs.iter().enumerate() {
            q0rg_indices.entry(q0rg.q0rg_id).or_insert(q0rg_index);
        }
        for (asset_index, asset) in project.assets.iter().enumerate() {
            asset_indices.entry(asset.id()).or_insert(asset_index);
        }
        Self {
            q0rg_indices,
            asset_indices,
            layer_indices,
            q0rg_local_bounds: HashMap::new(),
            q0rg_static_body_bounds: HashMap::new(),
            asset_outlines: HashMap::new(),
        }
    }

    fn invalidate_placement_transforms(&mut self) {
        self.q0rg_local_bounds.clear();
        self.q0rg_static_body_bounds.clear();
    }

    fn invalidate_asset(&mut self, asset_id: u16) {
        self.asset_outlines.remove(&asset_id);
        self.invalidate_placement_transforms();
    }

    pub(crate) fn layer<'a>(
        &mut self,
        project: &'a ProjectV2,
        q0rg_id: u16,
        layer_id: u16,
    ) -> Option<&'a q0s_format::v2::Layer> {
        let key = (q0rg_id, layer_id);
        if !self.layer_indices.contains_key(&key) {
            let q0rg_index = *self.q0rg_indices.get(&q0rg_id)?;
            let layer_index = project
                .q0rgs
                .get(q0rg_index)?
                .layers
                .iter()
                .position(|layer| layer.layer_id == layer_id)?;
            self.layer_indices.insert(key, (q0rg_index, layer_index));
        }
        let (q0rg_index, layer_index) = *self.layer_indices.get(&key)?;
        project.q0rgs.get(q0rg_index)?.layers.get(layer_index)
    }

    pub(crate) fn q0rg<'a>(&self, project: &'a ProjectV2, q0rg_id: u16) -> Option<&'a Q0rg> {
        project.q0rgs.get(*self.q0rg_indices.get(&q0rg_id)?)
    }

    pub(crate) fn asset<'a>(&self, project: &'a ProjectV2, asset_id: u16) -> Option<&'a Asset> {
        project.assets.get(*self.asset_indices.get(&asset_id)?)
    }

    pub(crate) fn target_local_bbox(
        &mut self,
        project: &ProjectV2,
        target: Target,
    ) -> Option<(f32, f32, f32, f32)> {
        match target {
            Target::Asset(asset_id) => {
                if !self.asset_outlines.contains_key(&asset_id) {
                    let asset = self.asset(project, asset_id)?;
                    self.asset_outlines.insert(
                        asset_id,
                        asset_local_visual_outline(project, asset_id, asset),
                    );
                }
                aabb_of(self.asset_outlines.get(&asset_id)?)
            }
            Target::Q0rg(q0rg_id) => self.q0rg_local_bbox(project, q0rg_id, Q0RG_RECURSION_LIMIT),
        }
    }

    pub(crate) fn q0rg_body_bounds_are_static(
        &mut self,
        project: &ProjectV2,
        q0rg_id: u16,
    ) -> bool {
        self.q0rg_body_bounds_are_static_at_depth(project, q0rg_id, Q0RG_RECURSION_LIMIT)
    }

    pub(crate) fn placement_bbox(
        &mut self,
        project: &ProjectV2,
        placement: &Placement,
    ) -> Option<(f32, f32, f32, f32)> {
        self.placement_bbox_at_depth(project, placement, Q0RG_RECURSION_LIMIT)
    }

    fn placement_bbox_at_depth(
        &mut self,
        project: &ProjectV2,
        placement: &Placement,
        depth: u8,
    ) -> Option<(f32, f32, f32, f32)> {
        if depth == 0 {
            return None;
        }
        let stroke_pad = match placement.target {
            Target::Asset(asset_id) => asset_stroke_padding(self.asset(project, asset_id)?),
            Target::Q0rg(_) => 0.0,
        };
        let affine = Affine::from_transform(placement.transform);
        let bounds = match placement.target {
            Target::Asset(asset_id) => {
                if !self.asset_outlines.contains_key(&asset_id) {
                    let asset = self.asset(project, asset_id)?;
                    self.asset_outlines.insert(
                        asset_id,
                        asset_local_visual_outline(project, asset_id, asset),
                    );
                }
                transformed_outline_bounds(self.asset_outlines.get(&asset_id)?, affine)?
            }
            Target::Q0rg(child_id) => {
                let local_bounds = self.q0rg_local_bbox(project, child_id, depth - 1)?;
                transformed_rect_bounds(local_bounds, affine)?
            }
        };
        if stroke_pad <= 0.0 {
            return Some(bounds);
        }
        let pad = stroke_pad * affine.uniform_scale();
        Some((
            bounds.0 - pad,
            bounds.1 - pad,
            bounds.2 + pad,
            bounds.3 + pad,
        ))
    }

    fn q0rg_local_bbox(
        &mut self,
        project: &ProjectV2,
        q0rg_id: u16,
        depth: u8,
    ) -> Option<(f32, f32, f32, f32)> {
        if depth == 0 {
            return None;
        }
        if let Some(cached) = self.q0rg_local_bounds.get(&(q0rg_id, depth)) {
            return *cached;
        }
        let q0rg_index = *self.q0rg_indices.get(&q0rg_id)?;
        let q0rg = project.q0rgs.get(q0rg_index)?;
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for layer in &q0rg.layers {
            if !symbol_content_layer_is_visible(project, q0rg_id, layer.layer_id) {
                continue;
            }
            for placement in &layer.placements {
                if let Some(bounds) = self.placement_bbox_at_depth(project, placement, depth) {
                    min_x = min_x.min(bounds.0);
                    min_y = min_y.min(bounds.1);
                    max_x = max_x.max(bounds.2);
                    max_y = max_y.max(bounds.3);
                }
            }
        }
        let bounds = min_x.is_finite().then_some((min_x, min_y, max_x, max_y));
        self.q0rg_local_bounds.insert((q0rg_id, depth), bounds);
        bounds
    }

    fn q0rg_body_bounds_are_static_at_depth(
        &mut self,
        project: &ProjectV2,
        q0rg_id: u16,
        depth: u8,
    ) -> bool {
        if depth == 0 {
            return false;
        }
        if let Some(cached) = self.q0rg_static_body_bounds.get(&(q0rg_id, depth)) {
            return *cached;
        }
        if q0s_format::rig::rig_for_q0rg(project, q0rg_id).is_some() {
            self.q0rg_static_body_bounds.insert((q0rg_id, depth), false);
            return false;
        }
        let Some(q0rg) = self.q0rg(project, q0rg_id) else {
            return false;
        };
        let mut children = Vec::new();
        let mut is_static = true;
        for layer in &q0rg.layers {
            if !symbol_content_layer_is_visible(project, q0rg_id, layer.layer_id) {
                continue;
            }
            for placement in &layer.placements {
                if !matches!(placement.tween, Tween::None) {
                    is_static = false;
                    break;
                }
                if let Target::Q0rg(child_id) = placement.target {
                    children.push(child_id);
                }
            }
            if !is_static {
                break;
            }
        }
        if is_static {
            for child_id in children {
                if !self.q0rg_body_bounds_are_static_at_depth(project, child_id, depth - 1) {
                    is_static = false;
                    break;
                }
            }
        }
        self.q0rg_static_body_bounds
            .insert((q0rg_id, depth), is_static);
        is_static
    }
}

fn transformed_outline_bounds(outline: &[Vec2], affine: Affine) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for point in outline {
        let world = affine.apply(*point);
        if !world.x.is_finite() || !world.y.is_finite() {
            continue;
        }
        min_x = min_x.min(world.x);
        min_y = min_y.min(world.y);
        max_x = max_x.max(world.x);
        max_y = max_y.max(world.y);
    }
    min_x.is_finite().then_some((min_x, min_y, max_x, max_y))
}

fn transformed_rect_bounds(
    bounds: (f32, f32, f32, f32),
    affine: Affine,
) -> Option<(f32, f32, f32, f32)> {
    let (min_x, min_y, max_x, max_y) = bounds;
    transformed_outline_bounds(
        &[
            Vec2::new(min_x, min_y),
            Vec2::new(max_x, min_y),
            Vec2::new(max_x, max_y),
            Vec2::new(min_x, max_y),
        ],
        affine,
    )
}

fn asset_local_visual_outline(project: &ProjectV2, asset_id: u16, asset: &Asset) -> Vec<Vec2> {
    #[cfg(feature = "appearance-mask-eraser")]
    if project.asset_appearances.contains_key(&asset_id) {
        return crate::appearance::asset_visible_material_bounds_fast(project, asset_id)
            .map(|(min_x, min_y, max_x, max_y)| {
                vec![
                    Vec2::new(min_x, min_y),
                    Vec2::new(max_x, min_y),
                    Vec2::new(max_x, max_y),
                    Vec2::new(min_x, max_y),
                ]
            })
            .unwrap_or_default();
    }
    #[cfg(not(feature = "appearance-mask-eraser"))]
    let _ = (project, asset_id);
    asset_local_outline(asset)
}

fn asset_local_outline(asset: &Asset) -> Vec<Vec2> {
    match asset {
        Asset::Bitmap(b) => vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(f32::from(b.width), 0.0),
            Vec2::new(f32::from(b.width), f32::from(b.height)),
            Vec2::new(0.0, f32::from(b.height)),
        ],
        Asset::Vector(v) => v.paths.iter().flat_map(flatten_path).collect(),
        Asset::Q0v(v) => q0video::q0v::probe_header(&v.bytes)
            .ok()
            .map(|media| {
                if !media.spec.video {
                    return Vec::new();
                }
                let width = media.spec.width as f32;
                let height = media.spec.height as f32;
                vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(width, 0.0),
                    Vec2::new(width, height),
                    Vec2::new(0.0, height),
                ]
            })
            .unwrap_or_default(),
        Asset::Rig(_) => Vec::new(),
    }
}

pub fn flatten_path_for_stroke(path: &VPath) -> Vec<Vec2> {
    let mut points = flatten_path(path);
    if path.closed
        && points.len() > 1
        && points
            .first()
            .zip(points.last())
            .is_some_and(|(first, last)| {
                let dx = first.x - last.x;
                let dy = first.y - last.y;
                dx * dx + dy * dy <= 1.0e-10
            })
    {
        points.pop();
    }
    points
}

pub fn flatten_path(path: &VPath) -> Vec<Vec2> {
    if path.anchors.is_empty() {
        return Vec::new();
    }
    if path.anchors.len() == 1 {
        return vec![path.anchors[0].point];
    }
    let mut out: Vec<Vec2> = Vec::new();
    out.push(path.anchors[0].point);
    let segment_count = if path.closed {
        path.anchors.len()
    } else {
        path.anchors.len() - 1
    };
    for i in 0..segment_count {
        let a = &path.anchors[i];
        let b = &path.anchors[(i + 1) % path.anchors.len()];
        sample_segment(a, b, &mut out);
    }
    out
}

fn sample_segment(a: &Anchor, b: &Anchor, out: &mut Vec<Vec2>) {
    let p0 = a.point;
    let p1 = a.out_handle.unwrap_or(a.point);
    let p2 = b.in_handle.unwrap_or(b.point);
    let p3 = b.point;
    let straight = a.out_handle.is_none() && b.in_handle.is_none();
    if straight {
        out.push(p3);
        return;
    }
    for i in 1..=BEZIER_SAMPLES_PER_SEGMENT {
        let t = i as f32 / BEZIER_SAMPLES_PER_SEGMENT as f32;
        out.push(cubic_bezier(p0, p1, p2, p3, t));
    }
}

fn cubic_bezier(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f32) -> Vec2 {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    Vec2::new(
        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    )
}

/// World-space coordinate of a local-space point under `t`. Kept as a
/// pub helper because `tools.rs` still uses it for hit-tests / bbox
/// math; the renderer itself has moved to `Affine` composition.
pub fn apply(t: Transform2D, p: Vec2) -> Vec2 {
    Affine::from_transform(t).apply(p)
}

fn stage_to_screen(p: Vec2, view: &StageView) -> Pos2 {
    view.stage_to_screen(p)
}

#[cfg(test)]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
fn lerp_transform(a: Transform2D, b: Transform2D, t: f32) -> Transform2D {
    Transform2D {
        tx: lerp(a.tx, b.tx, t),
        ty: lerp(a.ty, b.ty, t),
        sx: lerp(a.sx, b.sx, t),
        sy: lerp(a.sy, b.sy, t),
        rotation: lerp(a.rotation, b.rotation, t),
        skew_x: lerp(a.skew_x, b.skew_x, t),
        skew_y: lerp(a.skew_y, b.skew_y, t),
    }
}

#[cfg(feature = "appearance-mask-eraser")]
pub(crate) fn appearance_cache_signature(
    vector: &q0s_format::v2::VectorAsset,
    appearance: &VectorAppearance,
) -> (u64, Vec2) {
    let material_paths: &[q0s_format::v2::Path] = if appearance.material_source.is_empty() {
        &vector.paths
    } else {
        &appearance.material_source
    };
    let mut origin = Vec2::new(f32::INFINITY, f32::INFINITY);
    for path in material_paths
        .iter()
        .chain(&appearance.erase_mask)
        .chain(&appearance.clip_mask)
    {
        for anchor in &path.anchors {
            for point in std::iter::once(anchor.point)
                .chain(anchor.in_handle)
                .chain(anchor.out_handle)
            {
                if point.x.is_finite() && point.y.is_finite() {
                    origin.x = origin.x.min(point.x);
                    origin.y = origin.y.min(point.y);
                }
            }
        }
    }
    if !origin.x.is_finite() || !origin.y.is_finite() {
        origin = Vec2::new(0.0, 0.0);
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Some(fill) = vector.fill {
        [fill.r, fill.g, fill.b, fill.a].hash(&mut hasher);
    }
    match appearance.material {
        q0s_format::v2::VectorMaterial::Solid => 0u8.hash(&mut hasher),
        q0s_format::v2::VectorMaterial::SoftHalo { radius, opacity } => {
            1u8.hash(&mut hasher);
            radius.to_bits().hash(&mut hasher);
            opacity.to_bits().hash(&mut hasher);
        }
    }
    let mut hash_paths = |tag: u8, paths: &[q0s_format::v2::Path]| {
        tag.hash(&mut hasher);
        paths.len().hash(&mut hasher);
        for path in paths {
            path.closed.hash(&mut hasher);
            path.anchors.len().hash(&mut hasher);
            for anchor in &path.anchors {
                let hash_point =
                    |point: Vec2, hasher: &mut std::collections::hash_map::DefaultHasher| {
                        (point.x - origin.x).to_bits().hash(hasher);
                        (point.y - origin.y).to_bits().hash(hasher);
                    };
                hash_point(anchor.point, &mut hasher);
                match anchor.in_handle {
                    Some(point) => {
                        1u8.hash(&mut hasher);
                        hash_point(point, &mut hasher);
                    }
                    None => 0u8.hash(&mut hasher),
                }
                match anchor.out_handle {
                    Some(point) => {
                        1u8.hash(&mut hasher);
                        hash_point(point, &mut hasher);
                    }
                    None => 0u8.hash(&mut hasher),
                }
            }
        }
    };
    hash_paths(1, material_paths);
    hash_paths(2, &appearance.erase_mask);
    hash_paths(3, &appearance.clip_mask);
    (hasher.finish(), origin)
}

#[cfg(feature = "appearance-mask-eraser")]
fn cache_appearance_raster_tile(
    textures: &mut TextureCache,
    ctx: &Context,
    key: (u16, u16, u64),
    origin: Vec2,
    tile: q0s_format::raster::RasterizedVectorAppearance,
) {
    let image =
        ColorImage::from_rgba_unmultiplied([tile.width as usize, tile.height as usize], &tile.rgba);
    let texture = ctx.load_texture(
        format!("q0s_appearance_{}_{}_{}", key.0, key.1, key.2),
        image,
        TextureOptions::LINEAR,
    );
    textures.appearance_by_asset.insert(
        key,
        CachedAppearanceTexture {
            texture,
            local_min_offset: Vec2::new(tile.local_min.x - origin.x, tile.local_min.y - origin.y),
            width: tile.width,
            height: tile.height,
            pixels_per_unit: tile.pixels_per_unit,
        },
    );
}

fn build_dynamic_vector_body_geometry(
    vector: &VectorAsset,
    appearance: Option<&VectorAppearance>,
) -> CachedVectorRenderGeometry {
    #[cfg(feature = "appearance-mask-eraser")]
    if let Some(appearance) = appearance {
        if !appearance.erase_mask.is_empty() || !appearance.clip_mask.is_empty() {
            let visible =
                crate::appearance::visible_source_surface_for_vector(vector, Some(appearance));
            let local_paths = crate::brush::coverage_to_linear_paths(&visible);
            let contours = local_paths
                .iter()
                .filter(|path| path.closed)
                .map(flatten_path)
                .collect::<Vec<_>>();
            return build_cached_fill_geometry(&contours);
        }
    }
    let _ = appearance;
    build_cached_vector_render_geometry(vector)
}

#[cfg(feature = "appearance-mask-eraser")]
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "appearance-mask-eraser")]
fn paint_dynamic_vector_appearance_halo_color(
    painter: &Painter,
    vector: &VectorAsset,
    appearance: &VectorAppearance,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
    color_transform: q0glblend::ColorTransform,
    instance_id: u32,
) {
    let q0s_format::v2::VectorMaterial::SoftHalo { .. } = appearance.material else {
        return;
    };
    let Some(tile) = q0s_format::raster::rasterize_vector_halo_local(vector, appearance, 2.0)
    else {
        textures.dynamic_appearance_by_instance.remove(&instance_id);
        return;
    };
    let image =
        ColorImage::from_rgba_unmultiplied([tile.width as usize, tile.height as usize], &tile.rgba);
    if let Some(cached) = textures
        .dynamic_appearance_by_instance
        .get_mut(&instance_id)
    {
        cached.texture.set(image, TextureOptions::LINEAR);
        cached.local_min_offset = tile.local_min;
        cached.width = tile.width;
        cached.height = tile.height;
        cached.pixels_per_unit = tile.pixels_per_unit;
    } else {
        let texture = ctx.load_texture(
            format!("q0s_dynamic_appearance_{instance_id}"),
            image,
            TextureOptions::LINEAR,
        );
        textures.dynamic_appearance_by_instance.insert(
            instance_id,
            CachedAppearanceTexture {
                texture,
                local_min_offset: tile.local_min,
                width: tile.width,
                height: tile.height,
                pixels_per_unit: tile.pixels_per_unit,
            },
        );
    }
    let Some(cached) = textures.dynamic_appearance_by_instance.get(&instance_id) else {
        return;
    };
    let local_min = cached.local_min_offset;
    let local_max = Vec2::new(
        local_min.x + cached.width as f32 / cached.pixels_per_unit,
        local_min.y + cached.height as f32 / cached.pixels_per_unit,
    );
    let field_transform = Affine::compose(transform, appearance.field_transform);
    let local_corners = [
        local_min,
        Vec2::new(local_max.x, local_min.y),
        local_max,
        Vec2::new(local_min.x, local_max.y),
    ];
    let screen_corners: Vec<Pos2> = local_corners
        .iter()
        .map(|point| stage_to_screen(field_transform.apply(*point), view))
        .collect();
    let mut mesh = Mesh::with_texture(cached.texture.id());
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
            color: tint,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    q0glblend::paint_mesh_color(painter, mesh, color_transform);
}

#[cfg(feature = "appearance-mask-eraser")]
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn paint_vector_appearance_halo(
    painter: &Painter,
    vector: &VectorAsset,
    appearance: &VectorAppearance,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
) {
    paint_vector_appearance_halo_color(
        painter,
        vector,
        appearance,
        transform,
        view,
        textures,
        ctx,
        tint,
        q0glblend::ColorTransform::default(),
    );
}

#[cfg(feature = "appearance-mask-eraser")]
#[allow(clippy::too_many_arguments)]
fn paint_vector_appearance_halo_color(
    painter: &Painter,
    vector: &q0s_format::v2::VectorAsset,
    appearance: &VectorAppearance,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
    color_transform: q0glblend::ColorTransform,
) {
    let q0s_format::v2::VectorMaterial::SoftHalo { .. } = appearance.material else {
        return;
    };
    if vector.fill.is_none() || vector.stroke.is_some() {
        return;
    }
    let field_transform = Affine::compose(transform, appearance.field_transform);
    // Raster resolution belongs to the canonical material field, not to the
    // editor camera or to an affine placement. The fingerprint deliberately
    // ignores `field_transform`, so move/scale/rotate must reuse this texture too.
    // The real vector body stays resolution-independent; only the soft halo is a
    // linearly filtered GPU texture. Two samples per stage unit are enough for
    // this deliberately blurred layer and, crucially, never create zoom buckets.
    const MATERIAL_CACHE_PPU: f32 = 2.0;
    const MATERIAL_CACHE_BUCKET: u16 = 8;
    let bucket = MATERIAL_CACHE_BUCKET;
    let ppu = MATERIAL_CACHE_PPU;
    // The raster signature depends on frozen material/mask content, not on the
    // field affine. Computing it walks every anchor, so do it only after an
    // explicit texture-cache invalidation rather than on every repaint/drag tick.
    let (fingerprint, origin) = textures.appearance_signature(vector, appearance);
    let key = (vector.asset_id, bucket, fingerprint);
    if !textures.appearance_by_asset.contains_key(&key) {
        textures
            .appearance_by_asset
            .retain(|(asset_id, cached_bucket, cached_fingerprint), _| {
                *asset_id != vector.asset_id
                    || *cached_bucket != bucket
                    || *cached_fingerprint == fingerprint
            });
        textures.appearance_raster_jobs.retain(
            |(asset_id, cached_bucket, cached_fingerprint), _| {
                *asset_id != vector.asset_id
                    || *cached_bucket != bucket
                    || *cached_fingerprint == fingerprint
            },
        );

        match textures
            .appearance_raster_jobs
            .get(&key)
            .map(Receiver::try_recv)
        {
            Some(Ok(tile)) => {
                textures.appearance_raster_jobs.remove(&key);
                let Some(tile) = tile else {
                    return;
                };
                cache_appearance_raster_tile(textures, ctx, key, origin, tile);
            }
            Some(Err(TryRecvError::Empty)) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(8));
                return;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                textures.appearance_raster_jobs.remove(&key);
            }
            None => {}
        }

        if !textures.appearance_by_asset.contains_key(&key)
            && !textures.appearance_raster_jobs.contains_key(&key)
        {
            let (reply, receiver) = mpsc::channel();
            let request = AppearanceRasterRequest {
                vector: vector.clone(),
                appearance: appearance.clone(),
                pixels_per_unit: ppu,
                reply,
            };
            match appearance_raster_worker().send(request) {
                Ok(()) => {
                    textures.appearance_raster_jobs.insert(key, receiver);
                    ctx.request_repaint_after(std::time::Duration::from_millis(8));
                    return;
                }
                Err(error) => {
                    let request = error.0;
                    let Some(tile) = q0s_format::raster::rasterize_vector_halo_local(
                        &request.vector,
                        &request.appearance,
                        request.pixels_per_unit,
                    ) else {
                        return;
                    };
                    cache_appearance_raster_tile(textures, ctx, key, origin, tile);
                }
            }
        }
    }
    let Some(cached) = textures.appearance_by_asset.get(&key) else {
        return;
    };
    let local_min = Vec2::new(
        origin.x + cached.local_min_offset.x,
        origin.y + cached.local_min_offset.y,
    );
    let local_max = Vec2::new(
        local_min.x + cached.width as f32 / cached.pixels_per_unit,
        local_min.y + cached.height as f32 / cached.pixels_per_unit,
    );
    let local_corners = [
        local_min,
        Vec2::new(local_max.x, local_min.y),
        local_max,
        Vec2::new(local_min.x, local_max.y),
    ];
    let screen_corners: Vec<Pos2> = local_corners
        .iter()
        .map(|point| stage_to_screen(field_transform.apply(*point), view))
        .collect();
    let mut mesh = Mesh::with_texture(cached.texture.id());
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
            color: tint,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    q0glblend::paint_mesh_color(painter, mesh, color_transform);
}

#[cfg(feature = "appearance-mask-eraser")]
fn paint_masked_vector_body_cached(
    painter: &Painter,
    vector: &VectorAsset,
    appearance: &VectorAppearance,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    color: Color32,
) {
    if !textures
        .visible_body_mesh_by_asset
        .contains_key(&vector.asset_id)
    {
        let fallback_contours =
            masked_vector_body_contours(vector, appearance, transform, view, textures);
        let Some(local_contours) = textures.visible_body_by_asset.get(&vector.asset_id) else {
            paint_complex_fill(painter, &fallback_contours, color);
            return;
        };
        let geometry = build_cached_fill_geometry(local_contours);
        textures
            .visible_body_mesh_by_asset
            .insert(vector.asset_id, geometry);
    }
    let body_transform = Affine::compose(transform, appearance.field_transform);
    if let Some(geometry) = textures.visible_body_mesh_by_asset.get(&vector.asset_id) {
        paint_cached_vector_fill(painter, geometry, body_transform, view, color);
    }
}

#[cfg(feature = "appearance-mask-eraser")]
fn masked_vector_body_contours(
    vector: &q0s_format::v2::VectorAsset,
    appearance: &VectorAppearance,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
) -> Vec<Vec<Pos2>> {
    if appearance.erase_mask.is_empty() && appearance.clip_mask.is_empty() {
        // Preserve the exact normal vector-render path until there is an actual
        // post-material/erase clip. Merely enabling Glow must never polygonize the artwork.
        return vector
            .paths
            .iter()
            .filter(|path| path.closed)
            .map(|path| {
                flatten_path(path)
                    .iter()
                    .map(|point| stage_to_screen(transform.apply(*point), view))
                    .collect()
            })
            .collect();
    }

    // Fragment clips/erase masks used to run vector -> geo booleans -> paths ->
    // flattening on every repaint. A real Advanced smoke project spent >1 s in
    // this warm-frame path even with all glow textures already cached. Resolve
    // the body once in canonical field-space. Affine drag/scale/rotate then only
    // changes `field_transform`, so the same cached contours remain valid.
    let mut built_body_cache = false;
    if let std::collections::hash_map::Entry::Vacant(entry) =
        textures.visible_body_by_asset.entry(vector.asset_id)
    {
        let Some(visible) =
            crate::appearance::canonical_visible_source_surface_for_vector(vector, appearance)
        else {
            let visible =
                crate::appearance::visible_source_surface_for_vector(vector, Some(appearance));
            return crate::brush::coverage_to_linear_paths(&visible)
                .iter()
                .map(|path| {
                    flatten_path(path)
                        .iter()
                        .map(|point| stage_to_screen(transform.apply(*point), view))
                        .collect()
                })
                .collect();
        };
        // `visible` is already the authoritative boolean boundary. Re-fitting it
        // to Beziers and immediately flattening those Beziers again made first
        // render cost explode on dense fragmented artwork and could only move the
        // boundary away from the exact mask result. Cache the linear boundary
        // directly for both fill tessellation and interaction geometry.
        let local_contours: Vec<Vec<Vec2>> = crate::brush::coverage_to_linear_paths(&visible)
            .iter()
            .map(flatten_path)
            .collect();
        entry.insert(local_contours.clone());
        textures
            .visible_selection_body_by_asset
            .insert(vector.asset_id, local_contours);
        built_body_cache = true;
    }
    #[cfg(all(test, feature = "appearance-mask-eraser"))]
    if built_body_cache {
        textures.visible_body_build_count += 1;
    }
    #[cfg(not(all(test, feature = "appearance-mask-eraser")))]
    let _ = built_body_cache;

    let body_transform = Affine::compose(transform, appearance.field_transform);
    textures
        .visible_body_by_asset
        .get(&vector.asset_id)
        .expect("visible body inserted above")
        .iter()
        .map(|contour| {
            contour
                .iter()
                .map(|point| stage_to_screen(body_transform.apply(*point), view))
                .collect()
        })
        .collect()
}

fn rgba_to_color32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{Anchor, Layer, Path as VPath};

    #[test]
    fn radial_gradient_keeps_original_vector_mesh_for_analytic_gpu_sampling() {
        let vector = VectorAsset {
            asset_id: 4242,
            paths: vec![VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(-20.0, -20.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(20.0, -20.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(20.0, 20.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(-20.0, 20.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
            stroke: None,
        };
        let gradient = q0s_format::v2::GradientFill {
            kind: GradientKind::Radial,
            stops: vec![
                q0s_format::v2::GradientStop {
                    position: 0.0,
                    color: Rgba {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 180,
                    },
                },
                q0s_format::v2::GradientStop {
                    position: 1.0,
                    color: Rgba {
                        r: 255,
                        g: 240,
                        b: 140,
                        a: 0,
                    },
                },
            ],
            transform: Affine {
                a11: 20.0,
                a12: 0.0,
                a21: 0.0,
                a22: 20.0,
                tx: 3.0,
                ty: -4.0,
            },
        };
        let appearance = VectorAppearance {
            material: q0s_format::v2::VectorMaterial::Solid,
            erase_mask: Vec::new(),
            material_source: Vec::new(),
            clip_mask: Vec::new(),
            field_transform: Affine::IDENTITY,
            fill_gradient: Some(gradient.clone()),
        };
        let cached = build_cached_gradient_render_geometry(
            &vector,
            &appearance,
            gradient_render_signature(&appearance),
        );
        let mesh = cached.mesh.as_ref().expect("gradient vector mesh");
        assert_eq!(
            mesh.indices.len(),
            6,
            "quad must stay two triangles; radial color is evaluated in the fragment shader"
        );
        assert_eq!(
            mesh.vertices.len(),
            4,
            "analytic gradient must not recursively subdivide vector geometry"
        );
        assert_eq!(cached.stops.as_slice(), gradient.stops.as_slice());
        assert_eq!(cached.kind, GradientKind::Radial);
        let inverse = cached.inverse.expect("invertible gradient field");
        let center = inverse.apply(Vec2::new(3.0, -4.0));
        assert!(center.x.abs() < 1.0e-5 && center.y.abs() < 1.0e-5);
    }

    #[test]
    fn gradient_geometry_is_built_once_and_reused_across_repaints() {
        let vector = VectorAsset {
            asset_id: 4242,
            paths: vec![VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(-20.0, -20.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(20.0, -20.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(20.0, 20.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(-20.0, 20.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
            stroke: None,
        };
        let appearance = VectorAppearance {
            material: q0s_format::v2::VectorMaterial::Solid,
            erase_mask: Vec::new(),
            material_source: Vec::new(),
            clip_mask: Vec::new(),
            field_transform: Affine::IDENTITY,
            fill_gradient: Some(q0s_format::v2::GradientFill {
                kind: GradientKind::Radial,
                stops: vec![
                    q0s_format::v2::GradientStop {
                        position: 0.0,
                        color: Rgba {
                            r: 255,
                            g: 255,
                            b: 255,
                            a: 180,
                        },
                    },
                    q0s_format::v2::GradientStop {
                        position: 1.0,
                        color: Rgba {
                            r: 255,
                            g: 240,
                            b: 140,
                            a: 0,
                        },
                    },
                ],
                transform: Affine {
                    a11: 20.0,
                    a12: 0.0,
                    a21: 0.0,
                    a22: 20.0,
                    tx: 0.0,
                    ty: 0.0,
                },
            }),
        };
        let mut cache = TextureCache::default();
        let first = {
            let geometry = cache.gradient_render_geometry(&vector, &appearance);
            Arc::clone(geometry.mesh.as_ref().expect("gradient mesh"))
        };
        assert_eq!(cache.gradient_render_geometry_build_count(), 1);
        let second = {
            let geometry = cache.gradient_render_geometry(&vector, &appearance);
            Arc::clone(geometry.mesh.as_ref().expect("gradient mesh"))
        };
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            cache.gradient_render_geometry_build_count(),
            1,
            "unchanged gradient must reuse one local vector mesh and stop table across repaints"
        );
    }

    #[test]
    fn solid_vector_group_fill_is_cached_and_invalidated_with_its_asset() {
        let vector = VectorAsset {
            asset_id: 4243,
            paths: vec![VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(-20.0, -10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(40.0, -10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(40.0, 30.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(-20.0, 30.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 240,
                g: 120,
                b: 30,
                a: 200,
            }),
            stroke: None,
        };
        let mut cache = TextureCache::default();
        let first = cache.vector_group_fill(&vector).expect("solid fill mesh");
        assert_eq!(
            first.bounds,
            Rect::from_min_max(pos2(-20.0, -10.0), pos2(40.0, 30.0))
        );
        assert_eq!(first.mesh.indices.len(), 6);
        assert!(first
            .stops
            .iter()
            .all(|stop| stop.color == vector.fill.unwrap()));
        assert_eq!(cache.vector_render_geometry_build_count(), 1);

        let second = cache
            .vector_group_fill(&vector)
            .expect("cached solid fill mesh");
        assert!(Arc::ptr_eq(&first.mesh, &second.mesh));
        assert_eq!(cache.vector_render_geometry_build_count(), 1);

        cache.invalidate_asset(vector.asset_id);
        let rebuilt = cache
            .vector_group_fill(&vector)
            .expect("solid fill mesh after asset invalidation");
        assert!(!Arc::ptr_eq(&first.mesh, &rebuilt.mesh));
        assert_eq!(cache.vector_render_geometry_build_count(), 2);
    }

    #[test]
    fn cached_fill_remainder_split_preserves_hole_and_removes_nested_island() {
        fn square(min: f32, max: f32, clockwise: bool) -> Vec<Pos2> {
            let mut points = vec![
                pos2(min, min),
                pos2(max, min),
                pos2(max, max),
                pos2(min, max),
            ];
            if clockwise {
                points.reverse();
            }
            points
        }
        fn mesh_area(geometry: &CachedVectorRenderGeometry) -> f64 {
            geometry
                .fill_indices
                .chunks_exact(3)
                .map(|triangle| {
                    let a = geometry.fill_vertices[triangle[0] as usize];
                    let b = geometry.fill_vertices[triangle[1] as usize];
                    let c = geometry.fill_vertices[triangle[2] as usize];
                    f64::from(((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() * 0.5)
                })
                .sum()
        }

        // Donut + a separate island inside its hole is a deliberately awkward
        // bbox case: both disconnected components overlap in AABB space. The
        // warm-mesh split must remove only the island, keep the donut hole, and
        // never invent replacement triangles/chords.
        let outer = square(0.0, 100.0, false);
        let hole = square(20.0, 80.0, true);
        let island = square(40.0, 60.0, false);
        let source =
            build_cached_fill_geometry_from_pos2(vec![outer.clone(), hole.clone(), island.clone()]);
        let expected = build_cached_fill_geometry_from_pos2(vec![outer.clone(), hole.clone()]);
        let remainder =
            split_cached_fill_remainder(&source, std::slice::from_ref(&island), vec![outer, hole])
                .expect("nested island should be separable from the warm mesh");

        assert!(cached_fill_nonzero_contains(
            &remainder.fill_contours,
            pos2(10.0, 10.0)
        ));
        assert!(!cached_fill_nonzero_contains(
            &remainder.fill_contours,
            pos2(30.0, 30.0)
        ));
        assert!(!cached_fill_nonzero_contains(
            &remainder.fill_contours,
            pos2(50.0, 50.0)
        ));
        for triangle in remainder.fill_indices.chunks_exact(3) {
            let a = remainder.fill_vertices[triangle[0] as usize];
            let b = remainder.fill_vertices[triangle[1] as usize];
            let c = remainder.fill_vertices[triangle[2] as usize];
            let centroid = pos2((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
            assert!(
                cached_fill_nonzero_contains(&remainder.fill_contours, centroid),
                "split left a triangle outside the stationary donut: {centroid:?}"
            );
            assert!(
                !cached_fill_nonzero_contains(std::slice::from_ref(&island), centroid),
                "selected island ghost remained in the stationary mesh: {centroid:?}"
            );
        }
        assert!(remainder.fill_vertices.iter().all(|vertex| {
            source
                .fill_vertices
                .iter()
                .any(|source_vertex| source_vertex == vertex)
        }));
        assert!(
            (mesh_area(&remainder) - mesh_area(&expected)).abs() <= 1.0e-4,
            "warm mesh partition lost or added visible donut area"
        );
    }

    #[test]
    fn reflected_nested_q0rg_still_emits_render_shapes() {
        let mut project = crate::state::default_project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![VPath {
                anchors: [
                    Vec2::new(0.0, 0.0),
                    Vec2::new(20.0, 0.0),
                    Vec2::new(20.0, 20.0),
                    Vec2::new(0.0, 20.0),
                ]
                .into_iter()
                .map(|point| Anchor {
                    point,
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
                closed: true,
            }],
            fill: Some(Rgba {
                r: 200,
                g: 100,
                b: 50,
                a: 255,
            }),
            stroke: None,
        }));
        project.q0rgs.push(q0s_format::v2::Q0rg {
            q0rg_id: 2,
            name: "child".into(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "art".into(),
                explicit_keyframes: Vec::new(),
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(77),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
                }],
            }],
        });
        project.q0rgs[0].layers[0].push_placement(Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Q0rg(2),
            transform: Transform2D {
                tx: 50.0,
                ty: 40.0,
                sx: 1.0,
                sy: -1.0,
                rotation: 0.0,
                skew_x: 0.0,
                skew_y: 0.0,
            },
            tween: Tween::None,
            fx: Default::default(),
        });

        let ctx = Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(200.0, 200.0));
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: rect,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let mut cache = TextureCache::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("reflected-nested-q0rg"),
                ));
                render_stage(&painter, &project, 1, 0, &view, &mut cache, ctx);
            },
        );
        assert!(
            !output.shapes.is_empty(),
            "reflected q0rg emitted no shapes for its nested vector",
        );
    }

    #[test]
    fn stage_paints_the_in_between_drawing_of_a_shape_tween() {
        fn square(asset_id: u16, min: f32, max: f32) -> Asset {
            Asset::Vector(VectorAsset {
                asset_id,
                paths: vec![VPath {
                    anchors: [
                        Vec2::new(min, 10.0),
                        Vec2::new(max, 10.0),
                        Vec2::new(max, 30.0),
                        Vec2::new(min, 30.0),
                    ]
                    .into_iter()
                    .map(|point| Anchor {
                        point,
                        in_handle: None,
                        out_handle: None,
                    })
                    .collect(),
                    closed: true,
                }],
                fill: Some(Rgba {
                    r: 200,
                    g: 100,
                    b: 50,
                    a: 255,
                }),
                stroke: None,
            })
        }
        fn mesh_bounds(shape: &Shape, bounds: &mut Rect) {
            match shape {
                Shape::Mesh(mesh) => *bounds = bounds.union(mesh.calc_bounds()),
                Shape::Vec(shapes) => shapes.iter().for_each(|shape| mesh_bounds(shape, bounds)),
                _ => {}
            }
        }

        let mut project = crate::state::default_project();
        project.q0rgs[0].frame_count = 8;
        project.assets.push(square(77, 0.0, 20.0));
        project.assets.push(square(78, 100.0, 140.0));
        for (frame, asset_id, tween) in [
            (
                0,
                77,
                Tween::Shape {
                    to_frame: 4,
                    easing: q0s_format::v2::Easing::Linear,
                },
            ),
            (4, 78, Tween::None),
        ] {
            project.q0rgs[0].layers[0].push_placement(Placement {
                instance_id: 0,
                frame,
                target: Target::Asset(asset_id),
                transform: Transform2D::IDENTITY,
                tween,
                fx: Default::default(),
            });
        }
        q0s_format::v2::validate(&project).expect("shape tween project");

        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(200.0, 200.0));
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: rect,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let mut cache = TextureCache::default();
        let mut painted = |frame: u16| {
            let ctx = Context::default();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new("shape-tween"),
                    ));
                    render_stage(&painter, &project, 1, frame, &view, &mut cache, ctx);
                },
            );
            let mut bounds = Rect::NOTHING;
            for clipped in &output.shapes {
                mesh_bounds(&clipped.shape, &mut bounds);
            }
            bounds
        };

        // Halfway between 0..20 and 100..140, and again a quarter of the way.
        let half = painted(2);
        assert!((half.min.x - 50.0).abs() < 0.5, "{half:?}");
        assert!((half.max.x - 80.0).abs() < 0.5, "{half:?}");
        assert!((half.min.y - 10.0).abs() < 0.5 && (half.max.y - 30.0).abs() < 0.5);
        let quarter = painted(1);
        assert!((quarter.min.x - 25.0).abs() < 0.5, "{quarter:?}");
        assert!((quarter.max.x - 50.0).abs() < 0.5, "{quarter:?}");
        // Both keyframes are the authored drawings again.
        let first = painted(0);
        assert!((first.max.x - 20.0).abs() < 0.5, "{first:?}");
        let last = painted(4);
        assert!((last.min.x - 100.0).abs() < 0.5, "{last:?}");
    }

    #[test]
    fn library_preview_bbox_includes_glow_and_blur_without_changing_object_bbox() {
        for fx in [
            PlacementFx {
                glow: Some(q0s_format::v2::GlowFx {
                    color: Rgba {
                        r: 255,
                        g: 0,
                        b: 0,
                        a: 255,
                    },
                    radius_x: 10.0,
                    radius_y: 10.0,
                    strength: 1.0,
                    quality: 3,
                }),
                ..Default::default()
            },
            PlacementFx {
                blur: Some(q0s_format::v2::BlurFx {
                    radius_x: 10.0,
                    radius_y: 10.0,
                    quality: 3,
                }),
                ..Default::default()
            },
        ] {
            let mut project = crate::state::default_project();
            project.assets.push(Asset::Vector(VectorAsset {
                asset_id: 90,
                paths: vec![VPath {
                    anchors: [
                        Vec2::new(0.0, 0.0),
                        Vec2::new(20.0, 0.0),
                        Vec2::new(20.0, 20.0),
                        Vec2::new(0.0, 20.0),
                    ]
                    .into_iter()
                    .map(|point| Anchor {
                        point,
                        in_handle: None,
                        out_handle: None,
                    })
                    .collect(),
                    closed: true,
                }],
                fill: Some(Rgba {
                    r: 10,
                    g: 20,
                    b: 30,
                    a: 255,
                }),
                stroke: None,
            }));
            let placement = Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(90),
                transform: Transform2D {
                    tx: 100.0,
                    ty: 80.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx,
            };
            project.q0rgs[0].layers[0]
                .placements
                .push(placement.clone());

            assert_eq!(
                placement_bbox(&project, &placement),
                Some((100.0, 80.0, 120.0, 100.0))
            );
            assert_eq!(
                q0rg_frame_bbox(&project, 1, 0),
                Some((90.0, 70.0, 130.0, 110.0))
            );
        }
    }
    #[test]
    fn glow_and_shadow_keep_the_body_on_the_vector_render_path() {
        let glow = PlacementFx {
            glow: Some(q0s_format::v2::GlowFx {
                color: Rgba {
                    r: 255,
                    g: 40,
                    b: 20,
                    a: 255,
                },
                radius_x: 18.0,
                radius_y: 18.0,
                strength: 1.2,
                quality: 3,
            }),
            ..Default::default()
        };
        let shadow = PlacementFx {
            shadow: Some(q0s_format::v2::DropShadowFx {
                color: Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 180,
                },
                blur_radius: 12.0,
                offset_x: 8.0,
                offset_y: 9.0,
                strength: 1.0,
            }),
            ..Default::default()
        };
        assert!(placement_fx_keeps_vector_body(glow));
        assert!(placement_fx_keeps_vector_body(shadow));
        assert!(!placement_fx_keeps_vector_body(PlacementFx {
            blur: Some(q0s_format::v2::BlurFx {
                radius_x: 4.0,
                radius_y: 4.0,
                quality: 3
            }),
            glow: glow.glow,
            ..Default::default()
        }));
        assert!(!placement_fx_keeps_vector_body(PlacementFx {
            opacity: 0.5,
            glow: glow.glow,
            ..Default::default()
        }));
        assert!(!placement_fx_keeps_vector_body(PlacementFx {
            blend_mode: BlendMode::Multiply,
            glow: glow.glow,
            ..Default::default()
        }));
    }

    #[test]
    fn outer_fx_cache_resolution_does_not_grow_when_camera_zooms() {
        let project = crate::state::default_project();
        let ctx = Context::default();
        let near = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 480.0)),
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let zoomed = StageView {
            origin: Pos2::new(-3000.0, -2000.0),
            scale: 10.0,
            stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(6400.0, 4800.0)),
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let near_outer = fx_cache_render_size(&project, &near, &ctx, true, PlacementFx::default());
        let zoomed_outer =
            fx_cache_render_size(&project, &zoomed, &ctx, true, PlacementFx::default());
        assert_eq!(near_outer, zoomed_outer);
        assert_eq!(near_outer, (1280, 960));
        assert!(
            fx_cache_render_size(&project, &zoomed, &ctx, false, PlacementFx::default()).0
                > near_outer.0,
            "the legacy full-body offscreen path should remain distinguishable from the zoom-independent outer-fx cache",
        );
    }

    #[test]
    fn placement_activity_fast_path_matches_full_resolver() {
        let layer = Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "activity".into(),
            explicit_keyframes: vec![0, 4, 9],
            placements: vec![
                Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(1),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::Linear { to_frame: 3 },
                    fx: PlacementFx::default(),
                },
                Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(2),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: PlacementFx::default(),
                },
                Placement {
                    instance_id: 0,
                    frame: 3,
                    target: Target::Asset(1),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: PlacementFx::default(),
                },
                Placement {
                    instance_id: 0,
                    frame: 7,
                    target: Target::Asset(3),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: PlacementFx::default(),
                },
            ],
        };
        for frame in 0..=12 {
            let resolved = resolve_layer_at_frame(&layer, frame);
            for placement_idx in 0..=layer.placements.len() {
                assert_eq!(
                    placement_is_active_at(&layer, placement_idx, frame),
                    resolved.iter().any(|item| item.index == placement_idx),
                    "activity diverged at frame {frame}, placement {placement_idx}",
                );
            }
        }
    }

    #[test]
    fn blur_cache_resolution_does_not_grow_when_camera_zooms() {
        let project = crate::state::default_project();
        let ctx = Context::default();
        let blur = PlacementFx {
            blur: Some(q0s_format::v2::BlurFx {
                radius_x: 12.0,
                radius_y: 12.0,
                quality: 3,
            }),
            ..Default::default()
        };
        let near = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 480.0)),
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let zoomed = StageView {
            origin: Pos2::new(-3000.0, -2000.0),
            scale: 10.0,
            stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(6400.0, 4800.0)),
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let near_size = fx_cache_render_size(&project, &near, &ctx, false, blur);
        let zoomed_size = fx_cache_render_size(&project, &zoomed, &ctx, false, blur);
        assert_eq!(near_size, zoomed_size);
        assert_eq!(near_size, (1280, 960));
    }
    #[test]
    fn fx_crop_screen_rect_uses_preview_origin_and_translation() {
        let project = crate::state::default_project();
        let view = StageView {
            origin: pos2(30.0, 40.0),
            scale: 0.5,
            stage_rect: Rect::from_min_size(pos2(100.0, 100.0), egui::vec2(200.0, 150.0)),
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let base = fx_crop_screen_rect(
            &project,
            1280,
            960,
            [200, 100, 400, 200],
            Vec2::new(120.0, 90.0),
            Vec2::new(120.0, 90.0),
            &view,
        );
        assert!((base.min.x - 80.0).abs() < 1.0e-4);
        assert!((base.min.y - 65.0).abs() < 1.0e-4);
        assert!((base.max.x - 180.0).abs() < 1.0e-4);
        assert!((base.max.y - 115.0).abs() < 1.0e-4);

        let moved = fx_crop_screen_rect(
            &project,
            1280,
            960,
            [200, 100, 400, 200],
            Vec2::new(120.0, 90.0),
            Vec2::new(170.0, 110.0),
            &view,
        );
        assert!((moved.min.x - (base.min.x + 25.0)).abs() < 1.0e-4);
        assert!((moved.min.y - (base.min.y + 10.0)).abs() < 1.0e-4);
        assert!((moved.max.x - (base.max.x + 25.0)).abs() < 1.0e-4);
        assert!((moved.max.y - (base.max.y + 10.0)).abs() < 1.0e-4);
    }

    #[test]
    fn plain_vector_q0rg_blend_modes_never_enter_cpu_raster_fx_cache() {
        let mut project = crate::state::default_project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 87,
            paths: vec![VPath {
                anchors: [
                    Vec2::new(0.0, 0.0),
                    Vec2::new(60.0, 0.0),
                    Vec2::new(60.0, 45.0),
                    Vec2::new(0.0, 45.0),
                ]
                .into_iter()
                .map(|point| Anchor {
                    point,
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
                closed: true,
            }],
            fill: Some(Rgba {
                r: 210,
                g: 70,
                b: 40,
                a: 255,
            }),
            stroke: None,
        }));
        project.q0rgs.push(Q0rg {
            q0rg_id: 2,
            name: "blend child".to_string(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "child art".to_string(),
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(87),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: PlacementFx::default(),
                }],
                explicit_keyframes: Vec::new(),
            }],
        });
        project.q0rgs[0].layers[0].push_placement(Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Q0rg(2),
            transform: Transform2D {
                tx: 140.0,
                ty: 100.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: PlacementFx {
                blend_mode: BlendMode::Multiply,
                ..Default::default()
            },
        });

        let ctx = Context::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 480.0)),
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let mut cache = TextureCache::default();
        let render_once = |project: &ProjectV2, cache: &mut TextureCache, id: &'static str| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx
                        .layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new(id)));
                    render_stage(&painter, project, 1, 0, &view, cache, ctx);
                },
            );
        };

        let baseline_serial = cache.fx_serial;
        for (mode, id) in [
            (BlendMode::Multiply, "blend-multiply"),
            (BlendMode::Screen, "blend-screen"),
            (BlendMode::Add, "blend-add"),
            (BlendMode::Overlay, "blend-overlay"),
        ] {
            project.q0rgs[0].layers[0].placements[0].fx.blend_mode = mode;
            render_once(&project, &mut cache, id);
            assert_eq!(
                cache.fx_serial, baseline_serial,
                "{mode:?} entered the CPU q0rg raster FX path"
            );
            assert!(
                cache.fx_placement_textures.is_empty(),
                "{mode:?} allocated a CPU-rasterized q0rg texture"
            );
            assert_eq!(
                cache.vector_render_by_asset.len(),
                1,
                "{mode:?} did not reuse the ordinary vector tessellation cache"
            );
        }
    }

    #[test]
    fn plain_fill_text_vector_opacity_stays_resolution_independent() {
        let mut project = crate::state::default_project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 91,
            paths: vec![VPath {
                anchors: [
                    Vec2::new(0.0, 0.0),
                    Vec2::new(100.0, 0.0),
                    Vec2::new(100.0, 40.0),
                    Vec2::new(0.0, 40.0),
                ]
                .into_iter()
                .map(|point| Anchor {
                    point,
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
                closed: true,
            }],
            fill: Some(Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        }));
        project
            .asset_names
            .insert(91, crate::text_tool::encode_source("opacity text", 48.0));
        project.q0rgs[0].layers[0].push_placement(Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(91),
            transform: Transform2D {
                tx: 120.0,
                ty: 90.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: PlacementFx {
                opacity: 0.42,
                ..Default::default()
            },
        });

        let ctx = Context::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut cache = TextureCache::default();
        for (index, view) in [
            StageView {
                origin: Pos2::ZERO,
                scale: 1.0,
                stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 480.0)),
                edit_transform: q0s_format::transform::Affine::IDENTITY,
            },
            StageView {
                origin: Pos2::new(-2800.0, -2100.0),
                scale: 10.0,
                stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(6400.0, 4800.0)),
                edit_transform: q0s_format::transform::Affine::IDENTITY,
            },
        ]
        .into_iter()
        .enumerate()
        {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new(("text-opacity-vector", index)),
                    ));
                    render_stage(&painter, &project, 1, 0, &view, &mut cache, ctx);
                },
            );
            assert!(
                cache.fx_placement_textures.is_empty(),
                "plain vector text opacity entered the CPU raster FX cache"
            );
        }
        assert_eq!(cache.vector_render_geometry_build_count(), 1);
    }

    #[test]
    fn gradient_q0rg_opacity_stays_analytic_and_never_enters_cpu_fx_cache() {
        let mut project = crate::state::default_project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 92,
            paths: vec![VPath {
                anchors: [
                    Vec2::new(-30.0, -20.0),
                    Vec2::new(30.0, -20.0),
                    Vec2::new(30.0, 20.0),
                    Vec2::new(-30.0, 20.0),
                ]
                .into_iter()
                .map(|point| Anchor {
                    point,
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
                closed: true,
            }],
            fill: Some(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
            stroke: None,
        }));
        project.asset_appearances.insert(
            92,
            VectorAppearance {
                material: q0s_format::v2::VectorMaterial::Solid,
                erase_mask: Vec::new(),
                material_source: Vec::new(),
                clip_mask: Vec::new(),
                field_transform: Affine::IDENTITY,
                fill_gradient: Some(q0s_format::v2::GradientFill {
                    kind: GradientKind::Radial,
                    stops: vec![
                        q0s_format::v2::GradientStop {
                            position: 0.0,
                            color: Rgba {
                                r: 255,
                                g: 255,
                                b: 255,
                                a: 180,
                            },
                        },
                        q0s_format::v2::GradientStop {
                            position: 1.0,
                            color: Rgba {
                                r: 255,
                                g: 240,
                                b: 140,
                                a: 0,
                            },
                        },
                    ],
                    transform: Affine {
                        a11: 28.6,
                        a12: 0.0,
                        a21: 0.0,
                        a22: 28.6,
                        tx: 0.0,
                        ty: 0.0,
                    },
                }),
            },
        );
        project.q0rgs.push(Q0rg {
            q0rg_id: 2,
            name: "gradient child".to_string(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "gradient".to_string(),
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(92),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: PlacementFx::default(),
                }],
                explicit_keyframes: Vec::new(),
            }],
        });
        project.q0rgs[0].layers[0].push_placement(Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Q0rg(2),
            transform: Transform2D {
                tx: 160.0,
                ty: 120.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: PlacementFx {
                opacity: 0.42,
                ..Default::default()
            },
        });

        let ctx = Context::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut cache = TextureCache::default();
        for (index, scale) in [1.0, 10.0].into_iter().enumerate() {
            let view = StageView {
                origin: Pos2::ZERO,
                scale,
                stage_rect: Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(640.0 * scale, 480.0 * scale),
                ),
                edit_transform: q0s_format::transform::Affine::IDENTITY,
            };
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new(("gradient-q0rg-opacity", index)),
                    ));
                    render_stage(&painter, &project, 1, 0, &view, &mut cache, ctx);
                },
            );
            assert!(
                cache.fx_placement_textures.is_empty(),
                "analytic gradient q0rg opacity fell back to a CPU-rasterized texture"
            );
        }
        assert_eq!(
            cache.gradient_render_geometry_build_count(),
            1,
            "camera zoom must reuse the same local analytic gradient mesh"
        );
    }

    #[test]
    fn bitmap_q0rg_opacity_and_add_blend_never_enter_cpu_fx_cache() {
        for (label, fx) in [
            (
                "opacity",
                PlacementFx {
                    opacity: 0.5,
                    ..Default::default()
                },
            ),
            (
                "add",
                PlacementFx {
                    blend_mode: BlendMode::Add,
                    ..Default::default()
                },
            ),
        ] {
            let mut project = crate::state::default_project();
            project
                .assets
                .push(Asset::Bitmap(q0s_format::v2::BitmapAsset {
                    asset_id: 93,
                    width: 2,
                    height: 2,
                    rgba: vec![
                        255, 80, 20, 255, 255, 180, 40, 180, 100, 100, 100, 120, 0, 0, 0, 0,
                    ],
                }));
            project.q0rgs.push(Q0rg {
                q0rg_id: 2,
                name: "bitmap child".to_string(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    timeline_frame_count: None,
                    name: "bitmap".to_string(),
                    placements: vec![Placement {
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(93),
                        transform: Transform2D {
                            sx: 24.0,
                            sy: 24.0,
                            ..Transform2D::IDENTITY
                        },
                        tween: Tween::None,
                        fx: PlacementFx::default(),
                    }],
                    explicit_keyframes: Vec::new(),
                }],
            });
            project.q0rgs[0].layers[0].push_placement(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Q0rg(2),
                transform: Transform2D {
                    tx: 100.0,
                    ty: 80.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx,
            });

            let ctx = Context::default();
            let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
            let view = StageView {
                origin: Pos2::ZERO,
                scale: 2.0,
                stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 960.0)),
                edit_transform: q0s_format::transform::Affine::IDENTITY,
            };
            let mut cache = TextureCache::default();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new(("bitmap-group-gpu", label)),
                    ));
                    render_stage(&painter, &project, 1, 0, &view, &mut cache, ctx);
                },
            );
            assert!(
                cache.fx_placement_textures.is_empty(),
                "bitmap q0rg {label} fell back to CPU-rasterized placement fx"
            );
            assert!(cache.by_asset_id.contains_key(&93));
        }
    }
    #[test]
    fn translating_filtered_q0rg_stays_gpu_vector_through_affine_edits() {
        for (label, fx) in [
            (
                "glow",
                PlacementFx {
                    glow: Some(q0s_format::v2::GlowFx {
                        color: Rgba {
                            r: 255,
                            g: 40,
                            b: 20,
                            a: 255,
                        },
                        radius_x: 12.0,
                        radius_y: 12.0,
                        strength: 1.0,
                        quality: 3,
                    }),
                    ..Default::default()
                },
            ),
            (
                "blur",
                PlacementFx {
                    blur: Some(q0s_format::v2::BlurFx {
                        radius_x: 10.0,
                        radius_y: 10.0,
                        quality: 3,
                    }),
                    ..Default::default()
                },
            ),
        ] {
            let mut project = crate::state::default_project();
            project.assets.push(Asset::Vector(VectorAsset {
                asset_id: 88,
                paths: vec![VPath {
                    anchors: [
                        Vec2::new(0.0, 0.0),
                        Vec2::new(50.0, 0.0),
                        Vec2::new(50.0, 40.0),
                        Vec2::new(0.0, 40.0),
                    ]
                    .into_iter()
                    .map(|point| Anchor {
                        point,
                        in_handle: None,
                        out_handle: None,
                    })
                    .collect(),
                    closed: true,
                }],
                fill: Some(Rgba {
                    r: 20,
                    g: 30,
                    b: 40,
                    a: 255,
                }),
                stroke: None,
            }));
            project.q0rgs.push(Q0rg {
                q0rg_id: 2,
                name: "filtered child".to_string(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    timeline_frame_count: None,
                    name: "child art".to_string(),
                    placements: vec![Placement {
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(88),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: PlacementFx::default(),
                    }],
                    explicit_keyframes: Vec::new(),
                }],
            });
            project.q0rgs[0].layers[0].push_placement(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Q0rg(2),
                transform: Transform2D {
                    tx: 120.0,
                    ty: 90.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx,
            });

            let ctx = Context::default();
            let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
            let view = StageView {
                origin: Pos2::ZERO,
                scale: 1.0,
                stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 480.0)),
                edit_transform: q0s_format::transform::Affine::IDENTITY,
            };
            let mut cache = TextureCache::default();
            let render_once = |project: &ProjectV2, cache: &mut TextureCache, id: &'static str| {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(viewport),
                        ..Default::default()
                    },
                    |ctx| {
                        let painter = ctx.layer_painter(egui::LayerId::new(
                            egui::Order::Middle,
                            egui::Id::new((id, label)),
                        ));
                        render_stage(&painter, project, 1, 0, &view, cache, ctx);
                    },
                );
            };
            render_once(&project, &mut cache, "before-drag");
            assert!(
                cache.fx_placement_textures.is_empty(),
                "{label} entered the CPU fx texture cache on the warm render",
            );

            project.q0rgs[0].layers[0].placements[0].transform.tx += 73.0;
            project.q0rgs[0].layers[0].placements[0].transform.ty += 41.0;
            render_once(&project, &mut cache, "after-drag");
            assert!(
                cache.fx_placement_textures.is_empty(),
                "{label} translation created a CPU fx texture",
            );

            cache.interactive_transform = true;
            for step in 1..=24 {
                let transform = &mut project.q0rgs[0].layers[0].placements[0].transform;
                transform.rotation = step as f32 * 0.03;
                transform.sx = 1.0 + step as f32 * 0.02;
                transform.skew_x = step as f32 * 0.005;
                render_once(&project, &mut cache, "live-affine");
                assert!(
                    cache.fx_placement_textures.is_empty(),
                    "{label} affine edit fell back to CPU raster fx",
                );
            }
            cache.interactive_transform = false;
            render_once(&project, &mut cache, "released-affine");
            render_once(&project, &mut cache, "settled-affine");
            assert!(cache.fx_placement_textures.is_empty());
            assert_eq!(
                cache.vector_render_geometry_build_count(),
                1,
                "{label} affine edits rebuilt local vector geometry instead of reprojecting it",
            );
        }
    }
    #[test]
    fn filtered_q0rg_gpu_path_tracks_source_moves_and_falls_back_for_nested_blend() {
        let mut project = crate::state::default_project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 89,
            paths: vec![VPath {
                anchors: [
                    Vec2::new(0.0, 0.0),
                    Vec2::new(40.0, 0.0),
                    Vec2::new(40.0, 30.0),
                    Vec2::new(0.0, 30.0),
                ]
                .into_iter()
                .map(|point| Anchor {
                    point,
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
                closed: true,
            }],
            fill: Some(Rgba {
                r: 80,
                g: 90,
                b: 100,
                a: 255,
            }),
            stroke: None,
        }));
        project.q0rgs.push(Q0rg {
            q0rg_id: 2,
            name: "blur source".to_string(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "source".to_string(),
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(89),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: PlacementFx::default(),
                }],
                explicit_keyframes: Vec::new(),
            }],
        });
        project.q0rgs[0].layers[0].placements.extend([
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Q0rg(2),
                transform: Transform2D {
                    tx: 100.0,
                    ty: 80.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: PlacementFx {
                    blur: Some(q0s_format::v2::BlurFx {
                        radius_x: 9.0,
                        radius_y: 9.0,
                        quality: 3,
                    }),
                    ..Default::default()
                },
            },
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(89),
                transform: Transform2D {
                    tx: 300.0,
                    ty: 220.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: PlacementFx::default(),
            },
        ]);

        let ctx = Context::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 480.0)),
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let mut cache = TextureCache::default();
        let render_once = |project: &ProjectV2, cache: &mut TextureCache, id: &'static str| {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx
                        .layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new(id)));
                    render_stage(&painter, project, 1, 0, &view, cache, ctx);
                },
            );
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    Shape::Callback(callback) => Some(callback.rect),
                    _ => None,
                })
        };
        let warm_rect = render_once(&project, &mut cache, "filtered-source-warm")
            .expect("filtered q0rg should emit a GPU callback");
        assert!(cache.fx_placement_textures.is_empty());
        let warm_serial = cache.fx_serial;

        project.q0rgs[0].layers[0].placements[1].transform.tx += 25.0;
        project.q0rgs[0].layers[0].placements[1].transform.rotation += 0.5;
        project.q0rgs[0].layers[0].placements[1].transform.sx = 1.8;
        let unrelated_rect = render_once(&project, &mut cache, "filtered-source-unrelated-move")
            .expect("filtered q0rg should remain on the GPU path");
        assert_eq!(
            unrelated_rect, warm_rect,
            "moving unrelated artwork changed the filtered q0rg GPU callback bounds"
        );
        assert_eq!(cache.fx_serial, warm_serial);
        assert!(cache.fx_placement_textures.is_empty());

        project.q0rgs[1].layers[0].placements[0].transform.tx += 12.0;
        project.q0rgs[1].layers[0].placements[0].transform.rotation += 0.5;
        let moved_rect = render_once(&project, &mut cache, "filtered-source-internal-move")
            .expect("moving filtered source should still emit a GPU callback");
        assert_ne!(
            moved_rect, warm_rect,
            "moving artwork inside the filtered q0rg left stale GPU callback bounds"
        );
        assert!(cache.fx_placement_textures.is_empty());

        // Per-child blend inside a filtered parent is not representable by the current
        // flat group primitive stream. It must fall back rather than silently changing pixels.
        project.q0rgs[1].layers[0].placements[0].fx.blend_mode = BlendMode::Screen;
        let _ = render_once(&project, &mut cache, "filtered-source-internal-blend");
        assert!(
            cache.fx_serial > warm_serial && !cache.fx_placement_textures.is_empty(),
            "unsupported nested blend neither used the safe raster fallback nor invalidated it"
        );
    }
    #[test]
    fn q0rg_glow_zoom_stays_gpu_vector_and_never_enters_cpu_fx_cache() {
        let mut project = crate::state::default_project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![VPath {
                anchors: [
                    Vec2::new(0.0, 0.0),
                    Vec2::new(40.0, 0.0),
                    Vec2::new(40.0, 30.0),
                    Vec2::new(0.0, 30.0),
                ]
                .into_iter()
                .map(|point| Anchor {
                    point,
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
                closed: true,
            }],
            fill: Some(Rgba {
                r: 30,
                g: 80,
                b: 210,
                a: 255,
            }),
            stroke: None,
        }));
        project.q0rgs.push(Q0rg {
            q0rg_id: 2,
            name: "glow child".to_string(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "child art".to_string(),
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(77),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: PlacementFx::default(),
                }],
                explicit_keyframes: Vec::new(),
            }],
        });
        project.q0rgs[0].layers[0].push_placement(Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Q0rg(2),
            transform: Transform2D {
                tx: 120.0,
                ty: 90.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: PlacementFx {
                glow: Some(q0s_format::v2::GlowFx {
                    color: Rgba {
                        r: 255,
                        g: 40,
                        b: 20,
                        a: 255,
                    },
                    radius_x: 14.0,
                    radius_y: 14.0,
                    strength: 1.0,
                    quality: 3,
                }),
                ..Default::default()
            },
        });

        let ctx = Context::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut cache = TextureCache::default();
        for (index, view) in [
            StageView {
                origin: Pos2::ZERO,
                scale: 1.0,
                stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 480.0)),
                edit_transform: q0s_format::transform::Affine::IDENTITY,
            },
            StageView {
                origin: Pos2::new(-2800.0, -2100.0),
                scale: 10.0,
                stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(6400.0, 4800.0)),
                edit_transform: q0s_format::transform::Affine::IDENTITY,
            },
        ]
        .into_iter()
        .enumerate()
        {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new(("q0rg-glow-zoom", index)),
                    ));
                    render_stage(&painter, &project, 1, 0, &view, &mut cache, ctx);
                },
            );
            assert!(
                cache.fx_placement_textures.is_empty(),
                "q0rg glow fell back to a CPU-rasterized placement texture during camera zoom",
            );
        }
        assert_eq!(
            cache.vector_render_geometry_build_count(),
            1,
            "the glowing q0rg body did not stay on the cached vector render path",
        );
    }
    #[test]
    fn warm_fill_repaints_reuse_cached_vector_tessellation_across_zoom() {
        let anchors: Vec<Anchor> = (0..512)
            .map(|index| {
                let angle = index as f32 / 512.0 * std::f32::consts::TAU;
                let wobble = 4.0 * (angle * 13.0).sin();
                let radius = 80.0 + wobble;
                Anchor {
                    point: Vec2::new(120.0 + radius * angle.cos(), 120.0 + radius * angle.sin()),
                    in_handle: None,
                    out_handle: None,
                }
            })
            .collect();
        let asset = Asset::Vector(VectorAsset {
            asset_id: 91,
            paths: vec![VPath {
                anchors,
                closed: true,
            }],
            fill: Some(Rgba {
                r: 70,
                g: 80,
                b: 90,
                a: 255,
            }),
            stroke: None,
        });
        let ctx = Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut cache = TextureCache::default();

        for (index, scale) in [0.25_f32, 0.5, 1.0, 2.0, 4.0].into_iter().enumerate() {
            let view = StageView {
                origin: Pos2::new(20.0, 30.0),
                scale,
                stage_rect: rect,
                edit_transform: q0s_format::transform::Affine::IDENTITY,
            };
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new(("vector-render-cache", index)),
                    ));
                    render_asset(
                        &painter,
                        &asset,
                        None,
                        Affine::IDENTITY,
                        &view,
                        &mut cache,
                        ctx,
                        Color32::WHITE,
                        None,
                        0,
                        24,
                    );
                },
            );
        }
        assert_eq!(
            cache.vector_render_geometry_build_count(),
            1,
            "warm repaints/zoom rebuilt flattened+tessellated vector geometry",
        );

        let translated = Affine {
            tx: 37.0,
            ty: -11.0,
            ..Affine::IDENTITY
        };
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: rect,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("vector-render-cache-live-affine"),
                ));
                render_asset(
                    &painter,
                    &asset,
                    None,
                    translated,
                    &view,
                    &mut cache,
                    ctx,
                    Color32::WHITE,
                    None,
                    0,
                    24,
                );
            },
        );
        assert_eq!(
            cache.vector_render_geometry_build_count(),
            1,
            "moving a cached raw vector through a placement affine rebuilt its geometry",
        );

        #[cfg(feature = "appearance-mask-eraser")]
        if let Asset::Vector(vector) = &asset {
            let plain = q0s_format::v2::VectorAppearance {
                material: q0s_format::v2::VectorMaterial::Solid,
                erase_mask: Vec::new(),
                material_source: Vec::new(),
                clip_mask: Vec::new(),
                field_transform: Affine::IDENTITY,
                fill_gradient: None,
            };
            assert!(cache.selection_stage_mesh(vector, &plain, &[0]).is_some());
            assert_eq!(
                cache.vector_render_geometry_build_count(),
                1,
                "plain selection rebuilt geometry already cached by the stage renderer",
            );
        }

        cache.invalidate_asset(91);
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: rect,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("vector-render-cache-after-invalidate"),
                ));
                render_asset(
                    &painter,
                    &asset,
                    None,
                    Affine::IDENTITY,
                    &view,
                    &mut cache,
                    ctx,
                    Color32::WHITE,
                    None,
                    0,
                    24,
                );
            },
        );
        assert_eq!(cache.vector_render_geometry_build_count(), 2);
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn live_selection_pose_replaces_old_screen_paint_instead_of_accumulating() {
        let mut cache = TextureCache::default();
        let base = SelectionPaintKey {
            asset_id: 7,
            path_indices: vec![0, 2],
            field_transform_bits: [0; 6],
            placement_transform_bits: [0; 6],
            view_origin_bits: [0; 2],
            view_scale_bits: 1.0f32.to_bits(),
            view_transform_bits: [1.0f32.to_bits(), 0, 0, 1.0f32.to_bits(), 0, 0],
            accent_rgba: [255, 0, 255, 255],
            texture_id: TextureId::Managed(4),
        };
        for step in 0..200_u32 {
            let mut key = base.clone();
            key.placement_transform_bits[4] = (step as f32).to_bits();
            cache.insert_selection_paint(
                key,
                CachedSelectionPaint {
                    meshes: Vec::new(),
                    solid_overlay: None,
                    affine_reprojectable: false,
                    view_origin: Pos2::ZERO,
                    view_scale: 1.0,
                },
            );
            assert_eq!(
                cache.selection_paint_cache_len(),
                1,
                "live drag retained stale screen-pose selection meshes",
            );
        }
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn selection_paint_split_preserves_outline_triangles_exactly() {
        let mut stipple = Mesh::with_texture(TextureId::Managed(77));
        stipple.colored_vertex(pos2(1.0, 2.0), Color32::WHITE);
        stipple.colored_vertex(pos2(3.0, 2.0), Color32::WHITE);
        stipple.colored_vertex(pos2(1.0, 4.0), Color32::WHITE);
        stipple.indices.extend_from_slice(&[0, 1, 2]);

        let mut black = Mesh::default();
        black.colored_vertex(pos2(10.0, 11.0), Color32::from_black_alpha(180));
        black.colored_vertex(pos2(12.0, 11.0), Color32::from_black_alpha(180));
        black.colored_vertex(pos2(10.0, 13.0), Color32::from_black_alpha(180));
        black.indices.extend_from_slice(&[0, 1, 2]);
        let mut accent = Mesh::default();
        accent.colored_vertex(pos2(20.0, 21.0), Color32::from_rgb(20, 80, 220));
        accent.colored_vertex(pos2(22.0, 21.0), Color32::from_rgb(20, 80, 220));
        accent.colored_vertex(pos2(20.0, 23.0), Color32::from_rgb(20, 80, 220));
        accent.indices.extend_from_slice(&[0, 1, 2]);

        let mut expected = black.clone();
        expected.append(accent.clone());
        let (textured, solid) = split_selection_paint_meshes(vec![stipple.clone(), black, accent]);
        assert_eq!(textured.len(), 1);
        assert_eq!(textured[0].as_ref(), &stipple);
        let solid = solid.expect("outline GPU mesh");
        assert_eq!(solid.vertices, expected.vertices);
        assert_eq!(solid.indices, expected.indices);
        assert_eq!(solid.texture_id, expected.texture_id);
    }

    #[test]
    fn incremental_preview_outline_never_overpaints_existing_body() {
        let ctx = Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0));
        let mut cache = TextureCache::default();
        let fill = Color32::from_rgba_unmultiplied(200, 20, 20, 40);
        let outline = Color32::from_rgba_unmultiplied(200, 20, 20, 220);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let view = StageView {
                    origin: Pos2::ZERO,
                    scale: 1.0,
                    stage_rect: rect,
                    edit_transform: q0s_format::transform::Affine::IDENTITY,
                };
                cache.begin_classic_brush_preview(ctx, rect, &view);
                cache.raster_classic_brush_preview(
                    &[vec![
                        pos2(20.0, 20.0),
                        pos2(40.0, 20.0),
                        pos2(40.0, 40.0),
                        pos2(20.0, 40.0),
                    ]],
                    fill,
                );
                cache.raster_classic_brush_preview_outline(
                    &[vec![
                        pos2(18.0, 18.0),
                        pos2(42.0, 18.0),
                        pos2(42.0, 42.0),
                        pos2(18.0, 42.0),
                    ]],
                    outline,
                );

                let preview = cache.classic_brush_preview.as_ref().unwrap();
                let sample = |x: f32, y: f32| {
                    let px = ((x - preview.screen_rect.min.x) * preview.pixels_per_point).floor()
                        as usize;
                    let py = ((y - preview.screen_rect.min.y) * preview.pixels_per_point).floor()
                        as usize;
                    preview.pixels[py * preview.size[0] + px]
                };
                assert_eq!(
                    sample(30.0, 30.0),
                    fill,
                    "new outline painted an internal seam over old body"
                );
                assert_eq!(
                    sample(19.0, 30.0),
                    outline,
                    "external outline was not preserved"
                );
            },
        );
    }
    #[test]
    fn classic_brush_texture_updates_only_new_dirty_tail() {
        let ctx = Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut cache = TextureCache::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let view = StageView {
                    origin: Pos2::ZERO,
                    scale: 1.0,
                    stage_rect: rect,
                    edit_transform: q0s_format::transform::Affine::IDENTITY,
                };
                cache.begin_classic_brush_preview(ctx, rect, &view);
                let started = std::time::Instant::now();
                for index in 0..600 {
                    let x = 20.0 + (index % 120) as f32 * 6.0;
                    let y = 20.0 + ((index / 120) % 4) as f32 * 40.0;
                    cache.raster_classic_brush_preview(
                        &[vec![
                            pos2(x - 5.0, y - 5.0),
                            pos2(x + 5.0, y - 5.0),
                            pos2(x + 5.0, y + 5.0),
                            pos2(x - 5.0, y + 5.0),
                        ]],
                        Color32::from_rgba_unmultiplied(20, 30, 40, 128),
                    );
                }
                assert!(
                    started.elapsed() < std::time::Duration::from_secs(2),
                    "incremental classic preview regressed into whole-stroke work: {:?}",
                    started.elapsed()
                );
                let full_canvas_pixels = 800 * 600;
                assert!(
                    cache.classic_preview_uploaded_pixels < full_canvas_pixels * 2,
                    "partial updates rewrote the whole canvas repeatedly: {} uploaded pixels",
                    cache.classic_preview_uploaded_pixels
                );
                assert!(
                    cache.classic_preview_candidate_pixels < full_canvas_pixels * 2,
                    "raster work grew with stroke history: {} candidate pixels",
                    cache.classic_preview_candidate_pixels
                );
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("classic-preview-regression"),
                ));
                assert!(cache.paint_classic_brush_preview(&painter));
            },
        );
    }

    fn test_placement(frame: u16, asset_id: u16, tx: f32, tween: Tween) -> Placement {
        Placement {
            instance_id: 0,
            frame,
            target: Target::Asset(asset_id),
            transform: Transform2D {
                tx,
                ..Transform2D::IDENTITY
            },
            tween,
            fx: Default::default(),
        }
    }

    #[test]
    fn cached_layer_frame_index_matches_reference_for_tweens_and_blank_keyframes() {
        let layer = Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "indexed".into(),
            explicit_keyframes: vec![8, 14],
            placements: vec![
                test_placement(0, 7, 0.0, Tween::Linear { to_frame: 5 }),
                test_placement(0, 7, 20.0, Tween::Linear { to_frame: 5 }),
                test_placement(5, 7, 10.0, Tween::None),
                test_placement(5, 7, 40.0, Tween::None),
                test_placement(10, 8, 80.0, Tween::None),
            ],
        };
        let mut index = LayerFrameIndex::new(&layer);
        for frame in 0..=16 {
            assert_eq!(
                *index.resolve_shared(&layer, frame),
                q0s_format::raster::active_placement_states_at(&layer, frame),
                "frame {frame}"
            );
        }
    }

    #[test]
    fn viewport_culling_keeps_conservative_vector_stroke_margin() {
        let mut project = crate::state::default_project();
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 42,
            paths: vec![VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(0.0, 0.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(20.0, 20.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: false,
            }],
            fill: None,
            stroke: Some(q0s_format::v2::Stroke {
                color: Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                width: 2.0,
                cap: q0s_format::geom::CapShape::Round,
            }),
        }));
        let clip = Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0));
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: clip,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let mut cache = TextureCache::default();
        let asset = &project.assets[0];
        assert!(!cache.asset_intersects_clip(
            &project,
            42,
            asset,
            Affine {
                tx: 200.0,
                ..Affine::IDENTITY
            },
            &view,
            clip,
        ));
        assert!(cache.asset_intersects_clip(
            &project,
            42,
            asset,
            Affine {
                tx: 100.0,
                ..Affine::IDENTITY
            },
            &view,
            clip,
        ));
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn appearance_cache_signature_tracks_frozen_field_content_not_carrier_affine() {
        let path = VPath {
            anchors: [
                Vec2::new(0.0, 0.0),
                Vec2::new(20.0, 0.0),
                Vec2::new(20.0, 20.0),
                Vec2::new(0.0, 20.0),
            ]
            .into_iter()
            .map(|point| Anchor {
                point,
                in_handle: None,
                out_handle: None,
            })
            .collect(),
            closed: true,
        };
        let vector = q0s_format::v2::VectorAsset {
            asset_id: 7,
            paths: vec![path.clone()],
            fill: Some(Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        };
        let appearance = VectorAppearance {
            material: q0s_format::v2::VectorMaterial::SoftHalo {
                radius: 10.0,
                opacity: 0.5,
            },
            erase_mask: Vec::new(),
            material_source: vec![path.clone()],
            clip_mask: vec![path],
            field_transform: q0s_format::transform::Affine::IDENTITY,
            fill_gradient: None,
        };
        let (before_hash, before_origin) = appearance_cache_signature(&vector, &appearance);

        let delta = Vec2::new(73.0, -19.0);
        let mut moved_vector = vector.clone();
        for path in &mut moved_vector.paths {
            for anchor in &mut path.anchors {
                anchor.point.x += delta.x;
                anchor.point.y += delta.y;
            }
        }
        let moved_appearance = crate::appearance::transform_appearance(
            &appearance,
            Affine {
                tx: delta.x,
                ty: delta.y,
                ..Affine::IDENTITY
            },
        );
        let (moved_hash, moved_origin) =
            appearance_cache_signature(&moved_vector, &moved_appearance);
        assert_eq!(
            before_hash, moved_hash,
            "affine movement must reuse the frozen halo texture"
        );
        assert_eq!(before_origin, moved_origin);

        moved_vector.paths[0].anchors[1].point.x += 4.0;
        let (changed_hash, _) = appearance_cache_signature(&moved_vector, &moved_appearance);
        assert_eq!(
            moved_hash, changed_hash,
            "editing the carrier vector must not regenerate an already-frozen material field"
        );

        let mut changed_field = moved_appearance.clone();
        changed_field.material_source[0].anchors[1].point.x += 4.0;
        let (changed_field_hash, _) = appearance_cache_signature(&moved_vector, &changed_field);
        assert_ne!(
            moved_hash, changed_field_hash,
            "changing frozen field content must invalidate the halo texture"
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn selection_geometry_keeps_boolean_boundary_linear_without_16x_resampling() {
        let anchors: Vec<Anchor> = (0..256)
            .map(|index| {
                let angle = index as f32 / 256.0 * std::f32::consts::TAU;
                Anchor {
                    point: Vec2::new(angle.cos() * 50.0, angle.sin() * 50.0),
                    in_handle: None,
                    out_handle: None,
                }
            })
            .collect();
        let path = VPath {
            anchors,
            closed: true,
        };
        let vector = q0s_format::v2::VectorAsset {
            asset_id: 87,
            paths: vec![path.clone()],
            fill: Some(Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        };
        let appearance = VectorAppearance {
            material: q0s_format::v2::VectorMaterial::SoftHalo {
                radius: 6.0,
                opacity: 0.6,
            },
            erase_mask: Vec::new(),
            material_source: vec![path.clone()],
            clip_mask: vec![path],
            field_transform: Affine::IDENTITY,
            fill_gradient: None,
        };
        let mut cache = TextureCache::default();
        let geometry = cache
            .selection_geometry(&vector, &appearance, &[0])
            .expect("selection geometry");
        let point_count: usize = geometry.body.iter().map(Vec::len).sum();
        assert!(
            (200..=300).contains(&point_count),
            "linear boolean boundary was expanded into sampled pseudo-Beziers: {point_count} points",
        );
        assert!(geometry.fallback_material_support.is_empty());
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn halo_texture_cache_is_invariant_to_view_zoom_and_affine_scale() {
        let path = VPath {
            anchors: [
                Vec2::new(0.0, 0.0),
                Vec2::new(24.0, 0.0),
                Vec2::new(24.0, 24.0),
                Vec2::new(0.0, 24.0),
            ]
            .into_iter()
            .map(|point| Anchor {
                point,
                in_handle: None,
                out_handle: None,
            })
            .collect(),
            closed: true,
        };
        let vector = q0s_format::v2::VectorAsset {
            asset_id: 88,
            paths: vec![path.clone()],
            fill: Some(Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        };
        let appearance = VectorAppearance {
            material: q0s_format::v2::VectorMaterial::SoftHalo {
                radius: 8.0,
                opacity: 0.65,
            },
            erase_mask: Vec::new(),
            material_source: vec![path],
            clip_mask: Vec::new(),
            field_transform: Affine::IDENTITY,
            fill_gradient: None,
        };
        let ctx = Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 480.0));
        let mut cache = TextureCache::default();
        let warm_view = StageView {
            origin: Pos2::new(40.0, 30.0),
            scale: 1.0,
            stage_rect: rect,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let paint_once = |cache: &mut TextureCache, id: usize| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new(("halo-async-warm", id)),
                    ));
                    paint_vector_appearance_halo(
                        &painter,
                        &vector,
                        &appearance,
                        Affine::IDENTITY,
                        &warm_view,
                        cache,
                        ctx,
                        Color32::WHITE,
                    );
                },
            );
        };
        paint_once(&mut cache, 0);
        assert_eq!(
            cache
                .appearance_by_asset
                .keys()
                .filter(|(asset_id, _, _)| *asset_id == vector.asset_id)
                .count(),
            0,
            "first halo paint must not block the UI thread on CPU rasterization",
        );
        assert_eq!(
            cache
                .appearance_raster_jobs
                .keys()
                .filter(|(asset_id, _, _)| *asset_id == vector.asset_id)
                .count(),
            1,
            "first halo paint must schedule exactly one background raster job",
        );
        for attempt in 1..=100 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            paint_once(&mut cache, attempt);
            if cache
                .appearance_by_asset
                .keys()
                .any(|(asset_id, _, _)| *asset_id == vector.asset_id)
            {
                break;
            }
        }
        assert_eq!(
            cache
                .appearance_by_asset
                .keys()
                .filter(|(asset_id, _, _)| *asset_id == vector.asset_id)
                .count(),
            1,
            "background halo raster never reached the texture cache",
        );
        assert!(
            cache
                .appearance_raster_jobs
                .keys()
                .all(|(asset_id, _, _)| *asset_id != vector.asset_id),
            "completed halo job was not retired",
        );

        for (index, scale) in [0.25_f32, 0.5, 1.0, 2.0, 4.0, 8.0].into_iter().enumerate() {
            let view = StageView {
                origin: Pos2::new(40.0, 30.0),
                scale,
                stage_rect: rect,
                edit_transform: q0s_format::transform::Affine::IDENTITY,
            };
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new(("halo-view-cache", index)),
                    ));
                    paint_vector_appearance_halo(
                        &painter,
                        &vector,
                        &appearance,
                        Affine::IDENTITY,
                        &view,
                        &mut cache,
                        ctx,
                        Color32::WHITE,
                    );
                },
            );
            assert_eq!(
                cache
                    .appearance_by_asset
                    .keys()
                    .filter(|(asset_id, _, _)| *asset_id == vector.asset_id)
                    .count(),
                1,
                "viewport zoom created another CPU halo raster bucket",
            );
        }

        let scaled = Affine {
            a11: 3.0,
            a22: 3.0,
            tx: 120.0,
            ty: -40.0,
            ..Affine::IDENTITY
        };
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 3.0,
            stage_rect: rect,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("halo-affine-cache"),
                ));
                paint_vector_appearance_halo(
                    &painter,
                    &vector,
                    &appearance,
                    scaled,
                    &view,
                    &mut cache,
                    ctx,
                    Color32::WHITE,
                );
            },
        );
        let entries: Vec<_> = cache
            .appearance_by_asset
            .iter()
            .filter(|((asset_id, _, _), _)| *asset_id == vector.asset_id)
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "affine scale rebuilt canonical halo texture"
        );
        assert_eq!(entries[0].0 .1, 8);
        assert_eq!(entries[0].1.pixels_per_unit, 2.0);
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn per_asset_invalidation_preserves_unrelated_fragment_caches() {
        let mut cache = TextureCache::default();
        cache
            .appearance_signature_by_asset
            .insert(1, (11, Vec2::new(1.0, 2.0)));
        cache
            .appearance_signature_by_asset
            .insert(2, (22, Vec2::new(3.0, 4.0)));
        cache
            .visible_body_by_asset
            .insert(1, vec![vec![Vec2::new(1.0, 1.0)]]);
        cache
            .visible_body_by_asset
            .insert(2, vec![vec![Vec2::new(2.0, 2.0)]]);

        cache.invalidate_asset(1);

        assert!(!cache.appearance_signature_by_asset.contains_key(&1));
        assert!(!cache.visible_body_by_asset.contains_key(&1));
        assert_eq!(
            cache.appearance_signature_by_asset.get(&2),
            Some(&(22, Vec2::new(3.0, 4.0))),
        );
        assert_eq!(
            cache.visible_body_by_asset.get(&2),
            Some(&vec![vec![Vec2::new(2.0, 2.0)]]),
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn texture_cache_hashes_dense_appearance_only_once_until_invalidation() {
        let path = VPath {
            anchors: (0..4096)
                .map(|index| Anchor {
                    point: Vec2::new(index as f32 * 0.25, (index % 23) as f32),
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
            closed: true,
        };
        let vector = q0s_format::v2::VectorAsset {
            asset_id: 77,
            paths: vec![path.clone()],
            fill: Some(Rgba {
                r: 10,
                g: 20,
                b: 30,
                a: 255,
            }),
            stroke: None,
        };
        let appearance = VectorAppearance {
            material: q0s_format::v2::VectorMaterial::SoftHalo {
                radius: 18.0,
                opacity: 0.7,
            },
            erase_mask: Vec::new(),
            material_source: vec![path],
            clip_mask: Vec::new(),
            field_transform: Affine::IDENTITY,
            fill_gradient: None,
        };
        let mut cache = TextureCache::default();
        let first = cache.appearance_signature(&vector, &appearance);
        let mut moved = appearance.clone();
        moved.field_transform.tx = 300.0;
        moved.field_transform.ty = -125.0;
        let second = cache.appearance_signature(&vector, &moved);
        assert_eq!(first, second);
        assert_eq!(
            cache.appearance_signature_build_count, 1,
            "stationary/drag repaint rehashed every Advanced material anchor",
        );
        cache.invalidate();
        let _ = cache.appearance_signature(&vector, &moved);
        assert_eq!(cache.appearance_signature_build_count, 2);
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn fragmented_body_cache_survives_affine_move_without_rebuilding_booleans() {
        let path = VPath {
            anchors: [
                Vec2::new(0.0, 0.0),
                Vec2::new(20.0, 0.0),
                Vec2::new(20.0, 20.0),
                Vec2::new(0.0, 20.0),
            ]
            .into_iter()
            .map(|point| Anchor {
                point,
                in_handle: None,
                out_handle: None,
            })
            .collect(),
            closed: true,
        };
        let mut vector = q0s_format::v2::VectorAsset {
            asset_id: 91,
            paths: vec![path.clone()],
            fill: Some(Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        };
        let mut appearance = VectorAppearance {
            material: q0s_format::v2::VectorMaterial::SoftHalo {
                radius: 8.0,
                opacity: 0.5,
            },
            erase_mask: vec![VPath {
                anchors: [
                    Vec2::new(2.0, 2.0),
                    Vec2::new(4.0, 2.0),
                    Vec2::new(4.0, 4.0),
                    Vec2::new(2.0, 4.0),
                ]
                .into_iter()
                .map(|point| Anchor {
                    point,
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
                closed: true,
            }],
            material_source: vec![path],
            clip_mask: vec![VPath {
                anchors: [
                    Vec2::new(-2.0, -2.0),
                    Vec2::new(15.0, -2.0),
                    Vec2::new(15.0, 22.0),
                    Vec2::new(-2.0, 22.0),
                ]
                .into_iter()
                .map(|point| Anchor {
                    point,
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
                closed: true,
            }],
            field_transform: Affine::IDENTITY,
            fill_gradient: None,
        };
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(200.0, 200.0)),
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let mut cache = TextureCache::default();
        let first =
            masked_vector_body_contours(&vector, &appearance, Affine::IDENTITY, &view, &mut cache);
        assert_eq!(cache.visible_body_build_count, 1);

        let delta = Vec2::new(37.0, 19.0);
        for path in &mut vector.paths {
            for anchor in &mut path.anchors {
                anchor.point.x += delta.x;
                anchor.point.y += delta.y;
                if let Some(point) = &mut anchor.in_handle {
                    point.x += delta.x;
                    point.y += delta.y;
                }
                if let Some(point) = &mut anchor.out_handle {
                    point.x += delta.x;
                    point.y += delta.y;
                }
            }
        }
        appearance.field_transform.tx += delta.x;
        appearance.field_transform.ty += delta.y;
        let moved =
            masked_vector_body_contours(&vector, &appearance, Affine::IDENTITY, &view, &mut cache);
        assert_eq!(
            cache.visible_body_build_count, 1,
            "pure affine movement must reuse canonical fragment body cache",
        );
        assert_eq!(first.len(), moved.len());
        for (before, after) in first.iter().flatten().zip(moved.iter().flatten()) {
            assert!((after.x - before.x - delta.x).abs() < 1.0e-3);
            assert!((after.y - before.y - delta.y).abs() < 1.0e-3);
        }
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn vector_body_obeys_post_material_clip_even_without_erase_mask() {
        let vector = q0s_format::v2::VectorAsset {
            asset_id: 1,
            paths: vec![VPath {
                anchors: [
                    Vec2::new(0.0, 0.0),
                    Vec2::new(20.0, 0.0),
                    Vec2::new(20.0, 20.0),
                    Vec2::new(0.0, 20.0),
                ]
                .into_iter()
                .map(|point| Anchor {
                    point,
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
                closed: true,
            }],
            fill: Some(Rgba {
                r: 10,
                g: 20,
                b: 30,
                a: 255,
            }),
            stroke: None,
        };
        let appearance = VectorAppearance {
            material: q0s_format::v2::VectorMaterial::SoftHalo {
                radius: 8.0,
                opacity: 0.5,
            },
            erase_mask: Vec::new(),
            material_source: vector.paths.clone(),
            clip_mask: vec![VPath {
                anchors: [
                    Vec2::new(10.0, -20.0),
                    Vec2::new(40.0, -20.0),
                    Vec2::new(40.0, 40.0),
                    Vec2::new(10.0, 40.0),
                ]
                .into_iter()
                .map(|point| Anchor {
                    point,
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
                closed: true,
            }],
            field_transform: Affine::IDENTITY,
            fill_gradient: None,
        };
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0)),
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let mut cache = TextureCache::default();
        let contours =
            masked_vector_body_contours(&vector, &appearance, Affine::IDENTITY, &view, &mut cache);
        let min_x = contours
            .iter()
            .flatten()
            .map(|point| point.x)
            .fold(f32::INFINITY, f32::min);
        let max_x = contours
            .iter()
            .flatten()
            .map(|point| point.x)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            min_x >= 9.9,
            "body must be clipped at the post-material boundary: {min_x}"
        );
        assert!(max_x <= 20.1);
    }

    #[test]
    fn layer_keyframe_replaces_the_previous_complete_contents() {
        let layer = Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "layer".into(),
            explicit_keyframes: Vec::new(),
            placements: vec![
                test_placement(0, 1, 0.0, Tween::None),
                test_placement(0, 2, 0.0, Tween::None),
                test_placement(5, 3, 0.0, Tween::None),
            ],
        };
        assert_eq!(
            active_placements_at(&layer, 4)
                .into_iter()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            active_placements_at(&layer, 5)
                .into_iter()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(
            active_placements_at(&layer, 9)
                .into_iter()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn blank_keyframe_stops_the_previous_layer_hold() {
        let layer = Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "layer".into(),
            explicit_keyframes: vec![3],
            placements: vec![
                test_placement(0, 1, 0.0, Tween::None),
                test_placement(6, 2, 0.0, Tween::None),
            ],
        };

        assert_eq!(active_placements_at(&layer, 2).len(), 1);
        assert!(active_placements_at(&layer, 3).is_empty());
        assert!(active_placements_at(&layer, 5).is_empty());
        assert_eq!(active_placements_at(&layer, 6).len(), 1);
    }

    #[test]
    fn duplicate_asset_instances_survive_inside_one_layer_keyframe() {
        let layer = Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "layer".into(),
            explicit_keyframes: Vec::new(),
            placements: vec![
                test_placement(0, 1, 0.0, Tween::None),
                test_placement(0, 1, 20.0, Tween::None),
            ],
        };
        assert_eq!(active_placements_at(&layer, 3).len(), 2);
    }

    #[test]
    fn duplicate_target_tweens_match_by_occurrence_order() {
        let layer = Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "layer".into(),
            explicit_keyframes: Vec::new(),
            placements: vec![
                test_placement(0, 1, 0.0, Tween::Linear { to_frame: 10 }),
                test_placement(0, 1, 100.0, Tween::Linear { to_frame: 10 }),
                test_placement(10, 1, 10.0, Tween::None),
                test_placement(10, 1, 200.0, Tween::None),
            ],
        };
        let active = active_placements_at(&layer, 5);
        assert_eq!(active.len(), 2);
        assert!((active[0].1.tx - 5.0).abs() < 1.0e-5);
        assert!((active[1].1.tx - 150.0).abs() < 1.0e-5);
    }

    #[test]
    fn eased_tween_changes_the_resolved_transform() {
        use q0s_format::v2::{Easing, EasingFamily, EasingMode};

        let layer = Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "layer".into(),
            explicit_keyframes: Vec::new(),
            placements: vec![
                test_placement(
                    0,
                    1,
                    0.0,
                    Tween::Eased {
                        to_frame: 10,
                        easing: Easing::Preset {
                            family: EasingFamily::Quad,
                            mode: EasingMode::In,
                        },
                    },
                ),
                test_placement(10, 1, 100.0, Tween::None),
            ],
        };

        let active = active_placements_at(&layer, 5);
        assert_eq!(active.len(), 1);
        assert!((active[0].1.tx - 25.0).abs() < 1.0e-4);
    }

    #[test]
    fn display_outline_cleanup_removes_zoom_collapsed_flying_spike() {
        let points = [
            pos2(0.0, 0.0),
            pos2(0.01, 0.0),
            pos2(80.0, 120.0),
            pos2(0.02, 0.0),
            pos2(0.20, 0.0),
        ];
        let cleaned = sanitize_display_polyline(&points, 2.0, false);
        assert_eq!(cleaned, vec![pos2(0.0, 0.0), pos2(0.20, 0.0)]);
    }

    #[test]
    fn preview_cleanup_removes_subpixel_duplicates_and_hairpins() {
        let points = [
            pos2(0.0, 0.0),
            pos2(0.01, 0.0),
            pos2(0.10, 0.0),
            pos2(0.01, 0.0),
            pos2(0.20, 0.0),
        ];
        let cleaned = sanitize_preview_polyline(&points, 10.0);
        assert_eq!(cleaned, vec![pos2(0.0, 0.0), pos2(0.20, 0.0)]);
    }

    #[test]
    fn very_slow_preview_is_one_bounded_surface_without_flying_spikes() {
        let points: Vec<Pos2> = (0..40_000)
            .map(|index| {
                let x = index as f32 * 0.025;
                let y = ((index % 11) as f32 - 5.0) * 0.002;
                pos2(x, y)
            })
            .collect();
        let contours = round_stroke_preview_contours(&points, 10.0).expect("preview contours");
        assert_eq!(contours.len(), 1, "straight gesture must be one surface");
        let buffers = tessellate_round_stroke(&points, 10.0).expect("preview mesh");
        assert!(buffers
            .indices
            .iter()
            .all(|index| (*index as usize) < buffers.vertices.len()));
        assert!(buffers
            .vertices
            .iter()
            .all(|point| point.x.is_finite() && point.y.is_finite()));
        let min_x = buffers
            .vertices
            .iter()
            .map(|point| point.x)
            .fold(f32::INFINITY, f32::min);
        let max_x = buffers
            .vertices
            .iter()
            .map(|point| point.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = buffers
            .vertices
            .iter()
            .map(|point| point.y)
            .fold(f32::INFINITY, f32::min);
        let max_y = buffers
            .vertices
            .iter()
            .map(|point| point.y)
            .fold(f32::NEG_INFINITY, f32::max);
        let last_x = points.last().expect("last point").x;
        assert!(min_x >= -5.5, "preview spike at x={min_x}");
        assert!(max_x <= last_x + 5.5, "preview spike at x={max_x}");
        assert!(min_y >= -5.5, "preview spike at y={min_y}");
        assert!(max_y <= 5.5, "preview spike at y={max_y}");
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn fixed_selection_outline_encoding_preserves_exact_lyon_vertices() {
        let points = vec![
            pos2(3.0, 5.0),
            pos2(47.0, 8.0),
            pos2(51.0, 31.0),
            pos2(26.0, 49.0),
            pos2(7.0, 35.0),
        ];
        let color = Color32::from_rgba_unmultiplied(17, 91, 203, 230);
        let exact = closed_bevel_stroke_mesh(&points, 2.0, color).expect("exact lyon outline");
        let encoded = fixed_screen_stroke_mesh(&points, 2.0, color)
            .expect("outline should support affine reprojection");

        assert_eq!(encoded.indices, exact.indices, "triangle topology changed");
        assert_eq!(encoded.vertices.len(), exact.vertices.len());
        for (encoded, exact) in encoded.vertices.iter().zip(&exact.vertices) {
            assert_eq!(
                encoded.pos, exact.pos,
                "reference-pose outline vertex moved"
            );
            assert_eq!(encoded.color, exact.color);
        }
        assert!(
            encoded
                .vertices
                .iter()
                .any(|vertex| vertex.uv != Pos2::ZERO),
            "fixed-pixel extrusion metadata was not encoded",
        );
    }

    #[test]
    fn flatten_two_anchor_straight_segment() {
        let path = VPath {
            anchors: vec![
                Anchor {
                    point: Vec2::new(0.0, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(10.0, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
            closed: false,
        };
        let pts = flatten_path(&path);
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].x, 0.0);
        assert_eq!(pts[1].x, 10.0);
    }

    #[test]
    fn flatten_curved_segment_produces_samples() {
        let path = VPath {
            anchors: vec![
                Anchor {
                    point: Vec2::new(0.0, 0.0),
                    in_handle: None,
                    out_handle: Some(Vec2::new(0.0, 10.0)),
                },
                Anchor {
                    point: Vec2::new(10.0, 0.0),
                    in_handle: Some(Vec2::new(10.0, 10.0)),
                    out_handle: None,
                },
            ],
            closed: false,
        };
        let pts = flatten_path(&path);
        // 1 anchor + BEZIER_SAMPLES_PER_SEGMENT samples
        assert_eq!(pts.len(), 1 + BEZIER_SAMPLES_PER_SEGMENT);
        // Curve bows down (y > 0 in middle)
        let mid = pts[pts.len() / 2];
        assert!(mid.y > 1.0, "curve mid y should bow upward, got {mid:?}");
    }

    #[test]
    fn flatten_closed_triangle_walks_three_segments() {
        let path = VPath {
            anchors: vec![
                Anchor {
                    point: Vec2::new(0.0, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(10.0, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(5.0, 10.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
            closed: true,
        };
        let pts = flatten_path(&path);
        assert_eq!(pts.len(), 4);
    }

    #[test]
    fn closed_stroke_polyline_has_no_duplicate_seam_point() {
        let path = VPath {
            anchors: vec![
                Anchor {
                    point: Vec2::new(0.0, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(20.0, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(20.0, 10.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(0.0, 10.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
            closed: true,
        };
        let flattened = flatten_path(&path);
        assert_eq!(flattened.len(), 5);
        assert_eq!(flattened.first(), flattened.last());

        let stroke = flatten_path_for_stroke(&path);
        assert_eq!(stroke.len(), 4);
        assert_ne!(stroke.first(), stroke.last());
        assert_eq!(stroke[0], Vec2::new(0.0, 0.0));
        assert_eq!(stroke[3], Vec2::new(0.0, 10.0));
    }

    #[test]
    fn open_stroke_polyline_keeps_both_endpoints() {
        let path = VPath {
            anchors: vec![
                Anchor {
                    point: Vec2::new(0.0, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(10.0, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
            closed: false,
        };
        assert_eq!(flatten_path_for_stroke(&path), flatten_path(&path));
    }

    #[test]
    fn lerp_transform_midpoint() {
        let a = Transform2D::IDENTITY;
        let b = Transform2D {
            tx: 100.0,
            ty: 50.0,
            sx: 2.0,
            sy: 2.0,
            rotation: 1.0,
            ..Transform2D::IDENTITY
        };
        let mid = lerp_transform(a, b, 0.5);
        assert_eq!(mid.tx, 50.0);
        assert_eq!(mid.ty, 25.0);
        assert_eq!(mid.sx, 1.5);
        assert_eq!(mid.rotation, 0.5);
    }

    #[test]
    fn render_lookup_resolves_rig_from_cached_index_without_project_scan() {
        let mut project = crate::state::default_project();
        for asset_id in 10_000..10_200 {
            project.assets.push(Asset::Vector(VectorAsset {
                asset_id,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            }));
        }
        project.assets.push(Asset::Rig(q0s_format::v2::RigAsset {
            asset_id: 60_000,
            owner_q0rg_id: 1,
            nodes: Vec::new(),
            controls: Vec::new(),
            constraints: Vec::new(),
            channels: Vec::new(),
            drivers: Vec::new(),
            poses: Vec::new(),
            deformers: Vec::new(),
            pose_drivers: Vec::new(),
            mirror_pairs: Vec::new(),
            variants: Vec::new(),
        }));
        let index = Arc::new(RenderIndex::new(&project));
        let lookup = RenderLookup::new(&project, index, true);
        assert_eq!(lookup.rig(1).map(|rig| rig.asset_id), Some(60_000));
        assert!(lookup.rig(999).is_none());
    }

    #[test]
    fn render_index_reuses_identity_tables_until_id_layout_changes() {
        let mut project = crate::state::default_project();
        let mut textures = TextureCache::default();
        let first = textures.render_index(&project, None);
        let second = textures.render_index(&project, None);
        assert!(Arc::ptr_eq(&first, &second));

        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 65000,
            paths: Vec::new(),
            fill: None,
            stroke: None,
        }));
        let changed = textures.render_index(&project, None);
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(changed.assets.contains_key(&65000));
    }

    #[test]
    fn scoped_render_index_rebuilds_after_same_length_id_edits() {
        let mut project = crate::state::default_project();
        let mut textures = TextureCache::default();
        let original = textures.render_index(&project, Some((44, 1)));
        project.q0rgs[0].q0rg_id = 2;
        let changed = textures.render_index(&project, Some((44, 2)));
        assert!(!Arc::ptr_eq(&original, &changed));
        let lookup = RenderLookup::new(&project, Arc::clone(&changed), false);
        assert!(lookup.q0rg(2).is_some());
        assert!(lookup.q0rg(1).is_none());
    }

    #[test]
    fn incremental_transform_sync_advances_fx_snapshot_for_live_drag() {
        let mut project = crate::state::default_project();
        project.q0rgs[0].layers[0].push_placement(Placement {
            instance_id: 5,
            frame: 0,
            target: Target::Asset(99),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: PlacementFx {
                opacity: 0.5,
                ..PlacementFx::default()
            },
        });
        let mut textures = TextureCache::default();
        let revision = (8, 3);
        sync_fx_project_snapshot(&project, &mut textures, Some(revision));
        assert!(textures.fx_project_snapshot.is_some());

        project.q0rgs[0].layers[0].placements[0].transform.tx = 12.0;
        textures.placement_transforms_changed(&project, revision, &[(1, 1, 0)]);

        assert_eq!(textures.fx_project_revision, Some(revision));
        assert_eq!(
            textures.fx_project_snapshot.as_ref().unwrap().q0rgs[0].layers[0].placements[0]
                .transform
                .tx,
            12.0
        );
    }

    #[test]
    fn immutable_vector_stroke_tessellation_is_cached_once_per_asset() {
        let vector = VectorAsset {
            asset_id: 64001,
            paths: vec![VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(2.85, 16.10),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(2.85, 16.00),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(2.80, 15.95),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(4.55, 16.10),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: false,
            }],
            fill: None,
            stroke: Some(q0s_format::v2::Stroke {
                color: Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                width: 1.0,
                cap: q0s_format::geom::CapShape::Round,
            }),
        };
        let mut cache = TextureCache::default();
        {
            let geometry = cache.vector_render_geometry(&vector);
            assert!(!geometry.stroke_vertices.is_empty());
            assert!(!geometry.stroke_indices.is_empty());
        }
        assert_eq!(cache.vector_render_geometry_build_count(), 1);
        {
            let geometry = cache.vector_render_geometry(&vector);
            assert!(!geometry.stroke_vertices.is_empty());
        }
        assert_eq!(
            cache.vector_render_geometry_build_count(),
            1,
            "immutable stroke geometry must not be lyon-tessellated again on repaint",
        );
    }

    #[test]
    fn open_bounded_vector_stroke_cannot_grow_a_flash_hairpin_spike() {
        let points = vec![
            pos2(2.85, 16.10),
            pos2(2.85, 16.00),
            pos2(2.80, 15.95),
            pos2(2.80, 15.90),
            pos2(2.70, 15.55),
            pos2(4.55, 16.10),
        ];
        let buffers = tessellate_bounded_stroke(&points, 1.0, false).expect("bounded stroke");
        let min_x = buffers
            .vertices
            .iter()
            .map(|p| p.x)
            .fold(f32::INFINITY, f32::min);
        let max_x = buffers
            .vertices
            .iter()
            .map(|p| p.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = buffers
            .vertices
            .iter()
            .map(|p| p.y)
            .fold(f32::INFINITY, f32::min);
        let max_y = buffers
            .vertices
            .iter()
            .map(|p| p.y)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            min_x > 2.0 && max_x < 5.2,
            "unexpected x spike: {min_x}..{max_x}"
        );
        assert!(
            min_y > 14.8 && max_y < 16.8,
            "unexpected y spike: {min_y}..{max_y}"
        );
    }

    #[test]
    fn closed_bevel_selection_stroke_cannot_grow_unbounded_miter_spikes() {
        // Nearly reversing at a subpixel offset is exactly where egui's closed
        // miter join can explode. A bevel stroke must remain inside the source
        // bounds expanded by half the line width.
        let points = [
            pos2(0.0, 0.0),
            pos2(100.0, 0.0),
            pos2(0.01, 0.001),
            pos2(100.0, 1.0),
            pos2(0.0, 1.0),
        ];
        let buffers = tessellate_closed_bevel_stroke(&points, 2.0).expect("bevel stroke");
        assert!(!buffers.vertices.is_empty());
        for vertex in &buffers.vertices {
            assert!(
                vertex.x >= -1.01 && vertex.x <= 101.01 && vertex.y >= -1.01 && vertex.y <= 2.01,
                "bevel selection stroke escaped bounded source envelope: {vertex:?}",
            );
        }
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn lazy_parametric_selection_cache_preserves_lyon_across_topology_change() {
        let points = vec![
            pos2(0.0, 0.0),
            pos2(100.0, 0.0),
            pos2(0.01, 0.001),
            pos2(100.0, 1.0),
            pos2(0.0, 1.0),
        ];
        let mut stroke = CachedSelectionStroke {
            contour_stage: points,
            parametric: Vec::new(),
        };
        for width in [20.0_f32, 4.0, 2.0, 1.0, 0.9, 0.5, 0.25, 0.0625, 0.03125] {
            assert!(
                cache_parametric_range_for_width(&mut stroke, width),
                "width {width} should gain a cached parametric range",
            );
        }
        assert!(
            stroke.parametric.len() >= 2,
            "the hairpin must keep distinct topology ranges",
        );
        for range in &stroke.parametric {
            let probe_width = (range.min_width * range.max_width).sqrt();
            let exact = tessellate_closed_bevel_stroke(&stroke.contour_stage, probe_width)
                .expect("exact bevel");
            assert_eq!(exact.indices, range.indices);
            assert_eq!(exact.vertices.len(), range.base_vertices.len());
            let tolerance = 1.0e-3 * probe_width.max(1.0);
            for (index, vertex) in exact.vertices.iter().enumerate() {
                let cached = range.base_vertices[index] + range.width_vectors[index] * probe_width;
                assert!(
                    (cached.x - vertex.x).abs() <= tolerance
                        && (cached.y - vertex.y).abs() <= tolerance,
                    "cached range drifted from lyon at width {probe_width}, vertex {index}",
                );
            }
        }
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn advanced_selection_neighbor_zoom_reuses_cached_bevel_ranges() {
        let mut stroke = CachedSelectionStroke {
            contour_stage: vec![
                pos2(0.0, 0.0),
                pos2(100.0, 0.0),
                pos2(0.01, 0.001),
                pos2(100.0, 1.0),
                pos2(0.0, 1.0),
            ],
            parametric: Vec::new(),
        };
        assert!(cache_parametric_range_for_width(&mut stroke, 2.0));
        let cached_after_first_zoom = stroke.parametric.len();
        for width in [1.8_f32, 1.6, 1.4, 1.2, 1.05] {
            assert!(
                stroke
                    .parametric
                    .iter()
                    .any(|range| width >= range.min_width && width <= range.max_width),
                "neighbor width {width} unexpectedly needs lyon again",
            );
        }
        assert_eq!(stroke.parametric.len(), cached_after_first_zoom);

        assert!(cache_parametric_range_for_width(&mut stroke, 0.95));
        let cached_after_crossing = stroke.parametric.len();
        for width in [0.9_f32, 0.8, 0.7, 0.6, 0.5] {
            assert!(
                stroke
                    .parametric
                    .iter()
                    .any(|range| width >= range.min_width && width <= range.max_width),
                "post-transition width {width} unexpectedly needs lyon again",
            );
        }
        assert_eq!(stroke.parametric.len(), cached_after_crossing);
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn adaptive_selection_bezier_outline_preserves_anchors_without_fixed_16x_zoom_cost() {
        let anchors: Vec<Anchor> = (0..512)
            .map(|index| {
                let x = index as f32 * 2.0;
                let y = ((index % 7) as f32 - 3.0) * 0.2;
                Anchor {
                    point: Vec2::new(x, y),
                    in_handle: Some(Vec2::new(x - 0.5, y - 0.05)),
                    out_handle: Some(Vec2::new(x + 0.5, y + 0.05)),
                }
            })
            .collect();
        let path = VPath {
            anchors: anchors.clone(),
            closed: true,
        };
        let view = StageView {
            origin: Pos2::new(13.0, 17.0),
            scale: 0.1,
            stage_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0)),
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let outline = adaptive_selection_bezier_screen_path(&path, Affine::IDENTITY, &view);
        let fixed = flatten_path(&path);
        assert!(
            outline.len() * 4 < fixed.len(),
            "low-zoom selection still paid nearly the fixed 16-samples-per-segment cost: adaptive={} fixed={}",
            outline.len(),
            fixed.len(),
        );
        for anchor in &anchors {
            let expected = view.stage_to_screen(anchor.point);
            assert!(
                outline
                    .iter()
                    .any(|point| point.distance(expected) <= 1.0e-4),
                "adaptive outline skipped a source anchor/segment boundary at {expected:?}",
            );
        }
    }

    #[test]
    fn apply_translates_then_rotates() {
        let t = Transform2D {
            tx: 100.0,
            ..Transform2D::IDENTITY
        };
        let p = apply(t, Vec2::new(5.0, 0.0));
        assert!((p.x - 105.0).abs() < 1e-5);
        assert!(p.y.abs() < 1e-5);
    }

    #[test]
    fn triangulates_concave_brush_like_polygon_without_triangle_fan() {
        // A concave C-shaped contour: egui PathShape cannot fill this, but
        // ear clipping must cover it with n-2 local triangles.
        let points = vec![
            pos2(0.0, 0.0),
            pos2(12.0, 0.0),
            pos2(12.0, 3.0),
            pos2(4.0, 3.0),
            pos2(4.0, 9.0),
            pos2(12.0, 9.0),
            pos2(12.0, 12.0),
            pos2(0.0, 12.0),
        ];
        let triangles = triangulate_polygon(&points).expect("concave triangulation");
        assert_eq!(triangles.len(), (points.len() - 2) * 3);

        let triangle_area: f32 = triangles
            .chunks_exact(3)
            .map(|triangle| {
                cross_2d(
                    points[triangle[0] as usize],
                    points[triangle[1] as usize],
                    points[triangle[2] as usize],
                )
                .abs()
                    * 0.5
            })
            .sum();
        let indices: Vec<usize> = (0..points.len()).collect();
        let polygon_area = polygon_signed_area_indexed(&points, &indices).abs();
        assert!((triangle_area - polygon_area).abs() < 1.0e-3);
    }

    #[test]
    fn triangulates_real_brush_outline() {
        let centreline = vec![
            Vec2::new(10.0, 10.0),
            Vec2::new(30.0, 8.0),
            Vec2::new(45.0, 18.0),
            Vec2::new(38.0, 35.0),
            Vec2::new(20.0, 42.0),
            Vec2::new(8.0, 32.0),
        ];
        let outline = q0s_format::geom::brush_outline_with_caps(
            &centreline,
            4.0,
            8,
            q0s_format::geom::CapShape::Round,
        );
        let screen: Vec<Pos2> = outline.iter().map(|point| pos2(point.x, point.y)).collect();
        let triangles = triangulate_polygon(&screen).expect("real brush outline triangulation");
        assert!(!triangles.is_empty());
    }

    #[test]
    fn lyon_fill_keeps_self_intersecting_shape_filled() {
        let bow_tie = vec![
            pos2(0.0, 0.0),
            pos2(30.0, 30.0),
            pos2(0.0, 30.0),
            pos2(30.0, 0.0),
        ];
        let buffers = tessellate_complex_fill(&[bow_tie]).expect("self-intersection tessellation");
        assert!(!buffers.indices.is_empty());
    }
}
