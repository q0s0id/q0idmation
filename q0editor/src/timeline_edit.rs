use std::collections::{HashMap, HashSet};

use q0s_format::v2::{FrameScript, Layer, LayerKey, LayerMetadata, ProjectV2, RigChannel, Tween};

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

fn selection_spans_all_drawable_layers(
    project: &ProjectV2,
    q0rg_id: u16,
    selected_layer_ids: &[u16],
) -> bool {
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return false;
    };
    let drawable = q0rg
        .layers
        .iter()
        .filter(|layer| !project.layer_is_folder(q0rg_id, layer.layer_id))
        .map(|layer| layer.layer_id)
        .collect::<HashSet<_>>();
    !drawable.is_empty()
        && selected_layer_ids.len() == drawable.len()
        && selected_layer_ids.iter().all(|id| drawable.contains(id))
}

fn capture_rig_channels(
    project: &ProjectV2,
    q0rg_id: u16,
    first_frame: u16,
    last_frame: u16,
) -> Vec<RigChannel> {
    q0s_format::rig::rig_for_q0rg(project, q0rg_id)
        .map(|rig| {
            rig.channels
                .iter()
                .filter_map(|channel| {
                    let keys = channel
                        .keys
                        .iter()
                        .copied()
                        .filter(|key| (first_frame..=last_frame).contains(&key.frame))
                        .map(|mut key| {
                            key.frame -= first_frame;
                            key
                        })
                        .collect::<Vec<_>>();
                    (!keys.is_empty()).then_some(RigChannel {
                        property: channel.property,
                        keys,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
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
            explicit_keyframes,
            placements,
            frame_scripts,
        });
    }

    let rig_channels = if selection_spans_all_drawable_layers(project, q0rg_id, &layer_ids) {
        capture_rig_channels(project, q0rg_id, first_frame, last_frame)
    } else {
        Vec::new()
    };

    Some(TimelineFrameClipboard {
        width: last_frame - first_frame + 1,
        rows,
        rig_channels,
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
    let clear_rig = selection_spans_all_drawable_layers(project, q0rg_id, &layer_ids);
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
    if clear_rig {
        clear_rig_frame_range(project, q0rg_id, first_frame, last_frame);
    }
    Some(removed)
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
    let (layer_ids, first_frame, last_frame) = selected_frame_bounds(project, q0rg_id, selection)?;
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let mut insertions = Vec::<(u16, u16, Vec<q0s_format::v2::Placement>)>::new();
    let mut tween_retargets = HashMap::<(u16, usize), u16>::new();

    for layer_id in layer_ids {
        let layer = q0rg
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?;
        for frame in first_frame..=last_frame {
            if layer.has_keyframe(frame) {
                continue;
            }
            let mut placements = Vec::new();
            for (placement_idx, transform) in crate::render::active_placements_at(layer, frame) {
                let source = layer.placements.get(placement_idx)?;
                if source
                    .tween
                    .to_frame()
                    .is_some_and(|to_frame| source.frame < frame && frame <= to_frame)
                {
                    tween_retargets
                        .entry((layer_id, placement_idx))
                        .and_modify(|target| *target = (*target).min(frame))
                        .or_insert(frame);
                }
                let mut placement = source.clone();
                placement.frame = frame;
                placement.transform = transform;
                placement.tween = Tween::None;
                placements.push(placement);
            }
            insertions.push((layer_id, frame, placements));
        }
    }

    let q0rg_index = q0rg_index(project, q0rg_id)?;
    let q0rg = &mut project.q0rgs[q0rg_index];
    for ((layer_id, placement_idx), target_frame) in tween_retargets {
        let layer = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)?;
        let placement = layer.placements.get_mut(placement_idx)?;
        placement.tween = placement.tween.with_to_frame(target_frame);
    }
    let created = insertions.len();
    for (layer_id, frame, placements) in insertions {
        let layer = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)?;
        if placements.is_empty() {
            layer.ensure_explicit_keyframe(frame);
        } else {
            layer.placements.extend(placements);
        }
    }
    Some(created)
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

    let q0rg = &mut project.q0rgs[q0rg_index];
    if first_frame == 0 && last_frame == old_count - 1 {
        q0rg.frame_count = 1;
        for layer in &mut q0rg.layers {
            layer.explicit_keyframes.clear();
            layer.placements.clear();
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
    let clear_rig = selection_spans_all_drawable_layers(project, q0rg_id, &layer_ids);
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
    if clear_rig {
        clear_rig_frame_range(project, q0rg_id, first_frame, last_frame);
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
    project.runtime.frame_scripts.retain(|script| {
        script.q0rg_id != q0rg_id
            || !target_layer_set.contains(&script.layer_id)
            || !(target_frame..=last_frame).contains(&script.frame)
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
            clear_frame_range(layer, target_frame, last_frame, false);
            for relative_frame in &row.explicit_keyframes {
                layer.ensure_explicit_keyframe(target_frame.saturating_add(*relative_frame));
            }
            layer
                .placements
                .extend(prepared.into_iter().map(|mut placement| {
                    placement.frame = target_frame.saturating_add(placement.frame);
                    if let Some(to_frame) = placement.tween.to_frame() {
                        placement.tween = placement
                            .tween
                            .with_to_frame(target_frame.saturating_add(to_frame));
                    }
                    placement
                }));
            if !layer.has_keyframe(target_frame) {
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

pub fn move_frames(
    project: &mut ProjectV2,
    q0rg_id: u16,
    selection: TimelineSelection,
    target_layer_id: u16,
    target_frame: u16,
) -> Option<TimelineSelection> {
    let clipboard = capture_frames(project, q0rg_id, selection)?;
    remove_frames_for_move(project, q0rg_id, selection)?;
    paste_frames(project, q0rg_id, target_layer_id, target_frame, &clipboard)
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
            name: format!("Layer {next_id}"),
            explicit_keyframes: vec![0],
            placements: Vec::new(),
        });
    }
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

    fn placement(frame: u16, x: f32, tween: Tween) -> Placement {
        Placement {
            instance_id: 0,
            frame,
            target: Target::Q0rg(1),
            transform: Transform2D {
                tx: x,
                ..Transform2D::IDENTITY
            },
            tween,
            fx: Default::default(),
        }
    }

    #[test]
    fn frame_clipboard_preserves_content_blank_keys_and_internal_tweens() {
        let mut project = crate::state::default_project();
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
        let mut project = crate::state::default_project();
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
        let mut project = crate::state::default_project();
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
        let mut project = crate::state::default_project();
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
        let mut project = crate::state::default_project();
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
        let mut project = crate::state::default_project();
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
        let mut project = crate::state::default_project();
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

        let mut project = crate::state::default_project();
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
    fn removing_selected_frame_columns_shifts_keys_blanks_and_tween_targets() {
        let mut project = crate::state::default_project();
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
        let mut project = crate::state::default_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 6;
        q0rg.layers[0].explicit_keyframes.clear();
        q0rg.layers[0].placements = vec![placement(3, 30.0, Tween::None)];
        q0rg.layers.push(Layer {
            layer_id: 2,
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
        let mut project = crate::state::default_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 4;
        q0rg.layers[0].placements = vec![placement(0, 0.0, Tween::None)];
        q0rg.layers.push(Layer {
            layer_id: 2,
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
        let mut project = crate::state::default_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.frame_count = 5;
        q0rg.layers[0].placements = vec![placement(0, 10.0, Tween::None)];
        q0rg.layers.push(Layer {
            layer_id: 2,
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
        let mut project = crate::state::default_project();
        let q0rg = &mut project.q0rgs[0];
        q0rg.layers = vec![
            Layer {
                layer_id: 2,
                name: "child".to_string(),
                explicit_keyframes: vec![3],
                placements: vec![placement(3, 77.0, Tween::None)],
            },
            Layer {
                layer_id: 3,
                name: "folder".to_string(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 1,
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
        let mut project = crate::state::default_project();
        project.q0rgs[0].layers = vec![
            Layer {
                layer_id: 2,
                name: "leaf".into(),
                explicit_keyframes: vec![3],
                placements: Vec::new(),
            },
            Layer {
                layer_id: 3,
                name: "inner".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 4,
                name: "outer".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 1,
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
        let mut project = crate::state::default_project();
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
    fn whole_q0rg_frame_clipboard_copies_and_offsets_rig_keys() {
        let mut project = crate::state::default_project();
        project.q0rgs[0].frame_count = 12;
        add_timeline_test_rig(&mut project, &[(2, 0.2), (4, 0.4)]);
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 1,
            focus_frame: 4,
        };
        let clipboard = capture_frames(&project, 1, selection).expect("capture full q0rg time");
        assert_eq!(clipboard.rig_channels.len(), 1);
        assert_eq!(
            clipboard.rig_channels[0]
                .keys
                .iter()
                .map(|key| key.frame)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
        paste_frames(&mut project, 1, 1, 7, &clipboard).expect("paste full q0rg time");
        assert_eq!(rig_key_frames(&project), vec![2, 4, 7, 9]);
    }

    #[test]
    fn one_layer_copy_does_not_smuggle_global_rig_keys_when_other_layers_exist() {
        let mut project = crate::state::default_project();
        project.q0rgs[0].frame_count = 8;
        project.q0rgs[0].layers.push(Layer {
            layer_id: 2,
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
        let mut project = crate::state::default_project();
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
    fn moving_full_q0rg_frame_range_moves_rig_keys_not_duplicates_them() {
        let mut project = crate::state::default_project();
        project.q0rgs[0].frame_count = 12;
        add_timeline_test_rig(&mut project, &[(2, 0.2), (3, 0.3)]);
        let selection = TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 2,
            focus_layer_id: 1,
            focus_frame: 3,
        };
        move_frames(&mut project, 1, selection, 1, 7).expect("move full q0rg time");
        assert_eq!(rig_key_frames(&project), vec![7, 8]);
    }

    #[test]
    fn deleting_bound_layer_unbinds_rig_instead_of_leaving_invalid_reference() {
        let mut project = crate::state::default_project();
        let mut bound = placement(0, 0.0, Tween::None);
        bound.instance_id = 77;
        project.q0rgs[0].layers[0].placements.push(bound);
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
        let mut project = crate::state::default_project();
        project.q0rgs[0].frame_count = 8;
        let mut source = placement(1, 10.0, Tween::None);
        source.instance_id = 42;
        project.q0rgs[0].layers[0].placements = vec![source];
        project.q0rgs[0].layers.push(Layer {
            layer_id: 2,
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
        let mut project = crate::state::default_project();
        project.q0rgs[0].frame_count = 8;
        let mut source = placement(1, 10.0, Tween::None);
        source.instance_id = 52;
        project.q0rgs[0].layers[0].placements = vec![source];
        project.q0rgs[0].layers.push(Layer {
            layer_id: 2,
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
        let mut project = crate::state::default_project();
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
        let mut project = crate::state::default_project();
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
    fn layer_copy_preserves_frame_scripts_and_remaps_layer_identity() {
        let mut project = crate::state::default_project();
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
}
