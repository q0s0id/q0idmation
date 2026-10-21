use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use q0s_format::v2::{
    FrameScript, Layer, LayerKey, LayerMetadata, ProjectV2, RigChannel, RigConstraint, RigDeformer,
    RigPropertyRef, Tween,
};

use crate::state::{
    TimelineFrameClipboard, TimelineFrameClipboardRow, TimelineLayerClipboard,
    TimelineLayerSelection, TimelineSelection,
};

fn q0rg_index(project: &ProjectV2, q0rg_id: u16) -> Option<usize> {
    project
        .q0rgs
        .iter()
        .position(|q0rg| q0rg.q0rg_id == q0rg_id)
}

/// Folder rows summarize descendants; they never own independent hold frames.
pub(crate) fn layer_frame_counts(project: &ProjectV2, q0rg_id: u16) -> HashMap<u16, u16> {
    let Some(index) = q0rg_index(project, q0rg_id) else {
        return HashMap::new();
    };
    let q0rg = &project.q0rgs[index];
    let mut lengths = HashMap::<u16, u16>::new();
    // Valid model order puts descendants before their folder, so nested folder
    // lengths propagate upward in one pass, including collapsed/hidden layers.
    for layer in &q0rg.layers {
        let length = if project.layer_is_folder(q0rg_id, layer.layer_id) {
            lengths.get(&layer.layer_id).copied().unwrap_or(0)
        } else {
            layer
                .timeline_frame_count
                .unwrap_or(q0rg.frame_count)
                .min(q0rg.frame_count)
        };
        lengths.insert(layer.layer_id, length);
        if let Some(parent) = project.layer_parent_folder(q0rg_id, layer.layer_id) {
            let parent_length = lengths.entry(parent).or_default();
            *parent_length = (*parent_length).max(length);
        }
    }
    lengths
}

/// Reconcile stored folder spans and remove a shared tail no real track owns.
pub(crate) fn sync_folder_timelines(project: &mut ProjectV2, q0rg_id: u16) -> bool {
    let Some(index) = q0rg_index(project, q0rg_id) else {
        return false;
    };
    let lengths = layer_frame_counts(project, q0rg_id);
    let folders = project.q0rgs[index]
        .layers
        .iter()
        .filter(|layer| project.layer_is_folder(q0rg_id, layer.layer_id))
        .map(|layer| layer.layer_id)
        .collect::<HashSet<_>>();
    let old_count = project.q0rgs[index].frame_count;
    let mut new_count = project.q0rgs[index]
        .layers
        .iter()
        .filter(|layer| !folders.contains(&layer.layer_id))
        .filter_map(|layer| lengths.get(&layer.layer_id).copied())
        .max()
        .unwrap_or(0)
        .max(1);
    for key in project
        .runtime
        .frame_labels
        .keys()
        .filter(|key| key.q0rg_id == q0rg_id)
    {
        new_count = new_count.max(key.frame.saturating_add(1));
    }
    for script in project
        .runtime
        .frame_scripts
        .iter()
        .filter(|script| script.q0rg_id == q0rg_id)
    {
        new_count = new_count.max(script.frame.saturating_add(1));
    }
    for clip in project
        .audio_clips
        .iter()
        .filter(|clip| clip.q0rg_id == q0rg_id)
    {
        new_count = new_count.max(
            q0s_format::v2::audio_clip_end_frame(project, *clip)
                .unwrap_or_else(|| clip.start_frame.saturating_add(1)),
        );
    }
    if q0rg_id == project.meta.entry_q0rg_id {
        for key in &project.runtime.camera.keys {
            new_count = new_count.max(key.frame.saturating_add(1));
            if let Some(end) = key.tween.to_frame() {
                new_count = new_count.max(end.saturating_add(1));
            }
        }
    }
    if let Some(rig) = q0s_format::rig::rig_for_q0rg(project, q0rg_id) {
        for key in rig.channels.iter().flat_map(|channel| &channel.keys) {
            new_count = new_count.max(key.frame.saturating_add(1));
        }
    }
    new_count = new_count.min(old_count);
    let mut changed = new_count != old_count;
    let q0rg = &mut project.q0rgs[index];
    q0rg.frame_count = new_count;
    for layer in &mut q0rg.layers {
        let length = lengths
            .get(&layer.layer_id)
            .copied()
            .unwrap_or(0)
            .min(new_count);
        let stored = (length < new_count).then_some(length);
        changed |= layer.timeline_frame_count != stored;
        layer.timeline_frame_count = stored;
    }
    if new_count < old_count {
        // References into a shortened symbol must remain valid when saving.
        let last = new_count.saturating_sub(1);
        for placement in project
            .q0rgs
            .iter_mut()
            .flat_map(|q| &mut q.layers)
            .flat_map(|layer| &mut layer.placements)
            .filter(|placement| placement.target == q0s_format::v2::Target::Q0rg(q0rg_id))
        {
            let playback = &mut placement.fx.q0rg_playback;
            playback.single_frame = playback.single_frame.min(last);
            if !playback.placement_relative {
                playback.first_frame = playback.first_frame.min(last);
                playback.last_frame = playback.last_frame.map(|frame| frame.min(last));
            }
        }
    }
    changed
}

pub(crate) fn sync_project_folder_timelines(project: &mut ProjectV2) -> bool {
    let ids = project
        .q0rgs
        .iter()
        .filter(|q| {
            q.layers
                .iter()
                .any(|layer| project.layer_is_folder(q.q0rg_id, layer.layer_id))
        })
        .map(|q| q.q0rg_id)
        .collect::<Vec<_>>();
    let mut changed = false;
    for id in ids {
        changed |= sync_folder_timelines(project, id);
    }
    changed
}

fn used_instance_ids(project: &ProjectV2) -> HashSet<u32> {
    project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
        .filter_map(|placement| (placement.instance_id != 0).then_some(placement.instance_id))
        .collect()
}

fn allocate_instance_id(used: &mut HashSet<u32>, next: &mut u32) -> Option<u32> {
    while *next != 0 && used.contains(next) {
        *next = next.checked_add(1)?;
    }
    if *next == 0 {
        return None;
    }
    let value = *next;
    used.insert(value);
    *next = next.checked_add(1).unwrap_or(0);
    Some(value)
}

fn clear_rig_frame_range(project: &mut ProjectV2, q0rg_id: u16, first: u16, last: u16) {
    if let Some(rig) = q0s_format::rig::rig_for_q0rg_mut(project, q0rg_id) {
        for channel in &mut rig.channels {
            channel
                .keys
                .retain(|key| !(first..=last).contains(&key.frame));
        }
        rig.channels.retain(|channel| !channel.keys.is_empty());
    }
}

fn paste_rig_channels(
    project: &mut ProjectV2,
    q0rg_id: u16,
    target_frame: u16,
    width: u16,
    channels: &[RigChannel],
) {
    if channels.is_empty() || width == 0 {
        return;
    }
    let last = target_frame.saturating_add(width - 1);
    clear_rig_frame_range(project, q0rg_id, target_frame, last);
    let Some(rig) = q0s_format::rig::rig_for_q0rg_mut(project, q0rg_id) else {
        return;
    };
    for channel in channels {
        for key in &channel.keys {
            q0s_format::rig::upsert_channel_key(
                rig,
                channel.property,
                target_frame.saturating_add(key.frame),
                key.value,
                key.easing,
            );
        }
    }
}

fn layers_for_instance(project: &ProjectV2, q0rg_id: u16, instance_id: u32) -> HashSet<u16> {
    if instance_id == 0 {
        return HashSet::new();
    }
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .into_iter()
        .flat_map(|q0rg| &q0rg.layers)
        .filter(|layer| {
            layer
                .placements
                .iter()
                .any(|placement| placement.instance_id == instance_id)
        })
        .map(|layer| layer.layer_id)
        .collect()
}

fn layers_for_node(
    project: &ProjectV2,
    q0rg_id: u16,
    rig: &q0s_format::v2::RigAsset,
    node_id: u16,
) -> HashSet<u16> {
    let mut layers = HashSet::new();
    for candidate in &rig.nodes {
        let mut ancestor = Some(candidate.node_id);
        let mut hops = 0usize;
        while let Some(current) = ancestor {
            if current == node_id {
                if let Some(binding) = candidate.binding {
                    layers.extend(layers_for_instance(project, q0rg_id, binding.instance_id));
                }
                break;
            }
            ancestor = rig
                .nodes
                .iter()
                .find(|node| node.node_id == current)
                .and_then(|node| node.parent);
            hops += 1;
            if hops > rig.nodes.len() {
                break;
            }
        }
    }
    layers
}

fn layers_for_control(
    project: &ProjectV2,
    q0rg_id: u16,
    rig: &q0s_format::v2::RigAsset,
    control_id: u16,
) -> HashSet<u16> {
    let mut layers = HashSet::new();
    if let Some(node_id) = rig
        .controls
        .iter()
        .find(|control| control.control_id == control_id)
        .and_then(|control| control.target_node)
    {
        layers.extend(layers_for_node(project, q0rg_id, rig, node_id));
    }
    for deformer in &rig.deformers {
        let owns_control = match deformer {
            RigDeformer::Skin { .. } => false,
            RigDeformer::Bend {
                start_control,
                middle_control,
                end_control,
                ..
            } => [*start_control, *middle_control, *end_control].contains(&control_id),
            RigDeformer::Cage { controls, .. } => controls.contains(&control_id),
        };
        if owns_control {
            layers.extend(layers_for_instance(
                project,
                q0rg_id,
                deformer.instance_id(),
            ));
        }
    }
    layers
}

fn layers_for_property(
    project: &ProjectV2,
    q0rg_id: u16,
    rig: &q0s_format::v2::RigAsset,
    property: RigPropertyRef,
) -> HashSet<u16> {
    use RigPropertyRef as Property;

    match property {
        Property::NodeTx(node_id)
        | Property::NodeTy(node_id)
        | Property::NodeRotation(node_id)
        | Property::NodeScaleX(node_id)
        | Property::BindingA11(node_id)
        | Property::BindingA12(node_id)
        | Property::BindingA21(node_id)
        | Property::BindingA22(node_id)
        | Property::BindingTx(node_id)
        | Property::BindingTy(node_id)
        | Property::NodeScaleY(node_id) => layers_for_node(project, q0rg_id, rig, node_id),
        Property::ControlX(control_id)
        | Property::ControlY(control_id)
        | Property::ControlValue(control_id) => {
            layers_for_control(project, q0rg_id, rig, control_id)
        }
        Property::ConstraintWeight(constraint_id) => {
            let mut nodes = Vec::new();
            let mut controls = Vec::new();
            let Some(constraint) = rig
                .constraints
                .iter()
                .find(|constraint| constraint.id() == constraint_id)
            else {
                return HashSet::new();
            };
            match constraint {
                RigConstraint::RotationLimit { node_id, .. }
                | RigConstraint::PositionLimit { node_id, .. }
                | RigConstraint::Aim { node_id, .. }
                | RigConstraint::Distance { node_id, .. } => nodes.push(*node_id),
                RigConstraint::Transform {
                    node_id,
                    target_node,
                    ..
                } => {
                    nodes.push(*node_id);
                    nodes.push(*target_node);
                }
                RigConstraint::TwoBoneIk {
                    root_node,
                    mid_node,
                    tip_node,
                    target_control,
                    pole_control,
                    ..
                } => {
                    nodes.extend([*root_node, *mid_node, *tip_node]);
                    controls.push(*target_control);
                    controls.extend(*pole_control);
                }
            }
            if let RigConstraint::Aim { target_control, .. }
            | RigConstraint::Distance { target_control, .. } = constraint
            {
                controls.push(*target_control);
            }
            let mut layers = nodes
                .into_iter()
                .flat_map(|node_id| layers_for_node(project, q0rg_id, rig, node_id))
                .collect::<HashSet<_>>();
            for control_id in controls {
                layers.extend(layers_for_control(project, q0rg_id, rig, control_id));
            }
            layers
        }
    }
}

pub fn rig_layer_has_track(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> bool {
    let Some(rig) = q0s_format::rig::rig_for_q0rg(project, q0rg_id) else {
        return false;
    };
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return false;
    };
    let mut rig_instances = rig
        .nodes
        .iter()
        .filter_map(|node| node.binding.map(|binding| binding.instance_id))
        .collect::<HashSet<_>>();
    rig_instances.extend(rig.deformers.iter().map(RigDeformer::instance_id));
    layer
        .placements
        .iter()
        .any(|placement| rig_instances.contains(&placement.instance_id))
}

pub fn rig_key_frames_for_layer(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> BTreeSet<u16> {
    let Some(rig) = q0s_format::rig::rig_for_q0rg(project, q0rg_id) else {
        return BTreeSet::new();
    };
    rig.channels
        .iter()
        .filter(|channel| {
            layers_for_property(project, q0rg_id, rig, channel.property).contains(&layer_id)
        })
        .flat_map(|channel| channel.keys.iter().map(|key| key.frame))
        .collect()
}

pub fn insert_rig_key_for_layer(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> bool {
    let Some(rig_snapshot) = q0s_format::rig::rig_for_q0rg(project, q0rg_id).cloned() else {
        return false;
    };
    let values = q0s_format::rig::capture_timeline_pose_values(&rig_snapshot, f32::from(frame))
        .into_iter()
        .filter(|value| {
            layers_for_property(project, q0rg_id, &rig_snapshot, value.property).contains(&layer_id)
        })
        .map(|value| (value.property, value.value))
        .collect::<Vec<_>>();
    if values.is_empty() {
        return false;
    }
    let Some(q0rg) = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
    else {
        return false;
    };
    q0rg.frame_count = q0rg.frame_count.max(frame.saturating_add(1));
    let Some(rig) = q0s_format::rig::rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    for (property, value) in values {
        q0s_format::rig::upsert_pose_key(rig, property, frame, value);
    }
    true
}

pub fn delete_rig_key_for_layer(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> bool {
    let Some(snapshot) = q0s_format::rig::rig_for_q0rg(project, q0rg_id).cloned() else {
        return false;
    };
    let properties = snapshot
        .channels
        .iter()
        .filter(|channel| {
            layers_for_property(project, q0rg_id, &snapshot, channel.property).contains(&layer_id)
        })
        .map(|channel| channel.property)
        .collect::<HashSet<_>>();
    let Some(rig) = q0s_format::rig::rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    let mut changed = false;
    for channel in &mut rig.channels {
        if properties.contains(&channel.property) {
            let before = channel.keys.len();
            channel.keys.retain(|key| key.frame != frame);
            changed |= before != channel.keys.len();
        }
    }
    rig.channels.retain(|channel| !channel.keys.is_empty());
    changed
}

pub fn move_rig_key_for_layer(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    from_frame: u16,
    to_frame: u16,
) -> bool {
    if from_frame == to_frame {
        return false;
    }
    let Some(snapshot) = q0s_format::rig::rig_for_q0rg(project, q0rg_id).cloned() else {
        return false;
    };
    let moves = snapshot
        .channels
        .iter()
        .filter(|channel| {
            layers_for_property(project, q0rg_id, &snapshot, channel.property).contains(&layer_id)
        })
        .filter_map(|channel| {
            channel
                .keys
                .iter()
                .find(|key| key.frame == from_frame)
                .copied()
                .map(|key| (channel.property, key))
        })
        .collect::<Vec<_>>();
    if moves.is_empty()
        || moves.iter().any(|(property, _)| {
            snapshot.channels.iter().any(|channel| {
                channel.property == *property
                    && channel.keys.iter().any(|key| key.frame == to_frame)
            })
        })
    {
        return false;
    }
    let Some(q0rg) = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
    else {
        return false;
    };
    q0rg.frame_count = q0rg.frame_count.max(to_frame.saturating_add(1));
    let Some(rig) = q0s_format::rig::rig_for_q0rg_mut(project, q0rg_id) else {
        return false;
    };
    for (property, mut key) in moves {
        if let Some(channel) = rig
            .channels
            .iter_mut()
            .find(|channel| channel.property == property)
        {
            channel
                .keys
                .retain(|candidate| candidate.frame != from_frame);
            key.frame = to_frame;
            channel.keys.push(key);
            channel.keys.sort_by_key(|candidate| candidate.frame);
        }
    }
    true
}

fn shift_rig_keys_after_removed_columns(
    project: &mut ProjectV2,
    q0rg_id: u16,
    first_frame: u16,
    last_frame: u16,
    width: u16,
) {
    let Some(rig) = q0s_format::rig::rig_for_q0rg_mut(project, q0rg_id) else {
        return;
    };
    for channel in &mut rig.channels {
        channel
            .keys
            .retain(|key| !(first_frame..=last_frame).contains(&key.frame));
        for key in &mut channel.keys {
            if key.frame > last_frame {
                key.frame -= width;
            }
        }
        channel.keys.sort_by_key(|key| key.frame);
    }
    rig.channels.retain(|channel| !channel.keys.is_empty());
}

fn selected_frame_bounds(
    project: &ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Option<(Vec<u16>, u16, u16)> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let anchor = q0rg
        .layers
        .iter()
        .position(|layer| layer.layer_id == selection.anchor_layer_id)?;
    let focus = q0rg
        .layers
        .iter()
        .position(|layer| layer.layer_id == selection.focus_layer_id)?;
    let first_layer = anchor.min(focus);
    let last_layer = anchor.max(focus);
    let layer_ids = q0rg.layers[first_layer..=last_layer]
        .iter()
        .rev()
        .filter(|layer| !project.layer_is_folder(q0rg_id, layer.layer_id))
        .map(|layer| layer.layer_id)
        .collect::<Vec<_>>();
    if layer_ids.is_empty() || q0rg.frame_count == 0 {
        return None;
    }
    let first_frame = selection
        .anchor_frame
        .min(selection.focus_frame)
        .min(q0rg.frame_count - 1);
    let last_frame = selection
        .anchor_frame
        .max(selection.focus_frame)
        .min(q0rg.frame_count - 1);
    Some((layer_ids, first_frame, last_frame))
}

pub fn selected_frame_cells(
    project: &ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Option<Vec<(u16, u16)>> {
    let (layer_ids, first_frame, last_frame) = selected_frame_bounds(project, q0rg_id, selection)?;
    Some(
        layer_ids
            .into_iter()
            .flat_map(|layer_id| (first_frame..=last_frame).map(move |frame| (layer_id, frame)))
            .collect(),
    )
}

/// Return every cell in a rectangular selection, including cells in the
/// virtual tail past the q0rg's current shared frame count. This is used by
/// frame insertion commands, which can materialize those cells.
pub fn selected_frame_cells_including_virtual(
    project: &ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Option<Vec<(u16, u16)>> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let anchor = q0rg
        .layers
        .iter()
        .position(|layer| layer.layer_id == selection.anchor_layer_id)?;
    let focus = q0rg
        .layers
        .iter()
        .position(|layer| layer.layer_id == selection.focus_layer_id)?;
    let first_layer = anchor.min(focus);
    let last_layer = anchor.max(focus);
    let layer_ids = q0rg.layers[first_layer..=last_layer]
        .iter()
        .rev()
        .filter(|layer| !project.layer_is_folder(q0rg_id, layer.layer_id))
        .map(|layer| layer.layer_id)
        .collect::<Vec<_>>();
    if layer_ids.is_empty() {
        return None;
    }
    let first_frame = selection.anchor_frame.min(selection.focus_frame);
    let last_frame = selection.anchor_frame.max(selection.focus_frame);
    Some(
        layer_ids
            .into_iter()
            .flat_map(|layer_id| (first_frame..=last_frame).map(move |frame| (layer_id, frame)))
            .collect(),
    )
}

pub fn capture_frames(
    project: &ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Option<TimelineFrameClipboard> {
    let (layer_ids, first_frame, last_frame) = selected_frame_bounds(project, q0rg_id, selection)?;
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let mut rows = Vec::with_capacity(layer_ids.len());

    for layer_id in layer_ids.iter().copied() {
        let layer = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?;
        let mut explicit_keyframes = layer
            .explicit_keyframes
            .iter()
            .copied()
            .filter(|frame| (first_frame..=last_frame).contains(frame))
            .map(|frame| frame - first_frame)
            .collect::<Vec<_>>();
        let mut placements = Vec::new();

        // A selection may begin on a held or tweened frame. Bake that visible
        // state at relative frame zero so pasting never loses what the user saw.
        if !layer.has_keyframe(first_frame) {
            for (placement_idx, transform) in
                crate::render::active_placements_at(layer, first_frame)
            {
                let mut placement = layer.placements.get(placement_idx)?.clone();
                placement.frame = 0;
                placement.transform = transform;
                placement.tween = Tween::None;
                placements.push(placement);
            }
            if placements.is_empty() {
                explicit_keyframes.push(0);
            }
        }

        placements.extend(
            layer
                .placements
                .iter()
                .filter(|placement| (first_frame..=last_frame).contains(&placement.frame))
                .cloned()
                .map(|mut placement| {
                    placement.frame -= first_frame;
                    placement.tween = match placement.tween.to_frame() {
                        Some(to_frame) if (first_frame..=last_frame).contains(&to_frame) => {
                            placement.tween.with_to_frame(to_frame - first_frame)
                        }
                        Some(_) | None => Tween::None,
                    };
                    placement
                }),
        );
        explicit_keyframes.sort_unstable();
        explicit_keyframes.dedup();
        let frame_scripts = project
            .runtime
            .frame_scripts
            .iter()
            .filter(|script| {
                script.q0rg_id == q0rg_id
                    && script.layer_id == layer_id
                    && (first_frame..=last_frame).contains(&script.frame)
            })
            .cloned()
            .map(|mut script| {
                script.frame -= first_frame;
                script
            })
            .collect();
        rows.push(TimelineFrameClipboardRow {
            source_layer_id: layer_id,
            selected_frames: (0..=last_frame - first_frame).collect(),
            explicit_keyframes,
            placements,
            frame_scripts,
        });
    }

    Some(TimelineFrameClipboard {
        width: last_frame - first_frame + 1,
        rows,
        // Artwork row selections never implicitly copy rig subtracks. Rig
        // keys are edited and moved from their own dedicated timeline lanes.
        rig_channels: Vec::new(),
    })
}

pub fn capture_frame_cells(
    project: &ProjectV2,
    q0rg_id: u16,
    selected_cells: &[(u16, u16)],
) -> Option<TimelineFrameClipboard> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let visible_layers = q0rg
        .layers
        .iter()
        .rev()
        .filter(|layer| !project.layer_is_folder(q0rg_id, layer.layer_id))
        .map(|layer| layer.layer_id)
        .collect::<Vec<_>>();
    let selected_cells = selected_cells
        .iter()
        .copied()
        .filter(|(layer_id, frame)| *frame < q0rg.frame_count && visible_layers.contains(layer_id))
        .collect::<HashSet<_>>();
    let first_frame = selected_cells.iter().map(|(_, frame)| *frame).min()?;
    let last_frame = selected_cells.iter().map(|(_, frame)| *frame).max()?;
    let first_layer = visible_layers.iter().position(|layer_id| {
        selected_cells
            .iter()
            .any(|(selected, _)| selected == layer_id)
    })?;
    let last_layer = visible_layers.iter().rposition(|layer_id| {
        selected_cells
            .iter()
            .any(|(selected, _)| selected == layer_id)
    })?;
    let mut rows = Vec::with_capacity(last_layer - first_layer + 1);

    for layer_id in visible_layers[first_layer..=last_layer].iter().copied() {
        let layer = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?;
        let selected_frames = selected_cells
            .iter()
            .filter_map(|(selected_layer, frame)| {
                (*selected_layer == layer_id).then_some(*frame - first_frame)
            })
            .collect::<HashSet<_>>();
        let mut selected_frames = selected_frames.into_iter().collect::<Vec<_>>();
        selected_frames.sort_unstable();

        let mut explicit_keyframes = Vec::new();
        let mut placements = Vec::new();
        for relative_frame in selected_frames.iter().copied() {
            let frame = first_frame + relative_frame;
            if layer.has_keyframe(frame) {
                if layer.explicit_keyframes.contains(&frame) {
                    explicit_keyframes.push(relative_frame);
                }
                placements.extend(
                    layer
                        .placements
                        .iter()
                        .filter(|placement| placement.frame == frame)
                        .cloned()
                        .map(|mut placement| {
                            placement.frame = relative_frame;
                            placement.tween = match placement.tween.to_frame() {
                                Some(to_frame)
                                    if selected_cells.contains(&(layer_id, to_frame)) =>
                                {
                                    placement
                                        .tween
                                        .with_to_frame(to_frame.saturating_sub(first_frame))
                                }
                                Some(_) | None => Tween::None,
                            };
                            placement
                        }),
                );
            } else {
                let active = crate::render::active_placements_at(layer, frame);
                if active.is_empty() {
                    explicit_keyframes.push(relative_frame);
                } else {
                    for (placement_idx, transform) in active {
                        let mut placement = layer.placements.get(placement_idx)?.clone();
                        placement.frame = relative_frame;
                        placement.transform = transform;
                        placement.tween = Tween::None;
                        placements.push(placement);
                    }
                }
            }
        }
        explicit_keyframes.sort_unstable();
        explicit_keyframes.dedup();
        let frame_scripts = project
            .runtime
            .frame_scripts
            .iter()
            .filter(|script| {
                script.q0rg_id == q0rg_id
                    && script.layer_id == layer_id
                    && selected_cells.contains(&(layer_id, script.frame))
            })
            .cloned()
            .map(|mut script| {
                script.frame -= first_frame;
                script
            })
            .collect();
        rows.push(TimelineFrameClipboardRow {
            source_layer_id: layer_id,
            selected_frames,
            explicit_keyframes,
            placements,
            frame_scripts,
        });
    }

    Some(TimelineFrameClipboard {
        width: last_frame - first_frame + 1,
        rows,
        rig_channels: Vec::new(),
    })
}

fn clear_frame_range(layer: &mut Layer, first_frame: u16, last_frame: u16, blank: bool) {
    layer
        .explicit_keyframes
        .retain(|frame| !(first_frame..=last_frame).contains(frame));
    layer
        .placements
        .retain(|placement| !(first_frame..=last_frame).contains(&placement.frame));
    for placement in &mut layer.placements {
        if placement
            .tween
            .to_frame()
            .is_some_and(|to_frame| (first_frame..=last_frame).contains(&to_frame))
        {
            placement.tween = Tween::None;
        }
    }
    if blank {
        for frame in first_frame..=last_frame {
            layer.ensure_explicit_keyframe(frame);
        }
    }
}

pub fn clear_frames(
    project: &mut ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Option<usize> {
    let (layer_ids, first_frame, last_frame) = selected_frame_bounds(project, q0rg_id, selection)?;
    let selected_layers = layer_ids.iter().copied().collect::<HashSet<_>>();
    project.runtime.frame_scripts.retain(|script| {
        script.q0rg_id != q0rg_id
            || !selected_layers.contains(&script.layer_id)
            || !(first_frame..=last_frame).contains(&script.frame)
    });
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let q0rg = &mut project.q0rgs[q0rg_index];
    let mut removed = 0;
    for layer_id in layer_ids {
        let layer = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)?;
        let before = layer.placements.len();
        clear_frame_range(layer, first_frame, last_frame, true);
        removed += before - layer.placements.len();
    }
    Some(removed)
}

/// Resolve the state to inherit when creating a keyframe. Beyond cells stay
/// invisible during playback, but an F6 extension inherits the last existing
/// cell of a shortened layer. A real blank key still stops that inheritance.
pub(crate) fn keyframe_source_states_at(
    layer: &Layer,
    frame: u16,
) -> Vec<q0s_format::raster::ActivePlacementState> {
    let source_frame = match layer.timeline_frame_count {
        Some(0) => return Vec::new(),
        Some(frame_count) => frame.min(frame_count - 1),
        None => frame,
    };
    q0s_format::raster::active_placement_states_at(layer, source_frame)
}

/// A blank keyframe placed inside a shape tween. Whatever is drawn on it next
/// takes over the tween towards the original destination.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ShapeTweenContinuation {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub frame: u16,
    pub to_frame: u16,
    pub easing: q0s_format::v2::Easing,
}

fn is_drawing(project: &ProjectV2, target: q0s_format::v2::Target) -> bool {
    matches!(target, q0s_format::v2::Target::Asset(id)
    if project.assets.iter().any(|asset| {
        matches!(asset, q0s_format::v2::Asset::Vector(vector) if vector.asset_id == id)
    }))
}

/// End every shape tween that runs across `frame` at `frame`, which is about
/// to become a blank keyframe, and report where it was heading.
pub(crate) fn split_shape_tweens_at_blank_keyframe(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Option<ShapeTweenContinuation> {
    let layer = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == layer_id)?;
    let mut continuation = None;
    for placement in &mut layer.placements {
        if let Tween::Shape { to_frame, easing } = placement.tween {
            if placement.frame < frame && frame < to_frame {
                placement.tween = Tween::Shape {
                    to_frame: frame,
                    easing,
                };
                continuation = Some(ShapeTweenContinuation {
                    q0rg_id,
                    layer_id,
                    frame,
                    to_frame,
                    easing,
                });
            }
        }
    }
    continuation
}

/// Let freshly drawn artwork join the shape tween of its keyframe.
///
/// A tween belongs to the keyframe as the animator sees it, but is stored on
/// each drawing. So a stroke added to a keyframe that already morphs, or to
/// the blank keyframe described by `pending`, would otherwise just sit there
/// until the tween was removed and created again. Returns whether anything
/// changed; `pending` is cleared once it has been used or can no longer apply.
pub(crate) fn continue_shape_tweens(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    pending: &mut Option<ShapeTweenContinuation>,
) -> bool {
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return false;
    };
    // Keyframes whose drawings already morph, and where to.
    let mut spans = BTreeMap::<u16, Tween>::new();
    for placement in &layer.placements {
        if placement.tween.is_shape() {
            spans.entry(placement.frame).or_insert(placement.tween);
        }
    }
    if let Some(wanted) = *pending {
        if wanted.q0rg_id == q0rg_id && wanted.layer_id == layer_id {
            let drawn = layer
                .placements
                .iter()
                .any(|placement| placement.frame == wanted.frame);
            let still_valid = layer.has_keyframe(wanted.frame)
                && layer.has_keyframe(wanted.to_frame)
                && !spans.contains_key(&wanted.frame);
            if !still_valid {
                *pending = None;
            } else if drawn {
                spans.insert(
                    wanted.frame,
                    Tween::Shape {
                        to_frame: wanted.to_frame,
                        easing: wanted.easing,
                    },
                );
                *pending = None;
            }
        }
    }
    let joining = layer
        .placements
        .iter()
        .enumerate()
        .filter(|(_, placement)| {
            matches!(placement.tween, Tween::None)
                && spans.contains_key(&placement.frame)
                && is_drawing(project, placement.target)
        })
        .map(|(index, placement)| (index, spans[&placement.frame]))
        .collect::<Vec<_>>();
    if joining.is_empty() {
        return false;
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
        return false;
    };
    for (index, tween) in joining {
        layer.placements[index].tween = tween;
    }
    true
}

/// In-between drawings of shape tweens waiting to become real vector assets
/// once a new keyframe is committed.
pub(crate) struct BakedShapes {
    first_id: u16,
    shapes: Vec<q0s_format::v2::VectorAsset>,
}

impl BakedShapes {
    pub(crate) fn new(project: &ProjectV2) -> Self {
        Self {
            first_id: crate::tools::next_asset_id_pub(project),
            shapes: Vec::new(),
        }
    }

    /// The placements a new keyframe needs in place of `copy`, the plain copy
    /// of the source placement at `source_index`. Inside a shape tween the
    /// stage shows an in-between drawing, so the keyframe gets that drawing
    /// (several when a shape is splitting) instead of the first one again,
    /// and goes on morphing into the tween's original destination.
    pub(crate) fn keyframe_placements(
        &mut self,
        shape_morph: Option<&q0s_format::shape_tween::LayerMorph>,
        source_index: usize,
        source_tween: Tween,
        mut copy: q0s_format::v2::Placement,
    ) -> Vec<q0s_format::v2::Placement> {
        let Some(shapes) = shape_morph.and_then(|morph| morph.for_placement(source_index)) else {
            return vec![copy];
        };
        // Keep the plain copy if the asset id space cannot hold the drawing.
        if usize::from(self.first_id) + self.shapes.len() + shapes.len() > usize::from(u16::MAX) {
            return vec![copy];
        }
        if let Tween::Shape { to_frame, easing } = source_tween {
            if to_frame > copy.frame {
                copy.tween = Tween::Shape { to_frame, easing };
            }
        }
        shapes
            .iter()
            .map(|shape| {
                let asset_id = self.first_id + self.shapes.len() as u16;
                self.shapes.push(q0s_format::v2::VectorAsset {
                    asset_id,
                    ..shape.clone()
                });
                q0s_format::v2::Placement {
                    target: q0s_format::v2::Target::Asset(asset_id),
                    ..copy.clone()
                }
            })
            .collect()
    }

    pub(crate) fn commit(self, project: &mut ProjectV2) {
        project
            .assets
            .extend(self.shapes.into_iter().map(q0s_format::v2::Asset::Vector));
    }
}

/// Convert every selected non-key cell into a real keyframe using the exact
/// visible state from the original timeline. When a new key splits an incoming
/// tween, retarget that tween to the first inserted key so metadata and the
/// rendered span agree.
pub fn materialize_selected_keyframes(
    project: &mut ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Option<usize> {
    let cells = selected_frame_cells_including_virtual(project, q0rg_id, selection)?;
    materialize_keyframes_at_cells(project, q0rg_id, &cells)
}

/// Materialize real keyframes at the selected cells only. Unlike a rectangular
/// edit, each cell is independent: another layer's key at the same frame does
/// not block it, and virtual-tail cells extend only their own selected rows.
pub fn materialize_keyframes_at_cells(
    project: &mut ProjectV2,
    q0rg_id: u16,
    cells: &[(u16, u16)],
) -> Option<usize> {
    let mut audio_created = 0;
    for (layer_id, frame) in cells.iter().copied().collect::<BTreeSet<_>>() {
        audio_created += usize::from(crate::audio::keyframe_audio(
            project, q0rg_id, layer_id, frame, true,
        ));
    }
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let q0rg = &project.q0rgs[q0rg_index];
    if q0rg.frame_count == 0 {
        return None;
    }

    let mut selected = cells.iter().copied().collect::<BTreeSet<_>>();
    selected.retain(|(layer_id, _)| {
        q0rg.layers.iter().any(|layer| {
            layer.layer_id == *layer_id && !project.layer_is_folder(q0rg_id, *layer_id)
        })
    });
    if selected.is_empty() {
        return Some(audio_created);
    }

    let mut max_frame_by_layer = BTreeMap::<u16, u16>::new();
    let mut insertions = Vec::<(u16, u16, usize, q0s_format::v2::Placement)>::new();
    let mut blank_keys = Vec::<(u16, u16)>::new();
    let mut tween_retargets = HashMap::<(u16, usize), u16>::new();
    let mut created_cells = 0usize;
    let mut baked_shapes = BakedShapes::new(project);

    for (layer_id, frame) in selected {
        let layer = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?;
        if layer.has_keyframe(frame) {
            continue;
        }
        created_cells += 1;
        max_frame_by_layer
            .entry(layer_id)
            .and_modify(|last| *last = (*last).max(frame))
            .or_insert(frame);

        let mut cell_insertions = Vec::new();
        let shape_morph = q0s_format::shape_tween::project_layer_morph(project, layer, frame);
        for state in keyframe_source_states_at(layer, frame) {
            let source = layer.placements.get(state.index)?;
            if source.frame == frame {
                continue;
            }
            if source
                .tween
                .to_frame()
                .is_some_and(|to_frame| source.frame < frame && frame <= to_frame)
            {
                tween_retargets
                    .entry((layer_id, state.index))
                    .and_modify(|target| *target = (*target).min(frame))
                    .or_insert(frame);
            }
            let mut placement = source.clone();
            placement.frame = frame;
            placement.transform = state.transform;
            placement.fx = state.fx;
            placement.tween = Tween::None;
            cell_insertions.extend(
                baked_shapes
                    .keyframe_placements(shape_morph.as_ref(), state.index, source.tween, placement)
                    .into_iter()
                    .map(|placement| (state.index, placement)),
            );
        }
        if cell_insertions.is_empty() {
            blank_keys.push((layer_id, frame));
        } else {
            insertions.extend(
                cell_insertions
                    .into_iter()
                    .map(|(index, placement)| (layer_id, frame, index, placement)),
            );
        }
    }

    if max_frame_by_layer.is_empty() {
        return Some(audio_created);
    }
    if max_frame_by_layer
        .values()
        .any(|frame| frame.checked_add(1).is_none())
    {
        return None;
    }

    for (layer_id, frame) in &max_frame_by_layer {
        extend_layer_through_frame(project, q0rg_id, *layer_id, *frame, false)?;
    }
    baked_shapes.commit(project);

    let q0rg = &mut project.q0rgs[q0rg_index];
    for ((layer_id, placement_index), target_frame) in tween_retargets {
        let source = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)?
            .placements
            .get_mut(placement_index)?;
        source.tween = source.tween.with_to_frame(target_frame);
    }

    // Insert later source indices first so each copied object stays adjacent
    // to its source and preserves the layer's stacking order.
    insertions.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| right.1.cmp(&left.1))
    });
    for (layer_id, _frame, placement_index, placement) in insertions {
        let layer = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)?;
        let insert_at = placement_index
            .saturating_add(1)
            .min(layer.placements.len());
        layer.placements.insert(insert_at, placement);
    }
    for (layer_id, frame) in blank_keys {
        q0rg.layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)?
            .ensure_explicit_keyframe(frame);
    }
    Some(created_cells + audio_created)
}

/// Demote selected keyframes back to ordinary held frames. Frame zero is the
/// structural root of a drawable layer and therefore remains a keyframe.
pub fn remove_selected_keyframes(
    project: &mut ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Option<usize> {
    let (layer_ids, first_frame, last_frame) = selected_frame_bounds(project, q0rg_id, selection)?;
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let q0rg = &mut project.q0rgs[q0rg_index];
    let mut removed = 0usize;

    for layer_id in layer_ids {
        let layer = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)?;
        for frame in first_frame.max(1)..=last_frame {
            if !layer.has_keyframe(frame) {
                continue;
            }
            layer.remove_keyframe(frame);
            for placement in &mut layer.placements {
                if placement.tween.to_frame() == Some(frame) {
                    placement.tween = Tween::None;
                }
            }
            removed += 1;
        }
    }
    Some(removed)
}

/// Insert one ordinary frame before `frame` on a single layer. Keyframes and
/// layer-local frame scripts at or after the insertion point move right, while
/// the new cell inherits the preceding visual state (a classic F5 hold frame).
///
/// Frame zero is the structural root of a layer, so inserting there keeps that
/// root in place and lengthens its first span instead of creating an invalid
/// pre-root cell. Other layers are intentionally left untouched.
pub fn insert_hold_frame_before(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Option<u16> {
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let old_count = project.q0rgs[q0rg_index].frame_count;
    if old_count == 0 || frame >= old_count {
        return None;
    }
    let new_count = old_count.checked_add(1)?;
    let layer_index = project.q0rgs[q0rg_index]
        .layers
        .iter()
        .position(|layer| layer.layer_id == layer_id)?;
    if project.layer_is_folder(q0rg_id, layer_id) {
        return None;
    }

    for script in &mut project.runtime.frame_scripts {
        if script.q0rg_id == q0rg_id && script.layer_id == layer_id && script.frame >= frame {
            script.frame = script.frame.checked_add(1)?;
        }
    }

    for clip in &mut project.audio_clips {
        if clip.q0rg_id == q0rg_id && clip.layer_id == layer_id && clip.start_frame >= frame {
            clip.start_frame = clip.start_frame.checked_add(1)?;
        }
    }

    let q0rg = &mut project.q0rgs[q0rg_index];
    q0rg.frame_count = new_count;
    let layer = &mut q0rg.layers[layer_index];
    if let Some(layer_frame_count) = &mut layer.timeline_frame_count {
        let inserted_count = if frame < *layer_frame_count {
            layer_frame_count.saturating_add(1)
        } else {
            (*layer_frame_count)
                .saturating_add(1)
                .max(frame.saturating_add(1))
        };
        *layer_frame_count = inserted_count.min(new_count);
    }
    for placement in &mut layer.placements {
        if placement.frame >= frame && !(frame == 0 && placement.frame == 0) {
            placement.frame = placement.frame.checked_add(1)?;
        }
        if let Some(to_frame) = placement.tween.to_frame() {
            if to_frame >= frame {
                placement.tween = placement.tween.with_to_frame(to_frame.checked_add(1)?);
            }
        }
    }
    for keyframe in &mut layer.explicit_keyframes {
        if *keyframe >= frame && !(frame == 0 && *keyframe == 0) {
            *keyframe = keyframe.checked_add(1)?;
        }
    }
    layer.explicit_keyframes.sort_unstable();
    layer.explicit_keyframes.dedup();
    Some(new_count)
}

/// insert one hold frame after the last selected cell on each selected layer.
/// each row gets its own insertion point; other rows keep their frame positions
/// and duration even when the shared q0rg timeline grows.
pub fn insert_hold_frames_after_selected_cells(
    project: &mut ProjectV2,
    q0rg_id: u16,
    cells: &[(u16, u16)],
) -> Option<(u16, Vec<(u16, u16)>)> {
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let old_frame_count = project.q0rgs[q0rg_index].frame_count;
    if old_frame_count == 0 {
        return None;
    }

    let mut insertion_frames = BTreeMap::<u16, u16>::new();
    let mut extend_through_frames = BTreeMap::<u16, u16>::new();
    for &(layer_id, frame) in cells {
        if project.layer_is_folder(q0rg_id, layer_id) {
            continue;
        }
        let Some(layer) = project.q0rgs[q0rg_index]
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)
        else {
            continue;
        };
        let layer_frame_count = layer
            .timeline_frame_count
            .unwrap_or(old_frame_count)
            .min(old_frame_count);
        if frame >= layer_frame_count {
            extend_through_frames
                .entry(layer_id)
                .and_modify(|last| *last = (*last).max(frame))
                .or_insert(frame);
        } else {
            let insertion_frame = frame.checked_add(1)?;
            insertion_frames
                .entry(layer_id)
                .and_modify(|last| *last = (*last).max(insertion_frame))
                .or_insert(insertion_frame);
        }
    }
    // If a row selection crosses its end into beyond cells, materialize the
    // selected tail in place instead of also shifting the earlier cells.
    for layer_id in extend_through_frames.keys() {
        insertion_frames.remove(layer_id);
    }
    if insertion_frames.is_empty() && extend_through_frames.is_empty() {
        return Some((old_frame_count, Vec::new()));
    }

    let old_layer_lengths = project.q0rgs[q0rg_index]
        .layers
        .iter()
        .map(|layer| {
            (
                layer.layer_id,
                layer
                    .timeline_frame_count
                    .unwrap_or(old_frame_count)
                    .min(old_frame_count),
            )
        })
        .collect::<HashMap<_, _>>();
    let folder_layer_ids = project.q0rgs[q0rg_index]
        .layers
        .iter()
        .filter(|layer| project.layer_is_folder(q0rg_id, layer.layer_id))
        .map(|layer| layer.layer_id)
        .collect::<HashSet<_>>();
    let mut new_frame_count = old_frame_count;
    for layer_id in insertion_frames.keys() {
        let new_length = old_layer_lengths.get(layer_id)?.checked_add(1)?;
        new_frame_count = new_frame_count.max(new_length);
    }
    for (layer_id, last_frame) in &extend_through_frames {
        let new_length = last_frame
            .checked_add(1)?
            .max(*old_layer_lengths.get(layer_id)?);
        new_frame_count = new_frame_count.max(new_length);
    }

    // Check every shifted frame before mutating anything, so overflow cannot
    // leave only some selected layers updated.
    for script in &project.runtime.frame_scripts {
        if script.q0rg_id == q0rg_id
            && insertion_frames
                .get(&script.layer_id)
                .is_some_and(|at| script.frame >= *at && script.frame.checked_add(1).is_none())
        {
            return None;
        }
    }
    for clip in &project.audio_clips {
        if clip.q0rg_id == q0rg_id
            && insertion_frames.get(&clip.layer_id).is_some_and(|at| {
                clip.start_frame >= *at && clip.start_frame.checked_add(1).is_none()
            })
        {
            return None;
        }
    }
    for layer in &project.q0rgs[q0rg_index].layers {
        let Some(at) = insertion_frames.get(&layer.layer_id) else {
            continue;
        };
        if layer
            .placements
            .iter()
            .any(|placement| placement.frame >= *at && placement.frame.checked_add(1).is_none())
            || layer.placements.iter().any(|placement| {
                placement
                    .tween
                    .to_frame()
                    .is_some_and(|to_frame| to_frame >= *at && to_frame.checked_add(1).is_none())
            })
            || layer
                .explicit_keyframes
                .iter()
                .any(|frame| *frame >= *at && frame.checked_add(1).is_none())
        {
            return None;
        }
    }

    for script in &mut project.runtime.frame_scripts {
        if script.q0rg_id == q0rg_id {
            if let Some(at) = insertion_frames.get(&script.layer_id) {
                if script.frame >= *at {
                    script.frame += 1;
                }
            }
        }
    }
    for clip in &mut project.audio_clips {
        if clip.q0rg_id == q0rg_id {
            if let Some(at) = insertion_frames.get(&clip.layer_id) {
                if clip.start_frame >= *at {
                    clip.start_frame += 1;
                }
            }
        }
    }

    let mut inserted_cells = insertion_frames
        .iter()
        .map(|(layer_id, frame)| (*layer_id, *frame))
        .collect::<Vec<_>>();
    inserted_cells.extend(
        extend_through_frames
            .iter()
            .filter_map(|(layer_id, frame)| {
                let old_length = old_layer_lengths.get(layer_id).copied()?;
                (*frame >= old_length).then_some((*layer_id, *frame))
            }),
    );
    let q0rg = &mut project.q0rgs[q0rg_index];
    q0rg.frame_count = new_frame_count;
    for layer in &mut q0rg.layers {
        let old_length = old_layer_lengths.get(&layer.layer_id).copied()?;
        if let Some(last_frame) = extend_through_frames.get(&layer.layer_id).copied() {
            let new_length = old_length.max(last_frame.checked_add(1)?);
            layer.timeline_frame_count = (new_length < new_frame_count).then_some(new_length);
        } else if let Some(at) = insertion_frames.get(&layer.layer_id).copied() {
            for placement in &mut layer.placements {
                if placement.frame >= at {
                    placement.frame += 1;
                }
                if let Some(to_frame) = placement.tween.to_frame() {
                    if to_frame >= at {
                        placement.tween = placement.tween.with_to_frame(to_frame + 1);
                    }
                }
            }
            for frame in &mut layer.explicit_keyframes {
                if *frame >= at {
                    *frame += 1;
                }
            }
            layer.explicit_keyframes.sort_unstable();
            layer.explicit_keyframes.dedup();
            let new_length = old_length + 1;
            layer.timeline_frame_count = (new_length < new_frame_count).then_some(new_length);
        } else if new_frame_count > old_frame_count && !folder_layer_ids.contains(&layer.layer_id) {
            // Freeze rows that implicitly followed the shared length before it
            // grew; F5 must not lengthen unselected layers.
            layer.timeline_frame_count = Some(old_length);
        }
    }

    Some((new_frame_count, inserted_cells))
}

/// extend one layer through `frame`. when the added cell is exactly one past
/// the shared end, rows that ended at that same frame can continue with it.
/// distant insertions extend only the target row.
pub fn extend_layer_through_frame(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    continue_matching_end_layers: bool,
) -> Option<u16> {
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let old_frame_count = project.q0rgs[q0rg_index].frame_count;
    if old_frame_count == 0 || project.layer_is_folder(q0rg_id, layer_id) {
        return None;
    }
    let required_length = frame.checked_add(1)?;
    let old_layer_lengths = project.q0rgs[q0rg_index]
        .layers
        .iter()
        .map(|layer| {
            (
                layer.layer_id,
                layer
                    .timeline_frame_count
                    .unwrap_or(old_frame_count)
                    .min(old_frame_count),
            )
        })
        .collect::<HashMap<_, _>>();
    let folder_layer_ids = project.q0rgs[q0rg_index]
        .layers
        .iter()
        .filter(|layer| project.layer_is_folder(q0rg_id, layer.layer_id))
        .map(|layer| layer.layer_id)
        .collect::<HashSet<_>>();
    let target_old_length = old_layer_lengths.get(&layer_id).copied()?;
    let new_frame_count = old_frame_count.max(required_length);
    let continue_peers = continue_matching_end_layers
        && frame == old_frame_count
        && target_old_length == old_frame_count;

    let q0rg = &mut project.q0rgs[q0rg_index];
    q0rg.frame_count = new_frame_count;
    for layer in &mut q0rg.layers {
        let old_length = old_layer_lengths.get(&layer.layer_id).copied()?;
        if layer.layer_id == layer_id {
            let new_length = old_length.max(required_length);
            layer.timeline_frame_count = (new_length < new_frame_count).then_some(new_length);
        } else if new_frame_count > old_frame_count {
            let new_length = if continue_peers && old_length == old_frame_count {
                new_frame_count
            } else {
                old_length
            };
            if !folder_layer_ids.contains(&layer.layer_id) {
                layer.timeline_frame_count = (new_length < new_frame_count).then_some(new_length);
            }
        }
    }

    Some(new_frame_count)
}

/// Remove selected columns of time from the whole q0rg. Later keyframes,
/// explicit blank keys and tween targets shift left together. A q0rg always
/// keeps one frame; removing the complete timeline resets drawable layers to
/// one blank root keyframe.
pub fn remove_selected_frame_columns(
    project: &mut ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Option<u16> {
    let (_, first_frame, last_frame) = selected_frame_bounds(project, q0rg_id, selection)?;
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let drawable_layer_ids = project.q0rgs[q0rg_index]
        .layers
        .iter()
        .filter(|layer| !project.layer_is_folder(q0rg_id, layer.layer_id))
        .map(|layer| layer.layer_id)
        .collect::<HashSet<_>>();
    let old_count = project.q0rgs[q0rg_index].frame_count;
    if old_count <= 1 {
        return Some(0);
    }
    let width = last_frame - first_frame + 1;

    // Audio clips are timeline entities, not placements. Removing global frame
    // columns must therefore shift them explicitly. If the cut intersects a
    // clip, remove the whole clip: AudioClip currently has no source-offset or
    // crop fields, so keeping a remainder would silently restart the source.
    let mut shifted_audio = Vec::with_capacity(project.audio_clips.len());
    for mut clip in project.audio_clips.iter().copied() {
        if clip.q0rg_id != q0rg_id {
            shifted_audio.push(clip);
            continue;
        }
        let Some(end_frame) = q0s_format::v2::audio_clip_end_frame(project, clip) else {
            shifted_audio.push(clip);
            continue;
        };
        if clip.start_frame <= last_frame && first_frame < end_frame {
            continue;
        }
        if clip.start_frame > last_frame {
            clip.start_frame -= width;
        }
        shifted_audio.push(clip);
    }
    project.audio_clips = shifted_audio;
    let audio_at_zero_layers = project
        .audio_clips
        .iter()
        .filter(|clip| clip.q0rg_id == q0rg_id && clip.start_frame == 0)
        .map(|clip| clip.layer_id)
        .collect::<HashSet<_>>();

    if first_frame == 0 && last_frame == old_count - 1 {
        clear_rig_frame_range(project, q0rg_id, 0, old_count - 1);
    } else {
        shift_rig_keys_after_removed_columns(project, q0rg_id, first_frame, last_frame, width);
    }

    project.runtime.frame_scripts.retain_mut(|script| {
        if script.q0rg_id != q0rg_id {
            return true;
        }
        if (first_frame..=last_frame).contains(&script.frame) {
            return false;
        }
        if script.frame > last_frame {
            script.frame -= width;
        }
        true
    });
    project.runtime.frame_labels = std::mem::take(&mut project.runtime.frame_labels)
        .into_iter()
        .filter_map(|(mut key, label)| {
            if key.q0rg_id == q0rg_id {
                if (first_frame..=last_frame).contains(&key.frame) {
                    return None;
                }
                if key.frame > last_frame {
                    key.frame -= width;
                }
            }
            Some((key, label))
        })
        .collect();

    if q0rg_id == project.meta.entry_q0rg_id {
        if first_frame == 0 && last_frame == old_count - 1 {
            project.runtime.camera.keys.clear();
        } else {
            let mut shifted_camera = Vec::with_capacity(project.runtime.camera.keys.len());
            for mut key in project.runtime.camera.keys.drain(..) {
                if (first_frame..=last_frame).contains(&key.frame) {
                    continue;
                }
                if key.frame > last_frame {
                    key.frame -= width;
                }
                key.tween = match key.tween.to_frame() {
                    Some(to_frame) if (first_frame..=last_frame).contains(&to_frame) => Tween::None,
                    Some(to_frame) if to_frame > last_frame => {
                        key.tween.with_to_frame(to_frame - width)
                    }
                    Some(_) | None => key.tween,
                };
                shifted_camera.push(key);
            }
            shifted_camera.sort_by_key(|key| key.frame);
            project.runtime.camera.keys = shifted_camera;
        }
    }

    let q0rg = &mut project.q0rgs[q0rg_index];
    if first_frame == 0 && last_frame == old_count - 1 {
        q0rg.frame_count = 1;
        for layer in &mut q0rg.layers {
            layer.explicit_keyframes.clear();
            layer.placements.clear();
            if let Some(layer_frame_count) = &mut layer.timeline_frame_count {
                *layer_frame_count = (*layer_frame_count).min(1);
            }
            if drawable_layer_ids.contains(&layer.layer_id)
                && !audio_at_zero_layers.contains(&layer.layer_id)
            {
                layer.ensure_explicit_keyframe(0);
            }
        }
        return Some(old_count - 1);
    }

    q0rg.frame_count = old_count - width;
    for layer in &mut q0rg.layers {
        if let Some(layer_frame_count) = &mut layer.timeline_frame_count {
            let removed_from_layer = if first_frame >= *layer_frame_count {
                0
            } else {
                last_frame
                    .min(layer_frame_count.saturating_sub(1))
                    .saturating_sub(first_frame)
                    .saturating_add(1)
            };
            *layer_frame_count = layer_frame_count
                .saturating_sub(removed_from_layer)
                .min(q0rg.frame_count);
        }
        let mut shifted = Vec::with_capacity(layer.placements.len());
        for mut placement in layer.placements.drain(..) {
            if (first_frame..=last_frame).contains(&placement.frame) {
                continue;
            }
            if placement.frame > last_frame {
                placement.frame -= width;
            }
            placement.tween = match placement.tween.to_frame() {
                Some(to_frame) if (first_frame..=last_frame).contains(&to_frame) => Tween::None,
                Some(to_frame) if to_frame > last_frame => {
                    placement.tween.with_to_frame(to_frame - width)
                }
                Some(_) | None => placement.tween,
            };
            shifted.push(placement);
        }
        layer.placements = shifted;

        layer
            .explicit_keyframes
            .retain(|frame| !(first_frame..=last_frame).contains(frame));
        for frame in &mut layer.explicit_keyframes {
            if *frame > last_frame {
                *frame -= width;
            }
        }
        layer.explicit_keyframes.sort_unstable();
        layer.explicit_keyframes.dedup();
        if drawable_layer_ids.contains(&layer.layer_id)
            && !audio_at_zero_layers.contains(&layer.layer_id)
            && !layer.has_keyframe(0)
        {
            layer.ensure_explicit_keyframe(0);
        }
    }
    Some(width)
}

/// Remove selected frame cells from only their own layers. Later content on
/// each affected layer shifts left by the number of removed cells before it;
/// every unselected layer stays unchanged. The shared tail follows real tracks.
pub fn remove_selected_frame_cells(
    project: &mut ProjectV2,
    q0rg_id: u16,
    cells: &[(u16, u16)],
) -> Option<usize> {
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let frame_count = project.q0rgs[q0rg_index].frame_count;
    if frame_count == 0 {
        return Some(0);
    }

    let layer_frame_counts = project.q0rgs[q0rg_index]
        .layers
        .iter()
        .map(|layer| {
            (
                layer.layer_id,
                layer.timeline_frame_count.unwrap_or(frame_count),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut removed_by_layer = BTreeMap::<u16, BTreeSet<u16>>::new();
    for &(layer_id, frame) in cells {
        if frame >= layer_frame_counts.get(&layer_id).copied().unwrap_or(0)
            || project.layer_is_folder(q0rg_id, layer_id)
        {
            continue;
        }
        removed_by_layer.entry(layer_id).or_default().insert(frame);
    }
    removed_by_layer.retain(|_, frames| !frames.is_empty());
    if removed_by_layer.is_empty() {
        return Some(0);
    }

    project.runtime.frame_scripts.retain_mut(|script| {
        if script.q0rg_id != q0rg_id {
            return true;
        }
        let Some(removed_frames) = removed_by_layer.get(&script.layer_id) else {
            return true;
        };
        let Some(frame) = compact_frame_after_removal(script.frame, removed_frames) else {
            return false;
        };
        script.frame = frame;
        true
    });

    let old_audio = std::mem::take(&mut project.audio_clips);
    let mut shifted_audio = Vec::with_capacity(old_audio.len());
    for mut clip in old_audio {
        if clip.q0rg_id != q0rg_id {
            shifted_audio.push(clip);
            continue;
        }
        let Some(removed_frames) = removed_by_layer.get(&clip.layer_id) else {
            shifted_audio.push(clip);
            continue;
        };
        let end_frame = q0s_format::v2::audio_clip_end_frame(project, clip)
            .unwrap_or_else(|| clip.start_frame.saturating_add(1));
        if removed_frames
            .iter()
            .any(|frame| *frame >= clip.start_frame && *frame < end_frame)
        {
            continue;
        }
        let Some(start_frame) = compact_frame_after_removal(clip.start_frame, removed_frames)
        else {
            continue;
        };
        clip.start_frame = start_frame;
        shifted_audio.push(clip);
    }
    project.audio_clips = shifted_audio;

    let q0rg = &mut project.q0rgs[q0rg_index];
    for (layer_id, removed_frames) in &removed_by_layer {
        let layer = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == *layer_id)?;

        let mut shifted_placements = Vec::with_capacity(layer.placements.len());
        for mut placement in layer.placements.drain(..) {
            let Some(frame) = compact_frame_after_removal(placement.frame, removed_frames) else {
                continue;
            };
            placement.frame = frame;
            if let Some(to_frame) = placement.tween.to_frame() {
                placement.tween = match compact_frame_after_removal(to_frame, removed_frames) {
                    Some(to_frame) => placement.tween.with_to_frame(to_frame),
                    None => Tween::None,
                };
            }
            shifted_placements.push(placement);
        }
        layer.placements = shifted_placements;

        layer.explicit_keyframes = std::mem::take(&mut layer.explicit_keyframes)
            .into_iter()
            .filter_map(|frame| compact_frame_after_removal(frame, removed_frames))
            .collect();
        layer.explicit_keyframes.sort_unstable();
        layer.explicit_keyframes.dedup();

        let old_layer_length = layer_frame_counts
            .get(layer_id)
            .copied()
            .unwrap_or(frame_count);
        let new_layer_length = old_layer_length.saturating_sub(removed_frames.len() as u16);
        layer.timeline_frame_count = (new_layer_length < frame_count).then_some(new_layer_length);
    }

    sync_folder_timelines(project, q0rg_id);
    Some(removed_by_layer.values().map(BTreeSet::len).sum())
}

fn compact_frame_after_removal(frame: u16, removed_frames: &BTreeSet<u16>) -> Option<u16> {
    if removed_frames.contains(&frame) {
        return None;
    }
    let shift = u16::try_from(removed_frames.range(..frame).count()).ok()?;
    frame.checked_sub(shift)
}

/// Remove source cells for a drag-move without turning every vacated cell into
/// a blank keyframe. Frame zero is the only exception: a layer cannot inherit
/// anything from before the timeline, so moving its first key leaves one real
/// blank root key behind.
fn remove_frames_for_move(
    project: &mut ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Option<usize> {
    let (layer_ids, first_frame, last_frame) = selected_frame_bounds(project, q0rg_id, selection)?;
    let selected_layers = layer_ids.iter().copied().collect::<HashSet<_>>();
    project.runtime.frame_scripts.retain(|script| {
        script.q0rg_id != q0rg_id
            || !selected_layers.contains(&script.layer_id)
            || !(first_frame..=last_frame).contains(&script.frame)
    });
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let q0rg = &mut project.q0rgs[q0rg_index];
    let mut removed = 0;
    for layer_id in layer_ids {
        let layer = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)?;
        let before = layer.placements.len();
        clear_frame_range(layer, first_frame, last_frame, false);
        if first_frame == 0 {
            layer.ensure_explicit_keyframe(0);
        }
        removed += before - layer.placements.len();
    }
    Some(removed)
}

fn target_frame_layer_ids(
    project: &ProjectV2,
    q0rg_id: u16,
    start_layer_id: u16,
    count: usize,
) -> Vec<u16> {
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    let visible_order = q0rg
        .layers
        .iter()
        .rev()
        .filter(|layer| !project.layer_is_folder(q0rg_id, layer.layer_id))
        .map(|layer| layer.layer_id)
        .collect::<Vec<_>>();
    let Some(start) = visible_order
        .iter()
        .position(|layer_id| *layer_id == start_layer_id)
    else {
        return Vec::new();
    };
    visible_order[start..].iter().copied().take(count).collect()
}

fn ensure_target_frame_layers(
    project: &mut ProjectV2,
    q0rg_id: u16,
    start_layer_id: u16,
    count: usize,
) -> Vec<u16> {
    let existing = target_frame_layer_ids(project, q0rg_id, start_layer_id, count);
    if existing.len() >= count {
        return existing;
    }
    let Some(q0rg_index) = q0rg_index(project, q0rg_id) else {
        return Vec::new();
    };
    let missing = count - existing.len();
    let q0rg = &mut project.q0rgs[q0rg_index];
    let mut next_id = q0rg
        .layers
        .iter()
        .map(|layer| layer.layer_id)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
        .max(1);
    for _ in 0..missing {
        q0rg.layers.insert(
            0,
            Layer {
                layer_id: next_id,
                timeline_frame_count: None,
                name: format!("Layer {next_id}"),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            },
        );
        next_id = next_id.saturating_add(1);
    }
    target_frame_layer_ids(project, q0rg_id, start_layer_id, count)
}

pub fn paste_frames(
    project: &mut ProjectV2,
    q0rg_id: u16,
    target_layer_id: u16,
    target_frame: u16,
    clipboard: &TimelineFrameClipboard,
) -> Option<TimelineSelection> {
    if clipboard.width == 0 || clipboard.rows.is_empty() {
        return None;
    }
    q0s_format::v2::validate_q0rg_insertions(
        project,
        q0rg_id,
        clipboard
            .rows
            .iter()
            .flat_map(|row| &row.placements)
            .map(|p| p.target),
    )
    .ok()?;
    let target_layer_ids =
        ensure_target_frame_layers(project, q0rg_id, target_layer_id, clipboard.rows.len());
    if target_layer_ids.len() != clipboard.rows.len() {
        return None;
    }
    let last_frame = target_frame.saturating_add(clipboard.width - 1);
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let mut used_ids = used_instance_ids(project);
    let mut next_id = 1_u32;
    let mut prepared_rows = Vec::with_capacity(clipboard.rows.len());
    for (layer_id, row) in target_layer_ids.iter().copied().zip(&clipboard.rows) {
        let mut prepared = row.placements.clone();
        if layer_id != row.source_layer_id {
            // If an id still exists after any preceding cut/remove operation,
            // this is a copy into another layer and must become a new object.
            let existing = prepared
                .iter()
                .filter_map(|placement| {
                    (placement.instance_id != 0 && used_ids.contains(&placement.instance_id))
                        .then_some(placement.instance_id)
                })
                .collect::<HashSet<_>>();
            let mut remap = HashMap::<u32, u32>::new();
            for placement in &mut prepared {
                if existing.contains(&placement.instance_id) {
                    let fresh = match remap.get(&placement.instance_id).copied() {
                        Some(id) => id,
                        None => {
                            let id = allocate_instance_id(&mut used_ids, &mut next_id)?;
                            remap.insert(placement.instance_id, id);
                            id
                        }
                    };
                    placement.instance_id = fresh;
                }
            }
        }
        prepared_rows.push(prepared);
    }
    let target_layer_set = target_layer_ids.iter().copied().collect::<HashSet<_>>();
    let selected_target_cells = target_layer_ids
        .iter()
        .copied()
        .zip(&clipboard.rows)
        .flat_map(|(layer_id, row)| {
            row.selected_frames
                .iter()
                .copied()
                .map(move |relative_frame| (layer_id, target_frame.saturating_add(relative_frame)))
        })
        .collect::<HashSet<_>>();
    project.runtime.frame_scripts.retain(|script| {
        script.q0rg_id != q0rg_id
            || !target_layer_set.contains(&script.layer_id)
            || !selected_target_cells.contains(&(script.layer_id, script.frame))
    });
    let mut pasted_scripts = Vec::<FrameScript>::new();
    for (layer_id, row) in target_layer_ids.iter().copied().zip(&clipboard.rows) {
        pasted_scripts.extend(row.frame_scripts.iter().cloned().map(|mut script| {
            script.q0rg_id = q0rg_id;
            script.layer_id = layer_id;
            script.frame = target_frame.saturating_add(script.frame);
            script
        }));
    }
    {
        let q0rg = &mut project.q0rgs[q0rg_index];
        q0rg.frame_count = q0rg.frame_count.max(last_frame.saturating_add(1));

        for ((layer_id, row), prepared) in target_layer_ids
            .iter()
            .copied()
            .zip(&clipboard.rows)
            .zip(prepared_rows)
        {
            let layer = q0rg
                .layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)?;
            layer.extend_timeline_through(last_frame);
            for relative_frame in &row.selected_frames {
                let frame = target_frame.saturating_add(*relative_frame);
                clear_frame_range(layer, frame, frame, false);
            }
            for relative_frame in &row.explicit_keyframes {
                layer.ensure_explicit_keyframe(target_frame.saturating_add(*relative_frame));
            }
            for mut placement in prepared {
                placement.frame = target_frame.saturating_add(placement.frame);
                if let Some(to_frame) = placement.tween.to_frame() {
                    placement.tween = placement
                        .tween
                        .with_to_frame(target_frame.saturating_add(to_frame));
                }
                layer.push_placement(placement);
            }
            if row.selected_frames.contains(&0) && !layer.has_keyframe(target_frame) {
                layer.ensure_explicit_keyframe(target_frame);
            }
        }
    }
    paste_rig_channels(
        project,
        q0rg_id,
        target_frame,
        clipboard.width,
        &clipboard.rig_channels,
    );
    project.runtime.frame_scripts.extend(pasted_scripts);
    project
        .runtime
        .frame_scripts
        .sort_by_key(|script| (script.q0rg_id, script.frame, script.layer_id));

    Some(TimelineSelection {
        anchor_layer_id: target_layer_ids[0],
        anchor_frame: target_frame,
        focus_layer_id: *target_layer_ids.last()?,
        focus_frame: last_frame,
    })
}

pub fn pasted_frame_cells(
    project: &ProjectV2,
    q0rg_id: u16,
    target_layer_id: u16,
    target_frame: u16,
    clipboard: &TimelineFrameClipboard,
) -> Vec<(u16, u16)> {
    target_frame_layer_ids(project, q0rg_id, target_layer_id, clipboard.rows.len())
        .into_iter()
        .zip(&clipboard.rows)
        .flat_map(|(layer_id, row)| {
            row.selected_frames
                .iter()
                .copied()
                .map(move |relative_frame| (layer_id, target_frame.saturating_add(relative_frame)))
        })
        .collect()
}

pub fn duplicate_frames(
    project: &mut ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Option<TimelineSelection> {
    let (layer_ids, _, last_frame) = selected_frame_bounds(project, q0rg_id, selection)?;
    let target_layer_id = *layer_ids.first()?;
    let target_frame = last_frame.checked_add(1)?;
    let clipboard = capture_frames(project, q0rg_id, selection)?;
    paste_frames(project, q0rg_id, target_layer_id, target_frame, &clipboard)
}

/// A tween with exactly one end inside a selection that is about to move.
struct CrossingTween {
    layer_id: u16,
    /// Keyframe of the tweened placement and its position among that
    /// keyframe's placements, which survives the move.
    frame: u16,
    ordinal: usize,
    target: q0s_format::v2::Target,
    tween: Tween,
    source_moves: bool,
}

fn crossing_tweens(
    project: &ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
) -> Vec<CrossingTween> {
    let Some((layer_ids, first_frame, last_frame)) =
        selected_frame_bounds(project, q0rg_id, selection)
    else {
        return Vec::new();
    };
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    let selected = first_frame..=last_frame;
    let mut crossing = Vec::new();
    for layer in &q0rg.layers {
        if !layer_ids.contains(&layer.layer_id) {
            continue;
        }
        let mut ordinals = HashMap::<u16, usize>::new();
        for placement in &layer.placements {
            let ordinal = ordinals.entry(placement.frame).or_insert(0);
            let own = *ordinal;
            *ordinal += 1;
            let Some(to_frame) = placement.tween.to_frame() else {
                continue;
            };
            let source_moves = selected.contains(&placement.frame);
            if source_moves != selected.contains(&to_frame) {
                crossing.push(CrossingTween {
                    layer_id: layer.layer_id,
                    frame: placement.frame,
                    ordinal: own,
                    target: placement.target,
                    tween: placement.tween,
                    source_moves,
                });
            }
        }
    }
    crossing
}

/// Reconnect tweens whose keyframe was dragged along its own layer. A tween
/// stores the frame number it leads to, so moving either of its keyframes
/// used to cut it; the animator only meant to retime it.
fn reconnect_moved_tweens(
    project: &mut ProjectV2,
    q0rg_id: u16,
    crossing: Vec<CrossingTween>,
    stayed_on_layer: &HashSet<u16>,
    delta: i32,
) {
    let Some(q0rg) = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
    else {
        return;
    };
    let moved = |frame: u16| u16::try_from(i32::from(frame) + delta).ok();
    for link in crossing {
        if !stayed_on_layer.contains(&link.layer_id) {
            continue;
        }
        let Some(to_frame) = link.tween.to_frame() else {
            continue;
        };
        let (frame, to_frame) = if link.source_moves {
            (moved(link.frame), Some(to_frame))
        } else {
            (Some(link.frame), moved(to_frame))
        };
        let (Some(frame), Some(to_frame)) = (frame, to_frame) else {
            continue;
        };
        let Some(layer) = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == link.layer_id)
        else {
            continue;
        };
        // Still a tween between two neighbouring keyframes, in that order.
        let reachable = frame < to_frame
            && layer.has_keyframe(to_frame)
            && layer
                .timeline_frame_count
                .is_none_or(|frame_count| to_frame < frame_count)
            && !layer
                .keyframe_frames()
                .into_iter()
                .any(|keyframe| frame < keyframe && keyframe < to_frame);
        if !reachable {
            continue;
        }
        if let Some(placement) = layer
            .placements
            .iter_mut()
            .filter(|placement| placement.frame == frame)
            .nth(link.ordinal)
            .filter(|placement| {
                placement.target == link.target && matches!(placement.tween, Tween::None)
            })
        {
            placement.tween = link.tween.with_to_frame(to_frame);
        }
    }
}

pub fn move_frames(
    project: &mut ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
    target_layer_id: u16,
    target_frame: u16,
) -> Option<TimelineSelection> {
    let crossing = crossing_tweens(project, q0rg_id, selection);
    let (source_layer_ids, first_frame, _) = selected_frame_bounds(project, q0rg_id, selection)?;
    let clipboard = capture_frames(project, q0rg_id, selection)?;
    remove_frames_for_move(project, q0rg_id, selection)?;
    let moved = paste_frames(project, q0rg_id, target_layer_id, target_frame, &clipboard)?;
    let stayed_on_layer = source_layer_ids
        .iter()
        .zip(target_frame_layer_ids(
            project,
            q0rg_id,
            target_layer_id,
            source_layer_ids.len(),
        ))
        .filter(|(source, target)| **source == *target)
        .map(|(source, _)| *source)
        .collect::<HashSet<_>>();
    reconnect_moved_tweens(
        project,
        q0rg_id,
        crossing,
        &stayed_on_layer,
        i32::from(target_frame) - i32::from(first_frame),
    );
    Some(moved)
}

fn selected_layer_ids(
    project: &ProjectV2,
    q0rg_id: u16,
    selection: TimelineLayerSelection,
) -> Option<Vec<u16>> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let ui_order = q0rg
        .layers
        .iter()
        .rev()
        .map(|layer| layer.layer_id)
        .collect::<Vec<_>>();
    let anchor = ui_order
        .iter()
        .position(|layer_id| *layer_id == selection.anchor_layer_id)?;
    let focus = ui_order
        .iter()
        .position(|layer_id| *layer_id == selection.focus_layer_id)?;
    let first = anchor.min(focus);
    let last = anchor.max(focus);
    let mut selected = ui_order[first..=last]
        .iter()
        .copied()
        .collect::<HashSet<_>>();

    // A folder row owns its complete descendant subtree even while collapsed.
    // Selecting/copying a nested folder therefore cannot orphan grandchildren.
    let folders = selected
        .iter()
        .copied()
        .filter(|folder_id| project.layer_is_folder(q0rg_id, *folder_id))
        .collect::<Vec<_>>();
    for folder_id in folders {
        for layer in &q0rg.layers {
            if project.layer_is_descendant_of(q0rg_id, layer.layer_id, folder_id) {
                selected.insert(layer.layer_id);
            }
        }
    }
    Some(
        q0rg.layers
            .iter()
            .filter(|layer| selected.contains(&layer.layer_id))
            .map(|layer| layer.layer_id)
            .collect(),
    )
}

pub fn capture_layers(
    project: &ProjectV2,
    q0rg_id: u16,
    selection: TimelineLayerSelection,
) -> Option<TimelineLayerClipboard> {
    let ids = selected_layer_ids(project, q0rg_id, selection)?;
    if ids.is_empty() {
        return None;
    }
    let selected = ids.iter().copied().collect::<HashSet<_>>();
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let layers = q0rg
        .layers
        .iter()
        .filter(|layer| selected.contains(&layer.layer_id))
        .cloned()
        .collect::<Vec<_>>();
    let metadata = layers
        .iter()
        .map(|layer| {
            let mut metadata = project.layer_metadata(q0rg_id, layer.layer_id);
            if metadata
                .parent_folder_id
                .is_some_and(|parent| !selected.contains(&parent))
            {
                metadata.parent_folder_id = None;
            }
            (layer.layer_id, metadata)
        })
        .collect();
    let frame_scripts = project
        .runtime
        .frame_scripts
        .iter()
        .filter(|script| script.q0rg_id == q0rg_id && selected.contains(&script.layer_id))
        .cloned()
        .collect();
    Some(TimelineLayerClipboard {
        layers,
        metadata,
        frame_scripts,
    })
}

pub fn remove_layers(
    project: &mut ProjectV2,
    q0rg_id: u16,
    selection: TimelineLayerSelection,
) -> Option<usize> {
    let ids = selected_layer_ids(project, q0rg_id, selection)?;
    if ids.is_empty() {
        return None;
    }
    let removed = ids.iter().copied().collect::<HashSet<_>>();
    project
        .audio_clips
        .retain(|clip| clip.q0rg_id != q0rg_id || !removed.contains(&clip.layer_id));
    project
        .runtime
        .frame_scripts
        .retain(|script| script.q0rg_id != q0rg_id || !removed.contains(&script.layer_id));
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let removed_instance_ids = project.q0rgs[q0rg_index]
        .layers
        .iter()
        .filter(|layer| removed.contains(&layer.layer_id))
        .flat_map(|layer| &layer.placements)
        .filter_map(|placement| (placement.instance_id != 0).then_some(placement.instance_id))
        .collect::<HashSet<_>>();
    project.q0rgs[q0rg_index]
        .layers
        .retain(|layer| !removed.contains(&layer.layer_id));
    project
        .layer_metadata
        .retain(|key, _| key.q0rg_id != q0rg_id || !removed.contains(&key.layer_id));
    for (key, metadata) in &mut project.layer_metadata {
        if key.q0rg_id == q0rg_id
            && metadata
                .parent_folder_id
                .is_some_and(|parent| removed.contains(&parent))
        {
            metadata.parent_folder_id = None;
        }
    }
    if let Some(rig) = q0s_format::rig::rig_for_q0rg_mut(project, q0rg_id) {
        for node in &mut rig.nodes {
            if node
                .binding
                .is_some_and(|binding| removed_instance_ids.contains(&binding.instance_id))
            {
                node.binding = None;
            }
        }
        let unbound = rig
            .nodes
            .iter()
            .filter(|node| node.binding.is_none())
            .map(|node| node.node_id)
            .collect::<HashSet<_>>();
        rig.channels.retain(|channel| {
            channel
                .property
                .binding_node_id()
                .is_none_or(|node_id| !unbound.contains(&node_id))
        });
    }

    let has_drawable = project.q0rgs[q0rg_index]
        .layers
        .iter()
        .any(|layer| !project.layer_is_folder(q0rg_id, layer.layer_id));
    if !has_drawable {
        let next_id = project.q0rgs[q0rg_index]
            .layers
            .iter()
            .map(|layer| layer.layer_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        project.q0rgs[q0rg_index].layers.push(Layer {
            layer_id: next_id,
            timeline_frame_count: Some(1),
            name: format!("Layer {next_id}"),
            explicit_keyframes: vec![0],
            placements: Vec::new(),
        });
    }
    sync_folder_timelines(project, q0rg_id);
    Some(removed.len())
}

fn layer_insert_index(project: &ProjectV2, q0rg_id: u16, target_layer_id: u16) -> Option<usize> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let mut block_id = target_layer_id;
    let mut depth = 0usize;
    while let Some(parent_id) = project.layer_parent_folder(q0rg_id, block_id) {
        block_id = parent_id;
        depth += 1;
        if depth > q0s_format::v2::MAX_LAYER_FOLDER_NESTING_DEPTH {
            return None;
        }
    }
    q0rg.layers
        .iter()
        .position(|layer| layer.layer_id == block_id)
        .map(|index| index + 1)
}

pub fn paste_layers(
    project: &mut ProjectV2,
    q0rg_id: u16,
    target_layer_id: u16,
    clipboard: &TimelineLayerClipboard,
) -> Option<TimelineLayerSelection> {
    if clipboard.layers.is_empty() {
        return None;
    }
    q0s_format::v2::validate_q0rg_insertions(
        project,
        q0rg_id,
        clipboard
            .layers
            .iter()
            .flat_map(|layer| &layer.placements)
            .map(|p| p.target),
    )
    .ok()?;
    let insert_at = layer_insert_index(project, q0rg_id, target_layer_id)?;
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let mut next_id = project.q0rgs[q0rg_index]
        .layers
        .iter()
        .map(|layer| layer.layer_id)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
        .max(1);
    let mut id_map = HashMap::new();
    for layer in &clipboard.layers {
        id_map.insert(layer.layer_id, next_id);
        next_id = next_id.saturating_add(1);
    }

    let mut pasted_layers = clipboard.layers.clone();
    let mut used_ids = used_instance_ids(project);
    let mut next_instance = 1_u32;
    let mut identity_remap = HashMap::<u32, u32>::new();
    for layer in &mut pasted_layers {
        layer.layer_id = *id_map.get(&layer.layer_id)?;
        for placement in &mut layer.placements {
            if placement.instance_id == 0 {
                continue;
            }
            let fresh = match identity_remap.get(&placement.instance_id).copied() {
                Some(id) => id,
                None => {
                    let id = allocate_instance_id(&mut used_ids, &mut next_instance)?;
                    identity_remap.insert(placement.instance_id, id);
                    id
                }
            };
            placement.instance_id = fresh;
        }
    }
    let pasted_ui_top = pasted_layers.last()?.layer_id;
    let pasted_ui_bottom = pasted_layers.first()?.layer_id;
    let pasted_max_frame = pasted_layers
        .iter()
        .flat_map(|layer| {
            layer
                .placements
                .iter()
                .flat_map(|placement| {
                    let tween_end = placement.tween.to_frame();
                    std::iter::once(placement.frame).chain(tween_end)
                })
                .chain(layer.explicit_keyframes.iter().copied())
        })
        .max();

    project.q0rgs[q0rg_index]
        .layers
        .splice(insert_at..insert_at, pasted_layers);
    for (old_id, mut metadata) in clipboard.metadata.iter().copied() {
        let Some(new_id) = id_map.get(&old_id).copied() else {
            continue;
        };
        metadata.parent_folder_id = metadata
            .parent_folder_id
            .and_then(|parent| id_map.get(&parent).copied());
        if metadata != LayerMetadata::default() {
            project
                .layer_metadata
                .insert(LayerKey::new(q0rg_id, new_id), metadata);
        }
    }
    if let Some(max_frame) = pasted_max_frame {
        project.q0rgs[q0rg_index].frame_count = project.q0rgs[q0rg_index]
            .frame_count
            .max(max_frame.saturating_add(1));
    }
    for script in &clipboard.frame_scripts {
        let Some(layer_id) = id_map.get(&script.layer_id).copied() else {
            continue;
        };
        let mut script = script.clone();
        script.q0rg_id = q0rg_id;
        script.layer_id = layer_id;
        project.runtime.frame_scripts.push(script);
    }
    project
        .runtime
        .frame_scripts
        .sort_by_key(|script| (script.q0rg_id, script.frame, script.layer_id));

    Some(TimelineLayerSelection {
        anchor_layer_id: pasted_ui_top,
        focus_layer_id: pasted_ui_bottom,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{LayerKind, Placement, Target, Transform2D};

    // Timeline fixtures need a real child symbol, not a reference back to
    // their containing stage, now that paste rejects recursive references.
    fn timeline_project() -> ProjectV2 {
        let mut project = crate::state::default_project();
        let mut child = project.q0rgs[0].clone();
        child.q0rg_id = u16::MAX;
        child.name = "Timeline fixture child".into();
        project.q0rgs.push(child);
        project
    }

    fn placement(frame: u16, x: f32, tween: Tween) -> Placement {
        Placement {
            pivot: None,
            instance_id: 0,
            frame,
            target: Target::Q0rg(u16::MAX),
            transform: Transform2D {
                tx: x,
                ..Transform2D::IDENTITY
            },
            tween,
            fx: Default::default(),
        }
    }

    /// Keys at 2 and 6 joined by `tween`, with room to move either one.
    fn tweened_layer(tween: Tween) -> ProjectV2 {
        let mut project = timeline_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 16;
        q0rg.layers[0].explicit_keyframes.clear();
        q0rg.layers[0].placements =
            vec![placement(2, 10.0, tween), placement(6, 40.0, Tween::None)];
        project
    }

    fn tween_at(project: &ProjectV2, frame: u16) -> Tween {
        project.q0rgs[0].layers[0]
            .placements
            .iter()
            .find(|placement| placement.frame == frame)
            .unwrap_or_else(|| panic!("no keyframe content at frame {frame}"))
            .tween
    }

    #[test]
    fn dragging_the_last_keyframe_of_a_tween_retimes_it() {
        for tween in [
            Tween::Linear { to_frame: 6 },
            Tween::Shape {
                to_frame: 6,
                easing: q0s_format::v2::Easing::Hold,
            },
        ] {
            let mut project = tweened_layer(tween);
            move_frames(&mut project, 1, TimelineSelection::single(1, 6), 1, 9)
                .expect("move destination keyframe");
            assert_eq!(tween_at(&project, 2), tween.with_to_frame(9));
            assert_eq!(tween_at(&project, 9), Tween::None);
            q0s_format::v2::validate(&project).expect("retimed tween project");

            // And back again, shorter than it started.
            move_frames(&mut project, 1, TimelineSelection::single(1, 9), 1, 4)
                .expect("move destination keyframe back");
            assert_eq!(tween_at(&project, 2), tween.with_to_frame(4));
            q0s_format::v2::validate(&project).expect("shortened tween project");
        }
    }

    #[test]
    fn dragging_the_first_keyframe_of_a_tween_keeps_it_attached() {
        let tween = Tween::Linear { to_frame: 6 };
        let mut project = tweened_layer(tween);
        // Frame 0 holds the layer's root key; move the tween start from 2 to 4.
        move_frames(&mut project, 1, TimelineSelection::single(1, 2), 1, 4)
            .expect("move source keyframe");
        assert_eq!(tween_at(&project, 4), tween);
        assert!(project.q0rgs[0].layers[0]
            .placements
            .iter()
            .all(|placement| placement.frame != 2));
        q0s_format::v2::validate(&project).expect("moved tween start project");
    }

    #[test]
    fn a_middle_keyframe_keeps_both_of_its_tweens() {
        let mut project = tweened_layer(Tween::Linear { to_frame: 6 });
        let layer = &mut project.q0rgs[0].layers[0];
        layer.placements[1].tween = Tween::Linear { to_frame: 12 };
        layer.placements.push(placement(12, 90.0, Tween::None));

        move_frames(&mut project, 1, TimelineSelection::single(1, 6), 1, 8)
            .expect("move middle keyframe");
        assert_eq!(tween_at(&project, 2), Tween::Linear { to_frame: 8 });
        assert_eq!(tween_at(&project, 8), Tween::Linear { to_frame: 12 });
        q0s_format::v2::validate(&project).expect("retimed chain project");
    }

    #[test]
    fn a_tween_is_only_cut_when_the_move_makes_it_impossible() {
        // Past the tween's own start: there is nothing left to tween towards.
        let mut project = tweened_layer(Tween::Linear { to_frame: 6 });
        move_frames(&mut project, 1, TimelineSelection::single(1, 6), 1, 1)
            .expect("move destination before source");
        assert_eq!(tween_at(&project, 2), Tween::None);
        q0s_format::v2::validate(&project).expect("cut tween project");

        // Over another keyframe: the tween would have to run through it.
        let mut project = tweened_layer(Tween::Linear { to_frame: 6 });
        project.q0rgs[0].layers[0]
            .placements
            .push(placement(9, 70.0, Tween::None));
        move_frames(&mut project, 1, TimelineSelection::single(1, 6), 1, 11)
            .expect("move destination past another keyframe");
        assert_eq!(tween_at(&project, 2), Tween::None);
        q0s_format::v2::validate(&project).expect("blocked tween project");

        // To another layer: a tween cannot span two layers.
        let mut project = tweened_layer(Tween::Linear { to_frame: 6 });
        let mut other = project.q0rgs[0].layers[0].clone();
        other.layer_id = 2;
        other.placements.clear();
        project.q0rgs[0].layers.push(other);
        move_frames(&mut project, 1, TimelineSelection::single(1, 6), 2, 6)
            .expect("move destination to another layer");
        assert_eq!(tween_at(&project, 2), Tween::None);
        q0s_format::v2::validate(&project).expect("cross-layer move project");
    }

    #[test]
    fn frame_clipboard_preserves_content_blank_keys_and_internal_tweens() {
        let mut project = timeline_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 12;
        q0rg.layers[0].placements = vec![
            placement(2, 10.0, Tween::Linear { to_frame: 5 }),
            placement(5, 40.0, Tween::None),
        ];
        q0rg.layers[0].explicit_keyframes = vec![4];
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 1,
            focus_frame: 5,
        };

        let clipboard = capture_frames(&project, 1, selection).expect("capture frames");
        assert_eq!(clipboard.width, 4);
        assert_eq!(clipboard.rows[0].explicit_keyframes, vec![2]);
        assert_eq!(clipboard.rows[0].placements.len(), 2);
        assert_eq!(
            clipboard.rows[0].placements[0].tween,
            Tween::Linear { to_frame: 3 }
        );

        let pasted = paste_frames(&mut project, 1, 1, 7, &clipboard).expect("paste frames");
        assert_eq!(pasted.anchor_frame, 7);
        let layer = &project.q0rgs[0].layers[0];
        assert!(layer.explicit_keyframes.contains(&9));
        assert!(layer
            .placements
            .iter()
            .any(|item| { item.frame == 7 && item.tween == Tween::Linear { to_frame: 10 } }));
        assert!(layer.placements.iter().any(|item| item.frame == 10));
    }

    #[test]
    fn moving_selected_frames_keeps_payload_even_when_ranges_overlap() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 8;
        project.q0rgs[0].layers[0].placements = vec![
            placement(1, 10.0, Tween::None),
            placement(2, 20.0, Tween::None),
        ];
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 1,
            focus_layer_id: 1,
            focus_frame: 2,
        };
        move_frames(&mut project, 1, selection, 1, 2).expect("move frames");
        let layer = &project.q0rgs[0].layers[0];
        assert!(!layer.has_keyframe(1));
        assert!(layer
            .placements
            .iter()
            .any(|item| item.frame == 2 && item.transform.tx == 10.0));
        assert!(layer
            .placements
            .iter()
            .any(|item| item.frame == 3 && item.transform.tx == 20.0));
    }

    #[test]
    fn repeated_frame_moves_do_not_leave_blank_keyframe_trails() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 8;
        project.q0rgs[0].layers[0].placements = vec![placement(1, 10.0, Tween::None)];

        let first = TimelineSelection::single(1, 1);
        let moved = move_frames(&mut project, 1, first, 1, 2).expect("first move");
        move_frames(&mut project, 1, moved, 1, 3).expect("second move");

        let layer = &project.q0rgs[0].layers[0];
        assert_eq!(layer.keyframe_frames(), vec![0, 3]);
        assert!(!layer.has_keyframe(1));
        assert!(!layer.has_keyframe(2));
        assert!(layer.placements.iter().any(|item| item.frame == 3));
    }

    #[test]
    fn moving_first_frame_leaves_one_blank_root_keyframe() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 8;
        project.q0rgs[0].layers[0].placements = vec![placement(0, 10.0, Tween::None)];
        project.q0rgs[0].layers[0].explicit_keyframes.clear();

        move_frames(&mut project, 1, TimelineSelection::single(1, 0), 1, 3)
            .expect("move first frame");

        let layer = &project.q0rgs[0].layers[0];
        assert!(layer.is_blank_keyframe(0));
        assert_eq!(layer.keyframe_frames(), vec![0, 3]);
        assert!(layer.placements.iter().any(|item| item.frame == 3));
    }

    #[test]
    fn duplicate_frames_places_the_complete_range_immediately_after_itself() {
        let mut project = timeline_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 6;
        q0rg.layers[0].explicit_keyframes = vec![2];
        q0rg.layers[0].placements = vec![placement(1, 10.0, Tween::None)];
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 1,
            focus_layer_id: 1,
            focus_frame: 2,
        };

        let duplicated = duplicate_frames(&mut project, 1, selection).expect("duplicate frames");

        assert_eq!(duplicated.anchor_frame, 3);
        assert_eq!(duplicated.focus_frame, 4);
        let layer = &project.q0rgs[0].layers[0];
        assert!(layer
            .placements
            .iter()
            .any(|item| item.frame == 3 && item.transform.tx == 10.0));
        assert!(layer.is_blank_keyframe(4));
    }

    #[test]
    fn materializing_a_frame_range_creates_every_key_and_clips_incoming_tween() {
        let mut project = timeline_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 8;
        q0rg.layers[0].explicit_keyframes.clear();
        q0rg.layers[0].placements = vec![
            placement(0, 0.0, Tween::Linear { to_frame: 6 }),
            placement(6, 60.0, Tween::None),
        ];
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 1,
            focus_frame: 3,
        };

        let created = materialize_selected_keyframes(&mut project, 1, selection)
            .expect("materialize selected range");

        let layer = &project.q0rgs[0].layers[0];
        assert_eq!(created, 2);
        assert_eq!(layer.keyframe_frames(), vec![0, 2, 3, 6]);
        assert!(layer
            .placements
            .iter()
            .any(|item| item.frame == 2 && (item.transform.tx - 20.0).abs() < 0.001));
        assert!(layer
            .placements
            .iter()
            .any(|item| item.frame == 3 && (item.transform.tx - 30.0).abs() < 0.001));
        assert_eq!(layer.placements[0].tween, Tween::Linear { to_frame: 2 });
    }

    #[test]
    fn removing_selected_keyframes_demotes_range_but_preserves_root_key() {
        let mut project = timeline_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 8;
        q0rg.layers[0].explicit_keyframes = vec![4];
        q0rg.layers[0].placements = vec![
            placement(0, 0.0, Tween::Linear { to_frame: 3 }),
            placement(3, 30.0, Tween::None),
            placement(5, 50.0, Tween::None),
        ];
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 0,
            focus_layer_id: 1,
            focus_frame: 4,
        };

        let removed = remove_selected_keyframes(&mut project, 1, selection)
            .expect("remove selected keyframes");

        let layer = &project.q0rgs[0].layers[0];
        assert_eq!(removed, 2);
        assert_eq!(layer.keyframe_frames(), vec![0, 5]);
        assert!(layer.has_keyframe(0));
        assert!(!layer.has_keyframe(3));
        assert!(!layer.has_keyframe(4));
        assert_eq!(layer.placements[0].tween, Tween::None);
    }

    #[test]
    fn copy_and_paste_preserve_custom_tween_easing() {
        use q0s_format::v2::Easing;

        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 8;
        let layer_id = project.q0rgs[0].layers[0].layer_id;
        project.q0rgs[0].layers[0].explicit_keyframes.clear();
        project.q0rgs[0].layers[0].placements = vec![
            placement(
                0,
                0.0,
                Tween::Eased {
                    to_frame: 3,
                    easing: Easing::CubicBezier {
                        x1: 0.2,
                        y1: -0.4,
                        x2: 0.8,
                        y2: 1.4,
                    },
                },
            ),
            placement(3, 30.0, Tween::None),
        ];
        let selection = TimelineSelection {
            anchor_layer_id: layer_id,
            anchor_frame: 0,
            focus_layer_id: layer_id,
            focus_frame: 3,
        };
        let clipboard = capture_frames(&project, 1, selection).expect("capture tween");
        let pasted = paste_frames(&mut project, 1, layer_id, 4, &clipboard).expect("paste tween");
        assert_eq!(pasted.anchor_frame, 4);
        assert_eq!(pasted.focus_frame, 7);
        let pasted_tween = project.q0rgs[0].layers[0]
            .placements
            .iter()
            .find(|placement| placement.frame == 4)
            .expect("pasted source")
            .tween;
        assert_eq!(
            pasted_tween,
            Tween::Eased {
                to_frame: 7,
                easing: Easing::CubicBezier {
                    x1: 0.2,
                    y1: -0.4,
                    x2: 0.8,
                    y2: 1.4,
                },
            }
        );
    }

    #[test]
    fn inserting_hold_frame_shifts_only_target_layer_and_its_frame_script() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 6;
        project.q0rgs[0].layers[0].placements = vec![
            placement(0, 0.0, Tween::None),
            placement(4, 40.0, Tween::None),
        ];
        let mut other_layer = project.q0rgs[0].layers[0].clone();
        other_layer.layer_id = 2;
        other_layer.name = "other".to_string();
        other_layer.placements[1].transform.tx = 80.0;
        project.q0rgs[0].layers.push(other_layer);
        project.runtime.frame_scripts.push(FrameScript {
            q0rg_id: 1,
            layer_id: 1,
            frame: 4,
            source: "target".to_string(),
        });
        project.runtime.frame_scripts.push(FrameScript {
            q0rg_id: 1,
            layer_id: 2,
            frame: 4,
            source: "other".to_string(),
        });

        let new_count = insert_hold_frame_before(&mut project, 1, 1, 4)
            .expect("insert target-layer hold frame");

        assert_eq!(new_count, 7);
        let target = &project.q0rgs[0].layers[0];
        let other = &project.q0rgs[0].layers[1];
        assert!(target
            .placements
            .iter()
            .any(|item| item.frame == 5 && item.transform.tx == 40.0));
        assert_eq!(crate::render::active_placements_at(target, 4).len(), 1);
        assert!(other
            .placements
            .iter()
            .any(|item| item.frame == 4 && item.transform.tx == 80.0));
        assert_eq!(project.runtime.frame_scripts[0].frame, 5);
        assert_eq!(project.runtime.frame_scripts[1].frame, 4);
    }

    #[test]
    fn inserting_hold_frame_inside_tween_extends_tween_destination() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 6;
        project.q0rgs[0].layers[0].placements = vec![
            placement(0, 0.0, Tween::Linear { to_frame: 4 }),
            placement(4, 40.0, Tween::None),
        ];

        insert_hold_frame_before(&mut project, 1, 1, 2).expect("insert tween frame");

        let layer = &project.q0rgs[0].layers[0];
        assert_eq!(layer.placements[0].tween, Tween::Linear { to_frame: 5 });
        assert!(layer
            .placements
            .iter()
            .any(|item| item.frame == 5 && item.transform.tx == 40.0));
    }

    #[test]
    fn inserting_hold_at_frame_zero_keeps_root_key_and_shifts_later_keys() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 5;
        project.q0rgs[0].layers[0].placements = vec![
            placement(0, 0.0, Tween::None),
            placement(3, 30.0, Tween::None),
        ];

        insert_hold_frame_before(&mut project, 1, 1, 0).expect("insert root hold");

        let layer = &project.q0rgs[0].layers[0];
        assert!(layer.has_keyframe(0));
        assert!(layer
            .placements
            .iter()
            .any(|item| item.frame == 0 && item.transform.tx == 0.0));
        assert!(layer
            .placements
            .iter()
            .any(|item| item.frame == 4 && item.transform.tx == 30.0));
        assert_eq!(project.q0rgs[0].frame_count, 6);
    }

    #[test]
    fn removing_selected_frame_columns_shifts_keys_blanks_and_tween_targets() {
        let mut project = timeline_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 10;
        q0rg.layers[0].explicit_keyframes = vec![2, 4, 7];
        q0rg.layers[0].placements = vec![
            placement(0, 0.0, Tween::Linear { to_frame: 5 }),
            placement(4, 40.0, Tween::None),
            placement(5, 50.0, Tween::None),
            placement(8, 80.0, Tween::None),
        ];
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 3,
            focus_layer_id: 1,
            focus_frame: 4,
        };

        let removed = remove_selected_frame_columns(&mut project, 1, selection)
            .expect("remove selected frame columns");

        let q0rg = &project.q0rgs[0];
        let layer = &q0rg.layers[0];
        assert_eq!(removed, 2);
        assert_eq!(q0rg.frame_count, 8);
        assert_eq!(layer.explicit_keyframes, vec![2, 5]);
        assert_eq!(layer.placements[0].tween, Tween::Linear { to_frame: 3 });
        assert!(!layer
            .placements
            .iter()
            .any(|item| item.transform.tx == 40.0));
        assert!(layer
            .placements
            .iter()
            .any(|item| item.frame == 3 && item.transform.tx == 50.0));
        assert!(layer
            .placements
            .iter()
            .any(|item| item.frame == 6 && item.transform.tx == 80.0));
    }

    #[test]
    fn removing_leading_frame_columns_shifts_every_drawable_layer_and_restores_root_keys() {
        let mut project = timeline_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 6;
        q0rg.layers[0].explicit_keyframes.clear();
        q0rg.layers[0].placements = vec![placement(3, 30.0, Tween::None)];
        q0rg.layers.push(Layer {
            layer_id: 2,
            timeline_frame_count: None,
            name: "second".to_string(),
            explicit_keyframes: vec![4],
            placements: Vec::new(),
        });
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 0,
            focus_layer_id: 1,
            focus_frame: 1,
        };

        assert_eq!(
            remove_selected_frame_columns(&mut project, 1, selection),
            Some(2)
        );

        let q0rg = &project.q0rgs[0];
        assert_eq!(q0rg.frame_count, 4);
        let first = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == 1)
            .unwrap();
        let second = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == 2)
            .unwrap();
        assert!(first
            .placements
            .iter()
            .any(|item| item.frame == 1 && item.transform.tx == 30.0));
        assert!(first.has_keyframe(0));
        assert_eq!(second.explicit_keyframes, vec![0, 2]);
        assert!(second.is_blank_keyframe(0));
    }

    #[test]
    fn removing_the_complete_timeline_keeps_one_blank_root_on_every_drawable_layer() {
        let mut project = timeline_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 4;
        q0rg.layers[0].placements = vec![placement(0, 0.0, Tween::None)];
        q0rg.layers.push(Layer {
            layer_id: 2,
            timeline_frame_count: None,
            name: "second".to_string(),
            explicit_keyframes: vec![2],
            placements: vec![placement(3, 30.0, Tween::None)],
        });
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 0,
            focus_layer_id: 2,
            focus_frame: 3,
        };

        assert_eq!(
            remove_selected_frame_columns(&mut project, 1, selection),
            Some(3)
        );

        let q0rg = &project.q0rgs[0];
        assert_eq!(q0rg.frame_count, 1);
        for layer in &q0rg.layers {
            assert!(layer.placements.is_empty());
            assert_eq!(layer.explicit_keyframes, vec![0]);
            assert!(layer.is_blank_keyframe(0));
        }
    }

    #[test]
    fn materializing_a_multi_layer_range_updates_every_selected_drawable_row() {
        let mut project = timeline_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 5;
        q0rg.layers[0].placements = vec![placement(0, 10.0, Tween::None)];
        q0rg.layers.push(Layer {
            layer_id: 2,
            timeline_frame_count: None,
            name: "second".to_string(),
            explicit_keyframes: vec![0],
            placements: Vec::new(),
        });
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 2,
            focus_frame: 3,
        };

        assert_eq!(
            materialize_selected_keyframes(&mut project, 1, selection),
            Some(4)
        );

        let q0rg = &project.q0rgs[0];
        for layer in &q0rg.layers {
            assert!(layer.has_keyframe(2));
            assert!(layer.has_keyframe(3));
        }
        let first = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == 1)
            .unwrap();
        assert_eq!(crate::render::active_placements_at(first, 2).len(), 1);
        let second = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == 2)
            .unwrap();
        assert!(second.is_blank_keyframe(2));
        assert!(second.is_blank_keyframe(3));
    }

    #[test]
    fn layer_clipboard_remaps_folder_tree_and_preserves_animation() {
        let mut project = timeline_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.layers = vec![
            Layer {
                layer_id: 2,
                timeline_frame_count: None,
                name: "child".to_string(),
                explicit_keyframes: vec![3],
                placements: vec![placement(3, 77.0, Tween::None)],
            },
            Layer {
                layer_id: 3,
                timeline_frame_count: None,
                name: "folder".to_string(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "base".to_string(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
        ];
        project.layer_metadata.insert(
            LayerKey::new(1, 2),
            LayerMetadata {
                kind: LayerKind::Normal,
                parent_folder_id: Some(3),
                mask_layer_id: None,
                collapsed: false,
                hidden: false,
                locked: false,
            },
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 3),
            LayerMetadata {
                kind: LayerKind::Folder,
                parent_folder_id: None,
                mask_layer_id: None,
                collapsed: true,
                hidden: false,
                locked: false,
            },
        );

        let clipboard =
            capture_layers(&project, 1, TimelineLayerSelection::single(3)).expect("capture folder");
        let pasted = paste_layers(&mut project, 1, 1, &clipboard).expect("paste folder");
        let pasted_folder = pasted.anchor_layer_id;
        let pasted_child = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.name == "child" && layer.layer_id != 2)
            .expect("pasted child");
        assert_eq!(
            project.layer_parent_folder(1, pasted_child.layer_id),
            Some(pasted_folder)
        );
        assert_eq!(pasted_child.placements[0].transform.tx, 77.0);
        assert!(project.layer_is_folder(1, pasted_folder));
    }

    #[test]
    fn layer_clipboard_copies_and_remaps_nested_folder_subtrees() {
        let mut project = timeline_project();
        project.q0rgs[0].layers = vec![
            Layer {
                layer_id: 2,
                timeline_frame_count: None,
                name: "leaf".into(),
                explicit_keyframes: vec![3],
                placements: Vec::new(),
            },
            Layer {
                layer_id: 3,
                timeline_frame_count: None,
                name: "inner".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 4,
                timeline_frame_count: None,
                name: "outer".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "base".into(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            },
        ];
        project.layer_metadata.insert(
            LayerKey::new(1, 2),
            LayerMetadata {
                parent_folder_id: Some(3),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 3),
            LayerMetadata {
                kind: LayerKind::Folder,
                parent_folder_id: Some(4),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 4),
            LayerMetadata {
                kind: LayerKind::Folder,
                ..Default::default()
            },
        );
        q0s_format::v2::validate(&project).expect("nested clipboard fixture");

        let clipboard = capture_layers(&project, 1, TimelineLayerSelection::single(4))
            .expect("capture nested tree");
        assert_eq!(clipboard.layers.len(), 3);
        paste_layers(&mut project, 1, 1, &clipboard).expect("paste nested tree");

        let pasted_leaf = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.name == "leaf" && layer.layer_id != 2)
            .expect("pasted leaf");
        let pasted_inner = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.name == "inner" && layer.layer_id != 3)
            .expect("pasted inner");
        let pasted_outer = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.name == "outer" && layer.layer_id != 4)
            .expect("pasted outer");
        assert_eq!(
            project.layer_parent_folder(1, pasted_leaf.layer_id),
            Some(pasted_inner.layer_id)
        );
        assert_eq!(
            project.layer_parent_folder(1, pasted_inner.layer_id),
            Some(pasted_outer.layer_id)
        );
        assert_eq!(pasted_leaf.explicit_keyframes, vec![3]);
        q0s_format::v2::validate(&project).expect("pasted nested tree validates");
    }

    #[test]
    fn cutting_every_layer_leaves_a_drawable_layer() {
        let mut project = timeline_project();
        let removed = remove_layers(&mut project, 1, TimelineLayerSelection::single(1))
            .expect("remove layer");
        assert_eq!(removed, 1);
        assert_eq!(project.q0rgs[0].layers.len(), 1);
        let layer_id = project.q0rgs[0].layers[0].layer_id;
        assert!(!project.layer_is_folder(1, layer_id));
    }

    fn add_timeline_test_rig(project: &mut ProjectV2, keys: &[(u16, f32)]) {
        crate::rigging::ensure_rig(project, 1).expect("create test rig");
        let rig = q0s_format::rig::rig_for_q0rg_mut(project, 1).unwrap();
        rig.controls.push(q0s_format::v2::RigControl {
            control_id: 1,
            name: "pose".into(),
            kind: q0s_format::v2::RigControlKind::Slider,
            target_node: None,
            rest_x: 0.0,
            rest_y: 0.0,
            rest_value: 0.0,
            min_value: -10.0,
            max_value: 10.0,
            public_in_simple: true,
        });
        rig.channels.push(q0s_format::v2::RigChannel {
            property: q0s_format::v2::RigPropertyRef::ControlValue(1),
            keys: keys
                .iter()
                .map(|(frame, value)| q0s_format::v2::RigKey {
                    frame: *frame,
                    value: *value,
                    easing: q0s_format::v2::Easing::Linear,
                })
                .collect(),
        });
    }

    fn rig_key_frames(project: &ProjectV2) -> Vec<u16> {
        q0s_format::rig::rig_for_q0rg(project, 1).unwrap().channels[0]
            .keys
            .iter()
            .map(|key| key.frame)
            .collect()
    }

    #[test]
    fn artwork_frame_clipboard_keeps_rig_subtracks_separate() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 12;
        add_timeline_test_rig(&mut project, &[(2, 0.2), (4, 0.4)]);
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 1,
            focus_frame: 4,
        };
        let clipboard = capture_frames(&project, 1, selection).expect("capture full q0rg time");
        assert!(clipboard.rig_channels.is_empty());
        paste_frames(&mut project, 1, 1, 7, &clipboard).expect("paste full q0rg time");
        assert_eq!(rig_key_frames(&project), vec![2, 4]);
    }

    #[test]
    fn one_layer_copy_does_not_smuggle_global_rig_keys_when_other_layers_exist() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 8;
        project.q0rgs[0].layers.push(Layer {
            layer_id: 2,
            timeline_frame_count: None,
            name: "other".into(),
            explicit_keyframes: vec![0],
            placements: Vec::new(),
        });
        add_timeline_test_rig(&mut project, &[(3, 0.3)]);
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 1,
            focus_frame: 4,
        };
        let clipboard = capture_frames(&project, 1, selection).expect("capture one layer");
        assert!(clipboard.rig_channels.is_empty());
    }

    #[test]
    fn removing_frame_columns_removes_and_shifts_rig_keys_with_time() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 10;
        add_timeline_test_rig(&mut project, &[(1, 0.1), (3, 0.3), (7, 0.7)]);
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 1,
            focus_frame: 4,
        };
        assert_eq!(
            remove_selected_frame_columns(&mut project, 1, selection),
            Some(3)
        );
        assert_eq!(rig_key_frames(&project), vec![1, 4]);
    }

    #[test]
    fn moving_artwork_frames_does_not_move_rig_subtracks() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 12;
        add_timeline_test_rig(&mut project, &[(2, 0.2), (3, 0.3)]);
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 1,
            focus_frame: 3,
        };
        move_frames(&mut project, 1, selection, 1, 7).expect("move full q0rg time");
        assert_eq!(rig_key_frames(&project), vec![2, 3]);
    }

    #[test]
    fn deleting_bound_layer_unbinds_rig_instead_of_leaving_invalid_reference() {
        let mut project = timeline_project();
        let mut bound = placement(0, 0.0, Tween::None);
        bound.instance_id = 77;
        project.q0rgs[0].layers[0].push_placement(bound);
        crate::rigging::ensure_rig(&mut project, 1).unwrap();
        let rig = q0s_format::rig::rig_for_q0rg_mut(&mut project, 1).unwrap();
        rig.nodes.push(q0s_format::v2::RigNode {
            node_id: 1,
            name: "bound".into(),
            parent: None,
            rest: Transform2D::IDENTITY,
            length: 10.0,
            binding: Some(q0s_format::v2::RigBinding {
                instance_id: 77,
                bind_offset: q0s_format::transform::Affine::IDENTITY,
            }),
        });
        let selection = TimelineLayerSelection::single(1);
        remove_layers(&mut project, 1, selection).expect("remove bound layer");
        let rig = q0s_format::rig::rig_for_q0rg(&project, 1).unwrap();
        assert!(rig.nodes[0].binding.is_none());
    }

    #[test]
    fn frame_copy_to_another_layer_remaps_live_instance_identity() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 8;
        let mut source = placement(1, 10.0, Tween::None);
        source.instance_id = 42;
        project.q0rgs[0].layers[0].placements = vec![source];
        project.q0rgs[0].layers.push(Layer {
            layer_id: 2,
            timeline_frame_count: None,
            name: "copy target".into(),
            explicit_keyframes: vec![0],
            placements: Vec::new(),
        });
        let clipboard = capture_frames(&project, 1, TimelineSelection::single(1, 1))
            .expect("capture source frame");
        paste_frames(&mut project, 1, 2, 3, &clipboard).expect("paste cross-layer copy");
        let pasted = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == 2)
            .and_then(|layer| {
                layer
                    .placements
                    .iter()
                    .find(|placement| placement.frame == 3)
            })
            .expect("pasted placement");
        assert_ne!(pasted.instance_id, 0);
        assert_ne!(pasted.instance_id, 42);
        assert_eq!(project.q0rgs[0].layers[0].placements[0].instance_id, 42);
    }

    #[test]
    fn moving_whole_instance_to_another_layer_preserves_identity_when_source_is_gone() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 8;
        let mut source = placement(1, 10.0, Tween::None);
        source.instance_id = 52;
        project.q0rgs[0].layers[0].placements = vec![source];
        project.q0rgs[0].layers.push(Layer {
            layer_id: 2,
            timeline_frame_count: None,
            name: "move target".into(),
            explicit_keyframes: vec![0],
            placements: Vec::new(),
        });
        move_frames(&mut project, 1, TimelineSelection::single(1, 1), 2, 3)
            .expect("move instance cross-layer");
        assert!(project.q0rgs[0].layers[0]
            .placements
            .iter()
            .all(|placement| placement.instance_id != 52));
        let moved = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == 2)
            .and_then(|layer| {
                layer
                    .placements
                    .iter()
                    .find(|placement| placement.frame == 3)
            })
            .expect("moved placement");
        assert_eq!(moved.instance_id, 52);
    }

    #[test]
    fn layer_copy_remaps_instance_identity_consistently_across_keyframes() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 8;
        let mut first = placement(0, 10.0, Tween::None);
        first.instance_id = 91;
        let mut second = placement(4, 20.0, Tween::None);
        second.instance_id = 91;
        project.q0rgs[0].layers[0].placements = vec![first, second];
        let clipboard =
            capture_layers(&project, 1, TimelineLayerSelection::single(1)).expect("capture layer");
        let selection = paste_layers(&mut project, 1, 1, &clipboard).expect("paste layer copy");
        let pasted_layer = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == selection.anchor_layer_id)
            .expect("pasted layer");
        let ids = pasted_layer
            .placements
            .iter()
            .map(|placement| placement.instance_id)
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 1);
        let fresh = *ids.iter().next().unwrap();
        assert_ne!(fresh, 0);
        assert_ne!(fresh, 91);
        assert_eq!(
            project.q0rgs[0]
                .layers
                .iter()
                .find(|layer| layer.layer_id == 1)
                .unwrap()
                .placements[0]
                .instance_id,
            91
        );
    }

    #[test]
    fn frame_scripts_follow_copy_move_and_removed_frame_columns() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 10;
        project.runtime.frame_scripts = vec![
            FrameScript {
                q0rg_id: 1,
                layer_id: 1,
                frame: 2,
                source: "a = 1".into(),
            },
            FrameScript {
                q0rg_id: 1,
                layer_id: 1,
                frame: 7,
                source: "b = 2".into(),
            },
        ];

        let clipboard = capture_frames(&project, 1, TimelineSelection::single(1, 2))
            .expect("capture scripted frame");
        assert_eq!(clipboard.rows[0].frame_scripts.len(), 1);
        assert_eq!(clipboard.rows[0].frame_scripts[0].frame, 0);
        paste_frames(&mut project, 1, 1, 4, &clipboard).expect("paste scripted frame");
        assert!(project.runtime.frame_scripts.iter().any(|script| {
            script.q0rg_id == 1
                && script.layer_id == 1
                && script.frame == 4
                && script.source == "a = 1"
        }));

        move_frames(&mut project, 1, TimelineSelection::single(1, 4), 1, 5)
            .expect("move scripted frame");
        assert!(!project
            .runtime
            .frame_scripts
            .iter()
            .any(|script| script.q0rg_id == 1 && script.layer_id == 1 && script.frame == 4));
        assert!(project.runtime.frame_scripts.iter().any(|script| {
            script.q0rg_id == 1
                && script.layer_id == 1
                && script.frame == 5
                && script.source == "a = 1"
        }));

        remove_selected_frame_columns(
            &mut project,
            1,
            TimelineSelection {
                anchor_layer_id: 1,
                anchor_frame: 1,
                focus_layer_id: 1,
                focus_frame: 2,
            },
        )
        .expect("remove columns");
        assert!(!project
            .runtime
            .frame_scripts
            .iter()
            .any(|script| script.q0rg_id == 1 && script.frame == 2));
        assert!(project.runtime.frame_scripts.iter().any(|script| {
            script.q0rg_id == 1
                && script.layer_id == 1
                && script.frame == 3
                && script.source == "a = 1"
        }));
        assert!(project.runtime.frame_scripts.iter().any(|script| {
            script.q0rg_id == 1
                && script.layer_id == 1
                && script.frame == 5
                && script.source == "b = 2"
        }));
    }

    #[test]
    fn frame_labels_shift_with_deleted_columns_and_stay_scoped_to_symbols() {
        use q0s_format::v2::FrameLabelKey;
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 10;
        let mut child = project.q0rgs[0].clone();
        child.q0rg_id = 2;
        project.q0rgs.push(child);
        project.runtime.frame_labels.extend([
            (FrameLabelKey::new(1, 0), "start".into()),
            (FrameLabelKey::new(1, 2), "cut".into()),
            (FrameLabelKey::new(1, 7), "ending".into()),
            (FrameLabelKey::new(2, 7), "ending".into()),
        ]);
        remove_selected_frame_columns(
            &mut project,
            1,
            TimelineSelection {
                anchor_layer_id: 1,
                focus_layer_id: 1,
                anchor_frame: 1,
                focus_frame: 3,
            },
        )
        .unwrap();
        assert_eq!(project.runtime.resolve_frame_label(1, "start"), Some(0));
        assert_eq!(project.runtime.resolve_frame_label(1, "cut"), None);
        assert_eq!(project.runtime.resolve_frame_label(1, "ending"), Some(4));
        assert_eq!(project.runtime.resolve_frame_label(2, "ending"), Some(7));
        q0s_format::v2::validate(&project).unwrap();
    }

    #[test]
    fn layer_copy_preserves_frame_scripts_and_remaps_layer_identity() {
        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 6;
        project.runtime.frame_scripts.push(FrameScript {
            q0rg_id: 1,
            layer_id: 1,
            frame: 3,
            source: "pr \"copied\"".into(),
        });

        let clipboard = capture_layers(&project, 1, TimelineLayerSelection::single(1))
            .expect("capture scripted layer");
        assert_eq!(clipboard.frame_scripts.len(), 1);
        let pasted = paste_layers(&mut project, 1, 1, &clipboard).expect("paste scripted layer");
        assert_ne!(pasted.anchor_layer_id, 1);
        assert!(project.runtime.frame_scripts.iter().any(|script| {
            script.q0rg_id == 1
                && script.layer_id == pasted.anchor_layer_id
                && script.frame == 3
                && script.source == "pr \"copied\""
        }));

        remove_layers(
            &mut project,
            1,
            TimelineLayerSelection::single(pasted.anchor_layer_id),
        )
        .expect("remove copied layer");
        assert!(!project
            .runtime
            .frame_scripts
            .iter()
            .any(|script| script.layer_id == pasted.anchor_layer_id));
        assert!(project
            .runtime
            .frame_scripts
            .iter()
            .any(|script| script.layer_id == 1 && script.frame == 3));
    }

    #[test]
    fn removing_frame_columns_shifts_camera_keys_and_tween_target_with_entry_time() {
        use q0s_format::v2::{CameraKey, CameraState};

        let mut project = timeline_project();
        project.q0rgs[0].frame_count = 10;
        project.runtime.camera.enabled = true;
        let identity = project.default_camera_state();
        project.runtime.camera.keys = vec![
            CameraKey {
                frame: 0,
                state: identity,
                tween: Tween::Linear { to_frame: 6 },
            },
            CameraKey {
                frame: 2,
                state: CameraState {
                    center_x: identity.center_x + 10.0,
                    ..identity
                },
                tween: Tween::None,
            },
            CameraKey {
                frame: 6,
                state: CameraState {
                    center_y: identity.center_y + 20.0,
                    ..identity
                },
                tween: Tween::None,
            },
        ];

        remove_selected_frame_columns(
            &mut project,
            1,
            TimelineSelection {
                anchor_layer_id: 1,
                focus_layer_id: 1,
                anchor_frame: 1,
                focus_frame: 3,
            },
        )
        .expect("remove frame columns");

        assert_eq!(project.q0rgs[0].frame_count, 7);
        assert_eq!(
            project
                .runtime
                .camera
                .keys
                .iter()
                .map(|key| key.frame)
                .collect::<Vec<_>>(),
            vec![0, 3]
        );
        assert_eq!(project.runtime.camera.keys[0].tween.to_frame(), Some(3));
        q0s_format::v2::validate(&project).unwrap();
    }
}
