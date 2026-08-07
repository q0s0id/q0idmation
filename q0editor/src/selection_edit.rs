use std::collections::{BTreeMap, BTreeSet};

use geo::{Area, BooleanOps, MultiPolygon};
use q0s_format::v2::{
    Asset, Path as VPath, Placement, ProjectV2, Target, Transform2D, Tween, Vec2, VectorAsset,
};

use crate::state::{ClipboardPayload, PathRef, PlacementRef, RawVectorClipboard, Selection};

#[derive(Debug, Clone, Default)]
pub struct PasteResult {
    pub objects: Vec<PlacementRef>,
    pub raw_paths: Vec<PathRef>,
    pub raw_placements: Vec<PlacementRef>,
    pub raw_bounds: Option<(Vec2, Vec2)>,
}

pub fn selection_can_clip(selection: &Selection) -> bool {
    match selection {
        Selection::Placement { .. } | Selection::Path { .. } => true,
        Selection::Paths(items) => !items.is_empty(),
        Selection::Multi(items) => !items.is_empty(),
        Selection::Mixed { paths, objects } => !paths.is_empty() || !objects.is_empty(),
        Selection::RawArea {
            placements,
            objects,
            ..
        } => !placements.is_empty() || !objects.is_empty(),
        Selection::None
        | Selection::Asset(_)
        | Selection::Q0rg(_)
        | Selection::PathPoints { .. } => false,
    }
}

pub fn capture_clipboard(project: &ProjectV2, selection: &Selection) -> ClipboardPayload {
    let placements = selected_object_placements(project, selection);
    let raw_vectors = selected_raw_vectors(project, selection);
    ClipboardPayload {
        placements,
        raw_vectors,
        timeline: None,
    }
}

fn selected_object_placements(project: &ProjectV2, selection: &Selection) -> Vec<Placement> {
    let refs: Vec<PlacementRef> = match selection {
        Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } => vec![PlacementRef {
            q0rg_id: *q0rg_id,
            layer_id: *layer_id,
            placement_idx: *placement_idx,
        }],
        Selection::Multi(items) => items.clone(),
        Selection::Mixed { objects, .. } => objects.clone(),
        Selection::RawArea { objects, .. } => objects.clone(),
        _ => Vec::new(),
    };
    refs.into_iter()
        .filter_map(|reference| placement_clone(project, reference))
        .collect()
}

fn selected_raw_vectors(project: &ProjectV2, selection: &Selection) -> Vec<RawVectorClipboard> {
    let path_refs: Option<Vec<PathRef>> = match selection {
        Selection::Path {
            q0rg_id,
            layer_id,
            placement_idx,
            path_idx,
        } => Some(vec![PathRef {
            q0rg_id: *q0rg_id,
            layer_id: *layer_id,
            placement_idx: *placement_idx,
            path_idx: *path_idx,
        }]),
        Selection::Paths(refs) if !refs.is_empty() => Some(refs.clone()),
        Selection::Mixed { paths, .. } if !paths.is_empty() => Some(paths.clone()),
        _ => None,
    };

    if let Some(refs) = path_refs {
        let mut groups: BTreeMap<(u16, u16, usize), Vec<usize>> = BTreeMap::new();
        for reference in refs {
            groups
                .entry((
                    reference.q0rg_id,
                    reference.layer_id,
                    reference.placement_idx,
                ))
                .or_default()
                .push(reference.path_idx);
        }
        let mut vectors = Vec::new();
        for ((q0rg_id, layer_id, placement_idx), mut indices) in groups {
            let Some(placement) = placement_clone(
                project,
                PlacementRef {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                },
            ) else {
                continue;
            };
            let Target::Asset(asset_id) = placement.target else {
                continue;
            };
            let Some(Asset::Vector(vector)) =
                project.assets.iter().find(|asset| asset.id() == asset_id)
            else {
                continue;
            };
            indices.sort_unstable();
            indices.dedup();
            let paths: Vec<VPath> = indices
                .into_iter()
                .filter_map(|index| vector.paths.get(index).cloned())
                .collect();
            if paths.is_empty() {
                continue;
            }
            let snapshot = VectorAsset {
                asset_id: 0,
                paths,
                fill: vector.fill,
                stroke: vector.stroke,
            };
            #[cfg(feature = "appearance-mask-eraser")]
            let appearance = project
                .asset_appearances
                .get(&asset_id)
                .and_then(|appearance| {
                    let selected_geometry = crate::tools::vector_fill_geometry(&snapshot);
                    let partition = crate::appearance::material_support(
                        &selected_geometry,
                        appearance.material,
                    );
                    crate::appearance::partition_appearance(appearance, &vector.paths, &partition)
                        .map(|(_, selected)| selected)
                });
            #[cfg(not(feature = "appearance-mask-eraser"))]
            let appearance = None;
            vectors.push(RawVectorClipboard {
                vector: snapshot,
                appearance,
            });
        }
        return vectors;
    }

    let Selection::RawArea {
        placements,
        bounds_min,
        bounds_max,
        ..
    } = selection
    else {
        return Vec::new();
    };
    let clip = MultiPolygon(vec![crate::tools::rect_polygon((
        bounds_min.x,
        bounds_min.y,
        bounds_max.x,
        bounds_max.y,
    ))]);
    let mut vectors = Vec::new();
    for reference in placements {
        let Some(placement) = placement_clone(project, *reference) else {
            continue;
        };
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(Asset::Vector(vector)) =
            project.assets.iter().find(|asset| asset.id() == asset_id)
        else {
            continue;
        };
        let source = crate::tools::vector_fill_geometry(vector);

        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = project.asset_appearances.get(&asset_id) {
            let visible =
                crate::appearance::visible_material_surface_for_vector(vector, Some(appearance));
            let selected_visible = visible.intersection(&clip);
            if selected_visible.unsigned_area() <= 0.05 {
                continue;
            }
            let selected_body = source.intersection(&clip);
            let body_remainder = source.difference(&clip);
            let paths = if selected_body.unsigned_area() > 0.05 {
                if body_remainder.unsigned_area() <= 0.05 {
                    // Whole-body capture keeps the original curves verbatim.
                    vector.paths.clone()
                } else {
                    crate::tools::geo_multi_polygon_to_linear_paths(&selected_body)
                }
            } else {
                // Halo-only marquee: preserve hidden carrier geometry internally;
                // the post-material clip remains authoritative for what is visible.
                vector
                    .paths
                    .iter()
                    .filter(|path| path.closed)
                    .cloned()
                    .collect()
            };
            if paths.is_empty() {
                continue;
            }
            let selected_appearance = if visible.difference(&clip).unsigned_area() <= 0.05 {
                let mut frozen = appearance.clone();
                if frozen.material_source.is_empty() {
                    frozen.material_source = vector.paths.clone();
                }
                frozen
            } else {
                let Some((_, selected)) =
                    crate::appearance::partition_appearance(appearance, &vector.paths, &clip)
                else {
                    continue;
                };
                selected
            };
            vectors.push(RawVectorClipboard {
                vector: VectorAsset {
                    asset_id: 0,
                    paths,
                    fill: vector.fill,
                    stroke: None,
                },
                appearance: Some(selected_appearance),
            });
            continue;
        }

        let selected = source.intersection(&clip);
        let paths = crate::tools::geo_multi_polygon_to_linear_paths(&selected);
        if !paths.is_empty() {
            vectors.push(RawVectorClipboard {
                vector: VectorAsset {
                    asset_id: 0,
                    paths,
                    fill: vector.fill,
                    // Partial fill selection must not invent a stroke on the cut edge.
                    stroke: None,
                },
                appearance: None,
            });
        }
    }
    vectors
}

pub fn paste_payload(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    payload: &ClipboardPayload,
    offset: Vec2,
) -> PasteResult {
    let mut result = PasteResult::default();
    if crate::tools::materialize_layer_keyframe_for_edit(project, q0rg_id, layer_id, frame)
        .is_none()
    {
        return result;
    }

    for source in &payload.raw_vectors {
        let asset_id = next_asset_id(project);
        let mut vector = source.vector.clone();
        vector.asset_id = asset_id;
        translate_vector(&mut vector, offset);
        let path_count = vector.paths.len();
        let bounds = vector_bounds(&vector);
        project.assets.push(Asset::Vector(vector));
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = &source.appearance {
            let translated = crate::appearance::transform_appearance(
                appearance,
                q0s_format::transform::Affine {
                    tx: offset.x,
                    ty: offset.y,
                    ..q0s_format::transform::Affine::IDENTITY
                },
            );
            project.asset_appearances.insert(asset_id, translated);
        }

        let Some(layer) = project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == layer_id)
            })
        else {
            continue;
        };
        let placement_idx = layer.placements.len();
        layer.placements.push(Placement {
            frame,
            target: Target::Asset(asset_id),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        });
        let placement_ref = PlacementRef {
            q0rg_id,
            layer_id,
            placement_idx,
        };
        result.raw_placements.push(placement_ref);
        result
            .raw_paths
            .extend((0..path_count).map(|path_idx| PathRef {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            }));
        if let Some((min, max)) = bounds {
            result.raw_bounds = Some(match result.raw_bounds {
                None => (min, max),
                Some((old_min, old_max)) => (
                    Vec2::new(old_min.x.min(min.x), old_min.y.min(min.y)),
                    Vec2::new(old_max.x.max(max.x), old_max.y.max(max.y)),
                ),
            });
        }
    }

    if let Some(layer) = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)
        })
    {
        for source in &payload.placements {
            let mut placement = source.clone();
            placement.frame = frame;
            placement.transform.tx += offset.x;
            placement.transform.ty += offset.y;
            let placement_idx = layer.placements.len();
            layer.placements.push(placement);
            result.objects.push(PlacementRef {
                q0rg_id,
                layer_id,
                placement_idx,
            });
        }
    }

    result
}

pub fn remove_materialized_paths_and_objects(
    project: &mut ProjectV2,
    paths: &[PathRef],
    objects: &[PlacementRef],
    frame: u16,
) -> bool {
    if paths.is_empty() && objects.is_empty() {
        return false;
    }
    let mut by_asset: BTreeMap<u16, Vec<usize>> = BTreeMap::new();
    let mut affected_layers: BTreeSet<(u16, u16)> = objects
        .iter()
        .map(|reference| (reference.q0rg_id, reference.layer_id))
        .collect();
    for reference in paths {
        affected_layers.insert((reference.q0rg_id, reference.layer_id));
        let Some(asset_id) = placement_asset_id(
            project,
            PlacementRef {
                q0rg_id: reference.q0rg_id,
                layer_id: reference.layer_id,
                placement_idx: reference.placement_idx,
            },
        ) else {
            continue;
        };
        by_asset
            .entry(asset_id)
            .or_default()
            .push(reference.path_idx);
    }

    let mut emptied_assets = BTreeSet::new();
    let mut changed = !objects.is_empty();
    for (asset_id, indices) in &mut by_asset {
        indices.sort_unstable();
        indices.dedup();
        indices.reverse();
        let Some(Asset::Vector(vector)) = project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == *asset_id)
        else {
            continue;
        };
        for index in indices.iter().copied() {
            if index < vector.paths.len() {
                vector.paths.remove(index);
                changed = true;
            }
        }
        if vector.paths.is_empty() {
            emptied_assets.insert(*asset_id);
        }
    }
    if !changed {
        return false;
    }

    let mut remove_refs = objects.to_vec();
    if !emptied_assets.is_empty() {
        for q0rg in &project.q0rgs {
            for layer in &q0rg.layers {
                for (placement_idx, placement) in layer.placements.iter().enumerate() {
                    if matches!(placement.target, Target::Asset(id) if emptied_assets.contains(&id))
                    {
                        remove_refs.push(PlacementRef {
                            q0rg_id: q0rg.q0rg_id,
                            layer_id: layer.layer_id,
                            placement_idx,
                        });
                    }
                }
            }
        }
    }
    remove_refs.sort_by(|left, right| {
        left.q0rg_id
            .cmp(&right.q0rg_id)
            .then(left.layer_id.cmp(&right.layer_id))
            .then(right.placement_idx.cmp(&left.placement_idx))
    });
    remove_refs.dedup();
    for reference in remove_refs {
        if let Some(layer) = project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == reference.layer_id)
            })
        {
            if reference.placement_idx < layer.placements.len() {
                layer.placements.remove(reference.placement_idx);
            }
        }
    }
    if !emptied_assets.is_empty() {
        project
            .assets
            .retain(|asset| !emptied_assets.contains(&asset.id()));
    }
    for (q0rg_id, layer_id) in affected_layers {
        crate::tools::preserve_blank_keyframe_after_content_delete(
            project, q0rg_id, layer_id, frame,
        );
    }
    true
}

pub fn remove_raw_area_and_objects(
    project: &mut ProjectV2,
    raw_placements: &[PlacementRef],
    objects: &[PlacementRef],
    bounds_min: Vec2,
    bounds_max: Vec2,
    frame: u16,
) -> bool {
    let clip = MultiPolygon(vec![crate::tools::rect_polygon((
        bounds_min.x,
        bounds_min.y,
        bounds_max.x,
        bounds_max.y,
    ))]);
    let affected_layers: BTreeSet<(u16, u16)> = raw_placements
        .iter()
        .chain(objects.iter())
        .map(|reference| (reference.q0rg_id, reference.layer_id))
        .collect();

    let mut mappings: BTreeMap<(u16, u16), BTreeMap<usize, usize>> = BTreeMap::new();
    for reference in raw_placements.iter().chain(objects.iter()) {
        if mappings.contains_key(&(reference.q0rg_id, reference.layer_id)) {
            continue;
        }
        let Some(mapping) = crate::tools::materialize_layer_keyframe_for_edit(
            project,
            reference.q0rg_id,
            reference.layer_id,
            frame,
        ) else {
            return false;
        };
        mappings.insert((reference.q0rg_id, reference.layer_id), mapping);
    }
    let remap = |reference: PlacementRef| -> Option<PlacementRef> {
        let mapping = mappings.get(&(reference.q0rg_id, reference.layer_id))?;
        Some(PlacementRef {
            q0rg_id: reference.q0rg_id,
            layer_id: reference.layer_id,
            placement_idx: *mapping.get(&reference.placement_idx)?,
        })
    };
    let raw_placements: Vec<PlacementRef> =
        raw_placements.iter().copied().filter_map(remap).collect();
    let objects: Vec<PlacementRef> = objects.iter().copied().filter_map(remap).collect();

    let mut groups: BTreeMap<(u16, u16), BTreeSet<u16>> = BTreeMap::new();
    for reference in &raw_placements {
        if let Some(asset_id) = placement_asset_id(project, *reference) {
            groups
                .entry((reference.q0rg_id, reference.layer_id))
                .or_default()
                .insert(asset_id);
        }
    }
    for ((q0rg_id, source_layer_id), asset_ids) in groups {
        crate::brush::prepare_writable_raw_assets(
            project,
            q0rg_id,
            source_layer_id,
            frame,
            &asset_ids,
        );
    }

    let mut remove_refs = objects.clone();
    let mut changed = !objects.is_empty();
    for reference in &raw_placements {
        let Some(asset_id) = placement_asset_id(project, *reference) else {
            continue;
        };
        let Some(Asset::Vector(snapshot)) = project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
            .cloned()
        else {
            continue;
        };
        let surface = crate::tools::vector_fill_geometry(&snapshot);
        let selected = surface.intersection(&clip);
        #[cfg(feature = "appearance-mask-eraser")]
        let appearance = project.asset_appearances.get(&asset_id).cloned();
        #[cfg(feature = "appearance-mask-eraser")]
        let visible = appearance.as_ref().map(|appearance| {
            crate::appearance::visible_material_surface_for_vector(&snapshot, Some(appearance))
        });
        #[cfg(feature = "appearance-mask-eraser")]
        let selected_visual_area = visible
            .as_ref()
            .map(|visible| visible.intersection(&clip).unsigned_area());
        #[cfg(not(feature = "appearance-mask-eraser"))]
        let selected_visual_area: Option<f64> = None;
        if selected_visual_area.unwrap_or_else(|| selected.unsigned_area()) <= 0.05 {
            continue;
        }
        changed = true;
        let remainder = surface.difference(&clip);

        #[cfg(feature = "appearance-mask-eraser")]
        let visible_remainder_area = visible
            .as_ref()
            .map(|visible| visible.difference(&clip).unsigned_area());
        #[cfg(not(feature = "appearance-mask-eraser"))]
        let visible_remainder_area: Option<f64> = None;

        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = appearance {
            if let Some((source_appearance, _)) =
                crate::appearance::partition_appearance(&appearance, &snapshot.paths, &clip)
            {
                project
                    .asset_appearances
                    .insert(asset_id, source_appearance);
            }
            if visible_remainder_area.is_some_and(|area| area <= 0.05) {
                remove_refs.push(*reference);
                continue;
            }
        }

        if selected.unsigned_area() <= 0.05 {
            // Halo-only removal changed just the post-material clip. The carrier
            // vector stays untouched and remains invisible outside that clip.
            continue;
        }

        let mut paths: Vec<VPath> = snapshot
            .paths
            .iter()
            .filter(|path| !path.closed)
            .cloned()
            .collect();
        if remainder.unsigned_area() > 0.05 {
            paths.extend(crate::tools::geo_multi_polygon_to_linear_paths(&remainder));
        } else if visible_remainder_area.is_some_and(|area| area > 0.05) {
            // The body was fully selected but some resolved material remains.
            // Keep closed source paths only as an internal carrier for that halo.
            paths.extend(snapshot.paths.iter().filter(|path| path.closed).cloned());
        }
        if paths.is_empty() {
            remove_refs.push(*reference);
        } else if let Some(Asset::Vector(vector)) = project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == asset_id)
        {
            vector.paths = paths;
        }
    }

    remove_refs.sort_by(|left, right| {
        left.q0rg_id
            .cmp(&right.q0rg_id)
            .then(left.layer_id.cmp(&right.layer_id))
            .then(right.placement_idx.cmp(&left.placement_idx))
    });
    remove_refs.dedup();
    for reference in remove_refs {
        if let Some(layer) = project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == reference.layer_id)
            })
        {
            if reference.placement_idx < layer.placements.len() {
                layer.placements.remove(reference.placement_idx);
            }
        }
    }
    if changed {
        for (q0rg_id, layer_id) in affected_layers {
            crate::tools::preserve_blank_keyframe_after_content_delete(
                project, q0rg_id, layer_id, frame,
            );
        }
    }

    changed
}

pub fn selection_from_paste(result: PasteResult) -> Selection {
    if !result.raw_placements.is_empty() && !result.objects.is_empty() {
        let (bounds_min, bounds_max) = result
            .raw_bounds
            .unwrap_or((Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0)));
        return Selection::RawArea {
            placements: result.raw_placements,
            objects: result.objects,
            bounds_min,
            bounds_max,
        };
    }
    if !result.raw_paths.is_empty() {
        return match result.raw_paths.len() {
            1 => {
                let reference = result.raw_paths[0];
                Selection::Path {
                    q0rg_id: reference.q0rg_id,
                    layer_id: reference.layer_id,
                    placement_idx: reference.placement_idx,
                    path_idx: reference.path_idx,
                }
            }
            _ => Selection::Paths(result.raw_paths),
        };
    }
    match result.objects.len() {
        0 => Selection::None,
        1 => Selection::Placement {
            q0rg_id: result.objects[0].q0rg_id,
            layer_id: result.objects[0].layer_id,
            placement_idx: result.objects[0].placement_idx,
        },
        _ => Selection::Multi(result.objects),
    }
}

fn placement_clone(project: &ProjectV2, reference: PlacementRef) -> Option<Placement> {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == reference.layer_id)?
        .placements
        .get(reference.placement_idx)
        .cloned()
}

fn placement_asset_id(project: &ProjectV2, reference: PlacementRef) -> Option<u16> {
    match placement_clone(project, reference)?.target {
        Target::Asset(asset_id) => Some(asset_id),
        Target::Q0rg(_) => None,
    }
}

fn next_asset_id(project: &ProjectV2) -> u16 {
    project
        .assets
        .iter()
        .map(Asset::id)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
        .max(1)
}

fn translate_vector(vector: &mut VectorAsset, offset: Vec2) {
    for path in &mut vector.paths {
        for anchor in &mut path.anchors {
            anchor.point.x += offset.x;
            anchor.point.y += offset.y;
            if let Some(handle) = &mut anchor.in_handle {
                handle.x += offset.x;
                handle.y += offset.y;
            }
            if let Some(handle) = &mut anchor.out_handle {
                handle.x += offset.x;
                handle.y += offset.y;
            }
        }
    }
}

fn vector_bounds(vector: &VectorAsset) -> Option<(Vec2, Vec2)> {
    let mut min = Vec2::new(f32::INFINITY, f32::INFINITY);
    let mut max = Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut found = false;
    for path in &vector.paths {
        for anchor in &path.anchors {
            min.x = min.x.min(anchor.point.x);
            min.y = min.y.min(anchor.point.y);
            max.x = max.x.max(anchor.point.x);
            max.y = max.y.max(anchor.point.y);
            found = true;
        }
    }
    found.then_some((min, max))
}
