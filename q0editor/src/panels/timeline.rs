use std::collections::BTreeSet;

use egui::{
    pos2, vec2, Align2, Color32, FontId, PointerButton, Rect, ScrollArea, Sense, Stroke, Ui,
};

use crate::app::{Action, EditorApp, LayerDropTarget};
use crate::hotkeys::ShortcutCommand;
use crate::settings::Theme;
use crate::state::{
    CameraFrameDrag, CameraTimelineSelection, FrameLabelEdit, LayerRename, LibraryDragPayload,
    Selection, TimelineFrameDrag, TimelineLayerDrag, TimelineLayerSelection,
    TimelineRigKeySelection, TimelineSelection,
};
use crate::ui::{icon_button, icon_button_enabled, icon_toggle, Icon};

const FRAME_W: f32 = 12.0;
const ROW_H: f32 = 26.0;
const LAYER_LABEL_W: f32 = 174.0;
/// Gap above and below the cell fill РІР‚вЂќ leaves room for thin row separators
/// like the original Flash CS3 timeline.
const CELL_PAD_Y: f32 = 1.0;
/// How many "virtual" frame slots to render past `frame_count` so the user
/// sees there's room to extend the q0rg. They render as darker chequered
/// cells and can hold a virtual playhead until F5/F6/F7 materialises them.
const VIRTUAL_FRAMES_BUFFER: u16 = 30;

/// Visible slots with one neighbour on each side for glyphs and stroke edges.
/// Keep document coordinates unchanged: scrolling must never renumber frames.
fn visible_slots(
    origin: f32,
    size: f32,
    count: usize,
    min: f32,
    max: f32,
) -> std::ops::Range<usize> {
    if max <= min || max < origin || min > origin + size * count as f32 {
        return 0..0;
    }
    let start = (((min - origin) / size).floor().max(0.0) as usize).saturating_sub(1);
    let end = (((max - origin) / size).ceil().max(0.0) as usize).saturating_add(1);
    start.min(count)..end.min(count)
}

fn visible_frame_slots(painter: &egui::Painter, base_x: f32, count: u16) -> std::ops::Range<u16> {
    let clip = painter.clip_rect();
    let range = visible_slots(
        base_x,
        FRAME_W,
        usize::from(count),
        clip.left(),
        clip.right(),
    );
    range.start as u16..range.end as u16
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerMoveIcon {
    Up,
    Down,
    Indent,
    Outdent,
}

#[cfg(test)]
fn layer_move_icon_segments(icon: LayerMoveIcon, rect: Rect) -> Vec<[egui::Pos2; 2]> {
    let center = rect.center();
    let left = center.x - 4.0;
    let right = center.x + 4.0;
    let top = center.y - 4.0;
    let bottom = center.y + 4.0;
    match icon {
        LayerMoveIcon::Up => vec![
            [pos2(center.x, bottom), pos2(center.x, top)],
            [pos2(center.x, top), pos2(center.x - 3.0, top + 3.0)],
            [pos2(center.x, top), pos2(center.x + 3.0, top + 3.0)],
        ],
        LayerMoveIcon::Down => vec![
            [pos2(center.x, top), pos2(center.x, bottom)],
            [pos2(center.x, bottom), pos2(center.x - 3.0, bottom - 3.0)],
            [pos2(center.x, bottom), pos2(center.x + 3.0, bottom - 3.0)],
        ],
        LayerMoveIcon::Indent => vec![
            [pos2(left, center.y), pos2(right, center.y)],
            [pos2(right, center.y), pos2(right - 3.0, center.y - 3.0)],
            [pos2(right, center.y), pos2(right - 3.0, center.y + 3.0)],
            [pos2(left - 2.0, top), pos2(left - 2.0, bottom)],
        ],
        LayerMoveIcon::Outdent => vec![
            [pos2(right, center.y), pos2(left, center.y)],
            [pos2(left, center.y), pos2(left + 3.0, center.y - 3.0)],
            [pos2(left, center.y), pos2(left + 3.0, center.y + 3.0)],
            [pos2(right + 2.0, top), pos2(right + 2.0, bottom)],
        ],
    }
}

fn layer_move_icon_button(
    ui: &mut Ui,
    icon: LayerMoveIcon,
    hover_text: &'static str,
) -> egui::Response {
    icon_button(
        ui,
        match icon {
            LayerMoveIcon::Up => Icon::Up,
            LayerMoveIcon::Down => Icon::Down,
            LayerMoveIcon::Indent => Icon::Indent,
            LayerMoveIcon::Outdent => Icon::Outdent,
        },
        hover_text,
    )
}

pub(crate) fn visible_layer_indices(
    project: &q0s_format::v2::ProjectV2,
    q0rg_idx: usize,
) -> Vec<usize> {
    let Some(q0rg) = project.q0rgs.get(q0rg_idx) else {
        return Vec::new();
    };
    q0rg.layers
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, layer)| {
            // A group's collapsed state hides its descendants, not its own row.
            // Start from this layer's containing groups so a collapsed mask
            // remains visible while its masked layers are hidden.
            let mut pending = Vec::new();
            pending.extend(project.layer_parent_folder(q0rg.q0rg_id, layer.layer_id));
            pending.extend(project.layer_mask_sources(q0rg.q0rg_id, layer.layer_id));
            let mut visited = std::collections::HashSet::new();
            while let Some(group_id) = pending.pop() {
                if !visited.insert(group_id) {
                    continue;
                }
                if project.layer_metadata(q0rg.q0rg_id, group_id).collapsed {
                    return false;
                }
                pending.extend(project.layer_parent_folder(q0rg.q0rg_id, group_id));
                pending.extend(project.layer_mask_sources(q0rg.q0rg_id, group_id));
            }
            true
        })
        .map(|(index, _)| index)
        .collect()
}

pub(crate) fn visible_layer_ids(project: &q0s_format::v2::ProjectV2, q0rg_id: u16) -> Vec<u16> {
    let Some(q0rg_idx) = project
        .q0rgs
        .iter()
        .position(|q0rg| q0rg.q0rg_id == q0rg_id)
    else {
        return Vec::new();
    };
    visible_layer_indices(project, q0rg_idx)
        .into_iter()
        .map(|index| project.q0rgs[q0rg_idx].layers[index].layer_id)
        .collect()
}

fn layer_selection_contains(
    selection: Option<TimelineLayerSelection>,
    visible_layer_ids: &[u16],
    layer_id: u16,
) -> bool {
    let Some(selection) = selection else {
        return false;
    };
    let Some(anchor) = visible_layer_ids
        .iter()
        .position(|id| *id == selection.anchor_layer_id)
    else {
        return false;
    };
    let Some(focus) = visible_layer_ids
        .iter()
        .position(|id| *id == selection.focus_layer_id)
    else {
        return false;
    };
    let Some(index) = visible_layer_ids.iter().position(|id| *id == layer_id) else {
        return false;
    };
    (anchor.min(focus)..=anchor.max(focus)).contains(&index)
}

fn frame_selection_contains(
    selection: TimelineSelection,
    visible_layer_ids: &[u16],
    layer_id: u16,
    frame: u16,
) -> bool {
    let Some(anchor) = visible_layer_ids
        .iter()
        .position(|id| *id == selection.anchor_layer_id)
    else {
        return false;
    };
    let Some(focus) = visible_layer_ids
        .iter()
        .position(|id| *id == selection.focus_layer_id)
    else {
        return false;
    };
    let Some(index) = visible_layer_ids.iter().position(|id| *id == layer_id) else {
        return false;
    };
    (anchor.min(focus)..=anchor.max(focus)).contains(&index)
        && (selection.anchor_frame.min(selection.focus_frame)
            ..=selection.anchor_frame.max(selection.focus_frame))
            .contains(&frame)
}
#[cfg(test)]
fn rig_key_frames(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    selected_control: Option<u16>,
) -> BTreeSet<u16> {
    let Some(rig) = q0s_format::rig::rig_for_q0rg(project, q0rg_id) else {
        return BTreeSet::new();
    };
    rig.channels
        .iter()
        .filter(|channel| {
            selected_control.is_none_or(|control_id| match channel.property {
                q0s_format::v2::RigPropertyRef::ControlX(id)
                | q0s_format::v2::RigPropertyRef::ControlY(id)
                | q0s_format::v2::RigPropertyRef::ControlValue(id) => id == control_id,
                q0s_format::v2::RigPropertyRef::NodeTx(_)
                | q0s_format::v2::RigPropertyRef::NodeTy(_)
                | q0s_format::v2::RigPropertyRef::NodeRotation(_)
                | q0s_format::v2::RigPropertyRef::NodeScaleX(_)
                | q0s_format::v2::RigPropertyRef::NodeScaleY(_)
                | q0s_format::v2::RigPropertyRef::ConstraintWeight(_) => false,
            })
        })
        .flat_map(|channel| channel.keys.iter().map(|key| key.frame))
        .collect()
}

#[derive(Debug, Clone)]
struct RigTimelineTrack {
    layer_id: u16,
    layer_name: String,
    frames: BTreeSet<u16>,
}

fn rig_timeline_tracks(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    visible_layer_indices: &[usize],
) -> Vec<RigTimelineTrack> {
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    visible_layer_indices
        .iter()
        .filter_map(|index| q0rg.layers.get(*index))
        .filter(|layer| {
            !project.layer_is_folder(q0rg_id, layer.layer_id)
                && crate::timeline_edit::rig_layer_has_track(project, q0rg_id, layer.layer_id)
        })
        .map(|layer| RigTimelineTrack {
            layer_id: layer.layer_id,
            layer_name: layer.name.clone(),
            frames: crate::timeline_edit::rig_key_frames_for_layer(
                project,
                q0rg_id,
                layer.layer_id,
            ),
        })
        .collect()
}

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    let theme = app.settings.theme.clone();
    let label_width = f32::from(app.settings.timeline_layer_width).clamp(
        112.0,
        (ui.available_width() * 0.65).clamp(LAYER_LABEL_W, 520.0),
    );
    transport_bar(app, &theme, ui);
    ui.separator();

    let Some(q0rg_idx) = app
        .state
        .project
        .q0rgs
        .iter()
        .position(|q| q.q0rg_id == app.session.current_q0rg_id)
    else {
        ui.label("(no current q0rg)");
        return;
    };

    let q0rg_id = app.state.project.q0rgs[q0rg_idx].q0rg_id;
    let frame_count = app.state.project.q0rgs[q0rg_idx].frame_count;
    // Model order is back-to-front for rendering. The timeline displays the
    // conventional front-to-back order, so rows are collected in reverse.
    // Folder children live immediately before their folder in model order;
    // reverse traversal therefore shows the folder row before its children.
    let visible_layer_indices = visible_layer_indices(&app.state.project, q0rg_idx);
    let layer_count = visible_layer_indices.len();
    let rig_tracks = rig_timeline_tracks(&app.state.project, q0rg_id, &visible_layer_indices);
    let rig_track_count = rig_tracks.len();
    let timeline_row_count = layer_count + rig_track_count;
    let layer_ids: Vec<u16> = visible_layer_indices
        .iter()
        .map(|index| app.state.project.q0rgs[q0rg_idx].layers[*index].layer_id)
        .collect();
    let folder_layer_ids: Vec<u16> = layer_ids
        .iter()
        .copied()
        .filter(|layer_id| app.state.project.layer_is_folder(q0rg_id, *layer_id))
        .collect();
    // Virtual frame slots past `frame_count` are rendered too so the user
    // sees the q0rg's edge and has somewhere to drop "+ Frame" hits visually.
    let visible_frames = frame_count.saturating_add(VIRTUAL_FRAMES_BUFFER);

    let camera_row_enabled =
        q0rg_id == app.state.project.meta.entry_q0rg_id && app.state.project.runtime.camera.enabled;
    let camera_row_count = usize::from(camera_row_enabled);

    ScrollArea::both()
        .auto_shrink([false, false])
        .max_height(ui.available_height())
        .show(ui, |ui| {
            let total_w = label_width + (visible_frames as f32) * FRAME_W;
            let total_h = ROW_H * (timeline_row_count as f32 + camera_row_count as f32 + 1.0);
            let (rect, response) = ui.allocate_exact_size(
                vec2(total_w.max(ui.available_width()), total_h),
                Sense::click_and_drag(),
            );
            let painter = ui.painter_at(rect);
            let clip = painter.clip_rect();
            let splitter_rect = Rect::from_min_max(
                pos2(rect.min.x + label_width - 3.0, rect.top()),
                pos2(rect.min.x + label_width + 3.0, rect.bottom()),
            )
            .intersect(clip);
            let splitter = ui
                .interact(
                    splitter_rect,
                    ui.id().with("timeline_layer_splitter"),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                .on_hover_text("Drag to resize layer names; double-click to reset");
            if splitter.dragged() {
                let delta = ui.input(|input| input.pointer.delta().x);
                app.settings.timeline_layer_width =
                    (label_width + delta).clamp(112.0, 520.0).round() as u16;
            }
            if splitter.double_clicked() {
                app.settings.timeline_layer_width = LAYER_LABEL_W as u16;
            }
            if splitter.drag_stopped() || splitter.double_clicked() {
                app.settings.save();
            }
            let frame_slots =
                visible_frame_slots(&painter, rect.min.x + label_width, visible_frames);
            let layer_rows_y = rect.min.y + ROW_H * (camera_row_count as f32 + 1.0);
            let layer_row_slots =
                visible_slots(layer_rows_y, ROW_H, layer_count, clip.top(), clip.bottom());
            let rig_rows_y = layer_rows_y + ROW_H * layer_count as f32;
            let rig_row_slots = visible_slots(
                rig_rows_y,
                ROW_H,
                rig_track_count,
                clip.top(),
                clip.bottom(),
            );

            if let Some((target_q0rg_id, target_layer_id)) = app.session.timeline_scroll_to_layer {
                if target_q0rg_id != q0rg_id {
                    app.session.timeline_scroll_to_layer = None;
                } else if let Some(row_index) = visible_layer_indices.iter().position(|index| {
                    app.state.project.q0rgs[q0rg_idx].layers[*index].layer_id == target_layer_id
                }) {
                    let row_y =
                        rect.min.y + ROW_H * (row_index as f32 + camera_row_count as f32 + 1.0);
                    let target_rect =
                        Rect::from_min_size(pos2(rect.min.x, row_y), vec2(rect.width(), ROW_H));
                    ui.scroll_to_rect(target_rect, Some(egui::Align::Center));
                    app.session.timeline_scroll_to_layer = None;
                }
            }

            // Timeline base panel РІР‚вЂќ same as the editor's window colour
            // so it visually merges with the surrounding chrome.
            painter.rect_filled(rect, 0.0, theme.window.to_color32());

            // Header (frame numbers + second markers)
            let header_rect = Rect::from_min_size(rect.min, vec2(rect.width(), ROW_H));
            painter.rect_filled(header_rect, 0.0, theme.timeline_header.to_color32());
            let drawable_layer_ids: Vec<_> = app.state.project.q0rgs[q0rg_idx]
                .layers
                .iter()
                .filter(|layer| !app.state.project.layer_is_folder(q0rg_id, layer.layer_id))
                .map(|layer| layer.layer_id)
                .collect();
            let all_layers_locked = !drawable_layer_ids.is_empty()
                && drawable_layer_ids
                    .iter()
                    .all(|layer_id| app.state.project.layer_is_locked(q0rg_id, *layer_id));
            let lock_all_rect = Rect::from_center_size(
                pos2(rect.min.x + label_width - 13.0, rect.min.y + ROW_H * 0.5),
                vec2(18.0, 18.0),
            );
            draw_lock_icon(
                &painter,
                lock_all_rect.shrink(3.0),
                all_layers_locked,
                theme.text.to_color32(),
            );
            if ui
                .interact(
                    lock_all_rect,
                    egui::Id::new(("timeline_lock_all", q0rg_id)),
                    Sense::click(),
                )
                .on_hover_text(if all_layers_locked {
                    "Unlock all layers"
                } else {
                    "Lock all layers"
                })
                .clicked()
            {
                app.queue(Action::ToggleAllLayerLocks(q0rg_id));
            }
            // Bottom edge of header
            painter.line_segment(
                [
                    pos2(rect.min.x, rect.min.y + ROW_H),
                    pos2(rect.min.x + rect.width(), rect.min.y + ROW_H),
                ],
                Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
            );
            let fps = app.state.project.meta.fps as u32;
            for f in frame_slots.clone() {
                let x = rect.min.x + label_width + (f as f32) * FRAME_W;
                let frame_one_based = f + 1;
                let beyond = f >= frame_count;
                // Tick lines РІР‚вЂќ every 5th frame full-height, others only in
                // the layer area (below the header). Virtual cells past
                // `frame_count` get fainter ticks.
                if f % 5 == 0 {
                    let col = if beyond {
                        theme.empty_beyond_5.to_color32()
                    } else {
                        theme.timeline_grid_5.to_color32()
                    };
                    painter.line_segment(
                        [pos2(x, rect.min.y), pos2(x, rect.min.y + total_h)],
                        Stroke::new(1.0_f32, col),
                    );
                } else {
                    painter.line_segment(
                        [pos2(x, rect.min.y + ROW_H), pos2(x, rect.min.y + total_h)],
                        Stroke::new(1.0_f32, theme.timeline_grid.to_color32()),
                    );
                }
                // Frame number every 5th frame РІР‚вЂќ but only inside the q0rg's
                // own range. Virtual cells past frame_count get no number.
                if !beyond && (frame_one_based % 5 == 0 || f == 0) {
                    painter.text(
                        pos2(x + FRAME_W * 0.5, rect.min.y + ROW_H * 0.55),
                        Align2::CENTER_CENTER,
                        format!("{frame_one_based}"),
                        FontId::proportional(10.0),
                        theme.text_dim.to_color32(),
                    );
                }
                // Second marker (1s, 2s, РІР‚В¦) at every fps-th frame above the number.
                if !beyond && fps > 0 && (frame_one_based as u32).is_multiple_of(fps) {
                    let secs = (frame_one_based as u32) / fps;
                    painter.text(
                        pos2(x + FRAME_W * 0.5, rect.min.y + 4.0),
                        Align2::CENTER_TOP,
                        format!("{secs}s"),
                        FontId::proportional(9.0),
                        theme.text.to_color32(),
                    );
                }
            }
            for (key, label) in &app.state.project.runtime.frame_labels {
                if key.q0rg_id != q0rg_id || !frame_slots.contains(&key.frame) {
                    continue;
                }
                let x = rect.min.x + label_width + key.frame as f32 * FRAME_W + 1.0;
                let y = rect.min.y + 2.0;
                let color = theme.accent.to_color32();
                painter.line_segment([pos2(x, y), pos2(x, y + 9.0)], Stroke::new(1.3_f32, color));
                painter.add(egui::Shape::convex_polygon(
                    vec![pos2(x, y), pos2(x + 7.0, y + 2.5), pos2(x, y + 5.0)],
                    color,
                    Stroke::NONE,
                ));
                ui.interact(
                    Rect::from_min_size(pos2(x, rect.min.y), vec2(FRAME_W, ROW_H)),
                    egui::Id::new(("frame_label_hover", q0rg_id, key.frame)),
                    Sense::hover(),
                )
                .on_hover_text(format!("{label}\nframe {}", u32::from(key.frame) + 1));
            }
            // End-of-q0rg marker: dashed amber bar at the right edge of the
            // last real frame so the user can see exactly where `frame_count`
            // ends without confusing it with the solid theme-coloured playhead.
            let edge_x = rect.min.x + label_width + (frame_count as f32) * FRAME_W;
            let edge_color = Color32::from_rgb(0xC8, 0x95, 0x30);
            let mut y = rect.min.y + ((clip.top() - rect.min.y).max(0.0) / 7.0).floor() * 7.0;
            while edge_x >= clip.left()
                && edge_x <= clip.right()
                && y < (rect.min.y + total_h).min(clip.bottom())
            {
                let y_end = (y + 4.0).min(rect.min.y + total_h);
                painter.line_segment(
                    [pos2(edge_x, y), pos2(edge_x, y_end)],
                    Stroke::new(1.0_f32, edge_color),
                );
                y += 7.0;
            }

            if camera_row_enabled {
                draw_camera_layer_row(
                    app,
                    &painter,
                    &theme,
                    rect,
                    rect.min.y + ROW_H,
                    frame_count,
                    visible_frames,
                    label_width,
                );
            }

            // Layer rows
            let mut click_layer: Option<u16> = None;
            let mut click_frame: Option<u16> = None;
            let mut click_label_layer: Option<u16> = None;
            let mut open_frame_script: Option<(u16, u16, u16)> = None;
            let mut started_layer_drag: Option<(u16, TimelineLayerSelection)> = None;
            for (li, layer_index) in visible_layer_indices
                .iter()
                .copied()
                .enumerate()
                .take(layer_row_slots.end)
                .skip(layer_row_slots.start)
            {
                let layer = &app.state.project.q0rgs[q0rg_idx].layers[layer_index];
                let row_layer_id = layer.layer_id;
                let layer_frame_count = layer
                    .timeline_frame_count
                    .unwrap_or(frame_count)
                    .min(frame_count);
                let metadata = app.state.project.layer_metadata(q0rg_id, layer.layer_id);
                let is_folder = metadata.kind == q0s_format::v2::LayerKind::Folder;
                let is_mask = metadata.kind == q0s_format::v2::LayerKind::Mask;
                let effective_visible = app.state.project.layer_is_visible(q0rg_id, layer.layer_id);
                let effective_locked = app.state.project.layer_is_locked(q0rg_id, layer.layer_id);
                let row_y = rect.min.y + ROW_H * (li as f32 + camera_row_count as f32 + 1.0);
                let row_rect =
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(rect.width(), ROW_H));
                let is_current_layer = layer.layer_id == app.session.current_layer_id;
                let is_selected_layer = layer_selection_contains(
                    app.session.timeline_layer_selection,
                    &layer_ids,
                    layer.layer_id,
                );
                // Darker base behind everything (covers the layer-label
                // strip too); the per-cell fills drawn below paint over it.
                painter.rect_filled(row_rect, 0.0, theme.empty_beyond.to_color32());

                // Per-cell base fill: empty-inside vs. beyond-frame_count vs.
                // beyond-frame_count-and-every-5th. Span fills draw later
                // on top of these.
                draw_empty_cells(
                    &painter,
                    &theme,
                    rect.min.x + label_width,
                    row_y,
                    layer_frame_count,
                    visible_frames,
                    is_current_layer,
                );

                // Label (left-side fixed column) РІР‚вЂќ pull tones from the
                // panel/window pair so any theme reads as a coherent strip.
                let label_rect =
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(label_width, ROW_H));
                let label_bg = if is_current_layer {
                    theme.panel.to_color32()
                } else if li % 2 == 0 {
                    blend(theme.panel.to_color32(), theme.window.to_color32(), 0.5)
                } else {
                    theme.window.to_color32()
                };
                painter.rect_filled(label_rect, 0.0, label_bg);
                if is_selected_layer {
                    painter.rect_filled(
                        label_rect.shrink(1.0),
                        2.0,
                        theme.accent.to_color32().gamma_multiply(0.22),
                    );
                    painter.rect_stroke(
                        label_rect.shrink(1.0),
                        2.0,
                        Stroke::new(1.0_f32, theme.accent.to_color32()),
                    );
                }
                let folder_depth =
                    app.state
                        .project
                        .layer_folder_depth(q0rg_id, layer.layer_id) as f32;
                let hierarchy_indent = (folder_depth * 14.0
                    + if !app
                        .state
                        .project
                        .layer_mask_sources(q0rg_id, layer.layer_id)
                        .is_empty()
                    {
                        14.0
                    } else {
                        0.0
                    })
                .min((label_width - if is_mask { 100.0 } else { 78.0 }).max(0.0));
                let icon_rect = Rect::from_center_size(
                    label_rect.left_center() + vec2(11.0 + hierarchy_indent, 0.0),
                    vec2(13.0, 13.0),
                );
                draw_layer_icon(
                    &painter,
                    icon_rect,
                    metadata.kind,
                    metadata.collapsed,
                    theme.text.to_color32(),
                    theme.accent.to_color32(),
                );
                let indent = hierarchy_indent;
                let visibility_rect = Rect::from_center_size(
                    label_rect.right_center() - vec2(31.0, 0.0),
                    vec2(18.0, ROW_H),
                );
                let outline_rect = Rect::from_center_size(
                    label_rect.right_center() - vec2(49.0, 0.0),
                    vec2(18.0, ROW_H),
                );
                let lock_rect = Rect::from_center_size(
                    label_rect.right_center() - vec2(13.0, 0.0),
                    vec2(18.0, ROW_H),
                );
                let mask_art_rect = Rect::from_center_size(
                    label_rect.right_center() - vec2(72.0, 0.0),
                    vec2(18.0, ROW_H),
                );
                let mask_art_hidden = app.session.hidden_mask_art_layers.contains(&(
                    app.active_project_tab_id().unwrap_or(0),
                    q0rg_id,
                    layer.layer_id,
                ));
                let mask_art_response = is_mask.then(|| {
                    ui.interact(
                        mask_art_rect,
                        egui::Id::new(("mask-art-visibility", q0rg_id, layer.layer_id)),
                        egui::Sense::click(),
                    )
                    .on_hover_text(if mask_art_hidden {
                        "Show mask artwork and unmasked contents for editing"
                    } else {
                        "Hide mask artwork and preview clipping; contents stay editable"
                    })
                });
                let label_color = if effective_visible {
                    theme.text.to_color32()
                } else {
                    theme.text.to_color32().gamma_multiply(0.45)
                };
                let name_rect = Rect::from_min_max(
                    label_rect.left_top() + vec2(22.0 + indent, 0.0),
                    pos2(
                        if is_mask {
                            mask_art_rect.left()
                        } else {
                            outline_rect.left()
                        } - 3.0,
                        label_rect.bottom(),
                    ),
                );
                if label_width > 120.0 {
                    crate::ui::clipped_label(
                        &painter,
                        name_rect,
                        &layer.name,
                        FontId::proportional(11.0),
                        label_color,
                    );
                }
                ui.interact(
                    name_rect.intersect(label_rect),
                    ui.id().with(("layer_name_hint", row_layer_id)),
                    Sense::hover(),
                )
                .on_hover_text(&layer.name);
                for (control, hint) in [
                    (
                        visibility_rect,
                        if is_mask {
                            "Show or hide mask and all its contents"
                        } else {
                            "Show or hide layer"
                        },
                    ),
                    (outline_rect, "Toggle outline for this layer"),
                    (lock_rect, "Lock or unlock this layer"),
                ] {
                    ui.interact(
                        control,
                        ui.id().with((
                            "layer_control_hint",
                            row_layer_id,
                            control.center().x.to_bits(),
                        )),
                        Sense::hover(),
                    )
                    .on_hover_text(hint);
                }
                draw_visibility_icon(
                    &painter,
                    visibility_rect.shrink2(vec2(2.0, 4.0)),
                    effective_visible,
                    theme.text.to_color32(),
                );
                if is_mask {
                    draw_mask_art_visibility_icon(
                        &painter,
                        mask_art_rect.shrink2(vec2(2.0, 4.0)),
                        !mask_art_hidden,
                        theme.text.to_color32(),
                    );
                }
                let outline_enabled = app
                    .session
                    .outline_layers
                    .contains(&(q0rg_id, layer.layer_id));
                draw_outline_icon(
                    &painter,
                    outline_rect.shrink2(vec2(4.0, 5.0)),
                    outline_enabled,
                    theme.text.to_color32(),
                );
                draw_lock_icon(
                    &painter,
                    lock_rect.shrink2(vec2(2.0, 3.0)),
                    effective_locked,
                    theme.text.to_color32(),
                );
                painter.line_segment(
                    [
                        pos2(rect.min.x + label_width, row_y),
                        pos2(rect.min.x + label_width, row_y + ROW_H),
                    ],
                    Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
                );

                // Bottom row separator РІР‚вЂќ hairline in the theme's stroke colour.
                painter.line_segment(
                    [
                        pos2(rect.min.x, row_y + ROW_H),
                        pos2(rect.min.x + rect.width(), row_y + ROW_H),
                    ],
                    Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
                );

                // Now paint the frame strip on top of the dark base. Active
                // span cells become bright greys (the Flash look); cells
                // beyond frame_count are left dark.
                if !is_folder {
                    draw_layer_spans(
                        &painter,
                        &theme,
                        layer,
                        rect.min.x + label_width,
                        row_y,
                        frame_count,
                        is_current_layer,
                    );
                    draw_audio_waveforms(
                        &app.state.project,
                        &mut app.textures,
                        &painter,
                        &theme,
                        q0rg_id,
                        layer,
                        AudioWaveformLayout {
                            base_x: rect.min.x + label_width,
                            row_y,
                            frame_count: layer_frame_count,
                        },
                    );
                    draw_frame_script_markers(
                        &app.state.project,
                        &painter,
                        &theme,
                        q0rg_id,
                        layer.layer_id,
                        AudioWaveformLayout {
                            base_x: rect.min.x + label_width,
                            row_y,
                            frame_count: layer_frame_count,
                        },
                    );
                } else {
                    let folder_strip = Rect::from_min_size(
                        pos2(rect.min.x + label_width, row_y + CELL_PAD_Y),
                        vec2((visible_frames as f32) * FRAME_W, ROW_H - CELL_PAD_Y * 2.0),
                    );
                    painter.rect_filled(
                        folder_strip,
                        0.0,
                        blend(theme.panel.to_color32(), theme.window.to_color32(), 0.35),
                    );
                }

                if !is_folder && response.double_clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        if row_rect.contains(pos) && !label_rect.contains(pos) {
                            let rel_x = pos.x - (rect.min.x + label_width);
                            if rel_x >= 0.0 {
                                let frame = (rel_x / FRAME_W).floor() as u16;
                                if frame < frame_count
                                    && app.state.project.runtime.frame_scripts.iter().any(
                                        |script| {
                                            script.q0rg_id == q0rg_id
                                                && script.layer_id == layer.layer_id
                                                && script.frame == frame
                                        },
                                    )
                                {
                                    open_frame_script = Some((q0rg_id, layer.layer_id, frame));
                                }
                            }
                        }
                    }
                }

                // Dragging starts only from the fixed layer-label column.
                // Frame-strip drags keep their existing rectangular selection
                // behaviour and never accidentally reorder a layer.
                if let Some(pos) = response.interact_pointer_pos() {
                    let state_control = visibility_rect.contains(pos)
                        || outline_rect.contains(pos)
                        || lock_rect.contains(pos)
                        || (is_mask && mask_art_rect.contains(pos));
                    if label_rect.contains(pos)
                        && !state_control
                        && response.drag_started_by(PointerButton::Primary)
                    {
                        let selection = app
                            .session
                            .timeline_layer_selection
                            .filter(|selection| {
                                layer_selection_contains(
                                    Some(*selection),
                                    &layer_ids,
                                    layer.layer_id,
                                )
                            })
                            .unwrap_or_else(|| TimelineLayerSelection::single(layer.layer_id));
                        started_layer_drag = Some((layer.layer_id, selection));
                    }
                }

                if mask_art_response
                    .as_ref()
                    .is_some_and(|mask_response| mask_response.clicked())
                {
                    app.queue(Action::ToggleMaskArtworkVisibility(q0rg_id, layer.layer_id));
                    continue;
                }

                // Click in row РІвЂ вЂ™ set current_layer + current_frame
                if let Some(pos) = response.interact_pointer_pos() {
                    if row_rect.contains(pos) && response.clicked() {
                        if visibility_rect.contains(pos) {
                            app.queue(Action::ToggleLayerVisibility(q0rg_id, layer.layer_id));
                            continue;
                        }
                        if lock_rect.contains(pos) {
                            app.queue(Action::ToggleLayerLock(q0rg_id, layer.layer_id));
                            continue;
                        }
                        if outline_rect.contains(pos) && !is_folder {
                            app.queue(Action::ToggleLayerOutline(q0rg_id, layer.layer_id));
                            continue;
                        }
                        if label_rect.contains(pos) {
                            click_label_layer = Some(layer.layer_id);
                        }
                        click_layer = Some(layer.layer_id);
                        if is_folder {
                            click_frame = None;
                            app.session.timeline_selection = None;
                            app.session.timeline_cell_selection = None;
                            if label_rect.contains(pos) && icon_rect.contains(pos) {
                                app.queue(Action::ToggleLayerFolder(q0rg_id, row_layer_id));
                            }
                        } else {
                            if is_mask && icon_rect.contains(pos) {
                                app.queue(Action::ToggleLayerFolder(q0rg_id, row_layer_id));
                                click_frame = None;
                            }
                            let rel_x = pos.x - (rect.min.x + label_width);
                            if rel_x >= 0.0 {
                                let f = (rel_x / FRAME_W).floor() as i32;
                                if f >= 0 && f < visible_frames as i32 {
                                    click_frame = Some(f as u16);
                                }
                            } else if response.double_clicked() {
                                begin_layer_rename(app, q0rg_id, row_layer_id);
                            }
                        }
                    }
                }
            }

            // Rig tracks are dedicated sublayers after the artwork rows. They
            // only exist for layers with an actual rig binding or deformer.
            for (track_index, track) in rig_tracks
                .iter()
                .enumerate()
                .take(rig_row_slots.end)
                .skip(rig_row_slots.start)
            {
                let row_y = rig_rows_y + ROW_H * track_index as f32;
                let row_rect =
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(rect.width(), ROW_H));
                let label_rect =
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(label_width, ROW_H));
                let selection = app.session.timeline_rig_key_selection.filter(|selection| {
                    selection.q0rg_id == q0rg_id && selection.layer_id == track.layer_id
                });
                let selected_track = selection.is_some();
                painter.rect_filled(row_rect, 0.0, theme.empty_beyond.to_color32());
                draw_empty_cells(
                    &painter,
                    &theme,
                    rect.min.x + label_width,
                    row_y,
                    frame_count,
                    visible_frames,
                    selected_track,
                );
                painter.rect_filled(
                    label_rect,
                    0.0,
                    blend(
                        theme.panel.to_color32(),
                        theme.accent.to_color32(),
                        if selected_track { 0.22 } else { 0.08 },
                    ),
                );
                let icon_center = label_rect.left_center() + vec2(11.0, 0.0);
                painter.line_segment(
                    [icon_center + vec2(-3.0, 3.0), icon_center + vec2(3.0, -3.0)],
                    Stroke::new(1.4_f32, theme.accent.to_color32()),
                );
                painter.circle_filled(
                    icon_center + vec2(-3.0, 3.0),
                    2.1,
                    theme.accent.to_color32(),
                );
                painter.circle_filled(
                    icon_center + vec2(3.0, -3.0),
                    2.1,
                    theme.accent.to_color32(),
                );
                painter.text(
                    label_rect.left_center() + vec2(22.0, 0.0),
                    Align2::LEFT_CENTER,
                    format!("rig: {}", truncate_label(&track.layer_name, 21)),
                    FontId::proportional(10.0),
                    theme.accent.to_color32(),
                );
                painter.line_segment(
                    [
                        pos2(rect.min.x + label_width, row_y),
                        pos2(rect.min.x + label_width, row_y + ROW_H),
                    ],
                    Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
                );
                painter.line_segment(
                    [
                        pos2(rect.min.x, row_y + ROW_H),
                        pos2(rect.max.x, row_y + ROW_H),
                    ],
                    Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
                );
                if let Some(selection) = selection {
                    let selected_rect = Rect::from_min_size(
                        pos2(
                            rect.min.x + label_width + f32::from(selection.frame) * FRAME_W,
                            row_y + CELL_PAD_Y,
                        ),
                        vec2(FRAME_W, ROW_H - CELL_PAD_Y * 2.0),
                    );
                    painter.rect_stroke(
                        selected_rect,
                        1.0,
                        Stroke::new(1.0_f32, theme.accent.to_color32()),
                    );
                }
                for frame in &track.frames {
                    if *frame >= frame_count || !frame_slots.contains(frame) {
                        continue;
                    }
                    let center = pos2(
                        rect.min.x + label_width + (f32::from(*frame) + 0.5) * FRAME_W,
                        row_y + ROW_H * 0.5,
                    );
                    let radius = 4.0_f32;
                    let fill = if selection.is_some_and(|selection| selection.frame == *frame) {
                        theme.keyframe.to_color32()
                    } else {
                        theme.accent.to_color32()
                    };
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            pos2(center.x, center.y - radius),
                            pos2(center.x + radius, center.y),
                            pos2(center.x, center.y + radius),
                            pos2(center.x - radius, center.y),
                        ],
                        fill,
                        Stroke::new(0.8_f32, theme.stroke_dark.to_color32()),
                    ));
                    ui.interact(
                        Rect::from_center_size(center, vec2(FRAME_W, ROW_H)),
                        egui::Id::new(("rig_timeline_key", q0rg_id, track.layer_id, frame)),
                        Sense::hover(),
                    )
                    .on_hover_text(format!(
                        "rig key on {}\nframe {}",
                        track.layer_name,
                        u32::from(*frame) + 1
                    ));
                }
            }

            if let Some((q0rg_id, layer_id, frame)) = open_frame_script {
                app.queue(Action::OpenFrameScriptEditor(q0rg_id, layer_id, frame));
            }

            if let Some((layer_id, selection)) = started_layer_drag {
                app.session.timeline_layer_drag = Some(TimelineLayerDrag { q0rg_id, selection });
                app.session.current_layer_id = layer_id;
                app.session.timeline_selection = None;
                app.session.timeline_layer_selection = Some(selection);
                app.session.selection = Selection::None;
            }

            let layer_drop_preview = app
                .session
                .timeline_layer_drag
                .filter(|drag| drag.q0rg_id == q0rg_id)
                .and_then(|drag| {
                    let dragged_layer_ids = layer_ids
                        .iter()
                        .copied()
                        .filter(|layer_id| {
                            layer_selection_contains(Some(drag.selection), &layer_ids, *layer_id)
                        })
                        .collect::<Vec<_>>();
                    ui.input(|input| input.pointer.latest_pos())
                        .and_then(|pos| {
                            layer_drop_target_at(
                                &app.state.project,
                                q0rg_id,
                                rect,
                                pos,
                                &layer_ids,
                                &dragged_layer_ids,
                            )
                        })
                });
            if let Some(target) = layer_drop_preview {
                draw_layer_drop_preview(
                    &painter,
                    &theme,
                    &app.state.project,
                    q0rg_id,
                    rect,
                    &layer_ids,
                    target,
                    label_width,
                );
            }

            let library_payload = response.dnd_hover_payload::<LibraryDragPayload>();
            let library_drop_cell =
                ui.input(|input| input.pointer.latest_pos())
                    .and_then(|position| {
                        timeline_cell_at_with_label_width(
                            rect,
                            position,
                            frame_count,
                            &layer_ids,
                            &folder_layer_ids,
                            camera_row_count,
                            label_width,
                        )
                    });
            if library_payload.is_some() {
                if let Some((layer_id, frame)) = library_drop_cell {
                    if let Some(row) = layer_ids
                        .iter()
                        .position(|candidate| *candidate == layer_id)
                    {
                        let cell = Rect::from_min_size(
                            pos2(
                                rect.min.x + label_width + frame as f32 * FRAME_W,
                                rect.min.y + ROW_H * (row as f32 + camera_row_count as f32 + 1.0),
                            ),
                            vec2(FRAME_W, ROW_H),
                        )
                        .shrink(1.0);
                        painter.rect_filled(
                            cell,
                            1.0,
                            theme.accent.to_color32().gamma_multiply(0.22),
                        );
                        painter.rect_stroke(
                            cell,
                            1.0,
                            Stroke::new(1.5_f32, theme.accent.to_color32()),
                        );
                    }
                }
            }
            let released_library_item = response
                .dnd_release_payload::<LibraryDragPayload>()
                .map(|payload| *payload);
            if let (Some(payload), Some((layer_id, frame))) =
                (released_library_item, library_drop_cell)
            {
                app.queue(Action::PlaceLibraryDragOnTimeline(payload, layer_id, frame));
            }

            draw_timeline_selection(
                &painter,
                &theme,
                app.session.timeline_selection,
                &layer_ids,
                rect.min.x + label_width,
                rect.min.y + ROW_H * (camera_row_count as f32 + 1.0),
                visible_frames,
            );
            if let Some((selected_q0rg, cells)) = app.session.timeline_cell_selection.as_ref() {
                if *selected_q0rg == q0rg_id {
                    draw_timeline_cell_selection(
                        &painter,
                        &theme,
                        cells,
                        &layer_ids,
                        rect.min.x + label_width,
                        rect.min.y + ROW_H * (camera_row_count as f32 + 1.0),
                        visible_frames,
                    );
                }
            }

            // Playhead
            let playhead_x =
                rect.min.x + label_width + (app.session.timeline_frame() as f32 + 0.5) * FRAME_W;
            painter.line_segment(
                [
                    pos2(playhead_x, rect.min.y),
                    pos2(playhead_x, rect.min.y + total_h),
                ],
                Stroke::new(1.5_f32, theme.playhead.to_color32()),
            );

            // Header click РІвЂ вЂ™ set current frame
            if let Some(pos) = response.interact_pointer_pos() {
                if app.session.timeline_layer_drag.is_none()
                    && header_rect.contains(pos)
                    && (response.clicked() || response.dragged())
                {
                    let rel_x = pos.x - (rect.min.x + label_width);
                    if rel_x >= 0.0 {
                        let f = (rel_x / FRAME_W).floor() as i32;
                        if f >= 0 && f < visible_frames as i32 {
                            click_frame = Some(f as u16);
                        }
                    }
                }
            }

            let camera_row_rect = camera_row_enabled.then(|| {
                Rect::from_min_size(
                    pos2(rect.min.x, rect.min.y + ROW_H),
                    vec2(rect.width(), ROW_H),
                )
            });
            let camera_pointer_frame = response.interact_pointer_pos().and_then(|pos| {
                let row = camera_row_rect?;
                if !row.contains(pos) || pos.x < rect.min.x + label_width {
                    return None;
                }
                let frame = ((pos.x - (rect.min.x + label_width)) / FRAME_W).floor() as i32;
                (frame >= 0 && frame < i32::from(visible_frames)).then_some(frame as u16)
            });
            let pointer_on_camera_row = response
                .interact_pointer_pos()
                .zip(camera_row_rect)
                .is_some_and(|(pos, row)| row.contains(pos));
            let rig_track_pointer_row = response.interact_pointer_pos().and_then(|pos| {
                let relative_y = pos.y - rig_rows_y;
                (relative_y >= 0.0 && relative_y < ROW_H * rig_track_count as f32)
                    .then_some((relative_y / ROW_H).floor() as usize)
                    .filter(|index| *index < rig_track_count)
            });
            let rig_track_pointer_cell = response.interact_pointer_pos().and_then(|pos| {
                let track_index = rig_track_pointer_row?;
                let relative_x = pos.x - (rect.min.x + label_width);
                if relative_x < 0.0 {
                    return None;
                }
                let frame = (relative_x / FRAME_W).floor() as i32;
                (frame >= 0 && frame < i32::from(visible_frames))
                    .then_some((track_index, frame as u16))
            });
            let rig_drop_cell = ui
                .input(|input| input.pointer.latest_pos())
                .and_then(|pos| {
                    let relative_y = pos.y - rig_rows_y;
                    if relative_y < 0.0 || relative_y >= ROW_H * rig_track_count as f32 {
                        return None;
                    }
                    let track_index = (relative_y / ROW_H).floor() as usize;
                    let relative_x = pos.x - (rect.min.x + label_width);
                    if relative_x < 0.0 || track_index >= rig_track_count {
                        return None;
                    }
                    let frame = (relative_x / FRAME_W).floor() as i32;
                    (frame >= 0 && frame < i32::from(visible_frames))
                        .then_some((rig_tracks[track_index].layer_id, frame as u16))
                });

            if camera_row_enabled && pointer_on_camera_row && response.clicked() {
                app.session.camera_track_selected = true;
                app.session.timeline_selection = None;
                app.session.timeline_cell_selection = None;
                app.session.timeline_layer_selection = None;
                app.session.timeline_frame_drag = None;
                app.session.timeline_rig_key_selection = None;
                app.session.timeline_rig_key_drag = None;
                app.session.selection = Selection::None;
                if let Some(frame) = camera_pointer_frame {
                    let extend = ui.input(|input| input.modifiers.shift);
                    app.session.camera_timeline_selection = if extend {
                        app.session
                            .camera_timeline_selection
                            .map(|mut selection| {
                                selection.focus_frame = frame;
                                selection
                            })
                            .or_else(|| Some(CameraTimelineSelection::single(frame)))
                    } else {
                        Some(CameraTimelineSelection::single(frame))
                    };
                    app.session.set_timeline_frame(frame, frame_count);
                } else {
                    app.session.camera_timeline_selection = None;
                }
            }

            if let (Some(track_index), Some((cell_track_index, frame))) =
                (rig_track_pointer_row, rig_track_pointer_cell)
            {
                if track_index == cell_track_index && response.clicked() {
                    let track = &rig_tracks[track_index];
                    app.session.current_layer_id = track.layer_id;
                    app.session.camera_track_selected = false;
                    app.session.camera_timeline_selection = None;
                    app.session.timeline_selection = None;
                    app.session.timeline_cell_selection = None;
                    app.session.timeline_layer_selection = None;
                    app.session.timeline_frame_drag = None;
                    app.session.camera_frame_drag = None;
                    app.session.selection = Selection::None;
                    app.session.timeline_rig_key_selection = Some(TimelineRigKeySelection {
                        q0rg_id,
                        layer_id: track.layer_id,
                        frame,
                    });
                    app.session.set_timeline_frame(frame, frame_count);
                }
            } else if let Some(track_index) = rig_track_pointer_row.filter(|_| response.clicked()) {
                let track = &rig_tracks[track_index];
                app.session.current_layer_id = track.layer_id;
                app.session.camera_track_selected = false;
                app.session.camera_timeline_selection = None;
                app.session.timeline_selection = None;
                app.session.timeline_cell_selection = None;
                app.session.timeline_layer_selection = None;
                app.session.timeline_frame_drag = None;
                app.session.camera_frame_drag = None;
                app.session.selection = Selection::None;
                app.session.timeline_rig_key_selection = Some(TimelineRigKeySelection {
                    q0rg_id,
                    layer_id: track.layer_id,
                    frame: app.session.timeline_frame(),
                });
            }
            if response.clicked() && rig_track_pointer_row.is_none() {
                app.session.timeline_rig_key_selection = None;
                app.session.timeline_rig_key_drag = None;
            }

            if let Some((track_index, frame)) = rig_track_pointer_cell {
                let track = &rig_tracks[track_index];
                if response.drag_started_by(PointerButton::Primary) && track.frames.contains(&frame)
                {
                    let selection = TimelineRigKeySelection {
                        q0rg_id,
                        layer_id: track.layer_id,
                        frame,
                    };
                    app.session.timeline_rig_key_selection = Some(selection);
                    app.session.timeline_rig_key_drag = Some(selection);
                    app.session.current_layer_id = track.layer_id;
                    app.session.camera_track_selected = false;
                    app.session.timeline_selection = None;
                    app.session.timeline_cell_selection = None;
                    app.session.timeline_layer_selection = None;
                    app.session.selection = Selection::None;
                    app.session.set_timeline_frame(frame, frame_count);
                }
            }

            if let (Some(drag), Some((target_layer_id, target_frame))) =
                (app.session.timeline_rig_key_drag, rig_drop_cell)
            {
                if drag.q0rg_id == q0rg_id && drag.layer_id == target_layer_id {
                    let track_index = rig_tracks
                        .iter()
                        .position(|track| track.layer_id == target_layer_id)
                        .unwrap_or_default();
                    let preview = Rect::from_min_size(
                        pos2(
                            rect.min.x + label_width + f32::from(target_frame) * FRAME_W,
                            rig_rows_y + ROW_H * track_index as f32 + CELL_PAD_Y,
                        ),
                        vec2(FRAME_W, ROW_H - CELL_PAD_Y * 2.0),
                    );
                    painter.rect_filled(
                        preview,
                        1.0,
                        theme.accent.to_color32().gamma_multiply(0.22),
                    );
                    painter.rect_stroke(
                        preview,
                        1.0,
                        Stroke::new(1.5_f32, theme.accent.to_color32()),
                    );
                }
            }

            if camera_row_enabled
                && app.session.timeline_layer_drag.is_none()
                && library_payload.is_none()
            {
                if let Some(frame) = camera_pointer_frame {
                    if response.drag_started_by(PointerButton::Primary) {
                        app.session.camera_track_selected = true;
                        app.session.timeline_selection = None;
                        app.session.timeline_cell_selection = None;
                        app.session.timeline_layer_selection = None;
                        app.session.timeline_frame_drag = None;
                        app.session.timeline_rig_key_selection = None;
                        app.session.timeline_rig_key_drag = None;
                        app.session.selection = Selection::None;
                        let existing = app.session.camera_timeline_selection;
                        let inside_existing = existing.is_some_and(|selection| {
                            (selection.first_frame()..=selection.last_frame()).contains(&frame)
                        });
                        let existing_has_key = existing.is_some_and(|selection| {
                            app.state.project.runtime.camera.keys.iter().any(|key| {
                                (selection.first_frame()..=selection.last_frame())
                                    .contains(&key.frame)
                            })
                        });
                        if inside_existing && existing_has_key {
                            app.session.camera_frame_drag =
                                existing.map(|selection| CameraFrameDrag { selection });
                        } else {
                            app.session.camera_timeline_selection =
                                Some(CameraTimelineSelection::single(frame));
                            app.session.set_timeline_frame(frame, frame_count);
                        }
                    } else if response.dragged() && app.session.camera_frame_drag.is_none() {
                        app.session.camera_track_selected = true;
                        let mut selection = app
                            .session
                            .camera_timeline_selection
                            .unwrap_or_else(|| CameraTimelineSelection::single(frame));
                        selection.focus_frame = frame;
                        app.session.camera_timeline_selection = Some(selection);
                        app.session.set_timeline_frame(frame, frame_count);
                    }
                }
            }

            if let (Some(drag), Some(target_frame)) =
                (app.session.camera_frame_drag, camera_pointer_frame)
            {
                let width = drag
                    .selection
                    .last_frame()
                    .saturating_sub(drag.selection.first_frame())
                    .saturating_add(1);
                let preview = Rect::from_min_size(
                    pos2(
                        rect.min.x + label_width + f32::from(target_frame) * FRAME_W,
                        rect.min.y + ROW_H + CELL_PAD_Y,
                    ),
                    vec2(f32::from(width) * FRAME_W, ROW_H - CELL_PAD_Y * 2.0),
                );
                painter.rect_filled(preview, 1.0, theme.accent.to_color32().gamma_multiply(0.18));
                painter.rect_stroke(
                    preview,
                    1.0,
                    Stroke::new(1.5_f32, theme.accent.to_color32()),
                );
            }

            if let Some(id) = click_layer {
                app.session.current_layer_id = id;
                app.session.camera_track_selected = false;
                app.session.camera_timeline_selection = None;
                app.session.camera_frame_drag = None;
                app.session.timeline_rig_key_selection = None;
                app.session.timeline_rig_key_drag = None;
            }
            if let Some(layer_id) = click_label_layer {
                let extend = ui.input(|input| input.modifiers.shift);
                app.session.timeline_layer_selection = if extend {
                    app.session
                        .timeline_layer_selection
                        .map(|mut selection| {
                            selection.focus_layer_id = layer_id;
                            selection
                        })
                        .or_else(|| Some(TimelineLayerSelection::single(layer_id)))
                } else {
                    Some(TimelineLayerSelection::single(layer_id))
                };
                app.session.timeline_selection = None;
                app.session.timeline_cell_selection = None;
                app.session.selection = symbols_on_layer_at_frame(
                    &app.state.project,
                    q0rg_id,
                    layer_id,
                    app.session.current_frame,
                );
            }
            if let Some(f) = click_frame {
                app.session.set_timeline_frame(f, frame_count);
            }
            if let (Some(layer_id), Some(frame)) = (click_layer, click_frame) {
                app.session.selection = Selection::None;
                app.session.timeline_layer_selection = None;
                app.session.current_layer_id = layer_id;
                let (toggle_cell, extend) = ui.input(|input| {
                    (
                        input.modifiers.ctrl || input.modifiers.command,
                        input.modifiers.shift,
                    )
                });
                if toggle_cell {
                    let mut cells = app
                        .session
                        .timeline_cell_selection
                        .take()
                        .filter(|(selected_q0rg, _)| *selected_q0rg == q0rg_id)
                        .map(|(_, cells)| cells)
                        .unwrap_or_else(|| {
                            app.session
                                .timeline_selection
                                .and_then(|selection| {
                                    timeline_selection_bounds(selection, &layer_ids, frame_count)
                                })
                                .map(|(first_layer, last_layer, first_frame, last_frame)| {
                                    (first_layer..=last_layer)
                                        .flat_map(|layer_index| {
                                            let selected_layer = layer_ids[layer_index];
                                            (first_frame..=last_frame).map(move |selected_frame| {
                                                (selected_layer, selected_frame)
                                            })
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        });
                    if let Some(index) = cells.iter().position(|cell| *cell == (layer_id, frame)) {
                        cells.remove(index);
                    } else {
                        cells.push((layer_id, frame));
                    }
                    app.session.timeline_selection = None;
                    app.session.timeline_cell_selection =
                        (!cells.is_empty()).then_some((q0rg_id, cells));
                } else {
                    app.session.timeline_cell_selection = None;
                    app.session.timeline_selection = if extend {
                        app.session
                            .timeline_selection
                            .map(|mut selection| {
                                selection.focus_layer_id = layer_id;
                                selection.focus_frame = frame;
                                selection
                            })
                            .or_else(|| Some(TimelineSelection::single(layer_id, frame)))
                    } else {
                        Some(TimelineSelection::single(layer_id, frame))
                    };
                }
            }

            let frame_drop_cell =
                ui.input(|input| input.pointer.latest_pos())
                    .and_then(|position| {
                        timeline_cell_at_with_label_width(
                            rect,
                            position,
                            visible_frames,
                            &layer_ids,
                            &folder_layer_ids,
                            camera_row_count,
                            label_width,
                        )
                    });

            if app.session.timeline_layer_drag.is_none() && library_payload.is_none() {
                if let Some((layer_id, frame)) = frame_drop_cell {
                    if response.drag_started() {
                        app.session.selection = Selection::None;
                        app.session.timeline_layer_selection = None;
                        app.session.timeline_cell_selection = None;
                        app.session.timeline_rig_key_selection = None;
                        app.session.timeline_rig_key_drag = None;
                        if let Some(selection) =
                            app.session.timeline_selection.filter(|selection| {
                                frame_selection_contains(*selection, &layer_ids, layer_id, frame)
                            })
                        {
                            app.session.timeline_frame_drag =
                                Some(TimelineFrameDrag { q0rg_id, selection });
                        } else {
                            app.session.timeline_selection =
                                Some(TimelineSelection::single(layer_id, frame));
                            app.session.current_layer_id = layer_id;
                            app.session.set_timeline_frame(frame, frame_count);
                        }
                    } else if response.dragged() && app.session.timeline_frame_drag.is_none() {
                        app.session.selection = Selection::None;
                        app.session.timeline_layer_selection = None;
                        let mut selection = app
                            .session
                            .timeline_selection
                            .unwrap_or_else(|| TimelineSelection::single(layer_id, frame));
                        selection.focus_layer_id = layer_id;
                        selection.focus_frame = frame;
                        app.session.timeline_selection = Some(selection);
                        app.session.current_layer_id = layer_id;
                        app.session.set_timeline_frame(frame, frame_count);
                    }
                }
            }

            if let (Some(drag), Some((target_layer_id, target_frame))) =
                (app.session.timeline_frame_drag, frame_drop_cell)
            {
                if drag.q0rg_id == q0rg_id {
                    if let Some(target_row) = layer_ids
                        .iter()
                        .position(|layer_id| *layer_id == target_layer_id)
                    {
                        let width = drag
                            .selection
                            .anchor_frame
                            .abs_diff(drag.selection.focus_frame)
                            + 1;
                        let source_rows = layer_ids
                            .iter()
                            .position(|id| *id == drag.selection.anchor_layer_id)
                            .zip(
                                layer_ids
                                    .iter()
                                    .position(|id| *id == drag.selection.focus_layer_id),
                            )
                            .map(|(a, b)| a.abs_diff(b) + 1)
                            .unwrap_or(1);
                        let preview = Rect::from_min_size(
                            pos2(
                                rect.min.x + label_width + target_frame as f32 * FRAME_W,
                                rect.min.y
                                    + ROW_H * (target_row as f32 + camera_row_count as f32 + 1.0),
                            ),
                            vec2(width as f32 * FRAME_W, source_rows as f32 * ROW_H),
                        )
                        .shrink(1.0);
                        painter.rect_filled(
                            preview,
                            1.0,
                            theme.accent.to_color32().gamma_multiply(0.18),
                        );
                        painter.rect_stroke(
                            preview,
                            1.0,
                            Stroke::new(1.5_f32, theme.accent.to_color32()),
                        );
                    }
                }
            }

            if response.drag_stopped_by(PointerButton::Primary) {
                if let Some(drag) = app.session.timeline_rig_key_drag.take() {
                    if drag.q0rg_id == q0rg_id {
                        if let Some((target_layer_id, target_frame)) = rig_drop_cell {
                            if target_layer_id == drag.layer_id {
                                app.queue(Action::MoveRigTimelineKey(drag, target_frame));
                            }
                        }
                    }
                }
                if let Some(drag) = app.session.timeline_layer_drag.take() {
                    if drag.q0rg_id == q0rg_id {
                        if let Some(target) = layer_drop_preview {
                            let dragged_layer_ids = layer_ids
                                .iter()
                                .copied()
                                .filter(|layer_id| {
                                    layer_selection_contains(
                                        Some(drag.selection),
                                        &layer_ids,
                                        *layer_id,
                                    )
                                })
                                .collect::<Vec<_>>();
                            if dragged_layer_ids.len() > 1 {
                                app.queue(Action::DropLayers(q0rg_id, dragged_layer_ids, target));
                            } else if let Some(layer_id) = dragged_layer_ids.first().copied() {
                                app.queue(Action::DropLayer(q0rg_id, layer_id, target));
                            }
                        }
                    }
                }
                if let Some(drag) = app.session.timeline_frame_drag.take() {
                    if drag.q0rg_id == q0rg_id {
                        if let Some((target_layer_id, target_frame)) = frame_drop_cell {
                            app.queue(Action::MoveTimelineFrames(
                                drag.selection,
                                target_layer_id,
                                target_frame,
                            ));
                        }
                    }
                }
                if let Some(drag) = app.session.camera_frame_drag.take() {
                    if let Some(target_frame) = camera_pointer_frame {
                        app.queue(Action::MoveCameraFrames(drag.selection, target_frame));
                    }
                }
            } else if !ui.input(|input| input.pointer.primary_down()) {
                app.session.timeline_layer_drag = None;
                app.session.timeline_frame_drag = None;
                app.session.timeline_rig_key_drag = None;
                app.session.camera_frame_drag = None;
            }
            // Right-click on a frame cell: position the playhead there first,
            // then open the context menu so its actions act on the *clicked*
            // frame, not whatever was previously selected.
            let secondary_on_camera = response.secondary_clicked() && pointer_on_camera_row;
            let secondary_on_rig = response.secondary_clicked() && rig_track_pointer_row.is_some();
            if response.secondary_clicked() && !secondary_on_rig {
                app.session.timeline_rig_key_selection = None;
                app.session.timeline_rig_key_drag = None;
            }
            if response.secondary_clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let rel_x = pos.x - (rect.min.x + label_width);
                    let clicked_frame = if rel_x >= 0.0 {
                        let frame = (rel_x / FRAME_W).floor() as i32;
                        (frame >= 0 && frame < visible_frames as i32).then_some(frame as u16)
                    } else {
                        None
                    };
                    if let Some(frame) = clicked_frame {
                        app.session.set_timeline_frame(frame, frame_count);
                    }
                    if secondary_on_camera {
                        app.session.camera_track_selected = true;
                        app.session.timeline_selection = None;
                        app.session.timeline_cell_selection = None;
                        app.session.timeline_layer_selection = None;
                        app.session.timeline_rig_key_selection = None;
                        app.session.selection = Selection::None;
                        if let Some(frame) = camera_pointer_frame {
                            app.session.camera_timeline_selection =
                                Some(CameraTimelineSelection::single(frame));
                        }
                    } else if secondary_on_rig {
                        let track = &rig_tracks[rig_track_pointer_row.expect("rig row was hit")];
                        let frame = clicked_frame.unwrap_or_else(|| app.session.timeline_frame());
                        app.session.current_layer_id = track.layer_id;
                        app.session.camera_track_selected = false;
                        app.session.camera_timeline_selection = None;
                        app.session.timeline_selection = None;
                        app.session.timeline_cell_selection = None;
                        app.session.timeline_layer_selection = None;
                        app.session.timeline_rig_key_selection = Some(TimelineRigKeySelection {
                            q0rg_id,
                            layer_id: track.layer_id,
                            frame,
                        });
                        app.session.selection = Selection::None;
                        app.session.set_timeline_frame(frame, frame_count);
                    } else {
                        // Identify which ordinary layer row was clicked.
                        let row_idx = ((pos.y - rect.min.y) / ROW_H).floor() as i32
                            - 1
                            - camera_row_count as i32;
                        if row_idx >= 0 {
                            if let Some(layer_index) = visible_layer_indices.get(row_idx as usize) {
                                let layer = &app.state.project.q0rgs[q0rg_idx].layers[*layer_index];
                                app.session.current_layer_id = layer.layer_id;
                                app.session.camera_track_selected = false;
                                app.session.camera_timeline_selection = None;
                                app.session.timeline_rig_key_selection = None;
                                app.session.selection = Selection::None;
                                let clicked_label = pos.x < rect.min.x + label_width;
                                if clicked_label
                                    || app.state.project.layer_is_folder(q0rg_id, layer.layer_id)
                                {
                                    app.session.timeline_selection = None;
                                    app.session.timeline_cell_selection = None;
                                    app.session.timeline_layer_selection =
                                        Some(TimelineLayerSelection::single(layer.layer_id));
                                } else {
                                    app.session.timeline_layer_selection = None;
                                    let frame = clicked_frame
                                        .unwrap_or_else(|| app.session.timeline_frame());
                                    let keep_cell_selection =
                                        app.session.timeline_cell_selection.as_ref().is_some_and(
                                            |(selected_q0rg, cells)| {
                                                *selected_q0rg == q0rg_id
                                                    && cells.contains(&(layer.layer_id, frame))
                                            },
                                        );
                                    let keep_range_selection =
                                        app.session.timeline_selection.is_some_and(|selection| {
                                            frame_selection_contains(
                                                selection,
                                                &layer_ids,
                                                layer.layer_id,
                                                frame,
                                            )
                                        });
                                    if !keep_cell_selection && !keep_range_selection {
                                        app.session.timeline_cell_selection = None;
                                        app.session.timeline_selection =
                                            Some(TimelineSelection::single(layer.layer_id, frame));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if splitter.hovered() || splitter.dragged() {
                painter.rect_filled(
                    splitter_rect.shrink2(vec2(1.0, 0.0)),
                    1.0,
                    theme.accent.to_color32().gamma_multiply(0.65),
                );
            }
            if secondary_on_rig {
                response.context_menu(|ui| rig_timeline_context_menu(app, ui));
            } else if !secondary_on_camera {
                response.context_menu(|ui| timeline_context_menu(app, ui));
            }
        });
    render_layer_rename_dialog(app, ui.ctx());
    render_frame_label_dialog(app, ui.ctx());
}

fn layer_parent_for_drop_lane(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    target_layer_id: u16,
    x: f32,
    left: f32,
) -> Option<u16> {
    let target_depth = project.layer_folder_depth(q0rg_id, target_layer_id);
    let requested_depth = if x < left + 32.0 {
        0
    } else {
        let lane_depth = ((x - (left + 32.0)) / 14.0).floor() as usize + 1;
        lane_depth.min(target_depth)
    };
    if requested_depth == 0 {
        return None;
    }
    let ancestor_depth = requested_depth - 1;
    let mut parent = project.layer_parent_folder(q0rg_id, target_layer_id);
    while let Some(parent_id) = parent {
        if project.layer_folder_depth(q0rg_id, parent_id) == ancestor_depth {
            return Some(parent_id);
        }
        parent = project.layer_parent_folder(q0rg_id, parent_id);
    }
    None
}

fn mask_layer_above(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
) -> Option<u16> {
    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    let parent = project.layer_parent_folder(q0rg_id, layer_id);
    let siblings = q0rg
        .layers
        .iter()
        .rev()
        .filter(|layer| project.layer_parent_folder(q0rg_id, layer.layer_id) == parent)
        .map(|layer| layer.layer_id)
        .collect::<Vec<_>>();
    let index = siblings
        .iter()
        .position(|candidate| *candidate == layer_id)?;
    let above = *siblings.get(index.checked_sub(1)?)?;
    (project.layer_metadata(q0rg_id, above).kind == q0s_format::v2::LayerKind::Mask)
        .then_some(above)
}

fn layer_child_at_drop_parent(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    mut layer_id: u16,
    requested_parent: Option<u16>,
) -> Option<u16> {
    let mut hops = 0usize;
    loop {
        let parent = project.layer_parent_folder(q0rg_id, layer_id);
        if parent == requested_parent {
            return Some(layer_id);
        }
        layer_id = parent?;
        hops += 1;
        if hops > q0s_format::v2::MAX_LAYER_FOLDER_NESTING_DEPTH {
            return None;
        }
    }
}

fn layer_drop_target_at(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    rect: Rect,
    pos: egui::Pos2,
    visible_layer_ids: &[u16],
    dragged_layer_ids: &[u16],
) -> Option<LayerDropTarget> {
    if !rect.expand(6.0).contains(pos) {
        return None;
    }
    let row_count = visible_layer_ids.len();
    if row_count == 0 {
        return None;
    }
    let camera_rows =
        usize::from(q0rg_id == project.meta.entry_q0rg_id && project.runtime.camera.enabled);
    let first_row_y = rect.min.y + ROW_H * (camera_rows as f32 + 1.0);
    let raw_row = ((pos.y - first_row_y) / ROW_H).floor() as isize;
    if raw_row >= row_count as isize {
        return None;
    }
    let row_index = raw_row.max(0) as usize;
    let target_id = visible_layer_ids[row_index];
    if dragged_layer_ids.is_empty()
        || dragged_layer_ids.iter().any(|dragged_layer_id| {
            target_id == *dragged_layer_id
                || project.layer_is_descendant_of(q0rg_id, target_id, *dragged_layer_id)
        })
    {
        return None;
    }
    let target_metadata = project.layer_metadata(q0rg_id, target_id);

    let row_y = first_row_y + row_index as f32 * ROW_H;
    let row_fraction = ((pos.y - row_y) / ROW_H).clamp(0.0, 1.0);
    let requested_parent =
        layer_parent_for_drop_lane(project, q0rg_id, target_id, pos.x, rect.min.x);

    if matches!(
        target_metadata.kind,
        q0s_format::v2::LayerKind::Folder | q0s_format::v2::LayerKind::Mask
    ) {
        if (0.25..=0.75).contains(&row_fraction) {
            return Some(match target_metadata.kind {
                q0s_format::v2::LayerKind::Folder => LayerDropTarget::IntoFolder(target_id),
                q0s_format::v2::LayerKind::Mask => LayerDropTarget::IntoMask(target_id),
                q0s_format::v2::LayerKind::Normal => return None,
            });
        }
        return Some(if row_fraction < 0.5 {
            LayerDropTarget::Before {
                layer_id: target_id,
                parent_folder_id: requested_parent,
            }
        } else {
            LayerDropTarget::After {
                layer_id: target_id,
                parent_folder_id: requested_parent,
            }
        });
    }

    Some(if row_fraction < 0.5 {
        LayerDropTarget::Before {
            layer_id: target_id,
            parent_folder_id: requested_parent,
        }
    } else {
        LayerDropTarget::After {
            layer_id: target_id,
            parent_folder_id: requested_parent,
        }
    })
}

fn draw_layer_drop_preview(
    painter: &egui::Painter,
    theme: &Theme,
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    rect: Rect,
    visible_layer_ids: &[u16],
    target: LayerDropTarget,
    label_width: f32,
) {
    let accent = theme.accent.to_color32();
    let camera_rows =
        usize::from(q0rg_id == project.meta.entry_q0rg_id && project.runtime.camera.enabled);
    match target {
        LayerDropTarget::IntoFolder(folder_id) | LayerDropTarget::IntoMask(folder_id) => {
            let Some(index) = visible_layer_ids
                .iter()
                .position(|layer_id| *layer_id == folder_id)
            else {
                return;
            };
            let row_y = rect.min.y + ROW_H * (index as f32 + camera_rows as f32 + 1.0);
            let highlight = Rect::from_min_size(
                pos2(rect.min.x + 1.0, row_y + 1.0),
                vec2(label_width - 2.0, ROW_H - 2.0),
            );
            painter.rect_filled(
                highlight,
                2.0,
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 42),
            );
            painter.rect_stroke(highlight, 2.0, Stroke::new(1.5_f32, accent));
        }
        LayerDropTarget::OutOfMask => return,
        LayerDropTarget::Before {
            layer_id,
            parent_folder_id,
        }
        | LayerDropTarget::After {
            layer_id,
            parent_folder_id,
        } => {
            let before = matches!(
                target,
                LayerDropTarget::Before {
                    layer_id: _,
                    parent_folder_id: _
                }
            );
            let Some(container_target_id) =
                layer_child_at_drop_parent(project, q0rg_id, layer_id, parent_folder_id)
            else {
                return;
            };
            let Some(target_index) = visible_layer_ids
                .iter()
                .position(|candidate| *candidate == container_target_id)
            else {
                return;
            };
            let line_index = if before {
                target_index
            } else if project.layer_is_folder(q0rg_id, container_target_id) {
                visible_layer_ids
                    .iter()
                    .enumerate()
                    .filter(|(_, candidate)| {
                        project.layer_is_descendant_of(q0rg_id, **candidate, container_target_id)
                    })
                    .map(|(index, _)| index)
                    .max()
                    .unwrap_or(target_index)
            } else {
                target_index
            };
            let line_after = !before;
            let y = rect.min.y
                + ROW_H * (line_index as f32 + camera_rows as f32 + 1.0)
                + if line_after { ROW_H } else { 0.0 };
            let indent = parent_folder_id
                .map(|parent_id| {
                    (project.layer_folder_depth(q0rg_id, parent_id) as f32 + 1.0) * 14.0
                })
                .unwrap_or(0.0);
            let x0 = rect.min.x + 4.0 + indent;
            let x1 = rect.min.x + label_width - 4.0;
            painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(2.0_f32, accent));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(x0, y),
                    pos2(x0 + 5.0, y - 3.5),
                    pos2(x0 + 5.0, y + 3.5),
                ],
                accent,
                Stroke::NONE,
            ));
        }
    }
}

fn timeline_context_menu(app: &mut EditorApp, ui: &mut egui::Ui) {
    if app.session.timeline_layer_selection.is_some() {
        timeline_layer_context_menu(app, ui);
    } else {
        timeline_frame_context_menu(app, ui);
    }
}

fn timeline_layer_context_menu(app: &mut EditorApp, ui: &mut egui::Ui) {
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let metadata = app.state.project.layer_metadata(q0rg_id, layer_id);
    let layer_name = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
        .map(|layer| layer.name.as_str())
        .unwrap_or("Layer");
    ui.label(egui::RichText::new(layer_name).strong().small());
    ui.separator();
    if ui.button("Rename layer").clicked() {
        begin_layer_rename(app, q0rg_id, layer_id);
        ui.close_menu();
    }
    if ui.button("Move up").clicked() {
        app.queue(Action::MoveLayer(q0rg_id, layer_id, -1));
        ui.close_menu();
    }
    if ui.button("Move down").clicked() {
        app.queue(Action::MoveLayer(q0rg_id, layer_id, 1));
        ui.close_menu();
    }
    if matches!(
        metadata.kind,
        q0s_format::v2::LayerKind::Folder | q0s_format::v2::LayerKind::Mask
    ) && ui
        .button(if metadata.collapsed {
            if metadata.kind == q0s_format::v2::LayerKind::Mask {
                "Show masked layers"
            } else {
                "Expand folder"
            }
        } else {
            if metadata.kind == q0s_format::v2::LayerKind::Mask {
                "Hide masked layers"
            } else {
                "Collapse folder"
            }
        })
        .clicked()
    {
        app.queue(Action::ToggleLayerFolder(q0rg_id, layer_id));
        ui.close_menu();
    }
    if metadata.parent_folder_id.is_some() {
        if ui.button("Move out of folder").clicked() {
            app.queue(Action::OutdentLayer(q0rg_id, layer_id));
            ui.close_menu();
        }
    } else if ui.button("Move into folder above").clicked() {
        app.queue(Action::IndentLayer(q0rg_id, layer_id));
        ui.close_menu();
    }
    if metadata.mask_layer_id.is_some() {
        if ui.button("Move out of mask").clicked() {
            app.queue(Action::DropLayer(
                q0rg_id,
                layer_id,
                LayerDropTarget::OutOfMask,
            ));
            ui.close_menu();
        }
    } else if let Some(mask_id) = mask_layer_above(&app.state.project, q0rg_id, layer_id) {
        if ui.button("Move into mask above").clicked() {
            app.queue(Action::DropLayer(
                q0rg_id,
                layer_id,
                LayerDropTarget::IntoMask(mask_id),
            ));
            ui.close_menu();
        }
    }
    if ui.button("Delete layer").clicked() {
        app.queue(Action::DeleteLayer(q0rg_id, layer_id));
        ui.close_menu();
    }

    if metadata.kind == q0s_format::v2::LayerKind::Folder {
        ui.separator();
        ui.label(
            egui::RichText::new("Folders cannot contain keyframes")
                .small()
                .italics(),
        );
        return;
    }
}

fn timeline_frame_context_menu(app: &mut EditorApp, ui: &mut egui::Ui) {
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let layer_name = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
        .map(|layer| layer.name.as_str())
        .unwrap_or("Layer");
    ui.label(egui::RichText::new(layer_name).strong().small());
    ui.separator();
    frame_label_context_menu(app, ui);
    ui.separator();
    if ui
        .add(crate::ui::shortcut_button(
            &app.settings.hotkeys,
            ShortcutCommand::Cut,
            "Cut",
        ))
        .clicked()
    {
        app.queue(Action::CutSelection);
        ui.close_menu();
    }
    if ui
        .add(crate::ui::shortcut_button(
            &app.settings.hotkeys,
            ShortcutCommand::Copy,
            "Copy",
        ))
        .clicked()
    {
        app.queue(Action::CopySelection);
        ui.close_menu();
    }
    if ui
        .add_enabled(
            app.has_clipboard(),
            crate::ui::shortcut_button(&app.settings.hotkeys, ShortcutCommand::Paste, "Paste"),
        )
        .clicked()
    {
        app.queue(Action::Paste);
        ui.close_menu();
    }
    ui.separator();
    let frame = app.session.timeline_frame() + 1;
    ui.label(
        egui::RichText::new(format!("Frame {frame}"))
            .strong()
            .small(),
    );
    let zero_frame = frame - 1;
    let has_frame_code = app
        .state
        .project
        .runtime
        .frame_scripts
        .iter()
        .any(|script| {
            script.q0rg_id == q0rg_id && script.layer_id == layer_id && script.frame == zero_frame
        });
    if ui
        .button(if has_frame_code {
            "Edit Frame Code..."
        } else {
            "Add Frame Code..."
        })
        .clicked()
    {
        app.queue(Action::OpenFrameScriptEditor(q0rg_id, layer_id, zero_frame));
        ui.close_menu();
    }
    if has_frame_code && ui.button("Delete Frame Code").clicked() {
        app.queue(Action::DeleteFrameScript(q0rg_id, layer_id, zero_frame));
        ui.close_menu();
    }
    ui.separator();
    if ui
        .add(crate::ui::shortcut_button(
            &app.settings.hotkeys,
            ShortcutCommand::InsertFrame,
            "Insert frame",
        ))
        .clicked()
    {
        app.queue(Action::InsertFrame);
        ui.close_menu();
    }
    if ui
        .add(crate::ui::shortcut_button(
            &app.settings.hotkeys,
            ShortcutCommand::InsertKeyframe,
            "Insert keyframe",
        ))
        .clicked()
    {
        app.queue(Action::InsertKeyframe);
        ui.close_menu();
    }
    if ui
        .add(crate::ui::shortcut_button(
            &app.settings.hotkeys,
            ShortcutCommand::InsertBlankKeyframe,
            "Insert blank keyframe",
        ))
        .clicked()
    {
        app.queue(Action::InsertBlankKeyframe);
        ui.close_menu();
    }
    if ui
        .add(crate::ui::shortcut_button(
            &app.settings.hotkeys,
            ShortcutCommand::ClearKeyframe,
            "Clear keyframes",
        ))
        .clicked()
    {
        app.queue(Action::ClearKeyframe);
        ui.close_menu();
    }
    if ui
        .add(crate::ui::shortcut_button(
            &app.settings.hotkeys,
            ShortcutCommand::RemoveFrame,
            "Remove selected frames",
        ))
        .on_hover_text(
            "Shorten the selected layers and close their gaps; other layers keep their frames",
        )
        .clicked()
    {
        app.queue(Action::RemoveSelectedTimelineFrames);
        ui.close_menu();
    }
    ui.separator();
    if ui
        .add(crate::ui::shortcut_button(
            &app.settings.hotkeys,
            ShortcutCommand::MotionTween,
            "Create motion tween",
        ))
        .clicked()
    {
        app.queue(Action::CreateMotionTween);
        ui.close_menu();
    }
}

fn rig_timeline_context_menu(app: &mut EditorApp, ui: &mut Ui) {
    let Some(selection) = app.session.timeline_rig_key_selection else {
        return;
    };
    let layer_name = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == selection.q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .find(|layer| layer.layer_id == selection.layer_id)
        })
        .map(|layer| layer.name.as_str())
        .unwrap_or("layer");
    let has_key = crate::timeline_edit::rig_key_frames_for_layer(
        &app.state.project,
        selection.q0rg_id,
        selection.layer_id,
    )
    .contains(&selection.frame);
    ui.label(
        egui::RichText::new(format!("rig: {layer_name}"))
            .strong()
            .small(),
    );
    ui.separator();
    ui.label(
        egui::RichText::new(format!("frame {}", u32::from(selection.frame) + 1))
            .strong()
            .small(),
    );
    if ui
        .add(crate::ui::shortcut_button(
            &app.settings.hotkeys,
            ShortcutCommand::InsertKeyframe,
            "Insert / update rig key",
        ))
        .clicked()
    {
        app.queue(Action::InsertKeyframe);
        ui.close_menu();
    }
    if ui
        .add_enabled(
            has_key,
            crate::ui::shortcut_button(
                &app.settings.hotkeys,
                ShortcutCommand::Delete,
                "Delete rig key",
            ),
        )
        .clicked()
    {
        app.queue(Action::DeleteSelection);
        ui.close_menu();
    }
    ui.separator();
    ui.label(
        egui::RichText::new("drag a diamond to move the rig key")
            .small()
            .italics(),
    );
}

fn draw_camera_layer_row(
    app: &EditorApp,
    painter: &egui::Painter,
    theme: &Theme,
    rect: Rect,
    row_y: f32,
    frame_count: u16,
    visible_frames: u16,
    label_width: f32,
) {
    let selected = app.session.camera_track_selected;
    let row_rect = Rect::from_min_size(pos2(rect.min.x, row_y), vec2(rect.width(), ROW_H));
    painter.rect_filled(row_rect, 0.0, theme.empty_beyond.to_color32());
    draw_empty_cells(
        painter,
        theme,
        rect.min.x + label_width,
        row_y,
        frame_count,
        visible_frames,
        selected,
    );

    let label_rect = Rect::from_min_size(pos2(rect.min.x, row_y), vec2(label_width, ROW_H));
    let base_label = if selected {
        theme.panel.to_color32()
    } else {
        blend(theme.panel.to_color32(), theme.window.to_color32(), 0.5)
    };
    painter.rect_filled(
        label_rect,
        0.0,
        blend(base_label, theme.accent.to_color32(), 0.10),
    );
    if selected {
        painter.rect_filled(
            label_rect.shrink(1.0),
            2.0,
            theme.accent.to_color32().gamma_multiply(0.22),
        );
        painter.rect_stroke(
            label_rect.shrink(1.0),
            2.0,
            Stroke::new(1.0_f32, theme.accent.to_color32()),
        );
    }
    let icon_rect =
        Rect::from_center_size(label_rect.left_center() + vec2(11.0, 0.0), vec2(13.0, 13.0));
    draw_layer_icon(
        painter,
        icon_rect,
        q0s_format::v2::LayerKind::Normal,
        false,
        theme.text.to_color32(),
        theme.accent.to_color32(),
    );
    painter.text(
        label_rect.left_center() + vec2(22.0, 0.0),
        Align2::LEFT_CENTER,
        "Camera",
        FontId::proportional(11.0),
        theme.text.to_color32(),
    );
    painter.line_segment(
        [
            pos2(rect.min.x + label_width, row_y),
            pos2(rect.min.x + label_width, row_y + ROW_H),
        ],
        Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
    );
    painter.line_segment(
        [
            pos2(rect.min.x, row_y + ROW_H),
            pos2(rect.min.x + rect.width(), row_y + ROW_H),
        ],
        Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
    );

    draw_camera_spans(
        painter,
        theme,
        &app.state.project.runtime.camera,
        rect.min.x + label_width,
        row_y,
        frame_count,
        selected,
    );

    if let Some(selection) = app.session.camera_timeline_selection.filter(|_| selected) {
        let first = selection
            .first_frame()
            .min(visible_frames.saturating_sub(1));
        let last = selection.last_frame().min(visible_frames.saturating_sub(1));
        if first <= last {
            let accent = timeline_selection_color(theme);
            let fill = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 54);
            let selected_rect = Rect::from_min_max(
                pos2(
                    rect.min.x + label_width + f32::from(first) * FRAME_W,
                    row_y + CELL_PAD_Y,
                ),
                pos2(
                    rect.min.x + label_width + (f32::from(last) + 1.0) * FRAME_W,
                    row_y + ROW_H - CELL_PAD_Y,
                ),
            );
            painter.rect_filled(selected_rect, 0.0, fill);
            painter.rect_stroke(selected_rect, 0.0, Stroke::new(1.0_f32, accent));
        }
    }
}

fn draw_camera_spans(
    painter: &egui::Painter,
    theme: &Theme,
    camera: &q0s_format::v2::CameraTrack,
    base_x: f32,
    row_y: f32,
    frame_count: u16,
    is_current_layer: bool,
) {
    let keyframes = camera
        .keys
        .iter()
        .map(|key| key.frame)
        .filter(|frame| *frame < frame_count)
        .collect::<Vec<_>>();
    let frame_slots = visible_frame_slots(painter, base_x, frame_count);
    if frame_slots.is_empty() {
        return;
    }
    let cell_top = row_y + CELL_PAD_Y;
    let cell_bot = row_y + ROW_H - CELL_PAD_Y;
    let static_base = if is_current_layer {
        theme.extension_active.to_color32()
    } else {
        theme.extension.to_color32()
    };
    let static_fill = blend(static_base, theme.accent.to_color32(), 0.12);
    let tween_fill = blend(
        theme.tween_fill.to_color32(),
        theme.accent.to_color32(),
        0.12,
    );
    let tween_arrow = blend(
        theme.tween_arrow.to_color32(),
        theme.accent.to_color32(),
        0.18,
    );
    let stroke_dark = theme.stroke_dark.to_color32();

    for (position, keyframe) in keyframes.iter().copied().enumerate() {
        let span_end = keyframes
            .get(position + 1)
            .copied()
            .map(|next| next.saturating_sub(1))
            .unwrap_or_else(|| frame_count.saturating_sub(1));
        if span_end < frame_slots.start || keyframe >= frame_slots.end || span_end < keyframe {
            continue;
        }
        let has_tween = camera
            .keys
            .iter()
            .find(|key| key.frame == keyframe)
            .and_then(|key| key.tween.to_frame())
            .is_some();
        let x0 = base_x + f32::from(keyframe) * FRAME_W;
        let x1 = base_x + (f32::from(span_end) + 1.0) * FRAME_W;
        painter.rect_filled(
            Rect::from_min_max(pos2(x0, cell_top), pos2(x1, cell_bot)),
            0.0,
            if has_tween { tween_fill } else { static_fill },
        );
        if !has_tween && span_end > keyframe {
            let cap_x = base_x + (f32::from(span_end) + 1.0) * FRAME_W - 1.0;
            painter.line_segment(
                [pos2(cap_x, cell_top + 1.0), pos2(cap_x, cell_bot - 1.0)],
                Stroke::new(1.0_f32, stroke_dark),
            );
            let glyph = Rect::from_min_size(pos2(cap_x - 5.0, cell_bot - 6.0), vec2(4.0, 4.0));
            painter.rect_stroke(glyph, 0.0, Stroke::new(1.0_f32, stroke_dark));
        }
    }
    for keyframe in
        keyframe_separator_frames(&keyframes).filter(|frame| frame_slots.contains(frame))
    {
        let x = base_x + f32::from(keyframe) * FRAME_W;
        painter.line_segment(
            [pos2(x, cell_top), pos2(x, cell_bot)],
            Stroke::new(1.0_f32, stroke_dark),
        );
    }
    for key in &camera.keys {
        let Some(to_frame) = key.tween.to_frame() else {
            continue;
        };
        if key.frame >= frame_slots.end || to_frame < frame_slots.start || to_frame >= frame_count {
            continue;
        }
        let from_x = base_x + (f32::from(key.frame) + 0.5) * FRAME_W + 4.0;
        let to_x = base_x + (f32::from(to_frame) + 0.5) * FRAME_W - 4.0;
        let mid_y = row_y + ROW_H * 0.5;
        if to_x > from_x + 2.0 {
            painter.line_segment(
                [pos2(from_x, mid_y), pos2(to_x, mid_y)],
                Stroke::new(1.0_f32, tween_arrow),
            );
            painter.line_segment(
                [pos2(to_x, mid_y), pos2(to_x - 3.0, mid_y - 2.5)],
                Stroke::new(1.0_f32, tween_arrow),
            );
            painter.line_segment(
                [pos2(to_x, mid_y), pos2(to_x - 3.0, mid_y + 2.5)],
                Stroke::new(1.0_f32, tween_arrow),
            );
        }
    }
    let keyframe_y = row_y + ROW_H - 5.0;
    let keyframe_color = blend(theme.keyframe.to_color32(), theme.accent.to_color32(), 0.22);
    for keyframe in keyframes
        .into_iter()
        .filter(|frame| frame_slots.contains(frame))
    {
        let x = base_x + (f32::from(keyframe) + 0.5) * FRAME_W;
        painter.circle_filled(pos2(x, keyframe_y), 2.8, keyframe_color);
    }
}

fn transport_bar(app: &mut EditorApp, theme: &Theme, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Timeline").strong());
        ui.separator();
        let camera_available = app.session.current_q0rg_id == app.state.project.meta.entry_q0rg_id;
        let camera_enabled = app.state.project.runtime.camera.enabled;
        let camera_response = ui
            .add_enabled_ui(camera_available, |ui| {
                icon_toggle(
                    ui,
                    Icon::Camera,
                    "Enable or disable the project camera track",
                    camera_enabled,
                )
            })
            .inner;
        if camera_response.clicked() {
            let before = app.state.project.clone();
            app.state.project.runtime.camera.enabled = !camera_enabled;
            app.history.snapshot(&before);
            app.state.mark_dirty();
            app.session.camera_track_selected = app.state.project.runtime.camera.enabled;
            app.session.timeline_selection = None;
            app.session.timeline_layer_selection = None;
            app.session.selection = Selection::None;
            app.session.status = if app.state.project.runtime.camera.enabled {
                "camera track enabled".to_string()
            } else {
                "camera track disabled".to_string()
            };
        }
        ui.separator();
        let play_label = app.settings.hotkeys.hint(
            ShortcutCommand::PlaybackToggle,
            if app.session.playing { "Pause" } else { "Play" },
        );
        if icon_toggle(
            ui,
            if app.session.playing {
                Icon::Pause
            } else {
                Icon::Play
            },
            &play_label,
            app.session.playing,
        )
        .clicked()
        {
            app.queue(Action::TogglePlay);
        }
        if icon_button(
            ui,
            Icon::First,
            &app.settings
                .hotkeys
                .hint(ShortcutCommand::FirstFrame, "First frame"),
        )
        .clicked()
        {
            app.queue(Action::FirstFrame);
        }
        if icon_button(
            ui,
            Icon::Previous,
            &app.settings
                .hotkeys
                .hint(ShortcutCommand::PreviousFrame, "Previous frame"),
        )
        .clicked()
        {
            app.queue(Action::PreviousFrame);
        }
        if icon_button(
            ui,
            Icon::Next,
            &app.settings
                .hotkeys
                .hint(ShortcutCommand::NextFrame, "Next frame"),
        )
        .clicked()
        {
            app.queue(Action::NextFrame);
        }
        if icon_button(
            ui,
            Icon::Last,
            &app.settings
                .hotkeys
                .hint(ShortcutCommand::LastFrame, "Last frame"),
        )
        .clicked()
        {
            app.queue(Action::LastFrame);
        }
        ui.separator();
        let frame_count = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == app.session.current_q0rg_id)
            .map(|q| q.frame_count)
            .unwrap_or(1);
        let max = frame_count
            .saturating_add(VIRTUAL_FRAMES_BUFFER)
            .saturating_sub(1);
        let mut displayed_frame = u32::from(app.session.timeline_frame()) + 1;
        if ui
            .add(
                egui::DragValue::new(&mut displayed_frame)
                    .clamp_range(1..=u32::from(max) + 1)
                    .prefix("frame ")
                    .speed(0.25),
            )
            .on_hover_text("Current frame; drag or type a frame number")
            .changed()
        {
            app.session
                .set_timeline_frame((displayed_frame - 1) as u16, frame_count);
        }
        ui.menu_button("labels", |ui| {
            frame_label_context_menu(app, ui);
            let mut labels = app
                .state
                .project
                .runtime
                .frame_labels
                .iter()
                .filter(|(key, _)| key.q0rg_id == app.session.current_q0rg_id)
                .map(|(key, label)| (key.frame, label.clone()))
                .collect::<Vec<_>>();
            labels.sort_by_key(|(frame, _)| *frame);
            if !labels.is_empty() {
                ui.separator();
                for (frame, label) in labels {
                    if ui
                        .button(format!("{}  {label}", u32::from(frame) + 1))
                        .clicked()
                    {
                        app.session.set_timeline_frame(frame, frame_count);
                        ui.close_menu();
                    }
                }
            }
        });
        if let Some(label) = app
            .state
            .project
            .runtime
            .frame_label(app.session.current_q0rg_id, app.session.timeline_frame())
        {
            ui.label(
                egui::RichText::new(label)
                    .small()
                    .color(theme.accent.to_color32()),
            );
        }
        ui.separator();

        // Onion skin: toggle (SelectableLabel highlights when on) + before/
        // after spinners (visible only while it's on so the bar stays compact
        // otherwise).
        if icon_toggle(
            ui,
            Icon::Onion,
            "Onion skin: show neighbouring frames on the active layer",
            app.session.onion.enabled,
        )
        .clicked()
        {
            app.session.onion.enabled = !app.session.onion.enabled;
        }
        if app.session.onion.enabled {
            ui.label(
                egui::RichText::new("Before")
                    .color(theme.text_dim.to_color32())
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut app.session.onion.before)
                    .clamp_range(0..=6)
                    .speed(0.1),
            )
            .on_hover_text("Frames to ghost before current");
            ui.label(
                egui::RichText::new("After")
                    .color(theme.text_dim.to_color32())
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut app.session.onion.after)
                    .clamp_range(0..=6)
                    .speed(0.1),
            )
            .on_hover_text("Frames to ghost after current");
        }
        ui.separator();
        ui.label(
            egui::RichText::new(format!("{} fps", app.state.project.meta.fps))
                .color(theme.text_dim.to_color32()),
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().button_padding = vec2(8.0, 4.0);
        let selected_rig_key_exists = app
            .session
            .timeline_rig_key_selection
            .filter(|selection| selection.q0rg_id == app.session.current_q0rg_id)
            .is_some_and(|selection| {
                crate::timeline_edit::rig_key_frames_for_layer(
                    &app.state.project,
                    selection.q0rg_id,
                    selection.layer_id,
                )
                .contains(&selection.frame)
            });
        let has_selected_frame_cells = app.session.timeline_selection.is_some()
            || app
                .session
                .timeline_cell_selection
                .as_ref()
                .is_some_and(|(_, cells)| !cells.is_empty());
        let can_delete = selected_rig_key_exists
            || app.session.timeline_selection.is_some()
            || app
                .session
                .timeline_cell_selection
                .as_ref()
                .is_some_and(|(_, cells)| !cells.is_empty())
            || (app.camera_track_is_selected()
                && app
                    .state
                    .project
                    .runtime
                    .camera
                    .has_keyframe(app.session.current_frame))
            || super::menu::selection_can_be_deleted(&app.session.selection);
        let delete_hint = if has_selected_frame_cells {
            app.settings.hotkeys.hint(
                ShortcutCommand::RemoveFrame,
                "Remove selected frames from these layers",
            )
        } else {
            app.settings
                .hotkeys
                .hint(ShortcutCommand::Delete, "Delete selection")
        };
        if icon_button_enabled(
            ui,
            can_delete,
            if has_selected_frame_cells {
                Icon::RemoveFrames
            } else {
                Icon::Delete
            },
            &delete_hint,
        )
        .clicked()
        {
            if has_selected_frame_cells {
                app.queue(Action::RemoveSelectedTimelineFrames);
            } else {
                app.queue(Action::DeleteSelection);
            }
        }
        ui.separator();
        if icon_button(ui, Icon::Layer, "Add layer").clicked() {
            app.queue(Action::AddLayer);
        }
        if icon_button(ui, Icon::Folder, "Add layer folder").clicked() {
            app.queue(Action::AddLayerFolder);
        }
        if icon_button(ui, Icon::Mask, "Add a mask for the selected layer").clicked() {
            app.queue(Action::AddLayerMask);
        }
        let q0rg_id = app.session.current_q0rg_id;
        let layer_id = app.session.current_layer_id;
        let rig_track_selected = app
            .session
            .timeline_rig_key_selection
            .is_some_and(|selection| selection.q0rg_id == q0rg_id)
            && matches!(&app.session.selection, Selection::None);
        if layer_move_icon_button(ui, LayerMoveIcon::Up, "Move layer up").clicked() {
            app.queue(Action::MoveLayer(q0rg_id, layer_id, -1));
        }
        if layer_move_icon_button(ui, LayerMoveIcon::Down, "Move layer down").clicked() {
            app.queue(Action::MoveLayer(q0rg_id, layer_id, 1));
        }
        if layer_move_icon_button(ui, LayerMoveIcon::Indent, "Move layer into folder above")
            .clicked()
        {
            app.queue(Action::IndentLayer(q0rg_id, layer_id));
        }
        if layer_move_icon_button(ui, LayerMoveIcon::Outdent, "Move layer out of folder").clicked()
        {
            app.queue(Action::OutdentLayer(q0rg_id, layer_id));
        }
        if icon_button(ui, Icon::Symbol, "Create q0rg symbol").clicked() {
            app.queue(Action::AddQ0rg);
        }
        ui.separator();
        if icon_button(
            ui,
            Icon::Frame,
            &app.settings
                .hotkeys
                .hint(ShortcutCommand::InsertFrame, "Insert hold frame"),
        )
        .clicked()
        {
            app.queue(Action::InsertFrame);
        }
        let keyframes_enabled = rig_track_selected
            || app.camera_track_is_selected()
            || app.session.timeline_selection.is_some()
            || !app
                .state
                .project
                .layer_is_folder(app.session.current_q0rg_id, app.session.current_layer_id);
        if icon_button_enabled(
            ui,
            keyframes_enabled,
            Icon::Keyframe,
            &app.settings.hotkeys.hint(
                ShortcutCommand::InsertKeyframe,
                if rig_track_selected {
                    "Insert or update rig key"
                } else {
                    "Insert keyframe"
                },
            ),
        )
        .clicked()
        {
            app.queue(Action::InsertKeyframe);
        }
        if icon_button_enabled(
            ui,
            keyframes_enabled && !app.camera_track_is_selected() && !rig_track_selected,
            Icon::BlankKeyframe,
            &app.settings.hotkeys.hint(
                ShortcutCommand::InsertBlankKeyframe,
                "Insert blank keyframe",
            ),
        )
        .clicked()
        {
            app.queue(Action::InsertBlankKeyframe);
        }
        if icon_button(
            ui,
            Icon::RemoveColumns,
            "Remove time columns across all layers and close the gap",
        )
        .clicked()
        {
            app.queue(Action::RemoveFrame);
        }
        let can_tween = (app.session.timeline_selection.is_some()
            || app.camera_track_is_selected())
            && app.session.pending_timeline_frame.is_none();
        if icon_button_enabled(
            ui,
            can_tween,
            Icon::Tween,
            &app.settings.hotkeys.hint(
                ShortcutCommand::MotionTween,
                "Create motion tween between keyframes",
            ),
        )
        .clicked()
        {
            app.queue(Action::CreateMotionTween);
        }
    });
}

fn begin_layer_rename(app: &mut EditorApp, q0rg_id: u16, layer_id: u16) {
    let Some(draft) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
        .map(|layer| layer.name.clone())
    else {
        return;
    };
    app.session.layer_rename = Some(LayerRename {
        q0rg_id,
        layer_id,
        draft,
        focus_requested: true,
    });
}

fn render_layer_rename_dialog(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(mut rename) = app.session.layer_rename.take() else {
        return;
    };
    let mut open = true;
    let mut submit = false;
    let mut cancel = false;
    egui::Window::new("Rename layer")
        .id(egui::Id::new("timeline_layer_rename_dialog"))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut rename.draft)
                    .desired_width(280.0)
                    .hint_text("Layer name"),
            );
            if rename.focus_requested {
                response.request_focus();
                rename.focus_requested = false;
            }
            if (response.has_focus() || response.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                && !rename.draft.trim().is_empty()
            {
                submit = true;
            }
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!rename.draft.trim().is_empty(), egui::Button::new("Rename"))
                    .clicked()
                {
                    submit = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if submit && open && !cancel && !rename.draft.trim().is_empty() {
        app.queue(Action::RenameLayer(
            rename.q0rg_id,
            rename.layer_id,
            rename.draft,
        ));
    } else if open && !cancel {
        app.session.layer_rename = Some(rename);
    }
}

fn frame_label_context_menu(app: &mut EditorApp, ui: &mut Ui) {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.timeline_frame();
    let valid = app
        .state
        .project
        .q0rgs
        .iter()
        .any(|q| q.q0rg_id == q0rg_id && frame < q.frame_count);
    let existing = app
        .state
        .project
        .runtime
        .frame_label(q0rg_id, frame)
        .map(str::to_owned);
    if ui
        .add_enabled(
            valid,
            egui::Button::new(if existing.is_some() {
                "edit frame label..."
            } else {
                "add frame label..."
            }),
        )
        .clicked()
    {
        app.session.frame_label_edit = Some(FrameLabelEdit {
            q0rg_id,
            frame,
            draft: existing.clone().unwrap_or_default(),
            focus_requested: true,
        });
        ui.close_menu();
    }
    if existing.is_some() && ui.button("remove frame label").clicked() {
        app.queue(Action::SetFrameLabel(q0rg_id, frame, None));
        ui.close_menu();
    }
}

fn render_frame_label_dialog(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(mut edit) = app.session.frame_label_edit.take() else {
        return;
    };
    if !app
        .state
        .project
        .q0rgs
        .iter()
        .any(|q| q.q0rg_id == edit.q0rg_id && edit.frame < q.frame_count)
    {
        return;
    }
    let mut open = true;
    let mut submit = false;
    let mut cancel = false;
    egui::Window::new(format!("frame {} label", u32::from(edit.frame) + 1))
        .id(egui::Id::new("timeline_frame_label_dialog"))
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut edit.draft)
                    .desired_width(300.0)
                    .hint_text("intro"),
            );
            if edit.focus_requested {
                response.request_focus();
                edit.focus_requested = false;
            }
            let name = edit.draft.trim();
            let error = q0s_format::v2::validate_frame_label_name(name)
                .err()
                .map(|error| error.to_string())
                .or_else(|| {
                    app.state
                        .project
                        .runtime
                        .resolve_frame_label(edit.q0rg_id, name)
                        .filter(|frame| *frame != edit.frame)
                        .map(|frame| {
                            format!(
                                "this label is already used on frame {}",
                                u32::from(frame) + 1
                            )
                        })
                });
            ui.label("names belong to this symbol. update script targets after renaming.");
            if let Some(error) = &error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if response.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                && error.is_none()
            {
                submit = true;
            }
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(error.is_none(), egui::Button::new("save label"))
                    .clicked()
                {
                    submit = true;
                }
                if ui.button("cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if submit && open && !cancel {
        app.queue(Action::SetFrameLabel(
            edit.q0rg_id,
            edit.frame,
            Some(edit.draft),
        ));
    } else if open && !cancel {
        app.session.frame_label_edit = Some(edit);
    }
}

fn draw_visibility_icon(painter: &egui::Painter, rect: Rect, visible: bool, color: Color32) {
    let center = rect.center();
    let half_w = (rect.width() * 0.47).min(6.0);
    let half_h = (rect.height() * 0.30).min(3.0);
    let left = pos2(center.x - half_w, center.y);
    let right = pos2(center.x + half_w, center.y);
    let active = color;
    let dim = color.gamma_multiply(0.42);

    if visible {
        let upper = vec![
            left,
            pos2(center.x - half_w * 0.56, center.y - half_h * 0.82),
            pos2(center.x, center.y - half_h),
            pos2(center.x + half_w * 0.56, center.y - half_h * 0.82),
            right,
        ];
        let lower = vec![
            left,
            pos2(center.x - half_w * 0.56, center.y + half_h * 0.82),
            pos2(center.x, center.y + half_h),
            pos2(center.x + half_w * 0.56, center.y + half_h * 0.82),
            right,
        ];
        painter.add(egui::Shape::line(upper, Stroke::new(1.15_f32, active)));
        painter.add(egui::Shape::line(lower, Stroke::new(1.15_f32, active)));
        painter.circle_filled(center, 1.8, active);
    } else {
        let eyelid = vec![
            left,
            pos2(center.x - half_w * 0.52, center.y + half_h * 0.45),
            pos2(center.x, center.y + half_h * 0.62),
            pos2(center.x + half_w * 0.52, center.y + half_h * 0.45),
            right,
        ];
        painter.add(egui::Shape::line(eyelid, Stroke::new(1.15_f32, dim)));
        let lash_y = center.y + half_h * 0.5;
        for x in [-0.50_f32, 0.0, 0.50] {
            let px = center.x + half_w * x;
            painter.line_segment(
                [pos2(px, lash_y), pos2(px, lash_y + 1.6)],
                Stroke::new(0.9_f32, dim),
            );
        }
    }
}

fn draw_mask_art_visibility_icon(
    painter: &egui::Painter,
    rect: Rect,
    visible: bool,
    color: Color32,
) {
    let center = rect.center();
    // A square stencil with a solid diamond has a different silhouette from
    // the ordinary eye. The diagonal slash means "preview clipping".
    let square = Rect::from_center_size(center, vec2(11.0, 11.0));
    let stroke = Stroke::new(1.3_f32, color);
    painter.rect_stroke(square, 1.0, stroke);
    let points = vec![
        center - vec2(0.0, 3.1),
        center + vec2(3.1, 0.0),
        center + vec2(0.0, 3.1),
        center - vec2(3.1, 0.0),
    ];
    if visible {
        painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    } else {
        painter.add(egui::Shape::closed_line(
            points,
            Stroke::new(0.9_f32, color.gamma_multiply(0.4)),
        ));
        painter.line_segment(
            [
                square.left_bottom() + vec2(-1.0, 1.0),
                square.right_top() + vec2(1.0, -1.0),
            ],
            Stroke::new(1.9_f32, color),
        );
    }
}

fn draw_outline_icon(painter: &egui::Painter, rect: Rect, enabled: bool, color: Color32) {
    let stroke_color = if enabled {
        color
    } else {
        color.gamma_multiply(0.42)
    };
    let stroke = Stroke::new(1.15_f32, stroke_color);
    let corner = (rect.width().min(rect.height()) * 0.34).clamp(2.5, 3.5);
    let min = rect.min;
    let max = rect.max;
    for (x, y, sx, sy) in [
        (min.x, min.y, 1.0, 1.0),
        (max.x, min.y, -1.0, 1.0),
        (max.x, max.y, -1.0, -1.0),
        (min.x, max.y, 1.0, -1.0),
    ] {
        painter.line_segment([pos2(x, y), pos2(x + sx * corner, y)], stroke);
        painter.line_segment([pos2(x, y), pos2(x, y + sy * corner)], stroke);
    }
    if enabled {
        painter.circle_filled(rect.center(), 1.15, stroke_color);
    }
}

fn draw_lock_icon(painter: &egui::Painter, rect: Rect, locked: bool, color: Color32) {
    let center = rect.center();
    let active = color;
    let dim = color.gamma_multiply(0.42);
    let stroke_color = if locked { active } else { dim };
    let stroke = Stroke::new(1.1_f32, stroke_color);

    let body_w = (rect.width() * 0.58).clamp(6.5, 8.0);
    let body_h = (rect.height() * 0.42).clamp(4.5, 5.8);
    let body = Rect::from_center_size(pos2(center.x, center.y + 2.2), vec2(body_w, body_h));
    painter.rect_stroke(body, 1.3, stroke);

    let shackle_half_w = body_w * 0.31;
    let shackle_top = body.top() - 4.0;
    let shackle_bottom = body.top() + 0.6;
    if locked {
        let shackle = vec![
            pos2(center.x - shackle_half_w, shackle_bottom),
            pos2(center.x - shackle_half_w, shackle_top + 1.6),
            pos2(center.x - shackle_half_w * 0.55, shackle_top + 0.4),
            pos2(center.x, shackle_top),
            pos2(center.x + shackle_half_w * 0.55, shackle_top + 0.4),
            pos2(center.x + shackle_half_w, shackle_top + 1.6),
            pos2(center.x + shackle_half_w, shackle_bottom),
        ];
        painter.add(egui::Shape::line(shackle, stroke));
        painter.circle_filled(pos2(center.x, body.center().y - 0.4), 1.0, active);
    } else {
        let open_center_x = center.x + 1.0;
        let shackle = vec![
            pos2(center.x - shackle_half_w + 0.3, shackle_bottom),
            pos2(center.x - shackle_half_w + 0.3, shackle_top + 1.7),
            pos2(open_center_x - shackle_half_w * 0.45, shackle_top + 0.5),
            pos2(open_center_x + 0.2, shackle_top),
            pos2(open_center_x + shackle_half_w * 0.72, shackle_top + 0.8),
        ];
        painter.add(egui::Shape::line(shackle, stroke));
    }
}

fn draw_layer_icon(
    painter: &egui::Painter,
    rect: Rect,
    kind: q0s_format::v2::LayerKind,
    collapsed: bool,
    foreground: Color32,
    accent: Color32,
) {
    if kind == q0s_format::v2::LayerKind::Folder {
        let body = Rect::from_min_max(pos2(rect.left(), rect.top() + 3.0), rect.right_bottom());
        let tab = Rect::from_min_max(
            rect.left_top(),
            pos2(rect.center().x + 1.0, rect.top() + 4.5),
        );
        painter.rect_filled(
            body,
            1.5,
            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 45),
        );
        painter.rect_stroke(body, 1.5, Stroke::new(1.1_f32, accent));
        painter.rect_filled(tab, 1.0, accent);
        let center = body.center();
        let arrow = if collapsed {
            [
                pos2(center.x - 1.5, center.y - 2.5),
                pos2(center.x + 2.0, center.y),
                pos2(center.x - 1.5, center.y + 2.5),
            ]
        } else {
            [
                pos2(center.x - 2.5, center.y - 1.5),
                pos2(center.x, center.y + 2.0),
                pos2(center.x + 2.5, center.y - 1.5),
            ]
        };
        painter.add(egui::Shape::convex_polygon(
            arrow.to_vec(),
            foreground,
            Stroke::NONE,
        ));
    } else if kind == q0s_format::v2::LayerKind::Mask {
        let frame = rect.shrink2(vec2(1.0, 2.0));
        painter.rect_stroke(frame, 1.0, Stroke::new(1.1_f32, accent));
        let points = [
            pos2(frame.left() + 1.0, frame.bottom() - 1.0),
            pos2(frame.left() + 1.0, frame.top() + 2.0),
            pos2(frame.right() - 1.0, frame.top() + 2.0),
            pos2(frame.center().x, frame.bottom() - 1.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            points.to_vec(),
            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 50),
            Stroke::new(1.0_f32, foreground),
        ));
        painter.line_segment(
            [
                pos2(frame.left() + 2.0, frame.bottom() - 1.0),
                pos2(frame.right() - 1.0, frame.bottom() - 1.0),
            ],
            Stroke::new(1.0_f32, accent),
        );
        let arrow_center = pos2(frame.right() - 3.0, frame.bottom() - 2.5);
        let arrow = if collapsed {
            [
                pos2(arrow_center.x - 1.0, arrow_center.y - 1.5),
                pos2(arrow_center.x + 1.0, arrow_center.y),
                pos2(arrow_center.x - 1.0, arrow_center.y + 1.5),
            ]
        } else {
            [
                pos2(arrow_center.x - 1.5, arrow_center.y - 1.0),
                pos2(arrow_center.x, arrow_center.y + 1.0),
                pos2(arrow_center.x + 1.5, arrow_center.y - 1.0),
            ]
        };
        painter.add(egui::Shape::convex_polygon(
            arrow.to_vec(),
            foreground,
            Stroke::NONE,
        ));
    } else {
        for offset in [0.0_f32, 3.0, 6.0] {
            painter.line_segment(
                [
                    pos2(rect.left() + 1.0, rect.top() + 2.0 + offset),
                    pos2(rect.right() - 1.0, rect.top() + 2.0 + offset),
                ],
                Stroke::new(
                    if offset == 0.0 { 1.5_f32 } else { 1.0_f32 },
                    if offset == 0.0 { accent } else { foreground },
                ),
            );
        }
    }
}

/// Paint per-frame base cells before any span/keyframe glyphs go on top.
/// Three states:
///   * inside `frame_count`, no placement covering this frame    РІвЂ вЂ™ mid grey
///     (active layer is a touch brighter so it's clear which row drawing
///     will land in).
///   * beyond `frame_count`                                       РІвЂ вЂ™ very dark
///   * beyond + every-5th                                          РІвЂ вЂ™ slightly
///     brighter dark, producing a chequered "virtual frames" band.
fn draw_empty_cells(
    painter: &egui::Painter,
    theme: &Theme,
    base_x: f32,
    row_y: f32,
    frame_count: u16,
    visible_frames: u16,
    is_current_layer: bool,
) {
    let cell_top = row_y + CELL_PAD_Y;
    let cell_bot = row_y + ROW_H - CELL_PAD_Y;
    let inside = if is_current_layer {
        theme.empty_inside_active.to_color32()
    } else {
        theme.empty_inside.to_color32()
    };
    let beyond = theme.empty_beyond.to_color32();
    let beyond_5 = theme.empty_beyond_5.to_color32();
    for f in visible_frame_slots(painter, base_x, visible_frames) {
        let x0 = base_x + (f as f32) * FRAME_W;
        let x1 = x0 + FRAME_W;
        let cell = Rect::from_min_max(pos2(x0, cell_top), pos2(x1, cell_bot));
        let fill = if f < frame_count {
            inside
        } else if f % 5 == 4 {
            beyond_5
        } else {
            beyond
        };
        painter.rect_filled(cell, 0.0, fill);
    }
}

fn draw_frame_script_markers(
    project: &q0s_format::v2::ProjectV2,
    painter: &egui::Painter,
    theme: &Theme,
    q0rg_id: u16,
    layer_id: u16,
    layout: AudioWaveformLayout,
) {
    let color = theme.accent.to_color32();
    let frame_slots = visible_frame_slots(painter, layout.base_x, layout.frame_count);
    for script in &project.runtime.frame_scripts {
        if script.q0rg_id != q0rg_id
            || script.layer_id != layer_id
            || !frame_slots.contains(&script.frame)
        {
            continue;
        }
        let x = layout.base_x + f32::from(script.frame) * FRAME_W + FRAME_W * 0.5;
        painter.text(
            pos2(x, layout.row_y + 3.0),
            Align2::CENTER_TOP,
            "{}",
            FontId::monospace(7.0),
            color,
        );
    }
}

#[derive(Debug, Clone, Copy)]
struct AudioWaveformLayout {
    base_x: f32,
    row_y: f32,
    frame_count: u16,
}

fn draw_audio_waveforms(
    project: &q0s_format::v2::ProjectV2,
    textures: &mut crate::render::TextureCache,
    painter: &egui::Painter,
    theme: &Theme,
    q0rg_id: u16,
    layer: &q0s_format::v2::Layer,
    layout: AudioWaveformLayout,
) {
    let AudioWaveformLayout {
        base_x,
        row_y,
        frame_count,
    } = layout;
    let center_y = row_y + ROW_H * 0.5;
    let amplitude = (ROW_H - CELL_PAD_Y * 2.0 - 7.0).max(2.0) * 0.5;
    let clip_rect = painter.clip_rect();
    for (clip_idx, timeline_clip) in project.audio_clips.iter().enumerate() {
        if timeline_clip.q0rg_id != q0rg_id || timeline_clip.layer_id != layer.layer_id {
            continue;
        }
        let Some(clip) = crate::audio::audio_clip_ref(project, clip_idx) else {
            continue;
        };
        let end = clip.end_frame_exclusive.min(frame_count);
        if end <= clip.start_frame {
            continue;
        }
        let clip_left = base_x + f32::from(clip.start_frame) * FRAME_W;
        let clip_right = base_x + f32::from(end) * FRAME_W;
        let visible_left = clip_left.max(clip_rect.left());
        let visible_right = clip_right.min(clip_rect.right());
        if visible_right <= visible_left {
            continue;
        }
        let Some((waveform, waveform_complete)) = textures.audio_waveform(project, clip.asset_id)
        else {
            continue;
        };
        if !waveform_complete {
            painter.ctx().request_repaint();
        }
        if waveform.bins.is_empty() {
            continue;
        }
        let color = if timeline_clip.muted {
            theme.text_dim.to_color32().gamma_multiply(0.45)
        } else {
            theme.accent.to_color32().gamma_multiply(0.82)
        };
        let visible_width = visible_right - visible_left;
        let sample_columns = ((visible_width / 2.0).ceil() as usize).clamp(1, 4096);
        let clip_width = (clip_right - clip_left).max(1.0);
        for column in 0..sample_columns {
            let x = visible_left + (column as f32 + 0.5) * visible_width / sample_columns as f32;
            let clip_fraction = ((x - clip_left) / clip_width).clamp(0.0, 0.999_999);
            let bin = (clip_fraction * waveform.bins.len() as f32) as usize;
            let (min, max) = waveform.bins[bin.min(waveform.bins.len() - 1)];
            let y_top = center_y - max.clamp(-1.0, 1.0) * amplitude;
            let y_bottom = center_y - min.clamp(-1.0, 1.0) * amplitude;
            painter.line_segment(
                [pos2(x, y_top), pos2(x, y_bottom)],
                Stroke::new(1.0_f32, color),
            );
        }
        painter.line_segment(
            [
                pos2(visible_left, center_y),
                pos2(visible_right.max(visible_left), center_y),
            ],
            Stroke::new(0.65_f32, color.gamma_multiply(0.7)),
        );
    }
}

fn draw_layer_spans(
    painter: &egui::Painter,
    theme: &Theme,
    layer: &q0s_format::v2::Layer,
    base_x: f32,
    row_y: f32,
    frame_count: u16,
    is_current_layer: bool,
) {
    let layer_frame_count = layer
        .timeline_frame_count
        .unwrap_or(frame_count)
        .min(frame_count);
    let keyframes = visible_keyframes(layer, layer_frame_count);
    let frame_slots = visible_frame_slots(painter, base_x, layer_frame_count);
    if frame_slots.is_empty() {
        return;
    }

    let cell_top = row_y + CELL_PAD_Y;
    let cell_bot = row_y + ROW_H - CELL_PAD_Y;
    let static_fill = if is_current_layer {
        theme.extension_active.to_color32()
    } else {
        theme.extension.to_color32()
    };
    let tween_fill = theme.tween_fill.to_color32();
    let tween_arrow = theme.tween_arrow.to_color32();
    let stroke_dark = theme.stroke_dark.to_color32();

    for (position, keyframe) in keyframes.iter().copied().enumerate() {
        // This row can end before the q0rg. Keep the final hold inside that
        // layer's own timeline so cells after it remain beyond cells.
        let span_end = keyframes
            .get(position + 1)
            .copied()
            .map(|next| next.saturating_sub(1))
            .unwrap_or_else(|| layer_frame_count.saturating_sub(1));
        if span_end < frame_slots.start
            || keyframe >= frame_slots.end
            || span_end < keyframe
            || layer.is_blank_keyframe(keyframe)
        {
            continue;
        }
        let has_tween = layer
            .placements
            .iter()
            .any(|placement| placement.frame == keyframe && placement.tween.to_frame().is_some());
        let x0 = base_x + f32::from(keyframe) * FRAME_W;
        let x1 = base_x + (f32::from(span_end) + 1.0) * FRAME_W;
        painter.rect_filled(
            egui::Rect::from_min_max(pos2(x0, cell_top), pos2(x1, cell_bot)),
            0.0,
            if has_tween { tween_fill } else { static_fill },
        );
        if !has_tween && span_end > keyframe {
            let cap_x = base_x + (f32::from(span_end) + 1.0) * FRAME_W - 1.0;
            painter.line_segment(
                [pos2(cap_x, cell_top + 1.0), pos2(cap_x, cell_bot - 1.0)],
                Stroke::new(1.0_f32, stroke_dark),
            );
            let glyph =
                egui::Rect::from_min_size(pos2(cap_x - 5.0, cell_bot - 6.0), vec2(4.0, 4.0));
            painter.rect_stroke(glyph, 0.0, Stroke::new(1.0_f32, stroke_dark));
        }
    }

    // Every keyframe starts a new span. Draw the left edge explicitly so two
    // adjacent keys never visually fuse into one long cell.
    for keyframe in
        keyframe_separator_frames(&keyframes).filter(|frame| frame_slots.contains(frame))
    {
        let x = base_x + f32::from(keyframe) * FRAME_W;
        painter.line_segment(
            [pos2(x, cell_top), pos2(x, cell_bot)],
            Stroke::new(1.0_f32, stroke_dark),
        );
    }

    let mut arrows = std::collections::BTreeSet::new();
    for placement in &layer.placements {
        if let Some(to_frame) = placement.tween.to_frame() {
            if placement.frame < frame_slots.end
                && to_frame >= frame_slots.start
                && to_frame < layer_frame_count
            {
                arrows.insert((placement.frame, to_frame));
            }
        }
    }
    for (from_frame, to_frame) in arrows {
        let from_x = base_x + (f32::from(from_frame) + 0.5) * FRAME_W + 4.0;
        let to_x = base_x + (f32::from(to_frame) + 0.5) * FRAME_W - 4.0;
        let mid_y = row_y + ROW_H * 0.5;
        if to_x > from_x + 2.0 {
            painter.line_segment(
                [pos2(from_x, mid_y), pos2(to_x, mid_y)],
                Stroke::new(1.0_f32, tween_arrow),
            );
            painter.line_segment(
                [pos2(to_x, mid_y), pos2(to_x - 3.0, mid_y - 2.5)],
                Stroke::new(1.0_f32, tween_arrow),
            );
            painter.line_segment(
                [pos2(to_x, mid_y), pos2(to_x - 3.0, mid_y + 2.5)],
                Stroke::new(1.0_f32, tween_arrow),
            );
        }
    }

    let keyframe_color = theme.keyframe.to_color32();
    let keyframe_y = row_y + ROW_H - 5.0;
    for keyframe in keyframes
        .into_iter()
        .filter(|frame| frame_slots.contains(frame))
    {
        let x = base_x + (f32::from(keyframe) + 0.5) * FRAME_W;
        if layer.is_blank_keyframe(keyframe) {
            painter.circle_stroke(
                pos2(x, keyframe_y),
                2.8,
                Stroke::new(1.2_f32, keyframe_color),
            );
        } else {
            painter.circle_filled(pos2(x, keyframe_y), 2.8, keyframe_color);
        }
    }
}

fn visible_keyframes(layer: &q0s_format::v2::Layer, frame_count: u16) -> Vec<u16> {
    layer
        .keyframe_frames()
        .into_iter()
        .filter(|frame| *frame < frame_count)
        .collect()
}

fn symbols_on_layer_at_frame(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Selection {
    if !project.layer_is_visible(q0rg_id, layer_id) || project.layer_is_locked(q0rg_id, layer_id) {
        return Selection::None;
    }

    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return Selection::None;
    };

    let placements = q0s_format::raster::active_placement_states_at(layer, frame)
        .into_iter()
        .filter_map(|active| {
            matches!(
                layer
                    .placements
                    .get(active.index)
                    .map(|placement| placement.target),
                Some(q0s_format::v2::Target::Q0rg(_))
            )
            .then_some(crate::state::PlacementRef {
                q0rg_id,
                layer_id,
                placement_idx: active.index,
            })
        })
        .collect::<Vec<_>>();

    match placements.as_slice() {
        [] => Selection::None,
        [only] => Selection::Placement {
            q0rg_id: only.q0rg_id,
            layer_id: only.layer_id,
            placement_idx: only.placement_idx,
        },
        _ => Selection::Multi(placements),
    }
}

fn keyframe_separator_frames(keyframes: &[u16]) -> impl Iterator<Item = u16> + '_ {
    keyframes.iter().copied().filter(|frame| *frame > 0)
}

fn timeline_selection_bounds(
    selection: TimelineSelection,
    layer_ids: &[u16],
    frame_count: u16,
) -> Option<(usize, usize, u16, u16)> {
    if frame_count == 0 {
        return None;
    }
    let anchor_layer = layer_ids
        .iter()
        .position(|layer_id| *layer_id == selection.anchor_layer_id)?;
    let focus_layer = layer_ids
        .iter()
        .position(|layer_id| *layer_id == selection.focus_layer_id)?;
    Some((
        anchor_layer.min(focus_layer),
        anchor_layer.max(focus_layer),
        selection
            .anchor_frame
            .min(selection.focus_frame)
            .min(frame_count - 1),
        selection
            .anchor_frame
            .max(selection.focus_frame)
            .min(frame_count - 1),
    ))
}

#[cfg(test)]
fn timeline_cell_at(
    rect: Rect,
    pos: egui::Pos2,
    frame_count: u16,
    layer_ids: &[u16],
    folder_layer_ids: &[u16],
    camera_row_count: usize,
) -> Option<(u16, u16)> {
    timeline_cell_at_with_label_width(
        rect,
        pos,
        frame_count,
        layer_ids,
        folder_layer_ids,
        camera_row_count,
        LAYER_LABEL_W,
    )
}

fn timeline_cell_at_with_label_width(
    rect: Rect,
    pos: egui::Pos2,
    frame_count: u16,
    layer_ids: &[u16],
    folder_layer_ids: &[u16],
    camera_row_count: usize,
    label_width: f32,
) -> Option<(u16, u16)> {
    let rel_x = pos.x - (rect.min.x + label_width);
    if rel_x < 0.0 {
        return None;
    }
    let frame = (rel_x / FRAME_W).floor() as i32;
    if frame < 0 || frame >= i32::from(frame_count) {
        return None;
    }
    let row = ((pos.y - rect.min.y) / ROW_H).floor() as i32 - 1 - camera_row_count as i32;
    if row < 0 {
        return None;
    }
    layer_ids
        .get(row as usize)
        .copied()
        .filter(|layer_id| !folder_layer_ids.contains(layer_id))
        .map(|layer_id| (layer_id, frame as u16))
}

fn draw_timeline_selection(
    painter: &egui::Painter,
    theme: &Theme,
    selection: Option<TimelineSelection>,
    layer_ids: &[u16],
    base_x: f32,
    first_row_y: f32,
    frame_count: u16,
) {
    let Some(selection) = selection else {
        return;
    };
    let Some((first_layer, last_layer, first_frame, last_frame)) =
        timeline_selection_bounds(selection, layer_ids, frame_count)
    else {
        return;
    };
    let accent = timeline_selection_color(theme);
    let fill = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 54);
    let stroke = Stroke::new(1.0_f32, accent);

    let clip = painter.clip_rect();
    let rows = visible_slots(
        first_row_y,
        ROW_H,
        layer_ids.len(),
        clip.top(),
        clip.bottom(),
    );
    for layer_index in first_layer.max(rows.start)..last_layer.saturating_add(1).min(rows.end) {
        let row_y = first_row_y + layer_index as f32 * ROW_H;
        let x0 = base_x + f32::from(first_frame) * FRAME_W;
        let x1 = base_x + (f32::from(last_frame) + 1.0) * FRAME_W;
        let selected = Rect::from_min_max(
            pos2(x0, row_y + CELL_PAD_Y),
            pos2(x1, row_y + ROW_H - CELL_PAD_Y),
        );
        painter.rect_filled(selected, 0.0, fill);
        painter.rect_stroke(selected, 0.0, stroke);
    }
}

fn draw_timeline_cell_selection(
    painter: &egui::Painter,
    theme: &Theme,
    cells: &[(u16, u16)],
    layer_ids: &[u16],
    base_x: f32,
    first_row_y: f32,
    frame_count: u16,
) {
    let accent = timeline_selection_color(theme);
    let fill = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 64);
    let stroke = Stroke::new(1.25_f32, accent);
    for &(layer_id, frame) in cells {
        if frame >= frame_count {
            continue;
        }
        let Some(row) = layer_ids.iter().position(|id| *id == layer_id) else {
            continue;
        };
        let rect = Rect::from_min_size(
            pos2(
                base_x + f32::from(frame) * FRAME_W,
                first_row_y + row as f32 * ROW_H + CELL_PAD_Y,
            ),
            vec2(FRAME_W, ROW_H - CELL_PAD_Y * 2.0),
        );
        painter.rect_filled(rect, 1.0, fill);
        painter.rect_stroke(rect, 1.0, stroke);
    }
}

fn timeline_selection_color(theme: &Theme) -> Color32 {
    theme.accent.to_color32()
}

/// Linear blend `a + t*(b-a)` per channel. Used to derive the second-row
/// label tone from the panel/window pair.
fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut shortened: String = text.chars().take(keep).collect();
    shortened.push_str("...");
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{Layer, Placement, Target, Transform2D, Tween};

    #[test]
    fn viewport_slots_keep_partially_visible_frames_and_clamp_document_edges() {
        assert_eq!(visible_slots(174.0, 12.0, 65_535, 0.0, 774.0), 0..51);
        assert_eq!(visible_slots(-1200.0, 12.0, 65_535, 0.5, 600.5), 99..152);
        assert_eq!(visible_slots(174.0, 12.0, 10, 500.0, 800.0), 0..0);
        assert_eq!(visible_slots(174.0, 12.0, 10, 0.0, 170.0), 0..0);
        assert_eq!(visible_slots(0.0, ROW_H, 1000, 260.0, 390.0), 9..16);
    }

    #[test]
    fn timeline_cell_paint_work_depends_on_viewport_not_movie_length() {
        fn paint_count(frames: u16, base_x: f32) -> usize {
            let ctx = egui::Context::default();
            let output = ctx.run(Default::default(), |ctx| {
                let painter = ctx
                    .layer_painter(egui::LayerId::background())
                    .with_clip_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, ROW_H)));
                draw_empty_cells(
                    &painter,
                    &Theme::default(),
                    base_x,
                    0.0,
                    frames,
                    frames,
                    false,
                );
            });
            output.shapes.len()
        }
        let short = paint_count(100, 0.0);
        let long = paint_count(u16::MAX, 0.0);
        let scrolled = paint_count(u16::MAX, -12_000.0);
        assert!(short > 0 && short <= 52, "visible cells: {short}");
        assert_eq!(short, long);
        assert!(scrolled <= 53, "scrolled cells: {scrolled}");
    }

    #[test]
    fn held_span_remains_visible_when_its_keyframe_is_offscreen() {
        let layer = Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "layer".into(),
            explicit_keyframes: vec![],
            placements: vec![placement(0)],
        };
        let ctx = egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| {
            let painter = ctx
                .layer_painter(egui::LayerId::background())
                .with_clip_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, ROW_H)));
            draw_layer_spans(
                &painter,
                &Theme::default(),
                &layer,
                -1200.0,
                0.0,
                500,
                false,
            );
        });
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape, egui::Shape::Rect(rect) if rect.rect.left() < 0.0 && rect.rect.right() > 600.0
        )), "a held frame must fill the viewport even if its key is offscreen");
    }

    fn placement(frame: u16) -> Placement {
        Placement {
            instance_id: 0,
            frame,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }
    }

    #[test]
    fn timeline_selection_uses_theme_accent_not_legacy_playhead_red() {
        let theme = Theme {
            accent: crate::settings::ColorRgb::new(0x26, 0x8B, 0xD2),
            playhead: crate::settings::ColorRgb::new(0xCC, 0x33, 0x33),
            ..Theme::default()
        };

        assert_eq!(timeline_selection_color(&theme), theme.accent.to_color32());
    }

    #[test]
    fn layer_move_icons_are_drawn_geometry_with_the_expected_directions() {
        let rect = Rect::from_center_size(pos2(10.0, 10.0), vec2(14.0, 14.0));
        for icon in [
            LayerMoveIcon::Up,
            LayerMoveIcon::Down,
            LayerMoveIcon::Indent,
            LayerMoveIcon::Outdent,
        ] {
            let segments = layer_move_icon_segments(icon, rect);
            assert!(segments.len() >= 3);
            assert!(segments
                .iter()
                .flatten()
                .all(|point| rect.expand(0.1).contains(*point)));
        }

        let up = layer_move_icon_segments(LayerMoveIcon::Up, rect);
        let down = layer_move_icon_segments(LayerMoveIcon::Down, rect);
        let indent = layer_move_icon_segments(LayerMoveIcon::Indent, rect);
        let outdent = layer_move_icon_segments(LayerMoveIcon::Outdent, rect);
        assert!(up[0][1].y < up[0][0].y);
        assert!(down[0][1].y > down[0][0].y);
        assert!(indent[0][1].x > indent[0][0].x);
        assert!(outdent[0][1].x < outdent[0][0].x);
    }

    #[test]
    fn adjacent_keyframes_keep_an_explicit_separator() {
        let layer = Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "layer".into(),
            explicit_keyframes: vec![2],
            placements: vec![placement(0), placement(1)],
        };
        let keyframes = visible_keyframes(&layer, 4);
        assert_eq!(keyframes, vec![0, 1, 2]);
        assert_eq!(
            keyframe_separator_frames(&keyframes).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn timeline_selection_bounds_cover_dragged_layers_and_frames() {
        let selection = TimelineSelection {
            anchor_layer_id: 30,
            anchor_frame: 8,
            focus_layer_id: 10,
            focus_frame: 2,
        };
        assert_eq!(
            timeline_selection_bounds(selection, &[10, 20, 30], 12),
            Some((0, 2, 2, 8))
        );
    }

    #[test]
    fn collapsed_folder_hides_its_children_in_front_to_back_timeline_order() {
        let mut project = crate::state::default_project();
        project.q0rgs[0].layers = vec![
            Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "low child".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 2,
                timeline_frame_count: None,
                name: "high child".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 3,
                timeline_frame_count: None,
                name: "folder".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 4,
                timeline_frame_count: None,
                name: "front".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
        ];
        for child in [1, 2] {
            project.layer_metadata.insert(
                q0s_format::v2::LayerKey::new(1, child),
                q0s_format::v2::LayerMetadata {
                    kind: q0s_format::v2::LayerKind::Normal,
                    parent_folder_id: Some(3),
                    mask_layer_id: None,
                    collapsed: false,
                    hidden: false,
                    locked: false,
                },
            );
        }
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 3),
            q0s_format::v2::LayerMetadata {
                kind: q0s_format::v2::LayerKind::Folder,
                parent_folder_id: None,
                mask_layer_id: None,
                collapsed: false,
                hidden: false,
                locked: false,
            },
        );
        assert_eq!(visible_layer_indices(&project, 0), vec![3, 2, 1, 0]);

        project
            .layer_metadata
            .get_mut(&q0s_format::v2::LayerKey::new(1, 3))
            .unwrap()
            .collapsed = true;
        assert_eq!(visible_layer_indices(&project, 0), vec![3, 2]);
    }

    #[test]
    fn nested_folder_collapse_hides_the_complete_descendant_subtree() {
        let mut project = crate::state::default_project();
        project.q0rgs[0].layers = vec![
            Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "leaf".into(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            },
            Layer {
                layer_id: 2,
                timeline_frame_count: None,
                name: "inner".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 3,
                timeline_frame_count: None,
                name: "outer".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 4,
                timeline_frame_count: None,
                name: "outside".into(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            },
        ];
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 1),
            q0s_format::v2::LayerMetadata {
                parent_folder_id: Some(2),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 2),
            q0s_format::v2::LayerMetadata {
                kind: q0s_format::v2::LayerKind::Folder,
                parent_folder_id: Some(3),
                collapsed: true,
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 3),
            q0s_format::v2::LayerMetadata {
                kind: q0s_format::v2::LayerKind::Folder,
                ..Default::default()
            },
        );
        q0s_format::v2::validate(&project).expect("nested collapse fixture");

        assert_eq!(visible_layer_indices(&project, 0), vec![3, 2, 1]);
        project
            .layer_metadata
            .get_mut(&q0s_format::v2::LayerKey::new(1, 2))
            .unwrap()
            .collapsed = false;
        project
            .layer_metadata
            .get_mut(&q0s_format::v2::LayerKey::new(1, 3))
            .unwrap()
            .collapsed = true;
        assert_eq!(visible_layer_indices(&project, 0), vec![3, 2]);
    }

    #[test]
    fn nested_drag_horizontal_lanes_can_outdent_one_level_or_to_root() {
        let mut project = crate::state::default_project();
        project.q0rgs[0].layers = vec![
            Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "leaf".into(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            },
            Layer {
                layer_id: 2,
                timeline_frame_count: None,
                name: "inner".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 3,
                timeline_frame_count: None,
                name: "outer".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
        ];
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 1),
            q0s_format::v2::LayerMetadata {
                parent_folder_id: Some(2),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 2),
            q0s_format::v2::LayerMetadata {
                kind: q0s_format::v2::LayerKind::Folder,
                parent_folder_id: Some(3),
                ..Default::default()
            },
        );
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 3),
            q0s_format::v2::LayerMetadata {
                kind: q0s_format::v2::LayerKind::Folder,
                ..Default::default()
            },
        );
        q0s_format::v2::validate(&project).expect("nested drag lane fixture");

        assert_eq!(layer_parent_for_drop_lane(&project, 1, 1, 8.0, 0.0), None);
        assert_eq!(
            layer_parent_for_drop_lane(&project, 1, 1, 35.0, 0.0),
            Some(3)
        );
        assert_eq!(
            layer_parent_for_drop_lane(&project, 1, 1, 50.0, 0.0),
            Some(2)
        );
    }

    #[test]
    fn layer_drop_hit_testing_enters_folder_and_outdents_from_its_own_row() {
        let mut project = crate::state::default_project();
        project.q0rgs[0].layers = vec![
            Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "child".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 2,
                timeline_frame_count: None,
                name: "folder".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 3,
                timeline_frame_count: None,
                name: "outside".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
        ];
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 1),
            q0s_format::v2::LayerMetadata {
                kind: q0s_format::v2::LayerKind::Normal,
                parent_folder_id: Some(2),
                mask_layer_id: None,
                collapsed: false,
                hidden: false,
                locked: false,
            },
        );
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 2),
            q0s_format::v2::LayerMetadata {
                kind: q0s_format::v2::LayerKind::Folder,
                parent_folder_id: None,
                mask_layer_id: None,
                collapsed: false,
                hidden: false,
                locked: false,
            },
        );
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 120.0));
        let visible = [3, 2, 1];

        assert_eq!(
            layer_drop_target_at(&project, 1, rect, pos2(50.0, ROW_H * 2.5), &visible, &[3],),
            Some(LayerDropTarget::IntoFolder(2))
        );
        assert_eq!(
            layer_drop_target_at(&project, 1, rect, pos2(8.0, ROW_H * 3.75), &visible, &[1],),
            Some(LayerDropTarget::After {
                layer_id: 1,
                parent_folder_id: None,
            })
        );
    }

    #[test]
    fn timeline_cell_hit_testing_accepts_virtual_frames_but_ignores_header() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 100.0));
        let layers = [10, 20];
        assert_eq!(
            timeline_cell_at(
                rect,
                pos2(LAYER_LABEL_W + FRAME_W * 2.5, ROW_H * 1.5),
                5,
                &layers,
                &[],
                0,
            ),
            Some((10, 2))
        );
        assert_eq!(
            timeline_cell_at(
                rect,
                pos2(LAYER_LABEL_W + 2.0, ROW_H * 0.5),
                5,
                &layers,
                &[],
                0,
            ),
            None
        );
        assert_eq!(
            timeline_cell_at(
                rect,
                pos2(LAYER_LABEL_W + FRAME_W * 6.0, ROW_H * 1.5),
                35,
                &layers,
                &[],
                0,
            ),
            Some((10, 6))
        );
        assert_eq!(
            timeline_cell_at(
                rect,
                pos2(LAYER_LABEL_W + FRAME_W * 36.0, ROW_H * 1.5),
                35,
                &layers,
                &[],
                0,
            ),
            None
        );
    }

    #[test]
    fn camera_row_occupies_a_real_timeline_row_without_becoming_an_artwork_drop_target() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 200.0));
        let layers = [10_u16, 20_u16];
        assert_eq!(
            timeline_cell_at(
                rect,
                pos2(LAYER_LABEL_W + FRAME_W * 2.5, ROW_H * 1.5),
                20,
                &layers,
                &[],
                1,
            ),
            None,
            "camera row must not masquerade as artwork layer 10"
        );
        assert_eq!(
            timeline_cell_at(
                rect,
                pos2(LAYER_LABEL_W + FRAME_W * 2.5, ROW_H * 2.5),
                20,
                &layers,
                &[],
                1,
            ),
            Some((10, 2))
        );
    }

    #[test]
    fn rig_header_markers_can_filter_to_selected_control() {
        let mut project = crate::state::default_project();
        project.q0rgs[0].frame_count = 12;
        crate::rigging::ensure_rig(&mut project, 1).unwrap();
        let rig = q0s_format::rig::rig_for_q0rg_mut(&mut project, 1).unwrap();
        for id in [1, 2] {
            rig.controls.push(q0s_format::v2::RigControl {
                control_id: id,
                name: format!("control {id}"),
                kind: q0s_format::v2::RigControlKind::Slider,
                target_node: None,
                rest_x: 0.0,
                rest_y: 0.0,
                rest_value: 0.0,
                min_value: -1.0,
                max_value: 1.0,
                public_in_simple: true,
            });
        }
        rig.channels = vec![
            q0s_format::v2::RigChannel {
                property: q0s_format::v2::RigPropertyRef::ControlValue(1),
                keys: vec![q0s_format::v2::RigKey {
                    frame: 2,
                    value: 0.4,
                    easing: q0s_format::v2::Easing::Linear,
                }],
            },
            q0s_format::v2::RigChannel {
                property: q0s_format::v2::RigPropertyRef::ControlX(2),
                keys: vec![q0s_format::v2::RigKey {
                    frame: 7,
                    value: 10.0,
                    easing: q0s_format::v2::Easing::Linear,
                }],
            },
        ];

        assert_eq!(
            rig_key_frames(&project, 1, None)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![2, 7]
        );
        assert_eq!(
            rig_key_frames(&project, 1, Some(1))
                .into_iter()
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(
            rig_key_frames(&project, 1, Some(2))
                .into_iter()
                .collect::<Vec<_>>(),
            vec![7]
        );
    }
}
