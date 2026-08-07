use std::collections::BTreeSet;

use geo::{BooleanOps, Buffer, Contains, Coord, LineString, MultiPolygon, Point, Polygon};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Asset, Path as VPath, ProjectV2, Rgba, Target, Transform2D, Tween, Vec2, VectorAppearance,
    VectorAsset, VectorMaterial,
};

use crate::{app::EditorApp, brush::BrushSettings};

pub const DEFAULT_HALO_RADIUS: f32 = 10.0;
pub const DEFAULT_HALO_OPACITY: f32 = 0.55;

pub fn default_brush_appearance() -> VectorAppearance {
    VectorAppearance {
        material: VectorMaterial::SoftHalo {
            radius: DEFAULT_HALO_RADIUS,
            opacity: DEFAULT_HALO_OPACITY,
        },
        erase_mask: Vec::new(),
        material_source: Vec::new(),
        clip_mask: Vec::new(),
        field_transform: Affine::IDENTITY,
    }
}

/// Classic brush paint is plain vector fill. The material exists only when
/// Glow is explicitly enabled in brush properties.
pub(crate) fn brush_material(settings: BrushSettings) -> Option<VectorMaterial> {
    settings.glow.then_some(default_brush_appearance().material)
}

/// Build the appearance for the single merged raw-fill asset produced by a
/// classic brush commit. Existing mask cuts follow same-colour paint through
/// merge drawing, while the freshly painted region clears those cuts so a new
/// stroke can genuinely paint back over an erased area.
pub(crate) fn merged_brush_appearance(
    project: &ProjectV2,
    same_color_asset_ids: &BTreeSet<u16>,
    freshly_painted: &MultiPolygon<f64>,
    material: VectorMaterial,
) -> VectorAppearance {
    let mut mask = MultiPolygon(Vec::new());
    for asset_id in same_color_asset_ids {
        let Some(appearance) = project.asset_appearances.get(asset_id) else {
            continue;
        };
        let coverage = transform_surface(
            &mask_paths_to_coverage(&appearance.erase_mask),
            appearance.field_transform,
        );
        if !coverage.0.is_empty() {
            mask = mask.union(&coverage);
        }
    }
    if !freshly_painted.0.is_empty() && !mask.0.is_empty() {
        mask = mask.difference(freshly_painted);
    }
    VectorAppearance {
        material,
        erase_mask: crate::brush::coverage_to_paths(&mask),
        material_source: Vec::new(),
        clip_mask: Vec::new(),
        field_transform: Affine::IDENTITY,
    }
}

pub(crate) fn clone_asset_appearance(project: &mut ProjectV2, source: u16, destination: u16) {
    if let Some(appearance) = project.asset_appearances.get(&source).cloned() {
        project.asset_appearances.insert(destination, appearance);
    }
}

pub(crate) fn partition_appearance(
    original: &VectorAppearance,
    original_paths: &[VPath],
    partition_region: &MultiPolygon<f64>,
) -> Option<(VectorAppearance, VectorAppearance)> {
    let material_source = if original.material_source.is_empty() {
        original_paths.to_vec()
    } else {
        original.material_source.clone()
    };
    let material_geometry = paths_to_coverage(&material_source);
    if material_geometry.0.is_empty() {
        return None;
    }
    let full_support = material_support(&material_geometry, original.material);
    let old_clip = if original.clip_mask.is_empty() {
        full_support
    } else {
        mask_paths_to_coverage(&original.clip_mask)
    };
    let inverse_field = original.field_transform.inverse()?;
    let canonical_partition = transform_surface(partition_region, inverse_field);
    let selected_clip = old_clip.intersection(&canonical_partition);
    let source_clip = old_clip.difference(&canonical_partition);

    let make = |clip: MultiPolygon<f64>| VectorAppearance {
        material: original.material,
        erase_mask: original.erase_mask.clone(),
        material_source: material_source.clone(),
        clip_mask: crate::brush::coverage_to_paths(&clip),
        field_transform: original.field_transform,
    };
    Some((make(source_clip), make(selected_clip)))
}

pub(crate) fn split_asset_appearance(
    project: &mut ProjectV2,
    source_asset_id: u16,
    selected_asset_id: u16,
    original_paths: &[VPath],
    partition_region: &MultiPolygon<f64>,
    source_empty: bool,
) {
    let Some(original) = project.asset_appearances.get(&source_asset_id).cloned() else {
        return;
    };
    if source_empty {
        project
            .asset_appearances
            .insert(selected_asset_id, original);
        return;
    }
    let Some((source, selected)) =
        partition_appearance(&original, original_paths, partition_region)
    else {
        return;
    };
    project
        .asset_appearances
        .insert(selected_asset_id, selected);
    project.asset_appearances.insert(source_asset_id, source);
}

pub(crate) fn transform_appearance(
    appearance: &VectorAppearance,
    transform: Affine,
) -> VectorAppearance {
    let mut transformed = appearance.clone();
    transformed.field_transform = Affine::compose(transform, appearance.field_transform);
    transformed
}

pub(crate) fn transform_surface(
    surface: &MultiPolygon<f64>,
    transform: Affine,
) -> MultiPolygon<f64> {
    fn ring(line: &LineString<f64>, transform: Affine) -> LineString<f64> {
        LineString::new(
            line.0
                .iter()
                .map(|coord| {
                    let mapped =
                        transform.apply(q0s_format::v2::Vec2::new(coord.x as f32, coord.y as f32));
                    Coord {
                        x: f64::from(mapped.x),
                        y: f64::from(mapped.y),
                    }
                })
                .collect(),
        )
    }
    MultiPolygon(
        surface
            .0
            .iter()
            .map(|polygon| {
                Polygon::new(
                    ring(polygon.exterior(), transform),
                    polygon
                        .interiors()
                        .iter()
                        .map(|interior| ring(interior, transform))
                        .collect(),
                )
            })
            .collect(),
    )
}

pub(crate) fn paths_to_coverage(paths: &[VPath]) -> MultiPolygon<f64> {
    if paths.is_empty() {
        return MultiPolygon(Vec::new());
    }
    crate::brush::vector_fill_geometry(&VectorAsset {
        asset_id: 0,
        paths: paths.to_vec(),
        fill: Some(Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        }),
        stroke: None,
    })
}

pub(crate) fn mask_paths_to_coverage(paths: &[VPath]) -> MultiPolygon<f64> {
    paths_to_coverage(paths)
}

pub(crate) fn material_support(
    source: &MultiPolygon<f64>,
    material: VectorMaterial,
) -> MultiPolygon<f64> {
    match material {
        VectorMaterial::Solid => source.clone(),
        VectorMaterial::SoftHalo { radius, .. } => source.buffer(f64::from(radius.max(0.0))),
    }
}
fn point_segment_distance_sq(point: Point<f64>, a: Coord<f64>, b: Coord<f64>) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    if len_sq <= f64::EPSILON {
        let px = point.x() - a.x;
        let py = point.y() - a.y;
        return px * px + py * py;
    }
    let t = (((point.x() - a.x) * dx + (point.y() - a.y) * dy) / len_sq).clamp(0.0, 1.0);
    let x = a.x + dx * t;
    let y = a.y + dy * t;
    let px = point.x() - x;
    let py = point.y() - y;
    px * px + py * py
}

fn ring_near_point(ring: &LineString<f64>, point: Point<f64>, distance: f64) -> bool {
    if distance < 0.0 {
        return false;
    }
    let limit_sq = distance * distance;
    ring.0
        .windows(2)
        .any(|segment| point_segment_distance_sq(point, segment[0], segment[1]) <= limit_sq)
}

pub(crate) fn surface_contains_or_near(
    surface: &MultiPolygon<f64>,
    point: Vec2,
    distance: f32,
) -> bool {
    let point = Point::new(f64::from(point.x), f64::from(point.y));
    if surface.contains(&point) {
        return true;
    }
    let distance = f64::from(distance.max(0.0));
    if distance <= 0.0 {
        return false;
    }
    surface.0.iter().any(|polygon| {
        ring_near_point(polygon.exterior(), point, distance)
            || polygon
                .interiors()
                .iter()
                .any(|ring| ring_near_point(ring, point, distance))
    })
}

pub(crate) fn material_support_contains_point(
    source: &MultiPolygon<f64>,
    material: VectorMaterial,
    point: Vec2,
    edge_tolerance: f32,
) -> bool {
    let radius = match material {
        VectorMaterial::Solid => 0.0,
        VectorMaterial::SoftHalo { radius, .. } => radius.max(0.0),
    };
    surface_contains_or_near(source, point, radius + edge_tolerance.max(0.0))
}

/// Hit-test the resolved material without constructing a polygonal buffer.
/// This is mathematically the same finite support test for a soft halo: a
/// point belongs to the buffer iff its distance to the filled source is at
/// most the halo radius. Selection/hover calls this every frame, so avoiding
/// `geo::Buffer` here is critical for dense classic-brush contours.
pub(crate) fn visible_material_contains_point(
    vector: &VectorAsset,
    appearance: Option<&VectorAppearance>,
    point: Vec2,
    edge_tolerance: f32,
) -> bool {
    let Some(appearance) = appearance else {
        let source = crate::brush::vector_fill_geometry(vector);
        return surface_contains_or_near(&source, point, edge_tolerance);
    };
    let Some(inverse_field) = appearance.field_transform.inverse() else {
        return false;
    };
    let canonical_point = inverse_field.apply(point);
    let inside_support = if appearance.clip_mask.is_empty() {
        let source = material_source_surface(vector, appearance);
        material_support_contains_point(
            &source,
            appearance.material,
            canonical_point,
            edge_tolerance,
        )
    } else {
        let clip = mask_paths_to_coverage(&appearance.clip_mask);
        surface_contains_or_near(&clip, canonical_point, edge_tolerance)
    };
    if !inside_support {
        return false;
    }
    let erase = mask_paths_to_coverage(&appearance.erase_mask);
    !surface_contains_or_near(&erase, canonical_point, 0.001)
}

fn path_anchor_bounds(paths: &[VPath]) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut found = false;
    for anchor in paths
        .iter()
        .filter(|path| path.closed)
        .flat_map(|path| path.anchors.iter())
    {
        if !anchor.point.x.is_finite() || !anchor.point.y.is_finite() {
            continue;
        }
        min_x = min_x.min(anchor.point.x);
        min_y = min_y.min(anchor.point.y);
        max_x = max_x.max(anchor.point.x);
        max_y = max_y.max(anchor.point.y);
        found = true;
    }
    found.then_some((min_x, min_y, max_x, max_y))
}

fn transform_bounds(
    bounds: (f32, f32, f32, f32),
    transform: Affine,
) -> Option<(f32, f32, f32, f32)> {
    let (min_x, min_y, max_x, max_y) = bounds;
    let corners = [
        Vec2::new(min_x, min_y),
        Vec2::new(max_x, min_y),
        Vec2::new(max_x, max_y),
        Vec2::new(min_x, max_y),
    ];
    let mut out = (
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    );
    for corner in corners {
        let point = transform.apply(corner);
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        out.0 = out.0.min(point.x);
        out.1 = out.1.min(point.y);
        out.2 = out.2.max(point.x);
        out.3 = out.3.max(point.y);
    }
    Some(out)
}

fn intersect_bounds(
    left: (f32, f32, f32, f32),
    right: (f32, f32, f32, f32),
) -> Option<(f32, f32, f32, f32)> {
    let bounds = (
        left.0.max(right.0),
        left.1.max(right.1),
        left.2.min(right.2),
        left.3.min(right.3),
    );
    (bounds.0 <= bounds.2 && bounds.1 <= bounds.3).then_some(bounds)
}

/// Cheap conservative visual bounds for interactive selection UI. Unlike the
/// exact material surface this never buffers a dense brush polygon. Partial erase-mask
/// cutouts are ignored for the outer box unless they provably erase the complete
/// finite envelope. Hit-testing remains exact while the transform UI stays cheap.
pub(crate) fn fast_visible_material_bounds_for_paths(
    vector: &VectorAsset,
    appearance: Option<&VectorAppearance>,
    path_indices: &[usize],
) -> Option<(f32, f32, f32, f32)> {
    let selected_paths: Vec<VPath> = path_indices
        .iter()
        .filter_map(|index| vector.paths.get(*index).cloned())
        .filter(|path| path.closed)
        .collect();
    let selected_bounds = path_anchor_bounds(&selected_paths)?;
    let Some(appearance) = appearance else {
        return Some(selected_bounds);
    };

    let all_closed_selected = vector
        .paths
        .iter()
        .enumerate()
        .filter(|(_, path)| path.closed)
        .all(|(index, _)| path_indices.contains(&index));

    let canonical_whole_bounds = if appearance.clip_mask.is_empty() {
        let source_paths = if appearance.material_source.is_empty() {
            &vector.paths
        } else {
            &appearance.material_source
        };
        let mut bounds = path_anchor_bounds(source_paths)?;
        if let VectorMaterial::SoftHalo { radius, .. } = appearance.material {
            let radius = radius.max(0.0);
            bounds.0 -= radius;
            bounds.1 -= radius;
            bounds.2 += radius;
            bounds.3 += radius;
        }
        bounds
    } else {
        path_anchor_bounds(&appearance.clip_mask)?
    };

    if !appearance.erase_mask.is_empty() {
        // Usually a partial eraser cannot possibly cover the complete visual
        // envelope, so reject that case using anchor bounds alone. Only when
        // the erase bbox encloses the whole finite support do we pay for one
        // rectangle-vs-mask boolean operation. This preserves the important
        // "fully erased => no selection" invariant without ever buffering the
        // dense brush source in the per-frame UI path.
        if let Some(erase_bounds) = path_anchor_bounds(&appearance.erase_mask) {
            let can_cover_whole = erase_bounds.0 <= canonical_whole_bounds.0
                && erase_bounds.1 <= canonical_whole_bounds.1
                && erase_bounds.2 >= canonical_whole_bounds.2
                && erase_bounds.3 >= canonical_whole_bounds.3;
            if can_cover_whole {
                let (min_x, min_y, max_x, max_y) = canonical_whole_bounds;
                let envelope = MultiPolygon(vec![Polygon::new(
                    LineString::new(vec![
                        Coord {
                            x: f64::from(min_x),
                            y: f64::from(min_y),
                        },
                        Coord {
                            x: f64::from(max_x),
                            y: f64::from(min_y),
                        },
                        Coord {
                            x: f64::from(max_x),
                            y: f64::from(max_y),
                        },
                        Coord {
                            x: f64::from(min_x),
                            y: f64::from(max_y),
                        },
                        Coord {
                            x: f64::from(min_x),
                            y: f64::from(min_y),
                        },
                    ]),
                    Vec::new(),
                )]);
                let erase = mask_paths_to_coverage(&appearance.erase_mask);
                if envelope.difference(&erase).0.is_empty() {
                    return None;
                }
            }
        }
    }

    let whole_bounds = transform_bounds(canonical_whole_bounds, appearance.field_transform)?;

    if all_closed_selected {
        return Some(whole_bounds);
    }

    let radius = match appearance.material {
        VectorMaterial::Solid => 0.0,
        VectorMaterial::SoftHalo { radius, .. } => radius.max(0.0),
    };
    let row_x = (appearance.field_transform.a11 * appearance.field_transform.a11
        + appearance.field_transform.a12 * appearance.field_transform.a12)
        .sqrt();
    let row_y = (appearance.field_transform.a21 * appearance.field_transform.a21
        + appearance.field_transform.a22 * appearance.field_transform.a22)
        .sqrt();
    let partial = (
        selected_bounds.0 - radius * row_x,
        selected_bounds.1 - radius * row_y,
        selected_bounds.2 + radius * row_x,
        selected_bounds.3 + radius * row_y,
    );
    intersect_bounds(partial, whole_bounds).or(Some(partial))
}

/// Opaque/vector body that is still visible after the non-destructive erase
/// mask. This is what the editor tessellates as real vector geometry.
pub(crate) fn material_source_surface(
    vector: &VectorAsset,
    appearance: &VectorAppearance,
) -> MultiPolygon<f64> {
    if appearance.material_source.is_empty() {
        crate::brush::vector_fill_geometry(vector)
    } else {
        paths_to_coverage(&appearance.material_source)
    }
}

fn clip_surface(surface: MultiPolygon<f64>, appearance: &VectorAppearance) -> MultiPolygon<f64> {
    if appearance.clip_mask.is_empty() {
        surface
    } else {
        let clip = transform_surface(
            &mask_paths_to_coverage(&appearance.clip_mask),
            appearance.field_transform,
        );
        surface.intersection(&clip)
    }
}

/// Opaque/vector body that is still visible after the post-material fragment
/// clip and the non-destructive erase mask. The body itself remains real vector
/// geometry; the frozen material source is used only by the filter layer.
pub(crate) fn visible_source_surface_for_vector(
    vector: &VectorAsset,
    appearance: Option<&VectorAppearance>,
) -> MultiPolygon<f64> {
    let source = crate::brush::vector_fill_geometry(vector);
    let Some(appearance) = appearance else {
        return source;
    };
    let clipped = clip_surface(source, appearance);
    let mask = transform_surface(
        &mask_paths_to_coverage(&appearance.erase_mask),
        appearance.field_transform,
    );
    if mask.0.is_empty() {
        clipped
    } else {
        clipped.difference(&mask)
    }
}

/// Full selectable visual support. Split fragments keep a frozen material
/// source and a post-material clip, so moving a cut piece carries the exact
/// resolved slice instead of generating a fresh glow along the cut edge.
pub(crate) fn visible_material_surface_for_vector(
    vector: &VectorAsset,
    appearance: Option<&VectorAppearance>,
) -> MultiPolygon<f64> {
    let Some(appearance) = appearance else {
        return crate::brush::vector_fill_geometry(vector);
    };
    // A post-material fragment clip is already a partition of the finite
    // resolved material support. Re-buffering the frozen source on every
    // selection/hover frame is both redundant and extremely expensive.
    let clipped = if appearance.clip_mask.is_empty() {
        let material_source = material_source_surface(vector, appearance);
        material_support(&material_source, appearance.material)
    } else {
        mask_paths_to_coverage(&appearance.clip_mask)
    };
    let mask = mask_paths_to_coverage(&appearance.erase_mask);
    let canonical_visible = if mask.0.is_empty() {
        clipped
    } else {
        clipped.difference(&mask)
    };
    transform_surface(&canonical_visible, appearance.field_transform)
}

pub(crate) fn visible_material_surface_for_paths(
    vector: &VectorAsset,
    appearance: Option<&VectorAppearance>,
    path_indices: &[usize],
) -> MultiPolygon<f64> {
    let subset = VectorAsset {
        asset_id: vector.asset_id,
        paths: path_indices
            .iter()
            .filter_map(|index| vector.paths.get(*index).cloned())
            .collect(),
        fill: vector.fill,
        stroke: None,
    };
    let subset_source = crate::brush::vector_fill_geometry(&subset);
    let Some(appearance) = appearance else {
        return subset_source;
    };
    let all_closed_selected = vector
        .paths
        .iter()
        .enumerate()
        .filter(|(_, path)| path.closed)
        .all(|(index, _)| path_indices.contains(&index));
    if all_closed_selected {
        return visible_material_surface_for_vector(vector, Some(appearance));
    }
    let subset_support = material_support(&subset_source, appearance.material);
    visible_material_surface_for_vector(vector, Some(appearance)).intersection(&subset_support)
}

pub(crate) fn asset_visible_material_bounds_fast(
    project: &ProjectV2,
    asset_id: u16,
) -> Option<(f32, f32, f32, f32)> {
    let Asset::Vector(vector) = project.assets.iter().find(|asset| asset.id() == asset_id)? else {
        return None;
    };
    if vector.fill.is_none() || vector.stroke.is_some() {
        return None;
    }
    let path_indices: Vec<usize> = vector
        .paths
        .iter()
        .enumerate()
        .filter_map(|(index, path)| path.closed.then_some(index))
        .collect();
    fast_visible_material_bounds_for_paths(
        vector,
        project.asset_appearances.get(&asset_id),
        &path_indices,
    )
}
#[cfg(test)]
pub(crate) fn asset_visible_material_bounds(
    project: &ProjectV2,
    asset_id: u16,
) -> Option<(f32, f32, f32, f32)> {
    let Asset::Vector(vector) = project.assets.iter().find(|asset| asset.id() == asset_id)? else {
        return None;
    };
    if vector.fill.is_none() || vector.stroke.is_some() {
        return None;
    }
    let surface =
        visible_material_surface_for_vector(vector, project.asset_appearances.get(&asset_id));
    let bounds = geo::BoundingRect::bounding_rect(&surface)?;
    Some((
        bounds.min().x as f32,
        bounds.min().y as f32,
        bounds.max().x as f32,
        bounds.max().y as f32,
    ))
}

/// Erase the resolved appearance, not the source vector. The exact eraser
/// footprint is intersected with the finite material support and then unioned
/// into the asset-local mask. No radius is added to the eraser region.
pub fn erase_visible_region(app: &mut EditorApp, region: MultiPolygon<f64>) -> bool {
    if region.0.is_empty() {
        return false;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    let Some(layer) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return false;
    };

    let mut hit_asset_ids = BTreeSet::new();
    for (placement_idx, _) in crate::render::active_placements_at(layer, frame) {
        let Some(placement) = layer.placements.get(placement_idx) else {
            continue;
        };
        if placement.transform != Transform2D::IDENTITY || !matches!(placement.tween, Tween::None) {
            continue;
        }
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(appearance) = app.state.project.asset_appearances.get(&asset_id) else {
            continue;
        };
        let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
        else {
            continue;
        };
        if vector.fill.is_none() || vector.stroke.is_some() {
            continue;
        }
        let visible_support = visible_material_surface_for_vector(vector, Some(appearance));
        if !visible_support.intersection(&region).0.is_empty() {
            hit_asset_ids.insert(asset_id);
        }
    }
    if hit_asset_ids.is_empty() {
        return false;
    }

    app.history.snapshot(&app.state.project);
    if crate::tools::materialize_layer_keyframe_for_edit(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
    )
    .is_none()
    {
        return false;
    }
    let writable = crate::brush::prepare_writable_raw_assets(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
        &hit_asset_ids,
    );

    let mut changed = false;
    for original_asset_id in hit_asset_ids {
        let asset_id = writable
            .get(&original_asset_id)
            .copied()
            .unwrap_or(original_asset_id);
        let Some(appearance) = app.state.project.asset_appearances.get(&asset_id).cloned() else {
            continue;
        };
        let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
        else {
            continue;
        };
        let support = visible_material_surface_for_vector(vector, Some(&appearance));
        let visible_cut = support.intersection(&region);
        if visible_cut.0.is_empty() {
            continue;
        }
        let Some(inverse_field) = appearance.field_transform.inverse() else {
            continue;
        };
        let canonical_cut = transform_surface(&visible_cut, inverse_field);
        let old_mask = mask_paths_to_coverage(&appearance.erase_mask);
        let new_mask = if old_mask.0.is_empty() {
            canonical_cut
        } else {
            old_mask.union(&canonical_cut)
        };
        let new_paths = crate::brush::coverage_to_paths(&new_mask);
        if new_paths != appearance.erase_mask {
            if let Some(entry) = app.state.project.asset_appearances.get_mut(&asset_id) {
                entry.erase_mask = new_paths;
                changed = true;
            }
        }
    }

    if changed {
        app.textures.invalidate();
        app.session.selection = crate::state::Selection::None;
        app.session.status = "Appearance mask erased".to_string();
        app.state.dirty = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use geo::{Area, BoundingRect, Contains, Coord, LineString, Point, Polygon};
    use q0s_format::v2::{Anchor, Placement};

    use super::*;

    fn square_path(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> VPath {
        VPath {
            anchors: [
                (min_x, min_y),
                (max_x, min_y),
                (max_x, max_y),
                (min_x, max_y),
            ]
            .into_iter()
            .map(|(x, y)| Anchor {
                point: q0s_format::v2::Vec2::new(x, y),
                in_handle: None,
                out_handle: None,
            })
            .collect(),
            closed: true,
        }
    }

    fn rect_region(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> MultiPolygon<f64> {
        MultiPolygon(vec![Polygon::new(
            LineString::new(vec![
                Coord { x: min_x, y: min_y },
                Coord { x: max_x, y: min_y },
                Coord { x: max_x, y: max_y },
                Coord { x: min_x, y: max_y },
                Coord { x: min_x, y: min_y },
            ]),
            Vec::new(),
        )])
    }

    fn app_with_appearance() -> EditorApp {
        let mut app = EditorApp::default();
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![square_path(0.0, 0.0, 10.0, 10.0)],
            fill: Some(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
            stroke: None,
        }));
        app.state.project.asset_appearances.insert(
            1,
            VectorAppearance {
                material: VectorMaterial::SoftHalo {
                    radius: 10.0,
                    opacity: 0.5,
                },
                erase_mask: Vec::new(),
                material_source: Vec::new(),
                clip_mask: Vec::new(),
                field_transform: q0s_format::transform::Affine::IDENTITY,
            },
        );
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            });
        app
    }

    #[test]
    fn splitting_glowing_fill_partitions_original_material_instead_of_reblurring_fragments() {
        let mut app = app_with_appearance();
        let original_paths = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let original_vector = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.clone(),
            _ => unreachable!(),
        };
        let original_appearance = app.state.project.asset_appearances[&1].clone();
        let original_visible =
            visible_material_surface_for_vector(&original_vector, Some(&original_appearance));

        let left_path = square_path(0.0, 0.0, 5.0, 10.0);
        let right_path = square_path(5.0, 0.0, 10.0, 10.0);
        if let Asset::Vector(vector) = &mut app.state.project.assets[0] {
            vector.paths = vec![left_path];
        }
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 2,
            paths: vec![right_path],
            fill: original_vector.fill,
            stroke: None,
        }));
        let partition = rect_region(5.0, -20.0, 30.0, 30.0);
        split_asset_appearance(
            &mut app.state.project,
            1,
            2,
            &original_paths,
            &partition,
            false,
        );

        let left_appearance = &app.state.project.asset_appearances[&1];
        let right_appearance = &app.state.project.asset_appearances[&2];
        assert_eq!(left_appearance.material_source, original_paths);
        assert_eq!(right_appearance.material_source, original_paths);
        let left_vector = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        let right_vector = match &app.state.project.assets[1] {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        let left_visible = visible_material_surface_for_vector(left_vector, Some(left_appearance));
        let right_visible =
            visible_material_surface_for_vector(right_vector, Some(right_appearance));
        let reconstructed = left_visible.union(&right_visible);
        assert!(
            original_visible
                .difference(&reconstructed)
                .union(&reconstructed.difference(&original_visible))
                .unsigned_area()
                < 0.05,
            "post-material fragments must reconstruct the pre-split appearance field"
        );
        assert!(left_visible.intersection(&right_visible).unsigned_area() < 0.05);
        let right_bounds = right_visible
            .bounding_rect()
            .expect("right fragment bounds");
        assert!(
            right_bounds.min().x >= 4.95,
            "right fragment must not generate a new halo across the cut edge: {right_bounds:?}"
        );
    }

    #[test]
    fn mask_eraser_never_mutates_source_geometry() {
        let mut app = app_with_appearance();
        let before = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let eraser = rect_region(15.0, 4.0, 17.0, 6.0);
        assert!(erase_visible_region(&mut app, eraser));
        let after = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            before, after,
            "appearance erasing must not cut source paths"
        );
    }

    #[test]
    fn classic_geometry_eraser_refuses_to_cut_appearance_source_paths() {
        let mut app = app_with_appearance();
        let before = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let eraser = rect_region(2.0, 2.0, 8.0, 8.0);
        assert!(
            !crate::brush::erase_brush_region(&mut app, eraser),
            "appearance assets must be owned exclusively by the mask eraser"
        );
        let after = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        assert_eq!(before, after);
    }

    #[test]
    fn mask_eraser_records_exact_footprint_not_halo_expansion() {
        let mut app = app_with_appearance();
        let eraser = rect_region(15.0, 4.0, 17.0, 6.0);
        let eraser_area = eraser.unsigned_area();
        assert!(erase_visible_region(&mut app, eraser));
        let mask = mask_paths_to_coverage(&app.state.project.asset_appearances[&1].erase_mask);
        let bounds = mask.bounding_rect().expect("mask bounds");
        assert!((bounds.min().x - 15.0).abs() < 0.05);
        assert!((bounds.max().x - 17.0).abs() < 0.05);
        assert!((bounds.min().y - 4.0).abs() < 0.05);
        assert!((bounds.max().y - 6.0).abs() < 0.05);
        assert!((mask.unsigned_area() - eraser_area).abs() < 0.1);
    }
    #[test]
    fn fast_material_point_hit_matches_exact_halo_surface_and_erase_hole() {
        let mut app = app_with_appearance();
        app.state
            .project
            .asset_appearances
            .get_mut(&1)
            .unwrap()
            .erase_mask = vec![square_path(4.0, 4.0, 6.0, 6.0)];
        let vector = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        let appearance = &app.state.project.asset_appearances[&1];
        let exact = visible_material_surface_for_vector(vector, Some(appearance));

        for point in [
            Vec2::new(-5.0, 5.0),  // visible halo
            Vec2::new(5.0, 5.0),   // erased hole
            Vec2::new(-11.0, 5.0), // outside finite support
            Vec2::new(5.0, 19.0),  // halo near the lower edge
        ] {
            let exact_hit = exact.contains(&Point::new(f64::from(point.x), f64::from(point.y)));
            let fast_hit = visible_material_contains_point(vector, Some(appearance), point, 0.0);
            assert_eq!(
                fast_hit, exact_hit,
                "distance-based hot-path hit must match exact buffered support at {point:?}"
            );
        }
    }

    #[test]
    fn fast_visual_bounds_enclose_exact_glow_before_and_after_field_transform() {
        let mut app = app_with_appearance();
        let assert_fast_encloses_exact = |project: &ProjectV2| {
            let exact = asset_visible_material_bounds(project, 1).expect("exact glow bounds");
            let fast = asset_visible_material_bounds_fast(project, 1).expect("fast glow bounds");
            assert!(
                fast.0 <= exact.0 + 0.01
                    && fast.1 <= exact.1 + 0.01
                    && fast.2 >= exact.2 - 0.01
                    && fast.3 >= exact.3 - 0.01,
                "fast selection bounds must conservatively enclose visible glow: fast={fast:?}, exact={exact:?}"
            );
            assert!(
                [fast.0, fast.1, fast.2, fast.3]
                    .into_iter()
                    .all(f32::is_finite),
                "selection bounds must stay finite"
            );
        };

        assert_fast_encloses_exact(&app.state.project);
        app.state
            .project
            .asset_appearances
            .get_mut(&1)
            .unwrap()
            .field_transform = Affine {
            a11: 1.0,
            a12: 0.6,
            a21: 0.2,
            a22: 1.0,
            tx: 30.0,
            ty: -5.0,
        };
        assert_fast_encloses_exact(&app.state.project);
    }
}
