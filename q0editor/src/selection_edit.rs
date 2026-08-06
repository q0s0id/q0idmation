use std::collections::{BTreeMap, BTreeSet};

use geo::{Area, BooleanOps};
use q0s_format::v2::{
    Asset, Path as VPath, Placement, ProjectV2, Target, Transform2D, Tween, Vec2, VectorAsset,
};

use crate::state::{ClipboardPayload, PathRef, PlacementRef, Selection};

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
        Selection::RawArea { objects, .. } => objects.clone(),
        _ => Vec::new(),
    };
    refs.into_iter()
        .filter_map(|reference| placement_clone(project, reference))
        .collect()
}

fn selected_raw_vectors(project: &ProjectV2, selection: &Selection) -> Vec<VectorAsset> {
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
            if !paths.is_empty() {
                vectors.push(VectorAsset {
                    asset_id: 0,
                    paths,
                    fill: vector.fill,
                    stroke: vector.stroke,
                });
            }
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
    let clip = crate::tools::rect_polygon((bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y));
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
        let selected = crate::tools::vector_fill_geometry(vector).intersection(&clip);
        let paths = crate::tools::geo_multi_polygon_to_linear_paths(&selected);
        if !paths.is_empty() {
            vectors.push(VectorAsset {
                asset_id: 0,
                paths,
                fill: vector.fill,
                // Partial fill selection must not invent a stroke on the cut edge.
                stroke: None,
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
        let mut vector = source.clone();
        vector.asset_id = asset_id;
        translate_vector(&mut vector, offset);
        let path_count = vector.paths.len();
        let bounds = vector_bounds(&vector);
        project.assets.push(Asset::Vector(vector));

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

pub fn remove_raw_area_and_objects(
    project: &mut ProjectV2,
    raw_placements: &[PlacementRef],
    objects: &[PlacementRef],
    bounds_min: Vec2,
    bounds_max: Vec2,
    frame: u16,
) -> bool {
    let clip = crate::tools::rect_polygon((bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y));

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
        if selected.unsigned_area() <= 0.05 {
            continue;
        }
        changed = true;
        let remainder = surface.difference(&clip);
        let mut paths: Vec<VPath> = snapshot
            .paths
            .iter()
            .filter(|path| !path.closed)
            .cloned()
            .collect();
        paths.extend(crate::tools::geo_multi_polygon_to_linear_paths(&remainder));
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
