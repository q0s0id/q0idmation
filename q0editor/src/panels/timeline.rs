use egui::{
    pos2, vec2, Align2, Color32, FontId, PointerButton, Rect, ScrollArea, Sense, Stroke, Ui,
};

use crate::app::{Action, EditorApp, LayerDropTarget};
use crate::settings::Theme;
use crate::state::{
    LayerRename, LibraryItem, Selection, TimelineFrameDrag, TimelineLayerDrag,
    TimelineLayerSelection, TimelineSelection,
};

const FRAME_W: f32 = 12.0;
const ROW_H: f32 = 22.0;
const LAYER_LABEL_W: f32 = 110.0;
/// Gap above and below the cell fill — leaves room for thin row separators
/// like the original Flash CS3 timeline.
const CELL_PAD_Y: f32 = 1.0;
/// How many "virtual" frame slots to render past `frame_count` so the user
/// sees there's room to extend the q0rg. They render as darker chequered
/// cells and can hold a virtual playhead until F5/F6/F7 materialises them.
const VIRTUAL_FRAMES_BUFFER: u16 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerMoveIcon {
    Up,
    Down,
    Indent,
    Outdent,
}

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
    // Do not use `Button::new("")` here. The editor's custom font fallback can
    // still create a missing-glyph square for an empty galley. Allocate and
    // paint the entire control ourselves so these icons contain zero text.
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 18.0), Sense::click());
    let visuals = ui.style().interact(&response);
    let painted_rect = rect.expand(visuals.expansion);
    ui.painter()
        .rect_filled(painted_rect, visuals.rounding, visuals.bg_fill);
    ui.painter()
        .rect_stroke(painted_rect, visuals.rounding, visuals.bg_stroke);

    let stroke = Stroke::new(1.6_f32, visuals.fg_stroke.color);
    let icon_rect = Rect::from_center_size(rect.center(), vec2(14.0, 14.0));
    for segment in layer_move_icon_segments(icon, icon_rect) {
        ui.painter().line_segment(segment, stroke);
    }
    response.on_hover_text(hover_text)
}

fn visible_layer_indices(project: &q0s_format::v2::ProjectV2, q0rg_idx: usize) -> Vec<usize> {
    let Some(q0rg) = project.q0rgs.get(q0rg_idx) else {
        return Vec::new();
    };
    let mut visible = Vec::new();
    let mut collapsed_folder = None;
    for index in (0..q0rg.layers.len()).rev() {
        let layer = &q0rg.layers[index];
        if let Some(folder_id) = collapsed_folder {
            if project.layer_parent_folder(q0rg.q0rg_id, layer.layer_id) == Some(folder_id) {
                continue;
            }
            collapsed_folder = None;
        }
        visible.push(index);
        let metadata = project.layer_metadata(q0rg.q0rg_id, layer.layer_id);
        if metadata.kind == q0s_format::v2::LayerKind::Folder && metadata.collapsed {
            collapsed_folder = Some(layer.layer_id);
        }
    }
    visible
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
pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    let theme = app.settings.theme.clone();
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

    ScrollArea::both()
        .auto_shrink([false, false])
        .max_height(ui.available_height())
        .show(ui, |ui| {
            let total_w = LAYER_LABEL_W + (visible_frames as f32) * FRAME_W;
            let total_h = ROW_H * (layer_count as f32 + 1.0);
            let (rect, response) = ui.allocate_exact_size(
                vec2(total_w.max(ui.available_width()), total_h),
                Sense::click_and_drag(),
            );
            let painter = ui.painter_at(rect);

            // Timeline base panel — same as the editor's window colour
            // so it visually merges with the surrounding chrome.
            painter.rect_filled(rect, 0.0, theme.window.to_color32());

            // Header (frame numbers + second markers)
            let header_rect = Rect::from_min_size(rect.min, vec2(rect.width(), ROW_H));
            painter.rect_filled(header_rect, 0.0, theme.timeline_header.to_color32());
            // Bottom edge of header
            painter.line_segment(
                [
                    pos2(rect.min.x, rect.min.y + ROW_H),
                    pos2(rect.min.x + rect.width(), rect.min.y + ROW_H),
                ],
                Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
            );
            let fps = app.state.project.meta.fps as u32;
            for f in 0..visible_frames {
                let x = rect.min.x + LAYER_LABEL_W + (f as f32) * FRAME_W;
                let frame_one_based = f + 1;
                let beyond = f >= frame_count;
                // Tick lines — every 5th frame full-height, others only in
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
                // Frame number every 5th frame — but only inside the q0rg's
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
                // Second marker (1s, 2s, …) at every fps-th frame above the number.
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
            // End-of-q0rg marker: dashed amber bar at the right edge of the
            // last real frame so the user can see exactly where `frame_count`
            // ends without confusing it with the solid theme-coloured playhead.
            let edge_x = rect.min.x + LAYER_LABEL_W + (frame_count as f32) * FRAME_W;
            let edge_color = Color32::from_rgb(0xC8, 0x95, 0x30);
            let mut y = rect.min.y;
            while y < rect.min.y + total_h {
                let y_end = (y + 4.0).min(rect.min.y + total_h);
                painter.line_segment(
                    [pos2(edge_x, y), pos2(edge_x, y_end)],
                    Stroke::new(1.0_f32, edge_color),
                );
                y += 7.0;
            }

            // Layer rows
            let mut click_layer: Option<u16> = None;
            let mut click_frame: Option<u16> = None;
            let mut click_label_layer: Option<u16> = None;
            let mut started_layer_drag: Option<u16> = None;
            for (li, layer_index) in visible_layer_indices.iter().copied().enumerate() {
                let layer = &app.state.project.q0rgs[q0rg_idx].layers[layer_index];
                let metadata = app.state.project.layer_metadata(q0rg_id, layer.layer_id);
                let is_folder = metadata.kind == q0s_format::v2::LayerKind::Folder;
                let row_y = rect.min.y + ROW_H * (li as f32 + 1.0);
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
                    rect.min.x + LAYER_LABEL_W,
                    row_y,
                    frame_count,
                    visible_frames,
                    is_current_layer,
                );

                // Label (left-side fixed column) — pull tones from the
                // panel/window pair so any theme reads as a coherent strip.
                let label_rect =
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(LAYER_LABEL_W, ROW_H));
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
                let icon_rect = Rect::from_center_size(
                    label_rect.left_center() + vec2(11.0, 0.0),
                    vec2(13.0, 13.0),
                );
                draw_layer_icon(
                    &painter,
                    icon_rect,
                    is_folder,
                    metadata.collapsed,
                    theme.text.to_color32(),
                    theme.accent.to_color32(),
                );
                let indent = if metadata.parent_folder_id.is_some() {
                    14.0
                } else {
                    0.0
                };
                let layer_label = truncate_label(&layer.name, 14);
                painter.text(
                    label_rect.left_center() + vec2(22.0 + indent, 0.0),
                    Align2::LEFT_CENTER,
                    layer_label,
                    FontId::proportional(11.0),
                    theme.text.to_color32(),
                );
                painter.line_segment(
                    [
                        pos2(rect.min.x + LAYER_LABEL_W, row_y),
                        pos2(rect.min.x + LAYER_LABEL_W, row_y + ROW_H),
                    ],
                    Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
                );

                // Bottom row separator — hairline in the theme's stroke colour.
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
                        rect.min.x + LAYER_LABEL_W,
                        row_y,
                        frame_count,
                        is_current_layer,
                    );
                } else {
                    let folder_strip = Rect::from_min_size(
                        pos2(rect.min.x + LAYER_LABEL_W, row_y + CELL_PAD_Y),
                        vec2((visible_frames as f32) * FRAME_W, ROW_H - CELL_PAD_Y * 2.0),
                    );
                    painter.rect_filled(
                        folder_strip,
                        0.0,
                        blend(theme.panel.to_color32(), theme.window.to_color32(), 0.35),
                    );
                }

                // Dragging starts only from the fixed layer-label column.
                // Frame-strip drags keep their existing rectangular selection
                // behaviour and never accidentally reorder a layer.
                if let Some(pos) = response.interact_pointer_pos() {
                    if label_rect.contains(pos) && response.drag_started_by(PointerButton::Primary)
                    {
                        started_layer_drag = Some(layer.layer_id);
                    }
                }

                // Click in row → set current_layer + current_frame
                if let Some(pos) = response.interact_pointer_pos() {
                    if row_rect.contains(pos) && response.clicked() {
                        if label_rect.contains(pos) {
                            click_label_layer = Some(layer.layer_id);
                        }
                        click_layer = Some(layer.layer_id);
                        if is_folder {
                            click_frame = None;
                            app.session.timeline_selection = None;
                            if label_rect.contains(pos) && pos.x <= label_rect.left() + 22.0 {
                                app.queue(Action::ToggleLayerFolder(q0rg_id, layer.layer_id));
                            }
                        } else {
                            let rel_x = pos.x - (rect.min.x + LAYER_LABEL_W);
                            if rel_x >= 0.0 {
                                let f = (rel_x / FRAME_W).floor() as i32;
                                if f >= 0 && f < visible_frames as i32 {
                                    click_frame = Some(f as u16);
                                }
                            } else if response.double_clicked() {
                                begin_layer_rename(app, q0rg_id, layer.layer_id);
                            }
                        }
                    }
                }
            }

            if let Some(layer_id) = started_layer_drag {
                app.session.timeline_layer_drag = Some(TimelineLayerDrag { q0rg_id, layer_id });
                app.session.current_layer_id = layer_id;
                app.session.timeline_selection = None;
                app.session.timeline_layer_selection =
                    Some(TimelineLayerSelection::single(layer_id));
                app.session.selection = Selection::None;
            }

            let layer_drop_preview = app
                .session
                .timeline_layer_drag
                .filter(|drag| drag.q0rg_id == q0rg_id)
                .and_then(|drag| {
                    ui.input(|input| input.pointer.latest_pos())
                        .and_then(|pos| {
                            layer_drop_target_at(
                                &app.state.project,
                                q0rg_id,
                                rect,
                                pos,
                                &layer_ids,
                                drag.layer_id,
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
                );
            }

            let library_payload = response.dnd_hover_payload::<LibraryItem>();
            let library_drop_cell =
                ui.input(|input| input.pointer.latest_pos())
                    .and_then(|position| {
                        timeline_cell_at(rect, position, frame_count, &layer_ids, &folder_layer_ids)
                    });
            if library_payload.is_some() {
                if let Some((layer_id, frame)) = library_drop_cell {
                    if let Some(row) = layer_ids
                        .iter()
                        .position(|candidate| *candidate == layer_id)
                    {
                        let cell = Rect::from_min_size(
                            pos2(
                                rect.min.x + LAYER_LABEL_W + frame as f32 * FRAME_W,
                                rect.min.y + ROW_H * (row as f32 + 1.0),
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
                .dnd_release_payload::<LibraryItem>()
                .map(|payload| *payload);
            if let (Some(item), Some((layer_id, frame))) =
                (released_library_item, library_drop_cell)
            {
                app.queue(Action::PlaceLibraryItemOnTimeline(item, layer_id, frame));
            }

            draw_timeline_selection(
                &painter,
                &theme,
                app.session.timeline_selection,
                &layer_ids,
                rect.min.x + LAYER_LABEL_W,
                rect.min.y + ROW_H,
                visible_frames,
            );

            // Playhead
            let playhead_x =
                rect.min.x + LAYER_LABEL_W + (app.session.timeline_frame() as f32 + 0.5) * FRAME_W;
            painter.line_segment(
                [
                    pos2(playhead_x, rect.min.y),
                    pos2(playhead_x, rect.min.y + total_h),
                ],
                Stroke::new(1.5_f32, theme.playhead.to_color32()),
            );

            // Header click → set current frame
            if let Some(pos) = response.interact_pointer_pos() {
                if app.session.timeline_layer_drag.is_none()
                    && header_rect.contains(pos)
                    && (response.clicked() || response.dragged())
                {
                    let rel_x = pos.x - (rect.min.x + LAYER_LABEL_W);
                    if rel_x >= 0.0 {
                        let f = (rel_x / FRAME_W).floor() as i32;
                        if f >= 0 && f < visible_frames as i32 {
                            click_frame = Some(f as u16);
                        }
                    }
                }
            }

            if let Some(id) = click_layer {
                app.session.current_layer_id = id;
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
                app.session.selection = Selection::None;
            }
            if let Some(f) = click_frame {
                app.session.set_timeline_frame(f, frame_count);
            }
            if let (Some(layer_id), Some(frame)) = (click_layer, click_frame) {
                app.session.selection = Selection::None;
                app.session.timeline_layer_selection = None;
                let extend = ui.input(|input| input.modifiers.shift);
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

            let frame_drop_cell =
                ui.input(|input| input.pointer.latest_pos())
                    .and_then(|position| {
                        timeline_cell_at(
                            rect,
                            position,
                            visible_frames,
                            &layer_ids,
                            &folder_layer_ids,
                        )
                    });

            if app.session.timeline_layer_drag.is_none() && library_payload.is_none() {
                if let Some((layer_id, frame)) = frame_drop_cell {
                    if response.drag_started() {
                        app.session.selection = Selection::None;
                        app.session.timeline_layer_selection = None;
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
                                rect.min.x + LAYER_LABEL_W + target_frame as f32 * FRAME_W,
                                rect.min.y + ROW_H * (target_row as f32 + 1.0),
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
                if let Some(drag) = app.session.timeline_layer_drag.take() {
                    if drag.q0rg_id == q0rg_id {
                        if let Some(target) = layer_drop_preview {
                            app.queue(Action::DropLayer(q0rg_id, drag.layer_id, target));
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
            } else if !ui.input(|input| input.pointer.primary_down()) {
                app.session.timeline_layer_drag = None;
                app.session.timeline_frame_drag = None;
            }
            // Right-click on a frame cell: position the playhead there first,
            // then open the context menu so its actions act on the *clicked*
            // frame, not whatever was previously selected.
            if response.secondary_clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let rel_x = pos.x - (rect.min.x + LAYER_LABEL_W);
                    if rel_x >= 0.0 {
                        let f = (rel_x / FRAME_W).floor() as i32;
                        if f >= 0 && f < visible_frames as i32 {
                            app.session.set_timeline_frame(f as u16, frame_count);
                        }
                    }
                    // Identify which layer row was clicked.
                    let row_idx = ((pos.y - rect.min.y) / ROW_H).floor() as i32 - 1;
                    if row_idx >= 0 {
                        if let Some(layer_index) = visible_layer_indices.get(row_idx as usize) {
                            let layer = &app.state.project.q0rgs[q0rg_idx].layers[*layer_index];
                            app.session.current_layer_id = layer.layer_id;
                            app.session.selection = Selection::None;
                            let clicked_label = pos.x < rect.min.x + LAYER_LABEL_W;
                            if clicked_label
                                || app.state.project.layer_is_folder(q0rg_id, layer.layer_id)
                            {
                                app.session.timeline_selection = None;
                                app.session.timeline_layer_selection =
                                    Some(TimelineLayerSelection::single(layer.layer_id));
                            } else {
                                app.session.timeline_layer_selection = None;
                                app.session.timeline_selection = Some(TimelineSelection::single(
                                    layer.layer_id,
                                    app.session.timeline_frame(),
                                ));
                            }
                        }
                    }
                }
            }
            response.context_menu(|ui| timeline_context_menu(app, ui));
        });
    render_layer_rename_dialog(app, ui.ctx());
}

fn layer_drop_target_at(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    rect: Rect,
    pos: egui::Pos2,
    visible_layer_ids: &[u16],
    dragged_layer_id: u16,
) -> Option<LayerDropTarget> {
    if !rect.expand(6.0).contains(pos) {
        return None;
    }
    let row_count = visible_layer_ids.len();
    if row_count == 0 {
        return None;
    }
    let first_row_y = rect.min.y + ROW_H;
    let raw_row = ((pos.y - first_row_y) / ROW_H).floor() as isize;
    let row_index = raw_row.clamp(0, row_count as isize - 1) as usize;
    let target_id = visible_layer_ids[row_index];
    let target_metadata = project.layer_metadata(q0rg_id, target_id);
    let dragged_metadata = project.layer_metadata(q0rg_id, dragged_layer_id);
    let dragged_is_folder = dragged_metadata.kind == q0s_format::v2::LayerKind::Folder;
    if dragged_is_folder && target_metadata.parent_folder_id == Some(dragged_layer_id) {
        return None;
    }

    let row_y = first_row_y + row_index as f32 * ROW_H;
    let row_fraction = ((pos.y - row_y) / ROW_H).clamp(0.0, 1.0);
    let nested_lane = pos.x >= rect.min.x + 32.0;

    if target_metadata.kind == q0s_format::v2::LayerKind::Folder {
        if target_id == dragged_layer_id {
            return None;
        }
        if !dragged_is_folder && (0.25..=0.75).contains(&row_fraction) {
            return Some(LayerDropTarget::IntoFolder(target_id));
        }
        return Some(if row_fraction < 0.5 {
            LayerDropTarget::Before {
                layer_id: target_id,
                parent_folder_id: None,
            }
        } else {
            LayerDropTarget::After {
                layer_id: target_id,
                parent_folder_id: None,
            }
        });
    }

    let requested_parent = if nested_lane {
        target_metadata.parent_folder_id
    } else {
        None
    };
    if target_id == dragged_layer_id && requested_parent == dragged_metadata.parent_folder_id {
        return None;
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
) {
    let accent = theme.accent.to_color32();
    match target {
        LayerDropTarget::IntoFolder(folder_id) => {
            let Some(index) = visible_layer_ids
                .iter()
                .position(|layer_id| *layer_id == folder_id)
            else {
                return;
            };
            let row_y = rect.min.y + ROW_H * (index as f32 + 1.0);
            let highlight = Rect::from_min_size(
                pos2(rect.min.x + 1.0, row_y + 1.0),
                vec2(LAYER_LABEL_W - 2.0, ROW_H - 2.0),
            );
            painter.rect_filled(
                highlight,
                2.0,
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 42),
            );
            painter.rect_stroke(highlight, 2.0, Stroke::new(1.5_f32, accent));
        }
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
            let actual_parent = project.layer_parent_folder(q0rg_id, layer_id);
            let (line_index, line_after) = if parent_folder_id.is_none() {
                if let Some(folder_id) = actual_parent {
                    let Some(folder_index) = visible_layer_ids
                        .iter()
                        .position(|candidate| *candidate == folder_id)
                    else {
                        return;
                    };
                    if before {
                        (folder_index, false)
                    } else {
                        let last_child = visible_layer_ids
                            .iter()
                            .enumerate()
                            .filter(|(_, candidate)| {
                                project.layer_parent_folder(q0rg_id, **candidate) == Some(folder_id)
                            })
                            .map(|(index, _)| index)
                            .max()
                            .unwrap_or(folder_index);
                        (last_child, true)
                    }
                } else {
                    let Some(index) = visible_layer_ids
                        .iter()
                        .position(|candidate| *candidate == layer_id)
                    else {
                        return;
                    };
                    (index, !before)
                }
            } else {
                let Some(index) = visible_layer_ids
                    .iter()
                    .position(|candidate| *candidate == layer_id)
                else {
                    return;
                };
                (index, !before)
            };
            let y = rect.min.y
                + ROW_H * (line_index as f32 + 1.0)
                + if line_after { ROW_H } else { 0.0 };
            let indent = if parent_folder_id.is_some() {
                14.0
            } else {
                0.0
            };
            let x0 = rect.min.x + 4.0 + indent;
            let x1 = rect.min.x + LAYER_LABEL_W - 4.0;
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
    if ui.button("Cut  (Ctrl+X)").clicked() {
        app.queue(Action::CutSelection);
        ui.close_menu();
    }
    if ui.button("Copy  (Ctrl+C)").clicked() {
        app.queue(Action::CopySelection);
        ui.close_menu();
    }
    if ui
        .add_enabled(
            app.session.clipboard.is_some(),
            egui::Button::new("Paste  (Ctrl+V)"),
        )
        .clicked()
    {
        app.queue(Action::Paste);
        ui.close_menu();
    }
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
    if metadata.kind == q0s_format::v2::LayerKind::Folder {
        if ui
            .button(if metadata.collapsed {
                "Expand folder"
            } else {
                "Collapse folder"
            })
            .clicked()
        {
            app.queue(Action::ToggleLayerFolder(q0rg_id, layer_id));
            ui.close_menu();
        }
    } else if metadata.parent_folder_id.is_some() {
        if ui.button("Move out of folder").clicked() {
            app.queue(Action::OutdentLayer(q0rg_id, layer_id));
            ui.close_menu();
        }
    } else if ui.button("Move into folder above").clicked() {
        app.queue(Action::IndentLayer(q0rg_id, layer_id));
        ui.close_menu();
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

    let frame = app.session.timeline_frame() + 1;
    ui.separator();
    ui.label(
        egui::RichText::new(format!("Frame {frame}"))
            .strong()
            .small(),
    );
    if ui.button("Insert Frame  (F5)").clicked() {
        app.queue(Action::InsertFrame);
        ui.close_menu();
    }
    if ui.button("Insert Keyframe  (F6)").clicked() {
        app.queue(Action::InsertKeyframe);
        ui.close_menu();
    }
    if ui.button("Insert Blank Keyframe  (F7)").clicked() {
        app.queue(Action::InsertBlankKeyframe);
        ui.close_menu();
    }
    if ui.button("Clear Keyframe(s)  (Shift+F6)").clicked() {
        app.queue(Action::ClearKeyframe);
        ui.close_menu();
    }
    if ui.button("Remove Selected Frame(s)  (Shift+F5)").clicked() {
        app.queue(Action::RemoveFrame);
        ui.close_menu();
    }
    ui.separator();
    if ui
        .button("Create / Remove Motion Tween  (Ctrl+Alt+T)")
        .clicked()
    {
        app.queue(Action::ToggleMotionTween);
        ui.close_menu();
    }
}

fn transport_bar(app: &mut EditorApp, theme: &Theme, ui: &mut Ui) {
    ui.horizontal(|ui| {
        let play_label = if app.session.playing { "Pause" } else { "Play" };
        if ui.button(play_label).clicked() {
            app.queue(Action::TogglePlay);
        }
        if ui.button("|<").on_hover_text("First frame").clicked() {
            app.queue(Action::FirstFrame);
        }
        if ui.button("<").on_hover_text("Previous frame").clicked() {
            app.queue(Action::PreviousFrame);
        }
        if ui.button(">").on_hover_text("Next frame").clicked() {
            app.queue(Action::NextFrame);
        }
        if ui.button(">|").on_hover_text("Last frame").clicked() {
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
            .add(egui::Slider::new(&mut displayed_frame, 1..=u32::from(max) + 1).text("frame"))
            .changed()
        {
            app.session
                .set_timeline_frame((displayed_frame - 1) as u16, frame_count);
        }
        ui.separator();

        // Onion skin: toggle (SelectableLabel highlights when on) + before/
        // after spinners (visible only while it's on so the bar stays compact
        // otherwise).
        if ui
            .add(egui::SelectableLabel::new(
                app.session.onion.enabled,
                "Onion",
            ))
            .on_hover_text("Show ghost copies of neighbouring frames (toggle)")
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let can_delete = app.session.timeline_selection.is_some()
                || super::menu::selection_can_be_deleted(&app.session.selection);
            if ui
                .add_enabled(can_delete, egui::Button::new("Delete"))
                .on_hover_text("Delete selection (Del)")
                .clicked()
            {
                app.queue(Action::DeleteSelection);
            }
            if ui.button("+ Layer").clicked() {
                app.queue(Action::AddLayer);
            }
            if ui
                .button("+ Folder")
                .on_hover_text("Create layer folder")
                .clicked()
            {
                app.queue(Action::AddLayerFolder);
            }
            let q0rg_id = app.session.current_q0rg_id;
            let layer_id = app.session.current_layer_id;
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
            if layer_move_icon_button(ui, LayerMoveIcon::Outdent, "Move layer out of folder")
                .clicked()
            {
                app.queue(Action::OutdentLayer(q0rg_id, layer_id));
            }
            if ui.button("+ q0rg").clicked() {
                app.queue(Action::AddQ0rg);
            }
            if ui
                .button("+ Frame")
                .on_hover_text("Insert frame (F5)")
                .clicked()
            {
                app.queue(Action::InsertFrame);
            }
            let keyframes_enabled = app.session.timeline_selection.is_some()
                || !app
                    .state
                    .project
                    .layer_is_folder(app.session.current_q0rg_id, app.session.current_layer_id);
            if ui
                .add_enabled(keyframes_enabled, egui::Button::new("+ Keyframe"))
                .on_hover_text("Insert keyframe (F6)")
                .clicked()
            {
                app.queue(Action::InsertKeyframe);
            }
            if ui
                .add_enabled(keyframes_enabled, egui::Button::new("+ Blank Key"))
                .on_hover_text("Insert blank keyframe (F7)")
                .clicked()
            {
                app.queue(Action::InsertBlankKeyframe);
            }
            if ui
                .button("- Frame")
                .on_hover_text("Remove selected frame(s) and close the time gap (Shift+F5)")
                .clicked()
            {
                app.queue(Action::RemoveFrame);
            }
            let can_tween = matches!(
                app.session.selection,
                crate::state::Selection::Placement { .. }
            );
            if ui
                .add_enabled(can_tween, egui::Button::new("Tween"))
                .on_hover_text("Toggle motion tween from selected keyframe (Ctrl+Alt+T)")
                .clicked()
            {
                app.queue(Action::ToggleMotionTween);
            }
        });
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
            if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
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
    if submit && !rename.draft.trim().is_empty() {
        app.queue(Action::RenameLayer(
            rename.q0rg_id,
            rename.layer_id,
            rename.draft,
        ));
    } else if open && !cancel {
        app.session.layer_rename = Some(rename);
    }
}

fn draw_layer_icon(
    painter: &egui::Painter,
    rect: Rect,
    is_folder: bool,
    collapsed: bool,
    foreground: Color32,
    accent: Color32,
) {
    if is_folder {
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
///   * inside `frame_count`, no placement covering this frame    → mid grey
///     (active layer is a touch brighter so it's clear which row drawing
///     will land in).
///   * beyond `frame_count`                                       → very dark
///   * beyond + every-5th                                          → slightly
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
    for f in 0..visible_frames {
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

fn draw_layer_spans(
    painter: &egui::Painter,
    theme: &Theme,
    layer: &q0s_format::v2::Layer,
    base_x: f32,
    row_y: f32,
    frame_count: u16,
    is_current_layer: bool,
) {
    let keyframes = visible_keyframes(layer, frame_count);

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
        let span_end = keyframes
            .get(position + 1)
            .copied()
            .map(|next| next.saturating_sub(1))
            .unwrap_or_else(|| frame_count.saturating_sub(1));
        if span_end < keyframe || layer.is_blank_keyframe(keyframe) {
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
    for keyframe in keyframe_separator_frames(&keyframes) {
        let x = base_x + f32::from(keyframe) * FRAME_W;
        painter.line_segment(
            [pos2(x, cell_top), pos2(x, cell_bot)],
            Stroke::new(1.0_f32, stroke_dark),
        );
    }

    let mut arrows = std::collections::BTreeSet::new();
    for placement in &layer.placements {
        if let Some(to_frame) = placement.tween.to_frame() {
            if placement.frame < frame_count && to_frame < frame_count {
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
    for keyframe in keyframes {
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

fn timeline_cell_at(
    rect: Rect,
    pos: egui::Pos2,
    frame_count: u16,
    layer_ids: &[u16],
    folder_layer_ids: &[u16],
) -> Option<(u16, u16)> {
    let rel_x = pos.x - (rect.min.x + LAYER_LABEL_W);
    if rel_x < 0.0 {
        return None;
    }
    let frame = (rel_x / FRAME_W).floor() as i32;
    if frame < 0 || frame >= i32::from(frame_count) {
        return None;
    }
    let row = ((pos.y - rect.min.y) / ROW_H).floor() as i32 - 1;
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

    for layer_index in first_layer..=last_layer {
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

    fn placement(frame: u16) -> Placement {
        Placement {
            frame,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
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
                name: "low child".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 2,
                name: "high child".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 3,
                name: "folder".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 4,
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
                    collapsed: false,
                },
            );
        }
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 3),
            q0s_format::v2::LayerMetadata {
                kind: q0s_format::v2::LayerKind::Folder,
                parent_folder_id: None,
                collapsed: false,
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
    fn layer_drop_hit_testing_enters_folder_and_outdents_from_its_own_row() {
        let mut project = crate::state::default_project();
        project.q0rgs[0].layers = vec![
            Layer {
                layer_id: 1,
                name: "child".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 2,
                name: "folder".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 3,
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
                collapsed: false,
            },
        );
        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 2),
            q0s_format::v2::LayerMetadata {
                kind: q0s_format::v2::LayerKind::Folder,
                parent_folder_id: None,
                collapsed: false,
            },
        );
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 120.0));
        let visible = [3, 2, 1];

        assert_eq!(
            layer_drop_target_at(&project, 1, rect, pos2(50.0, ROW_H * 2.5), &visible, 3,),
            Some(LayerDropTarget::IntoFolder(2))
        );
        assert_eq!(
            layer_drop_target_at(&project, 1, rect, pos2(8.0, ROW_H * 3.75), &visible, 1,),
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
            ),
            None
        );
    }
}
