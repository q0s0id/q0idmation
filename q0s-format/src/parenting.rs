//! Layer parenting, as in Animate: a layer can hang from another layer of
//! the same timeline. Its objects then ride on the parent layer's object:
//! they move, turn, scale and skew with it, while their own keyframes and
//! tweens stay relative to it. Chains are allowed; loops are not.
//!
//! The object a parent layer offers is the first one on it at the frame
//! (Animate expects one per keyframe as well). A parent layer with nothing on
//! a frame leaves its children where their own transforms put them.
//!
//! Renderers see parenting as extra rig bindings: `q0rg_pose` is the pose of
//! a timeline frame with the rig's own bindings first, so everything that
//! already honours rig bindings (drawing, hit tests, collisions, the player)
//! places parented objects without knowing about layers.

use std::collections::HashSet;

use crate::error::Error;
use crate::rig::{ResolvedRigBinding, RigControlOverride, RigPose};
use crate::transform::Affine;
use crate::v2::{Layer, LayerKey, LayerKind, ProjectV2, Q0rg};

/// The node id parent bindings carry: they come from no rig node.
pub const PARENT_BINDING_NODE: u16 = u16::MAX;

/// The main timeline's camera in parent links. A layer that hangs from it
/// stays put in the camera's view (a HUD); a camera that hangs from a layer
/// follows where that layer's object goes, its own keys relative to it.
pub const CAMERA_LAYER: u16 = u16::MAX;

/// Chains longer than this are treated as broken.
const MAX_DEPTH: usize = 256;

/// Whether `layer_id` names the camera of `q0rg_id` (every scene's own
/// timeline has one).
pub fn is_camera(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> bool {
    layer_id == CAMERA_LAYER && project.is_scene_root(q0rg_id)
}

/// A layer of the timeline, or its camera, that can take part in a link.
fn linkable(project: &ProjectV2, q0rg: &Q0rg, layer_id: u16) -> bool {
    if is_camera(project, q0rg.q0rg_id, layer_id) {
        return true;
    }
    layer(q0rg, layer_id).is_some()
        && project.layer_metadata(q0rg.q0rg_id, layer_id).kind != LayerKind::Folder
}

/// Where the camera's view sits on the stage at `frame`: the stage's own
/// rectangle mapped through it, so the identity camera is the identity.
fn camera_frame(project: &ProjectV2, q0rg_id: u16, frame: u16) -> Option<Affine> {
    project.camera_affine_in(q0rg_id, frame).inverse()
}

/// How far the camera's parent has carried it at `frame`: where the parent
/// layer's object sits, added to the camera's own keyed centre. None when the
/// camera follows nothing or its parent is empty there.
pub fn camera_offset(project: &ProjectV2, frame: u16) -> Option<(f32, f32)> {
    camera_offset_in(project, project.meta.entry_q0rg_id, frame)
}

/// `camera_offset` for the camera of the scene rooted at `scene`.
pub fn camera_offset_in(project: &ProjectV2, scene: u16, frame: u16) -> Option<(f32, f32)> {
    let parent = parent_of(project, scene, CAMERA_LAYER)?;
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == scene)?;
    let rig = crate::rig::rig_for_q0rg(project, scene)
        .map(|rig| crate::rig::evaluate_rig(rig, f32::from(frame), &[]));
    let world = layer_world(project, q0rg, parent, frame, rig.as_ref())?;
    Some((world.tx, world.ty))
}

pub fn parent_of(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> Option<u16> {
    project
        .runtime
        .layer_parents
        .get(&LayerKey { q0rg_id, layer_id })
        .copied()
}

/// Whether any layer of the timeline has a parent.
pub fn has_parents(project: &ProjectV2, q0rg_id: u16) -> bool {
    project
        .runtime
        .layer_parents
        .keys()
        .any(|key| key.q0rg_id == q0rg_id)
}

/// Whether hanging `layer_id` from `parent_id` would close a loop.
pub fn would_loop(project: &ProjectV2, q0rg_id: u16, layer_id: u16, parent_id: u16) -> bool {
    let mut current = Some(parent_id);
    for _ in 0..MAX_DEPTH {
        match current {
            Some(id) if id == layer_id => return true,
            Some(id) => current = parent_of(project, q0rg_id, id),
            None => return false,
        }
    }
    true
}

/// How many ancestors `layer_id` hangs below (0 for a layer without one).
pub fn depth(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> usize {
    let mut count = 0;
    let mut current = parent_of(project, q0rg_id, layer_id);
    while let Some(id) = current {
        count += 1;
        if count >= MAX_DEPTH {
            break;
        }
        current = parent_of(project, q0rg_id, id);
    }
    count
}

fn layer(q0rg: &Q0rg, layer_id: u16) -> Option<&Layer> {
    q0rg.layers.iter().find(|layer| layer.layer_id == layer_id)
}

/// Where the object `layer_id` offers its children sits on the timeline at
/// `frame`: its first object, after the layer's own parents (or its rig
/// binding, which wins).
pub fn layer_world(
    project: &ProjectV2,
    q0rg: &Q0rg,
    layer_id: u16,
    frame: u16,
    rig: Option<&RigPose>,
) -> Option<Affine> {
    let mut chain = Vec::new();
    let mut current = Some(layer_id);
    while let Some(id) = current {
        if chain.len() >= MAX_DEPTH || chain.contains(&id) {
            return None;
        }
        chain.push(id);
        current = parent_of(project, q0rg.q0rg_id, id);
    }
    // From the top of the chain down: each layer's object after its parent's.
    let mut world: Option<Affine> = None;
    for id in chain.into_iter().rev() {
        let own = if is_camera(project, q0rg.q0rg_id, id) {
            // The camera's place already has its own parent in it.
            camera_frame(project, q0rg.q0rg_id, frame).map(|affine| (affine, true))
        } else {
            first_object(q0rg, id, frame, rig)
        };
        let Some(own) = own else {
            // An empty ancestor passes its own parent's placement through.
            continue;
        };
        world = Some(match (own, world) {
            ((affine, true), _) => affine,
            ((affine, false), Some(parent)) => Affine::compose(parent, affine),
            ((affine, false), None) => affine,
        });
    }
    world
}

/// The first object on a layer at `frame` and whether a rig places it.
fn first_object(
    q0rg: &Q0rg,
    layer_id: u16,
    frame: u16,
    rig: Option<&RigPose>,
) -> Option<(Affine, bool)> {
    let layer = layer(q0rg, layer_id)?;
    let active = crate::raster::active_placement_states_at(layer, frame)
        .into_iter()
        .next()?;
    let placement = layer.placements.get(active.index)?;
    Some(
        match rig.and_then(|pose| pose.binding_transform(placement.instance_id)) {
            Some(bound) => (bound, true),
            None => (Affine::from_transform(active.transform), false),
        },
    )
}

/// The parent placement `layer_id`'s objects ride on at `frame`, when it has
/// a parent with an object on that frame.
pub fn parent_world(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Option<Affine> {
    let parent = parent_of(project, q0rg_id, layer_id)?;
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let rig = crate::rig::rig_for_q0rg(project, q0rg_id)
        .map(|rig| crate::rig::evaluate_rig(rig, f32::from(frame), &[]));
    layer_world(project, q0rg, parent, frame, rig.as_ref())
}

/// Where the objects of parented layers go at `frame`, as rig bindings.
/// Objects a rig binds keep the rig's placement.
pub fn parent_bindings(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    rig: Option<&RigPose>,
) -> Vec<ResolvedRigBinding> {
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    let mut keys: Vec<_> = project
        .runtime
        .layer_parents
        .iter()
        .filter(|(key, _)| key.q0rg_id == q0rg_id)
        .collect();
    keys.sort();
    let mut bindings = Vec::new();
    for (key, &parent) in keys {
        let Some(parent_world) = layer_world(project, q0rg, parent, frame, rig) else {
            continue;
        };
        let Some(layer) = layer(q0rg, key.layer_id) else {
            continue;
        };
        for active in crate::raster::active_placement_states_at(layer, frame) {
            let Some(placement) = layer.placements.get(active.index) else {
                continue;
            };
            if placement.instance_id == 0
                || rig.is_some_and(|pose| pose.binding_transform(placement.instance_id).is_some())
            {
                continue;
            }
            bindings.push(ResolvedRigBinding {
                node_id: PARENT_BINDING_NODE,
                instance_id: placement.instance_id,
                transform: Affine::compose(parent_world, Affine::from_transform(active.transform)),
            });
        }
    }
    bindings
}

/// The pose a renderer uses for `q0rg_id` at `frame`: the rig's, with the
/// objects of parented layers bound too. None when the timeline has neither.
pub fn q0rg_pose(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: f32,
    overrides: &[RigControlOverride],
) -> Option<RigPose> {
    let rig = crate::rig::rig_for_q0rg(project, q0rg_id)
        .map(|rig| crate::rig::evaluate_rig(rig, frame, overrides));
    let whole_frame = frame.round().clamp(0.0, f32::from(u16::MAX)) as u16;
    merge_into(project, q0rg_id, whole_frame, rig)
}

/// `rig` (a timeline's rig pose, if it has a rig) with the objects of its
/// parented layers bound as well: what every renderer asks a pose for.
pub fn merge_into(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    rig: Option<RigPose>,
) -> Option<RigPose> {
    if !has_parents(project, q0rg_id) {
        return rig;
    }
    let bindings = parent_bindings(project, q0rg_id, frame, rig.as_ref());
    if bindings.is_empty() {
        return rig;
    }
    let mut pose = rig.unwrap_or_else(|| RigPose {
        node_local: Default::default(),
        node_world: Default::default(),
        controls: Default::default(),
        constraint_weights: Default::default(),
        bindings: Vec::new(),
    });
    pose.bindings.extend(bindings);
    Some(pose)
}

/// Gives every object on a parented layer an instance id of its own, which
/// the bindings need. Returns whether anything changed.
pub fn give_parented_objects_ids(project: &mut ProjectV2) -> bool {
    if project.runtime.layer_parents.is_empty() {
        return false;
    }
    let parented: HashSet<LayerKey> = project.runtime.layer_parents.keys().copied().collect();
    let needs_ids = project.q0rgs.iter().any(|q0rg| {
        q0rg.layers.iter().any(|layer| {
            parented.contains(&LayerKey {
                q0rg_id: q0rg.q0rg_id,
                layer_id: layer.layer_id,
            }) && layer
                .placements
                .iter()
                .any(|placement| placement.instance_id == 0)
        })
    });
    if !needs_ids {
        return false;
    }
    let mut next = project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
        .map(|placement| placement.instance_id)
        .max()
        .unwrap_or(0);
    let mut changed = false;
    for q0rg in &mut project.q0rgs {
        let q0rg_id = q0rg.q0rg_id;
        for layer in &mut q0rg.layers {
            if !parented.contains(&LayerKey {
                q0rg_id,
                layer_id: layer.layer_id,
            }) {
                continue;
            }
            for placement in &mut layer.placements {
                if placement.instance_id == 0 {
                    let Some(id) = next.checked_add(1) else {
                        return changed;
                    };
                    next = id;
                    placement.instance_id = id;
                    changed = true;
                }
            }
        }
    }
    changed
}

/// Hangs `layer_id` from `parent` (or frees it with None) and rewrites the
/// layer's keyframes so its objects stay exactly where they are: each one
/// becomes relative to the parent's object on its own frame.
pub fn set_parent(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    parent: Option<u16>,
) -> Result<(), Error> {
    if let Some(parent) = parent {
        let q0rg = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .ok_or(Error::Validation("layer parent on a missing timeline"))?;
        for id in [layer_id, parent] {
            if !linkable(project, q0rg, id) {
                return Err(
                    if layer(q0rg, id).is_some() || is_camera(project, q0rg_id, id) {
                        Error::Validation("folders cannot take part in layer parenting")
                    } else {
                        Error::Validation("layer parent names a missing layer")
                    },
                );
            }
        }
        if parent == layer_id || would_loop(project, q0rg_id, layer_id, parent) {
            return Err(Error::Validation("layer parents must not loop"));
        }
    }
    let key = LayerKey { q0rg_id, layer_id };
    if project.runtime.layer_parents.get(&key).copied() == parent {
        return Ok(());
    }
    if is_camera(project, q0rg_id, layer_id) {
        set_camera_parent(project, key, parent);
        return Ok(());
    }
    // Each keyframe's place on the stage, under the old parent.
    let frames: Vec<(usize, u16)> = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| layer(q0rg, layer_id))
        .map(|layer| {
            layer
                .placements
                .iter()
                .enumerate()
                .map(|(index, placement)| (index, placement.frame))
                .collect()
        })
        .unwrap_or_default();
    let worlds: Vec<(usize, Affine)> = frames
        .iter()
        .filter_map(|&(index, frame)| {
            let transform = project
                .q0rgs
                .iter()
                .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                .and_then(|q0rg| layer(q0rg, layer_id))
                .and_then(|layer| layer.placements.get(index))?
                .transform;
            let own = Affine::from_transform(transform);
            Some((
                index,
                match parent_world(project, q0rg_id, layer_id, frame) {
                    Some(old) => Affine::compose(old, own),
                    None => own,
                },
            ))
        })
        .collect();
    match parent {
        Some(parent) => project.runtime.layer_parents.insert(key, parent),
        None => project.runtime.layer_parents.remove(&key),
    };
    // ...and the same place under the new one.
    let locals: Vec<(usize, Affine)> = worlds
        .into_iter()
        .map(|(index, world)| {
            let frame = frames
                .iter()
                .find(|(candidate, _)| *candidate == index)
                .map_or(0, |(_, frame)| *frame);
            let local = parent_world(project, q0rg_id, layer_id, frame)
                .and_then(|new| new.inverse())
                .map_or(world, |inverse| Affine::compose(inverse, world));
            (index, local)
        })
        .collect();
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
        for (index, local) in locals {
            if let (Some(placement), Some(transform)) =
                (layer.placements.get_mut(index), affine_to_transform(local))
            {
                placement.transform = transform;
            }
        }
    }
    give_parented_objects_ids(project);
    Ok(())
}

/// The camera's half of `set_parent`: its keys' centres move by the old
/// parent's place minus the new one's, so the view stays where it was on
/// every key. A camera without keys gets one at frame 0 first.
fn set_camera_parent(project: &mut ProjectV2, key: LayerKey, parent: Option<u16>) {
    let scene = key.q0rg_id;
    if parent.is_some() {
        let rest = project.default_camera_state();
        if let Some(track) = project.scene_camera_mut(scene) {
            if !track.enabled || track.keys.is_empty() {
                track.enabled = true;
                if track.keys.is_empty() {
                    track.insert_key(0, rest);
                }
            }
        }
    }
    let frames: Vec<u16> = project
        .scene_camera(scene)
        .keys
        .iter()
        .map(|key| key.frame)
        .collect();
    let offsets = |project: &ProjectV2| -> Vec<(f32, f32)> {
        frames
            .iter()
            .map(|&frame| camera_offset_in(project, scene, frame).unwrap_or((0.0, 0.0)))
            .collect()
    };
    let before = offsets(project);
    match parent {
        Some(parent) => project.runtime.layer_parents.insert(key, parent),
        None => project.runtime.layer_parents.remove(&key),
    };
    let after = offsets(project);
    let Some(track) = project.scene_camera_mut(scene) else {
        return;
    };
    for ((camera_key, old), new) in track.keys.iter_mut().zip(before).zip(after) {
        camera_key.state.center_x += old.0 - new.0;
        camera_key.state.center_y += old.1 - new.1;
    }
}

/// The scale, shear, turn and move that rebuild `affine` through
/// `Affine::from_transform`, or None for a flat matrix.
pub fn affine_to_transform(affine: Affine) -> Option<crate::v2::Transform2D> {
    let sx = affine.a11.hypot(affine.a21);
    let determinant = affine.a11 * affine.a22 - affine.a12 * affine.a21;
    if !sx.is_finite() || sx <= 1.0e-7 || !determinant.is_finite() || determinant.abs() <= 1.0e-9 {
        return None;
    }
    // from_transform with skew_y = 0: [cos -sin; sin cos] · [sx, sy·tan(kx); 0, sy].
    let rotation = affine.a21.atan2(affine.a11);
    let (sin, cos) = rotation.sin_cos();
    // Undo the turn, leaving the upper-triangular scale and shear.
    let m12 = cos * affine.a12 + sin * affine.a22;
    let m22 = -sin * affine.a12 + cos * affine.a22;
    let sy = m22;
    let skew_x = if sy.abs() > 1.0e-9 {
        (m12 / sy).atan()
    } else {
        0.0
    };
    Some(crate::v2::Transform2D {
        tx: affine.tx,
        ty: affine.ty,
        sx,
        sy,
        rotation,
        skew_x,
        skew_y: 0.0,
    })
}

/// Frees the children of layers that are about to be deleted, keeping them
/// where they are on the stage, and forgets the leaving layers' own links.
pub fn release_children(project: &mut ProjectV2, q0rg_id: u16, leaving: &[u16]) {
    let mut children: Vec<u16> = project
        .runtime
        .layer_parents
        .iter()
        .filter(|(key, parent)| {
            key.q0rg_id == q0rg_id && leaving.contains(parent) && !leaving.contains(&key.layer_id)
        })
        .map(|(key, _)| key.layer_id)
        .collect();
    children.sort_unstable();
    for child in children {
        let _ = set_parent(project, q0rg_id, child, None);
    }
    project
        .runtime
        .layer_parents
        .retain(|key, _| !(key.q0rg_id == q0rg_id && leaving.contains(&key.layer_id)));
}

/// Drops links that no longer hold (a layer gone, turned into a folder, a
/// loop) and gives new objects on parented layers their ids. Cheap when
/// nothing is parented. Returns whether the project changed.
pub fn tidy(project: &mut ProjectV2) -> bool {
    if project.runtime.layer_parents.is_empty() {
        return false;
    }
    let before = project.runtime.layer_parents.len();
    let q0rgs = &project.q0rgs;
    let metadata = &project.layer_metadata;
    let scenes = project
        .scenes()
        .into_iter()
        .map(|scene| scene.q0rg_id)
        .collect::<Vec<_>>();
    let usable = |q0rg_id: u16, layer_id: u16| {
        if layer_id == CAMERA_LAYER && scenes.contains(&q0rg_id) {
            return q0rgs.iter().any(|q0rg| q0rg.q0rg_id == q0rg_id);
        }
        q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .is_some_and(|q0rg| layer(q0rg, layer_id).is_some())
            && metadata
                .get(&LayerKey { q0rg_id, layer_id })
                .is_none_or(|meta| meta.kind != LayerKind::Folder)
    };
    project.runtime.layer_parents.retain(|key, parent| {
        *parent != key.layer_id && usable(key.q0rg_id, key.layer_id) && usable(key.q0rg_id, *parent)
    });
    let mut keys: Vec<(LayerKey, u16)> = project
        .runtime
        .layer_parents
        .iter()
        .map(|(key, parent)| (*key, *parent))
        .collect();
    keys.sort();
    for (key, parent) in keys {
        if would_loop(project, key.q0rg_id, key.layer_id, parent) {
            project.runtime.layer_parents.remove(&key);
        }
    }
    let pruned = project.runtime.layer_parents.len() != before;
    give_parented_objects_ids(project) || pruned
}

/// Checks every parent link: both layers exist on the timeline, neither is
/// a folder, and no chain loops.
pub fn validate(project: &ProjectV2) -> Result<(), Error> {
    for (key, &parent) in &project.runtime.layer_parents {
        let q0rg = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == key.q0rg_id)
            .ok_or(Error::Validation("layer parent on a missing timeline"))?;
        for id in [key.layer_id, parent] {
            if !linkable(project, q0rg, id) {
                return Err(if layer(q0rg, id).is_some() {
                    Error::Validation("folders cannot take part in layer parenting")
                } else {
                    Error::Validation("layer parent names a missing layer")
                });
            }
        }
        if would_loop(project, key.q0rg_id, key.layer_id, parent) {
            return Err(Error::Validation("layer parents must not loop"));
        }
    }
    Ok(())
}

pub(crate) fn write(out: &mut Vec<u8>, project: &ProjectV2) -> Result<(), Error> {
    let count = u16::try_from(project.runtime.layer_parents.len())
        .map_err(|_| Error::Overflow("layer parent count exceeds u16"))?;
    out.extend_from_slice(&count.to_le_bytes());
    let mut links: Vec<_> = project.runtime.layer_parents.iter().collect();
    links.sort();
    for (key, parent) in links {
        out.extend_from_slice(&key.q0rg_id.to_le_bytes());
        out.extend_from_slice(&key.layer_id.to_le_bytes());
        out.extend_from_slice(&parent.to_le_bytes());
    }
    Ok(())
}

pub(crate) fn read(
    c: &mut crate::io::Cursor<'_>,
) -> Result<std::collections::HashMap<LayerKey, u16>, Error> {
    let count = c.read_u16()?;
    let mut links = std::collections::HashMap::with_capacity(usize::from(count));
    for _ in 0..count {
        let key = LayerKey {
            q0rg_id: c.read_u16()?,
            layer_id: c.read_u16()?,
        };
        let parent = c.read_u16()?;
        if links.insert(key, parent).is_some() {
            return Err(Error::Validation("a layer has two parents"));
        }
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::{
        Asset, Placement, PlacementFx, ProjectMeta, Rgba, Target, Transform2D, Tween, VectorAsset,
    };

    fn placement(frame: u16, tx: f32, ty: f32, rotation: f32) -> Placement {
        Placement {
            pivot: None,
            instance_id: 0,
            frame,
            target: Target::Asset(1),
            transform: Transform2D {
                tx,
                ty,
                rotation,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: PlacementFx::default(),
        }
    }

    fn layer_with(layer_id: u16, name: &str, placement: Placement) -> Layer {
        Layer {
            layer_id,
            timeline_frame_count: None,
            name: name.into(),
            explicit_keyframes: vec![0],
            placements: vec![placement],
        }
    }

    /// An arm turned a quarter at (100, 100) and a hand 50 px to its right.
    fn project() -> ProjectV2 {
        ProjectV2 {
            meta: ProjectMeta {
                name: "parenting".into(),
                fps: 24,
                stage_width: 400,
                stage_height: 300,
                background_color: Rgba {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                },
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            })],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![
                    layer_with(
                        1,
                        "arm",
                        placement(0, 100.0, 100.0, std::f32::consts::FRAC_PI_2),
                    ),
                    layer_with(2, "hand", placement(0, 150.0, 100.0, 0.0)),
                ],
            }],
        }
    }

    fn shown(project: &ProjectV2, layer_id: u16) -> Affine {
        let q0rg = &project.q0rgs[0];
        let placement = &layer(q0rg, layer_id).unwrap().placements[0];
        q0rg_pose(project, q0rg.q0rg_id, 0.0, &[])
            .and_then(|pose| pose.binding_transform(placement.instance_id))
            .unwrap_or_else(|| Affine::from_transform(placement.transform))
    }

    fn same_place(a: Affine, b: Affine) -> bool {
        (a.tx - b.tx).abs() < 1e-3 && (a.ty - b.ty).abs() < 1e-3
    }

    #[test]
    fn a_child_keeps_its_place_and_then_follows_its_parent() {
        let mut project = project();
        let before = shown(&project, 2);
        set_parent(&mut project, 1, 2, Some(1)).unwrap();
        assert_ne!(
            project.q0rgs[0].layers[1].placements[0].instance_id, 0,
            "children get ids"
        );
        assert!(same_place(shown(&project, 2), before));
        // Stored relative to the arm, which is turned a quarter: 50 px to
        // the right on the stage is 50 px "up" the arm.
        let local = project.q0rgs[0].layers[1].placements[0].transform;
        assert!(
            local.tx.abs() < 1e-3 && (local.ty + 50.0).abs() < 1e-3,
            "{local:?}"
        );

        // Moving the arm carries the hand.
        project.q0rgs[0].layers[0].placements[0].transform.tx += 30.0;
        let moved = shown(&project, 2);
        assert!((moved.tx - (before.tx + 30.0)).abs() < 1e-3);
        crate::v2::validate(&project).expect("valid with a parent");

        // Freed, it stays where it is now.
        set_parent(&mut project, 1, 2, None).unwrap();
        assert!(same_place(shown(&project, 2), moved));
    }

    #[test]
    fn loops_and_missing_layers_are_refused_or_tidied() {
        let mut project = project();
        set_parent(&mut project, 1, 2, Some(1)).unwrap();
        assert!(set_parent(&mut project, 1, 1, Some(2)).is_err());
        assert!(set_parent(&mut project, 1, 1, Some(1)).is_err());
        assert!(set_parent(&mut project, 1, 1, Some(9)).is_err());

        let hand = shown(&project, 2);
        release_children(&mut project, 1, &[1]);
        assert!(project.runtime.layer_parents.is_empty());
        assert!(same_place(shown(&project, 2), hand));

        project.runtime.layer_parents.insert(
            LayerKey {
                q0rg_id: 1,
                layer_id: 2,
            },
            7,
        );
        assert!(tidy(&mut project));
        assert!(project.runtime.layer_parents.is_empty());
    }

    #[test]
    fn parents_survive_the_file_format() {
        let mut project = project();
        set_parent(&mut project, 1, 2, Some(1)).unwrap();
        let bytes = crate::v2::write(&project).expect("write");
        let read = crate::v2::parse(&bytes).expect("read");
        assert_eq!(read.runtime.layer_parents, project.runtime.layer_parents);
    }

    #[test]
    fn the_camera_can_lead_a_layer_and_follow_one() {
        // A HUD: the hand hangs from the camera and stays put in its view.
        let mut hud = project();
        hud.runtime.camera.enabled = true;
        let rest = hud.default_camera_state();
        hud.runtime.camera.insert_key(0, rest);
        let before = shown(&hud, 2);
        set_parent(&mut hud, 1, 2, Some(CAMERA_LAYER)).unwrap();
        assert!(same_place(shown(&hud, 2), before));
        hud.runtime.camera.keys[0].state.center_x += 40.0;
        let moved = shown(&hud, 2);
        assert!((moved.tx - (before.tx + 40.0)).abs() < 1e-3, "{moved:?}");
        let on_screen = Affine::compose(hud.camera_affine_at(0), moved);
        assert!(same_place(on_screen, before), "{on_screen:?}");
        crate::v2::validate(&hud).expect("valid with the camera as a parent");

        // A follow cam: the camera hangs from the arm, keeps its view, then
        // follows the arm.
        let mut follow = project();
        set_parent(&mut follow, 1, CAMERA_LAYER, Some(1)).unwrap();
        assert!(
            follow.runtime.camera.enabled,
            "hanging the camera turns it on"
        );
        assert_eq!(follow.camera_world_state_at(0), rest);
        follow.q0rgs[0].layers[0].placements[0].transform.tx += 25.0;
        let carried = follow.camera_world_state_at(0).center_x;
        assert!((carried - (rest.center_x + 25.0)).abs() < 1e-3, "{carried}");
        assert!(
            set_parent(&mut follow, 1, 1, Some(CAMERA_LAYER)).is_err(),
            "the arm cannot then hang from the camera"
        );
        crate::v2::validate(&follow).expect("valid with the camera as a child");
        let bytes = crate::v2::write(&follow).expect("write");
        let read = crate::v2::parse(&bytes).expect("read");
        assert_eq!(read.runtime.layer_parents, follow.runtime.layer_parents);

        // Freed, it stays where it is now.
        set_parent(&mut follow, 1, CAMERA_LAYER, None).unwrap();
        let freed = follow.camera_world_state_at(0).center_x;
        assert!((freed - carried).abs() < 1e-3, "{freed}");
        // A nested symbol has no camera to hang from.
        assert!(set_parent(&mut follow, 2, 1, Some(CAMERA_LAYER)).is_err());
    }

    #[test]
    fn affines_rebuild_through_their_transform() {
        let original = Transform2D {
            tx: 3.0,
            ty: -4.0,
            sx: 1.5,
            sy: 0.75,
            rotation: 0.6,
            skew_x: 0.2,
            skew_y: 0.0,
        };
        let affine = Affine::from_transform(original);
        let rebuilt = Affine::from_transform(affine_to_transform(affine).unwrap());
        for (a, b) in [
            (affine.a11, rebuilt.a11),
            (affine.a12, rebuilt.a12),
            (affine.a21, rebuilt.a21),
            (affine.a22, rebuilt.a22),
            (affine.tx, rebuilt.tx),
            (affine.ty, rebuilt.ty),
        ] {
            assert!((a - b).abs() < 1e-4, "{affine:?} vs {rebuilt:?}");
        }
    }
}
