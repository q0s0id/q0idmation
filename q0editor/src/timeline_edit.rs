use std::collections::{HashMap, HashSet};

use q0s_format::v2::{Layer, LayerKey, LayerMetadata, ProjectV2, Tween};

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

    for layer_id in layer_ids {
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
                    placement.tween = match placement.tween {
                        Tween::Linear { to_frame }
                            if (first_frame..=last_frame).contains(&to_frame) =>
                        {
                            Tween::Linear {
                                to_frame: to_frame - first_frame,
                            }
                        }
                        _ => Tween::None,
                    };
                    placement
                }),
        );
        explicit_keyframes.sort_unstable();
        explicit_keyframes.dedup();
        rows.push(TimelineFrameClipboardRow {
            explicit_keyframes,
            placements,
        });
    }

    Some(TimelineFrameClipboard {
        width: last_frame - first_frame + 1,
        rows,
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
        if matches!(
            placement.tween,
            Tween::Linear { to_frame } if (first_frame..=last_frame).contains(&to_frame)
        ) {
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
    let q0rg = &mut project.q0rgs[q0rg_index];
    q0rg.frame_count = q0rg.frame_count.max(last_frame.saturating_add(1));

    for (layer_id, row) in target_layer_ids.iter().copied().zip(&clipboard.rows) {
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
            .extend(row.placements.iter().cloned().map(|mut placement| {
                placement.frame = target_frame.saturating_add(placement.frame);
                placement.tween = match placement.tween {
                    Tween::Linear { to_frame } => Tween::Linear {
                        to_frame: target_frame.saturating_add(to_frame),
                    },
                    Tween::None => Tween::None,
                };
                placement
            }));
        if !layer.has_keyframe(target_frame) {
            layer.ensure_explicit_keyframe(target_frame);
        }
    }

    Some(TimelineSelection {
        anchor_layer_id: target_layer_ids[0],
        anchor_frame: target_frame,
        focus_layer_id: *target_layer_ids.last()?,
        focus_frame: last_frame,
    })
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

    // A folder row owns its children even while collapsed. Copying/cutting the
    // folder therefore always keeps its real block intact.
    let folders = selected.iter().copied().collect::<Vec<_>>();
    for folder_id in folders {
        if project.layer_is_folder(q0rg_id, folder_id) {
            for layer in &q0rg.layers {
                if project.layer_parent_folder(q0rg_id, layer.layer_id) == Some(folder_id) {
                    selected.insert(layer.layer_id);
                }
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
    Some(TimelineLayerClipboard { layers, metadata })
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
    let q0rg_index = q0rg_index(project, q0rg_id)?;
    project.q0rgs[q0rg_index]
        .layers
        .retain(|layer| !removed.contains(&layer.layer_id));
    project
        .layer_metadata
        .retain(|key, _| key.q0rg_id != q0rg_id || !removed.contains(&key.layer_id));
    for metadata in project.layer_metadata.values_mut() {
        if metadata
            .parent_folder_id
            .is_some_and(|parent| removed.contains(&parent))
        {
            metadata.parent_folder_id = None;
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
    let block_id = project
        .layer_parent_folder(q0rg_id, target_layer_id)
        .unwrap_or(target_layer_id);
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
    for layer in &mut pasted_layers {
        layer.layer_id = *id_map.get(&layer.layer_id)?;
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
                    let tween_end = match placement.tween {
                        Tween::Linear { to_frame } => Some(to_frame),
                        Tween::None => None,
                    };
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
            frame,
            target: Target::Q0rg(1),
            transform: Transform2D {
                tx: x,
                ..Transform2D::IDENTITY
            },
            tween,
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
            },
        );
        project.layer_metadata.insert(
            LayerKey::new(1, 3),
            LayerMetadata {
                kind: LayerKind::Folder,
                parent_folder_id: None,
                collapsed: true,
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
    fn cutting_every_layer_leaves_a_drawable_layer() {
        let mut project = crate::state::default_project();
        let removed = remove_layers(&mut project, 1, TimelineLayerSelection::single(1))
            .expect("remove layer");
        assert_eq!(removed, 1);
        assert_eq!(project.q0rgs[0].layers.len(), 1);
        let layer_id = project.q0rgs[0].layers[0].layer_id;
        assert!(!project.layer_is_folder(1, layer_id));
    }
}
