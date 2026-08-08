use std::collections::HashMap;
#[cfg(feature = "appearance-mask-eraser")]
use std::hash::{Hash, Hasher};

use egui::epaint::{PathShape, Vertex};
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
    FillOptions, FillRule as LyonFillRule, FillTessellator, LineJoin, StrokeOptions,
    StrokeTessellator,
};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Anchor, Asset, Path as VPath, Placement, ProjectV2, Rgba, Target, Transform2D, Tween, Vec2,
    VectorAppearance,
};

const Q0RG_RECURSION_LIMIT: u8 = 8;
const BEZIER_SAMPLES_PER_SEGMENT: usize = 16;

pub struct StageView {
    /// Top-left corner of the stage rectangle in screen pixels.
    pub origin: Pos2,
    /// pixels-per-stage-unit; same on x and y (square pixels).
    pub scale: f32,
    /// Stage rect in screen pixels, used to clip if desired.
    pub stage_rect: Rect,
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
    pub view_origin_bits: [u32; 2],
    pub view_scale_bits: u32,
    pub accent_rgba: [u8; 4],
    pub texture_id: TextureId,
}

#[cfg(feature = "appearance-mask-eraser")]
#[derive(Clone, Default)]
pub(crate) struct CachedSelectionPaint {
    pub meshes: Vec<Mesh>,
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

#[derive(Default)]
pub struct TextureCache {
    by_asset_id: HashMap<u16, TextureHandle>,
    by_q0v_frame: HashMap<(u16, u32), TextureHandle>,
    q0v_media: HashMap<u16, q0video::q0v::Q0vFile>,
    #[cfg(feature = "appearance-mask-eraser")]
    appearance_by_asset: HashMap<(u16, u16, u64), CachedAppearanceTexture>,
    #[cfg(feature = "appearance-mask-eraser")]
    appearance_signature_by_asset: HashMap<u16, (u64, Vec2)>,
    #[cfg(feature = "appearance-mask-eraser")]
    visible_body_by_asset: HashMap<u16, Vec<Vec<Vec2>>>,
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
}

impl TextureCache {
    pub fn invalidate(&mut self) {
        self.by_asset_id.clear();
        self.by_q0v_frame.clear();
        self.q0v_media.clear();
        #[cfg(feature = "appearance-mask-eraser")]
        {
            self.appearance_by_asset.clear();
            self.appearance_signature_by_asset.clear();
            self.visible_body_by_asset.clear();
            self.visible_selection_body_by_asset.clear();
            self.appearance_hit_by_asset.clear();
            self.raw_selection_components_by_key.clear();
            self.selection_source_surface_by_key.clear();
            self.selection_geometry_by_key.clear();
            self.selection_stage_mesh_by_key.clear();
            self.selection_paint_by_key.clear();
        }
    }

    /// Drop cached render data only for assets whose model data actually changed.
    /// Brush commits used to clear every halo/body texture in the project, so the
    /// Nth Advanced stroke forced all previous strokes through raster/boolean work
    /// again. Unrelated assets are immutable and can safely keep their caches.
    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn invalidate_asset(&mut self, asset_id: u16) {
        self.by_asset_id.remove(&asset_id);
        self.by_q0v_frame.retain(|(id, _), _| *id != asset_id);
        self.q0v_media.remove(&asset_id);
        #[cfg(feature = "appearance-mask-eraser")]
        {
            self.appearance_by_asset
                .retain(|(id, _, _), _| *id != asset_id);
            self.appearance_signature_by_asset.remove(&asset_id);
            self.visible_body_by_asset.remove(&asset_id);
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
        }
    }

    #[cfg(feature = "appearance-mask-eraser")]
    pub(crate) fn invalidate_assets(&mut self, asset_ids: impl IntoIterator<Item = u16>) {
        for asset_id in asset_ids {
            self.invalidate_asset(asset_id);
        }
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
            let source = crate::brush::vector_fill_geometry(vector);
            let mut components = Vec::with_capacity(source.0.len());
            let mut remembered = Vec::new();
            for polygon in source.0 {
                let mut path_indices: Vec<usize> = vector
                    .paths
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| path.closed)
                    .filter_map(|(index, path)| {
                        path.anchors
                            .iter()
                            .any(|anchor| polygon_boundary_near_point(&polygon, anchor.point, 0.5))
                            .then_some(index)
                    })
                    .collect();
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
            let stage_contours: Vec<Vec<Pos2>> = if appearance.erase_mask.is_empty()
                && appearance.clip_mask.is_empty()
            {
                path_indices
                    .iter()
                    .filter_map(|index| vector.paths.get(*index))
                    .map(flatten_path)
                    .filter(|contour| contour.len() >= 3)
                    .map(|contour| {
                        contour
                            .into_iter()
                            .map(|point| pos2(point.x, point.y))
                            .collect()
                    })
                    .collect()
            } else {
                let canonical = {
                    let geometry = self.selection_geometry(vector, appearance, &path_indices)?;
                    if geometry.body.is_empty() {
                        geometry.fallback_material_support.clone()
                    } else {
                        geometry.body.clone()
                    }
                };
                canonical
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
                    .collect()
            };
            let stage_mesh = build_selection_stage_mesh(&stage_contours)?;
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
    pub(crate) fn selection_screen_meshes(
        source: &mut CachedSelectionStageMesh,
        view: &StageView,
        texture_id: TextureId,
        tile_size_points: f32,
        accent: Color32,
    ) -> Vec<Mesh> {
        build_selection_screen_meshes(source, view, texture_id, tile_size_points, accent)
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
                || existing.field_transform_bits != key.field_transform_bits
                || existing.accent_rgba != key.accent_rgba
                || existing.texture_id != key.texture_id
                || (existing.view_origin_bits == key.view_origin_bits
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

pub fn render_stage(
    painter: &Painter,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
) {
    render_stage_tinted(
        painter,
        project,
        q0rg_id,
        frame,
        view,
        textures,
        ctx,
        Color32::WHITE,
    );
}

/// Same as `render_stage` but every emitted shape's RGBA is multiplied by
/// `tint` (per-channel Р вЂњРІР‚вЂќ tint / 255). Used for onion-skin renders: pass a
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
    render_q0rg(
        painter,
        project,
        q0rg_id,
        frame,
        Affine::IDENTITY,
        view,
        textures,
        ctx,
        0,
        tint,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_q0rg(
    painter: &Painter,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    depth: u8,
    tint: Color32,
) {
    if depth > Q0RG_RECURSION_LIMIT {
        return;
    }
    let Some(q0rg) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
        return;
    };
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };

    for layer in &q0rg.layers {
        for resolved in resolve_layer_at_frame(layer, local_frame) {
            let placement = resolved.placement;
            let interp = resolved.interp;
            // Affine matrix composition Р Р†Р вЂљРІР‚Сњ propagates parent skew and
            // non-uniform scale through to children, which the old
            // `compose(Transform2D, Transform2D)` discarded.
            let composed = Affine::compose(parent, Affine::from_transform(interp));
            match placement.target {
                Target::Asset(asset_id) => {
                    if let Some(asset) = project.assets.iter().find(|a| a.id() == asset_id) {
                        render_asset(
                            painter,
                            asset,
                            project.asset_appearances.get(&asset_id),
                            composed,
                            view,
                            textures,
                            ctx,
                            tint,
                            local_frame.saturating_sub(placement.frame),
                            project.meta.fps,
                        );
                    }
                }
                Target::Q0rg(child_id) => {
                    if child_id != q0rg_id {
                        render_q0rg(
                            painter,
                            project,
                            child_id,
                            local_frame,
                            composed,
                            view,
                            textures,
                            ctx,
                            depth + 1,
                            tint,
                        );
                    }
                }
            }
        }
    }
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
    active_transform_for_placement(layer, placement_idx, frame).is_some()
}

struct Resolved<'a> {
    placement: &'a Placement,
    interp: Transform2D,
    /// Index of the active placement within `layer.placements` Р Р†Р вЂљРІР‚Сњ exposed so
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
    let Some(keyframe) = layer
        .keyframe_frames()
        .into_iter()
        .filter(|candidate| *candidate <= frame)
        .max()
    else {
        return Vec::new();
    };

    let source_indices: Vec<usize> = layer
        .placements
        .iter()
        .enumerate()
        .filter_map(|(index, placement)| (placement.frame == keyframe).then_some(index))
        .collect();

    source_indices
        .iter()
        .enumerate()
        .map(|(source_order, index)| {
            let placement = &layer.placements[*index];
            let interp = match placement.tween.to_frame() {
                Some(to_frame) if to_frame > keyframe && frame >= keyframe => {
                    let occurrence = source_indices[..source_order]
                        .iter()
                        .filter(|candidate| {
                            layer.placements[**candidate].target == placement.target
                        })
                        .count();
                    let target_transform = layer
                        .placements
                        .iter()
                        .filter(|candidate| {
                            candidate.frame == to_frame && candidate.target == placement.target
                        })
                        .nth(occurrence)
                        .map(|candidate| candidate.transform)
                        .unwrap_or(placement.transform);
                    let denominator = f32::from(to_frame - keyframe);
                    let raw_t = (f32::from(frame - keyframe) / denominator).clamp(0.0, 1.0);
                    let t = placement.tween.easing().sample(raw_t);
                    lerp_transform(placement.transform, target_transform, t)
                }
                Some(_) | None => placement.transform,
            };
            Resolved {
                placement,
                interp,
                index: *index,
            }
        })
        .collect()
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
            painter.add(Shape::Mesh(mesh));
        }
        Asset::Q0v(v) => {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                textures.q0v_media.entry(v.asset_id)
            {
                let Ok(media) = q0video::q0v::Q0vFile::parse(v.bytes.clone()) else {
                    return;
                };
                entry.insert(media);
            }
            let Some(media) = textures.q0v_media.get(&v.asset_id) else {
                return;
            };
            let Some(frame_index) = media.spec.video_frame_for_host_frame(
                u32::from(elapsed_host_frames),
                u32::from(host_fps.max(1)),
            ) else {
                return;
            };
            let key = (v.asset_id, frame_index as u32);
            if let std::collections::hash_map::Entry::Vacant(entry) =
                textures.by_q0v_frame.entry(key)
            {
                let Ok(rgba) = media.decode_frame_rgba(frame_index) else {
                    return;
                };
                let image = ColorImage::from_rgba_unmultiplied(
                    [media.spec.width as usize, media.spec.height as usize],
                    &rgba,
                );
                let texture = ctx.load_texture(
                    format!("q0s_q0v_{}_{}", v.asset_id, frame_index),
                    image,
                    TextureOptions::LINEAR,
                );
                entry.insert(texture);
            }
            let Some(tex) = textures.by_q0v_frame.get(&key) else {
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
                    color: tint,
                });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            painter.add(Shape::Mesh(mesh));
        }
        Asset::Vector(v) => {
            #[cfg(feature = "appearance-mask-eraser")]
            if let Some(appearance) = appearance {
                paint_vector_appearance_halo(
                    painter, v, appearance, transform, view, textures, ctx, tint,
                );
                if let Some(fill) = v.fill {
                    let fill_color = modulate(rgba_to_color32(fill), tint);
                    let contours =
                        masked_vector_body_contours(v, appearance, transform, view, textures);
                    paint_complex_fill(painter, &contours, fill_color);
                }
                // Appearance metadata is validated only for fill-only vectors.
                // The body above stays real tessellated vector geometry; only
                // the soft halo is a filtered texture.
                return;
            }
            #[cfg(not(feature = "appearance-mask-eraser"))]
            let _ = appearance;
            // Multiply the on-disk stroke width by the composed area
            // scale so a 2Р вЂњРІР‚вЂќ scaled q0rg actually renders 2Р вЂњРІР‚вЂќ-thick
            // outlines. Otherwise stroked vectors look comically thin
            // when shrunk and thin-as-paper when blown up.
            let parent_scale = transform.uniform_scale();
            let fill_color = v
                .fill
                .map(|color| modulate(rgba_to_color32(color), tint))
                .unwrap_or(Color32::TRANSPARENT);
            if fill_color != Color32::TRANSPARENT {
                let contours: Vec<Vec<Pos2>> = v
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
                paint_complex_fill(painter, &contours, fill_color);
            }
            for path in &v.paths {
                let polyline_local = flatten_path_for_stroke(path);
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
                        modulate(rgba_to_color32(s.color), tint),
                    ),
                    None => Stroke::NONE,
                };
                // egui 0.27 only supports fills for *convex* PathShape
                // polygons. Brush outlines are normally concave; feeding
                // them directly to PathShape creates the giant diagonal
                // triangle fans that look like lasso selections. Tessellate
                // closed fills ourselves, then ask egui only for the outline.
                if stroke != Stroke::NONE {
                    painter.add(Shape::Path(PathShape {
                        points: polyline_screen.clone(),
                        closed: path.closed,
                        fill: Color32::TRANSPARENT,
                        stroke,
                    }));
                }
                // egui draws stroked open paths with butt caps; if the
                // user picked Round we tack a filled circle on each end
                // of the centerline so the visual cap matches what
                // q0player and the exporter will produce.
                if let Some(s) = &v.stroke {
                    if !path.closed
                        && polyline_screen.len() >= 2
                        && matches!(s.cap, q0s_format::geom::CapShape::Round)
                    {
                        let r = s.width.max(0.5) * parent_scale * view.scale * 0.5;
                        let col = modulate(rgba_to_color32(s.color), tint);
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

fn tessellate_closed_bevel_stroke(
    points: &[Pos2],
    width: f32,
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
    path_builder.end(true);
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
        let midpoint = tessellate_closed_bevel_stroke(points, midpoint_width)?;
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
    })
}

#[cfg(feature = "appearance-mask-eraser")]
fn stage_pos_to_screen(point: Pos2, view: &StageView) -> Pos2 {
    pos2(
        view.origin.x + point.x * view.scale,
        view.origin.y + point.y * view.scale,
    )
}

#[cfg(feature = "appearance-mask-eraser")]
fn parametric_stroke_screen_mesh(
    stroke: &CachedParametricStroke,
    width_points: f32,
    color: Color32,
    view: &StageView,
) -> Option<Mesh> {
    let scale = view.scale.abs();
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
    let scale = view.scale.abs();
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
fn build_selection_screen_meshes(
    source: &mut CachedSelectionStageMesh,
    view: &StageView,
    texture_id: TextureId,
    tile_size_points: f32,
    accent: Color32,
) -> Vec<Mesh> {
    let mut meshes = Vec::with_capacity(1 + source.strokes.len() * 2);
    if !source.fill_vertices.is_empty() && !source.fill_indices.is_empty() {
        let tile = tile_size_points.max(1.0);
        let mut fill = Mesh::with_texture(texture_id);
        fill.vertices.reserve(source.fill_vertices.len());
        for stage in &source.fill_vertices {
            let pos = stage_pos_to_screen(*stage, view);
            fill.vertices.push(Vertex {
                pos,
                uv: pos2(pos.x / tile, pos.y / tile),
                color: Color32::WHITE,
            });
        }
        fill.indices = source.fill_indices.clone();
        meshes.push(fill);
    }
    for stroke in &mut source.strokes {
        if let Some(mesh) =
            cached_selection_stroke_screen_mesh(stroke, 2.0, Color32::from_black_alpha(180), view)
        {
            meshes.push(mesh);
        }
        if let Some(mesh) = cached_selection_stroke_screen_mesh(stroke, 1.0, accent, view) {
            meshes.push(mesh);
        }
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

    // Flattened BР вЂњР’В©ziers contain many collinear samples, and a closed path can
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

/// Per-channel Р вЂњРІР‚вЂќ tint / 255. Result keeps tint==WHITE as identity (so the
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

/// Local-space AABB of a placement's *content* Р Р†Р вЂљРІР‚Сњ i.e. the bounds of the
/// asset/q0rg before the placement's own transform is applied.  Used by the
/// transform tool to compute resize handles relative to the untransformed
/// shape.
pub fn placement_local_bbox(
    project: &ProjectV2,
    placement: &Placement,
) -> Option<(f32, f32, f32, f32)> {
    match placement.target {
        Target::Asset(id) => {
            let asset = project.assets.iter().find(|a| a.id() == id)?;
            let pts = asset_local_visual_outline(project, id, asset);
            aabb_of(&pts)
        }
        Target::Q0rg(child_id) => q0rg_local_bbox(project, child_id, Q0RG_RECURSION_LIMIT),
    }
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
        for resolved in resolve_layer_at_frame(layer, local_frame) {
            let mut placement = resolved.placement.clone();
            placement.transform = resolved.interp;
            if let Some((x0, y0, x1, y1)) =
                placement_bbox_at_depth(project, &placement, Q0RG_RECURSION_LIMIT)
            {
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
    let affine = Affine::from_transform(transform);
    match target {
        Target::Asset(asset_id) => {
            if let Some(asset) = project.assets.iter().find(|asset| asset.id() == asset_id) {
                render_asset(
                    painter,
                    asset,
                    project.asset_appearances.get(&asset_id),
                    affine,
                    view,
                    textures,
                    ctx,
                    tint,
                    frame,
                    project.meta.fps,
                );
            }
        }
        Target::Q0rg(q0rg_id) => render_q0rg(
            painter, project, q0rg_id, frame, affine, view, textures, ctx, 0, tint,
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
        frame: 0,
        target,
        transform: Transform2D::IDENTITY,
        tween: Tween::None,
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
    let Some(asset) = project.assets.iter().find(|asset| asset.id() == asset_id) else {
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
        // Inflate by half the stroke width Р вЂњРІР‚вЂќ placement's area scale so
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
        Asset::Bitmap(_) | Asset::Q0v(_) => 0.0,
    }
}

fn q0rg_local_bbox(project: &ProjectV2, q0rg_id: u16, depth: u8) -> Option<(f32, f32, f32, f32)> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for layer in &q.layers {
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
        Asset::Q0v(v) => q0video::q0v::Q0vFile::parse(v.bytes.clone())
            .ok()
            .map(|media| {
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
    pos2(
        view.origin.x + p.x * view.scale,
        view.origin.y + p.y * view.scale,
    )
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

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
#[allow(clippy::too_many_arguments)]
fn paint_vector_appearance_halo(
    painter: &Painter,
    vector: &q0s_format::v2::VectorAsset,
    appearance: &VectorAppearance,
    transform: Affine,
    view: &StageView,
    textures: &mut TextureCache,
    ctx: &Context,
    tint: Color32,
) {
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
            .retain(|(asset_id, cached_bucket, _), _| {
                *asset_id != vector.asset_id || *cached_bucket != bucket
            });
    }
    let cached = match textures.appearance_by_asset.entry(key) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::hash_map::Entry::Vacant(entry) => {
            let Some(tile) =
                q0s_format::raster::rasterize_vector_halo_local(vector, appearance, ppu)
            else {
                return;
            };
            let image = ColorImage::from_rgba_unmultiplied(
                [tile.width as usize, tile.height as usize],
                &tile.rgba,
            );
            let texture = ctx.load_texture(
                format!(
                    "q0s_appearance_{}_{}_{}",
                    vector.asset_id, bucket, fingerprint
                ),
                image,
                TextureOptions::LINEAR,
            );
            entry.insert(CachedAppearanceTexture {
                texture,
                local_min_offset: Vec2::new(
                    tile.local_min.x - origin.x,
                    tile.local_min.y - origin.y,
                ),
                width: tile.width,
                height: tile.height,
                pixels_per_unit: tile.pixels_per_unit,
            })
        }
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
    painter.add(Shape::Mesh(mesh));
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
            return crate::brush::coverage_to_paths(&visible)
                .iter()
                .map(|path| {
                    flatten_path(path)
                        .iter()
                        .map(|point| stage_to_screen(transform.apply(*point), view))
                        .collect()
                })
                .collect();
        };
        let local_contours: Vec<Vec<Vec2>> = crate::brush::coverage_to_paths(&visible)
            .iter()
            .map(flatten_path)
            .collect();
        let selection_contours: Vec<Vec<Vec2>> = crate::brush::coverage_to_linear_paths(&visible)
            .iter()
            .map(flatten_path)
            .collect();
        entry.insert(local_contours);
        textures
            .visible_selection_body_by_asset
            .insert(vector.asset_id, selection_contours);
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

    fn test_placement(frame: u16, asset_id: u16, tx: f32, tween: Tween) -> Placement {
        Placement {
            frame,
            target: Target::Asset(asset_id),
            transform: Transform2D {
                tx,
                ..Transform2D::IDENTITY
            },
            tween,
        }
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
        };
        let ctx = Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 480.0));
        let mut cache = TextureCache::default();

        for (index, scale) in [0.25_f32, 0.5, 1.0, 2.0, 4.0, 8.0].into_iter().enumerate() {
            let view = StageView {
                origin: Pos2::new(40.0, 30.0),
                scale,
                stage_rect: rect,
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
        };
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(200.0, 200.0)),
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
        };
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0)),
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
