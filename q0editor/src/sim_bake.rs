//! Shared baking for generated artwork. Liquids and particle emitters draw
//! onto layers of their own, with their drawings filed in library folders of
//! their own, so a bake can be redone without touching anything else.

use std::collections::{BTreeSet, HashSet};

use q0s_format::v2::{
    Asset, Layer, LibraryItemReference, Placement, ProjectV2, Rgba, Target, VectorAsset,
};

pub const PHYSICS_FOLDER: &str = "q0 physics";
pub const PARTICLES_FOLDER: &str = "q0 particles";

/// Hands out free asset ids for one bake.
pub struct AssetIds {
    used: HashSet<u16>,
    next: u16,
}

impl AssetIds {
    pub fn new(project: &ProjectV2) -> Self {
        Self {
            used: project.assets.iter().map(Asset::id).collect(),
            next: 1,
        }
    }

    pub fn take(&mut self) -> Result<u16, String> {
        while self.used.contains(&self.next) {
            self.next = self
                .next
                .checked_add(1)
                .ok_or("the project has no asset ids left; bake fewer frames")?;
        }
        let id = self.next;
        self.used.insert(id);
        Ok(id)
    }
}

/// The layer `layer_id` names in `q0rg_id`, or a new one called `name`,
/// drawn above everything else.
pub fn effect_layer(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: Option<u16>,
    name: &str,
) -> Result<u16, String> {
    if let Some(layer_id) = layer_id {
        let exists = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .is_some_and(|q0rg| q0rg.layers.iter().any(|layer| layer.layer_id == layer_id))
            && !project.layer_is_folder(q0rg_id, layer_id);
        if exists {
            if project.layer_is_locked(q0rg_id, layer_id) {
                return Err(format!("unlock the layer \"{name}\" first"));
            }
            return Ok(layer_id);
        }
    }
    let q0rg = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .ok_or("the timeline is gone")?;
    let next_id = q0rg
        .layers
        .iter()
        .map(|layer| layer.layer_id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("this timeline has no layer ids left")?;
    q0rg.layers.push(Layer {
        layer_id: next_id,
        timeline_frame_count: None,
        name: name.to_string(),
        explicit_keyframes: vec![0],
        placements: Vec::new(),
    });
    Ok(next_id)
}

/// Empties frames `start..=end` of `layer_id`, and deletes the drawings an
/// earlier bake filed under `folder` once nothing uses them any more.
pub fn clear_range(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    start: u16,
    end: u16,
    folder: &str,
) {
    let mut released = BTreeSet::new();
    let Some(layer) = layer_mut(project, q0rg_id, layer_id) else {
        return;
    };
    layer.placements.retain(|placement| {
        let inside = (start..=end).contains(&placement.frame);
        if inside {
            if let Target::Asset(id) = placement.target {
                released.insert(id);
            }
        }
        !inside
    });
    layer
        .explicit_keyframes
        .retain(|frame| *frame == 0 || !(start..=end).contains(frame));
    if released.is_empty() {
        return;
    }
    let still_used = project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
        .filter_map(|placement| match placement.target {
            Target::Asset(id) => Some(id),
            Target::Q0rg(_) => None,
        })
        .collect::<HashSet<_>>();
    let doomed = released
        .into_iter()
        .filter(|id| !still_used.contains(id) && filed_under(project, *id, folder))
        .collect::<Vec<_>>();
    crate::tools::remove_assets_and_metadata(project, doomed);
}

fn filed_under(project: &ProjectV2, asset_id: u16, folder: &str) -> bool {
    project
        .runtime
        .library_item_folders
        .get(&LibraryItemReference::Asset(asset_id))
        .is_some_and(|path| path == folder || path.starts_with(&format!("{folder}/")))
}

/// Adds a filled drawing filed under `folder`; returns its id.
pub fn add_drawing(
    project: &mut ProjectV2,
    ids: &mut AssetIds,
    paths: Vec<q0s_format::v2::Path>,
    fill: Rgba,
    name: String,
    folder: &str,
) -> Result<u16, String> {
    let asset_id = ids.take()?;
    project.assets.push(Asset::Vector(VectorAsset {
        asset_id,
        paths,
        fill: Some(fill),
        stroke: None,
    }));
    project.asset_names.insert(asset_id, name);
    file_under(project, LibraryItemReference::Asset(asset_id), folder);
    Ok(asset_id)
}

/// Adds a drawing of lines, `stroke` thick, filed under `folder`; returns
/// its id.
pub fn add_line_drawing(
    project: &mut ProjectV2,
    ids: &mut AssetIds,
    paths: Vec<q0s_format::v2::Path>,
    stroke: q0s_format::v2::Stroke,
    name: String,
    folder: &str,
) -> Result<u16, String> {
    let asset_id = ids.take()?;
    project.assets.push(Asset::Vector(VectorAsset {
        asset_id,
        paths,
        fill: None,
        stroke: Some(stroke),
    }));
    project.asset_names.insert(asset_id, name);
    file_under(project, LibraryItemReference::Asset(asset_id), folder);
    Ok(asset_id)
}

/// Puts `item` in the library folder `path`, making the folder as needed.
pub fn file_under(project: &mut ProjectV2, item: LibraryItemReference, path: &str) {
    let mut prefix = String::new();
    for part in path.split('/').filter(|part| !part.is_empty()) {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(part);
        if !project.runtime.library_folders.contains(&prefix) {
            project.runtime.library_folders.push(prefix.clone());
        }
    }
    project
        .runtime
        .library_item_folders
        .insert(item, path.to_string());
}

/// Keys `frame` of `layer_id` with `placements`; an empty frame gets a blank
/// keyframe, so whatever came before stops there.
pub fn key_frame(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    placements: Vec<Placement>,
) {
    let Some(layer) = layer_mut(project, q0rg_id, layer_id) else {
        return;
    };
    layer.ensure_explicit_keyframe(frame);
    for placement in placements {
        layer.push_placement(placement);
    }
}

/// Makes the timeline of `q0rg_id` reach `frame` on `layer_id`.
pub fn reach_frame(project: &mut ProjectV2, q0rg_id: u16, layer_id: u16, frame: u16) {
    let _ =
        crate::timeline_edit::extend_layer_through_frame(project, q0rg_id, layer_id, frame, false);
}

fn layer_mut(project: &mut ProjectV2, q0rg_id: u16, layer_id: u16) -> Option<&mut Layer> {
    project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == layer_id)
}

/// A closed straight-edged path through `points`.
pub fn polygon_path(points: &[[f32; 2]]) -> q0s_format::v2::Path {
    q0s_format::v2::Path {
        anchors: points
            .iter()
            .map(|point| q0s_format::v2::Anchor {
                point: q0s_format::v2::Vec2::new(point[0], point[1]),
                in_handle: None,
                out_handle: None,
            })
            .collect(),
        closed: true,
    }
}
