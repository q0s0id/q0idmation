use std::collections::BTreeSet;

use egui::{
    pos2, vec2, Align2, Color32, FontId, PointerButton, Rect, ScrollArea, Sense, Stroke, Ui,
};

use crate::app::{Action, EditorApp, LayerDropTarget};
use crate::hotkeys::ShortcutCommand;
use crate::settings::{Theme, TimelineButton};
use crate::state::{
    CameraFrameDrag, CameraTimelineSelection, FrameLabelEdit, LayerRename, LibraryDragPayload,
    Selection, TimelineFrameDrag, TimelineLayerDrag, TimelineLayerSelection,
    TimelineRigKeySelection, TimelineSelection,
};
use crate::ui::{icon_button, icon_button_enabled, icon_toggle, FitToScreen, Icon};

use super::timeline_look as look;

/// The ruler's height; it keeps it however tall the rows are.
const RULER_H: f32 = 26.0;

thread_local! {
    /// Frame width and row height, from the settings, set as each
    /// timeline is drawn.
    static SIZES: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((12.0, 26.0)) };
    /// While a layer row is drawn: its own height.
    static ROW_NOW: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
    /// Each visible layer row's top below the first, and one past the last.
    static ROW_TOPS: std::cell::RefCell<Vec<f32>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// How wide a frame is.
fn frame_w() -> f32 {
    SIZES.with(|sizes| sizes.get().0)
}

/// How tall the row being drawn is: a layer row's own height while it is
/// drawn, otherwise the usual height.
fn row_h() -> f32 {
    ROW_NOW.with(std::cell::Cell::get).unwrap_or_else(rows_h)
}

/// The usual row height: the camera and rig tracks, and layers without
/// their own.
fn rows_h() -> f32 {
    SIZES.with(|sizes| sizes.get().1)
}

/// Where layer row `index` starts, below the first layer row.
fn layer_top(index: usize) -> f32 {
    ROW_TOPS.with(|tops| {
        let tops = tops.borrow();
        match tops.get(index) {
            Some(top) => *top,
            None => {
                let known = tops.len().saturating_sub(1).min(index);
                tops.get(known).copied().unwrap_or(0.0) + (index - known) as f32 * rows_h()
            }
        }
    })
}

fn layer_height(index: usize) -> f32 {
    layer_top(index + 1) - layer_top(index)
}

/// The layer row `offset` below the first one; negative above it.
fn layer_row_at(offset: f32) -> isize {
    if offset < 0.0 {
        return (offset / rows_h()).floor() as isize;
    }
    ROW_TOPS.with(|tops| {
        let tops = tops.borrow();
        let end = tops.last().copied().unwrap_or(0.0);
        if tops.len() >= 2 && offset < end {
            tops.partition_point(|top| *top <= offset) as isize - 1
        } else {
            let known = tops.len().saturating_sub(1);
            known as isize + ((offset - end) / rows_h()).floor() as isize
        }
    })
}

/// Layer rows that show between `top` and `bottom` of the screen.
fn visible_layer_rows(
    first_row_y: f32,
    count: usize,
    top: f32,
    bottom: f32,
) -> std::ops::Range<usize> {
    let clamp = |row: isize| row.clamp(0, count as isize) as usize;
    let start = clamp(layer_row_at(top - first_row_y));
    let end = clamp(layer_row_at(bottom - first_row_y) + 1);
    start..end.max(start)
}

/// Reads the sizes for this frame of drawing: the settings' frame width
/// and row height, and each visible layer's own height where it has one.
/// A row still `opening` (a new layer, folder or mask) takes that share of
/// its height, so the rows below slide down to make room for it.
fn set_sizes(app: &EditorApp, q0rg_id: u16, layer_ids: &[u16], opening: &[(u16, f32)]) {
    let frame = app.settings.timeline_frame_width.clamp(
        *crate::settings::TIMELINE_FRAME_WIDTHS.start(),
        *crate::settings::TIMELINE_FRAME_WIDTHS.end(),
    );
    let row = app.settings.timeline_row_height.clamp(
        *crate::settings::TIMELINE_ROW_HEIGHTS.start(),
        *crate::settings::TIMELINE_ROW_HEIGHTS.end(),
    );
    SIZES.with(|sizes| sizes.set((frame, row)));
    ROW_NOW.with(|now| now.set(None));
    ROW_TOPS.with(|tops| {
        let mut tops = tops.borrow_mut();
        tops.clear();
        let mut y = 0.0;
        tops.push(y);
        for layer_id in layer_ids {
            let share = opening
                .iter()
                .find(|(id, _)| id == layer_id)
                .map_or(1.0, |(_, share)| *share);
            y += app
                .session
                .timeline_row_heights
                .get(&(q0rg_id, *layer_id))
                .copied()
                .unwrap_or(row)
                * share;
            tops.push(y);
        }
    });
}

fn layer_row_tops() -> Vec<f32> {
    ROW_TOPS.with(|tops| tops.borrow().clone())
}
const LAYER_LABEL_W: f32 = 174.0;
/// How long a new layer row takes to open.
const ROW_OPEN_SECS: f32 = 0.24;
/// Gap above and below the cell fill — leaves room for thin row separators
/// like the original Flash CS3 timeline.
const CELL_PAD_Y: f32 = 1.0;
/// How many "virtual" frame slots to render past `frame_count` so the user
/// sees there's room to extend the q0rg. They render as darker chequered
/// cells and can hold a virtual playhead until F5/F6/F7 materialises them.
const VIRTUAL_FRAMES_BUFFER: u16 = 500;

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
        frame_w(),
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
                | q0s_format::v2::RigPropertyRef::ConstraintWeight(_)
                | q0s_format::v2::RigPropertyRef::BindingA11(_)
                | q0s_format::v2::RigPropertyRef::BindingA12(_)
                | q0s_format::v2::RigPropertyRef::BindingA21(_)
                | q0s_format::v2::RigPropertyRef::BindingA22(_)
                | q0s_format::v2::RigPropertyRef::BindingTx(_)
                | q0s_format::v2::RigPropertyRef::BindingTy(_) => false,
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

/// The bar that scrolls the frames: a rounded track that thickens under
/// the pointer, marks where the playhead and the timeline's end are, and a
/// thumb that lights while it is held.
fn timeline_frame_scrollbar(
    ui: &mut Ui,
    theme: &Theme,
    (label_width, viewport_width, content_width): (f32, f32, f32),
    (playhead_x, end_x): (f32, f32),
    offset: &mut f32,
) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
    let area = Rect::from_min_size(
        pos2(row.left() + label_width, row.top()),
        vec2(viewport_width, row.height()),
    );
    let max_scroll = (content_width - viewport_width).max(0.0);
    let thumb_width = (viewport_width * viewport_width / content_width.max(viewport_width))
        .clamp(24.0_f32.min(viewport_width), viewport_width);
    let travel = (viewport_width - thumb_width).max(0.0);
    let thumb_x = area.left()
        + if max_scroll > 0.0 {
            *offset / max_scroll * travel
        } else {
            0.0
        };
    let response = ui.interact(
        area,
        ui.id().with("timeline_frame_scrollbar"),
        Sense::click_and_drag(),
    );
    if response.drag_started() {
        let grab = response
            .interact_pointer_pos()
            .map(|point| {
                if (thumb_x..=thumb_x + thumb_width).contains(&point.x) {
                    point.x - thumb_x
                } else {
                    thumb_width * 0.5
                }
            })
            .unwrap_or(thumb_width * 0.5);
        ui.data_mut(|data| data.insert_temp(response.id, grab));
    }
    if (response.dragged() || response.clicked()) && travel > 0.0 {
        if let Some(point) = response.interact_pointer_pos() {
            let grab = if response.clicked() {
                thumb_width * 0.5
            } else {
                ui.data(|data| data.get_temp::<f32>(response.id))
                    .unwrap_or(thumb_width * 0.5)
            };
            *offset = ((point.x - grab - area.left()) / travel * max_scroll).clamp(0.0, max_scroll);
        }
    }
    let held = look::fade(ui.ctx(), response.id.with("held"), response.dragged(), 0.12);
    let hot = look::fade(
        ui.ctx(),
        response.id.with("hot"),
        response.hovered() || response.dragged(),
        0.14,
    );
    let thickness = 6.0 + 4.0 * hot;
    let track = Rect::from_center_size(area.center(), vec2(area.width(), thickness));
    let painter = ui.painter_at(row);
    painter.rect_filled(track, thickness * 0.5, ui.visuals().extreme_bg_color);
    // Where the playhead and the timeline's end sit along the whole strip.
    let along = |x: f32| track.left() + x / content_width.max(1.0) * track.width();
    let end = along(end_x);
    if end < track.right() - 1.0 {
        painter.line_segment(
            [
                pos2(end, track.top() - 1.0),
                pos2(end, track.bottom() + 1.0),
            ],
            Stroke::new(
                1.5_f32,
                Color32::from_rgb(0xC8, 0x95, 0x30).gamma_multiply(0.8),
            ),
        );
    }
    let thumb = Rect::from_min_size(
        pos2(thumb_x, track.top()),
        vec2(thumb_width, track.height()),
    );
    let accent = theme.accent.to_color32();
    let base = look::mix(theme.panel.to_color32(), theme.text.to_color32(), 0.28);
    let fill = look::mix(base, accent, 0.25 * hot + 0.45 * held);
    painter.add(crate::ui::gradient_shape(
        ui.ctx(),
        thumb,
        thickness * 0.5,
        [
            look::mix(fill, Color32::WHITE, 0.12),
            look::mix(fill, Color32::BLACK, 0.06),
        ],
    ));
    painter.rect_stroke(
        thumb,
        thickness * 0.5,
        Stroke::new(1.0_f32, theme.stroke_dark.to_color32().gamma_multiply(0.6)),
    );
    let head = along(playhead_x);
    painter.line_segment(
        [
            pos2(head, track.top() - 2.0),
            pos2(head, track.bottom() + 2.0),
        ],
        Stroke::new(2.0_f32, theme.playhead.to_color32()),
    );
    response
        .on_hover_text("Scroll frames; layer names stay fixed. Shift + wheel also scrolls frames.");
}

fn timeline_ruler(
    app: &mut EditorApp,
    theme: &Theme,
    ui: &mut Ui,
    q0rg_idx: usize,
    label_width: f32,
    scroll_x: f32,
    visible_frames: u16,
) {
    let q0rg_id = app.state.project.q0rgs[q0rg_idx].q0rg_id;
    let frame_count = app.state.project.q0rgs[q0rg_idx].frame_count;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), RULER_H), Sense::click_and_drag());
    let label_edge_x = rect.left() + label_width;
    let frame_base_x = label_edge_x - scroll_x;
    let painter = ui.painter_at(rect);
    let header = theme.timeline_header.to_color32();
    painter.add(crate::ui::gradient_shape(
        ui.ctx(),
        rect,
        0.0,
        [
            look::mix(header, Color32::WHITE, 0.05),
            look::mix(header, Color32::BLACK, 0.06),
        ],
    ));
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
    );
    let drawable_layer_ids: Vec<_> = app.state.project.q0rgs[q0rg_idx]
        .layers
        .iter()
        .filter(|layer| !app.state.project.layer_is_folder(q0rg_id, layer.layer_id))
        .map(|layer| layer.layer_id)
        .collect();
    let has_layers = !drawable_layer_ids.is_empty();
    let all_locked = has_layers
        && drawable_layer_ids
            .iter()
            .all(|id| app.state.project.layer_is_locked(q0rg_id, *id));
    let all_hidden = has_layers
        && drawable_layer_ids
            .iter()
            .all(|id| !app.state.project.layer_is_visible(q0rg_id, *id));
    let all_outlined = has_layers
        && drawable_layer_ids
            .iter()
            .all(|id| app.session.outline_layers.contains(&(q0rg_id, *id)));
    for (offset, enabled, action, hint) in [
        (
            49.0,
            all_outlined,
            Action::ToggleAllLayerOutlines(q0rg_id),
            if all_outlined {
                "Disable all layer outlines"
            } else {
                "Outline all layers"
            },
        ),
        (
            31.0,
            all_hidden,
            Action::ToggleAllLayerVisibility(q0rg_id),
            if all_hidden {
                "Show the layers that Hide all hid. Shift-click: show every layer"
            } else {
                "Hide all layers. Shift-click: show every layer"
            },
        ),
        (
            13.0,
            all_locked,
            Action::ToggleAllLayerLocks(q0rg_id),
            if all_locked {
                "Unlock the layers that Lock all locked. Shift-click: unlock every layer"
            } else {
                "Lock all layers. Shift-click: unlock every layer"
            },
        ),
    ] {
        let control = Rect::from_center_size(
            pos2(label_edge_x - offset, rect.center().y),
            vec2(18.0, RULER_H),
        );
        let control_response = ui
            .interact(
                control,
                egui::Id::new(("timeline_all_layers", q0rg_id, offset as u16)),
                if has_layers {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            )
            .on_hover_text(hint);
        let switch_id = egui::Id::new(("timeline_all_layers_look", q0rg_id, offset as u16));
        let lit = look::fade(
            ui.ctx(),
            switch_id.with("hover"),
            control_response.hovered(),
            0.1,
        );
        if lit > 0.01 {
            painter.rect_filled(
                control.shrink(1.0),
                3.0,
                theme.accent.to_color32().gamma_multiply(0.15 * lit),
            );
        }
        let on = look::fade(ui.ctx(), switch_id, enabled, 0.16);
        let color = if has_layers {
            look::mix(theme.text.to_color32(), theme.accent.to_color32(), on)
        } else {
            theme.text_dim.to_color32()
        };
        match action {
            Action::ToggleAllLayerOutlines(_) => {
                draw_outline_icon(&painter, control.shrink2(vec2(4.0, 5.0)), on, color);
            }
            Action::ToggleAllLayerVisibility(_) => {
                draw_visibility_icon(&painter, control.shrink2(vec2(2.0, 4.0)), 1.0 - on, color);
            }
            _ => draw_lock_icon(&painter, control.shrink2(vec2(2.0, 3.0)), on, color),
        }
        if control_response.clicked() {
            let shift = ui.input(|input| input.modifiers.shift);
            app.queue(match action {
                Action::ToggleAllLayerLocks(id) if shift => Action::UnlockAllLayers(id),
                Action::ToggleAllLayerVisibility(id) if shift => Action::ShowAllLayers(id),
                other => other,
            });
        }
    }
    // The parenting view's switch, above the column it opens.
    let parenting = Rect::from_center_size(
        pos2(label_edge_x - 72.0, rect.center().y),
        vec2(18.0, RULER_H),
    );
    let shown = crate::layer_parenting_ui::shown(app);
    let parenting_response = ui
        .interact(
            parenting,
            egui::Id::new(("timeline_parenting_toggle", q0rg_id)),
            Sense::click(),
        )
        .on_hover_text(
            "Parenting view: hang layers from other layers, so they move, turn and scale with them",
        );
    if parenting_response.hovered() {
        painter.rect_filled(
            parenting.shrink(1.0),
            2.0,
            theme.accent.to_color32().gamma_multiply(0.15),
        );
    }
    crate::ui::paint_icon(
        &painter,
        Rect::from_center_size(parenting.center(), vec2(12.0, 12.0)),
        Icon::Parenting,
        if shown {
            theme.accent.to_color32()
        } else {
            theme.text.to_color32()
        },
    );
    if parenting_response.clicked() {
        crate::layer_parenting_ui::toggle(app);
    }
    let frame_painter = ui.painter_at(Rect::from_min_max(pos2(label_edge_x, rect.top()), rect.max));
    let mut adjusting_range = false;
    if app.session.onion.enabled || app.session.onion.edit_frames {
        let playhead = app.session.timeline_frame();
        let start = playhead.saturating_sub(app.session.onion.before);
        let end = playhead
            .saturating_add(app.session.onion.after)
            .min(visible_frames.saturating_sub(1));
        let left = frame_base_x + f32::from(start) * frame_w();
        let right = frame_base_x + (f32::from(end) + 1.0) * frame_w();
        frame_painter.rect_filled(
            Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
            0.0,
            Color32::from_white_alpha(18),
        );
        for (past, x, color) in [
            (true, left, theme.onion_past.to_color32()),
            (false, right, theme.onion_future.to_color32()),
        ] {
            let handle = Rect::from_center_size(pos2(x, rect.center().y), vec2(8.0, RULER_H))
                .intersect(frame_painter.clip_rect());
            let drag = ui
                .interact(
                    handle,
                    egui::Id::new(("frame_range_handle", q0rg_id, past)),
                    Sense::drag(),
                )
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                .on_hover_text(if past {
                    "Drag to set the first visible frame"
                } else {
                    "Drag to set the last visible frame"
                });
            adjusting_range |= drag.dragged() || drag.drag_started() || drag.hovered();
            if drag.dragged() {
                if let Some(pointer) = drag.interact_pointer_pos() {
                    if app.session.onion.edit_frames {
                        app.session.timeline_cell_selection = None;
                    }
                    let boundary = ((pointer.x - frame_base_x) / frame_w())
                        .round()
                        .clamp(0.0, f32::from(visible_frames))
                        as u16;
                    if past {
                        app.session.onion.before = playhead.saturating_sub(boundary.min(playhead));
                    } else {
                        app.session.onion.after = boundary
                            .saturating_sub(1)
                            .max(playhead)
                            .saturating_sub(playhead);
                    }
                }
            }
            frame_painter.line_segment(
                [pos2(x, rect.top() + 2.0), pos2(x, rect.bottom() - 2.0)],
                Stroke::new(2.5_f32, color),
            );
            let direction = if past { 1.0 } else { -1.0 };
            for y in [rect.top() + 2.0, rect.bottom() - 2.0] {
                frame_painter.line_segment(
                    [pos2(x, y), pos2(x + direction * 5.0, y)],
                    Stroke::new(2.0_f32, color),
                );
            }
        }
    }
    let frame_slots = visible_frame_slots(&frame_painter, frame_base_x, visible_frames);
    let fps = u32::from(app.state.project.meta.fps);
    let fw = frame_w();
    // A number every fifth frame, or fewer when frames are too narrow to
    // fit them; a tick at every frame that has room for one.
    let every = if fw * 5.0 >= 22.0 {
        5
    } else if fw * 10.0 >= 22.0 {
        10
    } else {
        20
    };
    let tick = theme.text_dim.to_color32();
    // Seconds every so many, so their pills never run into each other.
    let seconds_every = [1_u32, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600]
        .into_iter()
        .find(|every| *every as f32 * fps.max(1) as f32 * fw >= 40.0)
        .unwrap_or(7200);
    for f in frame_slots.clone() {
        let x = frame_base_x + f32::from(f) * fw;
        let frame = u32::from(f) + 1;
        if f >= frame_count {
            continue;
        }
        let major = f % 5 == 0;
        if major || fw >= 6.0 {
            let height = if major { 6.0 } else { 3.0 };
            frame_painter.line_segment(
                [pos2(x, rect.bottom() - height), pos2(x, rect.bottom())],
                Stroke::new(1.0_f32, tick.gamma_multiply(if major { 0.6 } else { 0.32 })),
            );
        }
        if fps > 0 && frame.is_multiple_of(fps * seconds_every) {
            let galley = frame_painter.layout_no_wrap(
                format!("{}s", frame / fps),
                FontId::proportional(9.0),
                theme.text.to_color32(),
            );
            let pill = Rect::from_center_size(
                pos2(x + fw * 0.5, rect.top() + 6.0),
                galley.size() + vec2(8.0, 0.0),
            );
            frame_painter.rect_filled(
                pill,
                pill.height() * 0.5,
                theme.accent.to_color32().gamma_multiply(0.24),
            );
            frame_painter.galley(
                pill.center() - galley.size() * 0.5,
                galley,
                theme.text.to_color32(),
            );
        }
        if frame.is_multiple_of(every) || f == 0 {
            frame_painter.text(
                pos2(x + fw * 0.5, rect.top() + 15.0),
                Align2::CENTER_CENTER,
                format!("{frame}"),
                FontId::proportional(10.0),
                theme.text_dim.to_color32(),
            );
        }
    }
    for (key, label) in &app.state.project.runtime.frame_labels {
        if key.q0rg_id != q0rg_id || !frame_slots.contains(&key.frame) {
            continue;
        }
        let x = frame_base_x + f32::from(key.frame) * frame_w() + 1.0;
        let y = rect.top() + 2.0;
        let color = theme.accent.to_color32();
        frame_painter.line_segment([pos2(x, y), pos2(x, y + 9.0)], Stroke::new(1.3_f32, color));
        frame_painter.add(egui::Shape::convex_polygon(
            vec![pos2(x, y), pos2(x + 7.0, y + 2.5), pos2(x, y + 5.0)],
            color,
            Stroke::NONE,
        ));
        ui.interact(
            Rect::from_min_size(pos2(x, rect.top()), vec2(frame_w(), RULER_H))
                .intersect(frame_painter.clip_rect()),
            egui::Id::new(("frame_label_hover", q0rg_id, key.frame)),
            Sense::hover(),
        )
        .on_hover_text(format!("{label}\nframe {}", u32::from(key.frame) + 1));
    }
    let edge_x = frame_base_x + f32::from(frame_count) * frame_w();
    for offset in [0.0, 7.0, 14.0, 21.0] {
        frame_painter.line_segment(
            [
                pos2(edge_x, rect.top() + offset),
                pos2(edge_x, rect.top() + offset + 4.0),
            ],
            Stroke::new(1.0_f32, Color32::from_rgb(0xC8, 0x95, 0x30)),
        );
    }
    // Scrubbing: the playhead follows the pointer over the frames. Pushed
    // past either edge of them (the layer names count as past the left
    // one), it runs on that way, faster the further out the pointer is.
    let carry_id = egui::Id::new(("timeline_scrub_edge_carry", q0rg_id));
    let mut carry = 0.0_f32;
    let pressed_on_frames = ui
        .input(|input| input.pointer.press_origin())
        .is_some_and(|origin| origin.x >= label_edge_x);
    if !adjusting_range
        && app.session.timeline_layer_drag.is_none()
        && (response.clicked_by(PointerButton::Primary)
            || (pressed_on_frames && response.dragged_by(PointerButton::Primary)))
    {
        if let Some(pos) = response
            .interact_pointer_pos()
            .filter(|pos| response.dragged() || pos.x >= label_edge_x)
        {
            let frames_right = label_edge_x + (rect.width() - label_width - 12.0).max(frame_w());
            let past = if pos.x < label_edge_x {
                pos.x - label_edge_x
            } else if pos.x > frames_right {
                pos.x - frames_right
            } else {
                0.0
            };
            let last = visible_frames.saturating_sub(1);
            if past == 0.0 || !response.dragged() {
                let frame = ((pos.x - frame_base_x) / frame_w()).floor();
                if frame >= 0.0 && frame < f32::from(visible_frames) {
                    app.session.set_timeline_frame(frame as u16, frame_count);
                }
            } else {
                let dt = ui.input(|input| input.unstable_dt).clamp(0.0, 0.1);
                carry = ui
                    .data(|data| data.get_temp::<f32>(carry_id))
                    .unwrap_or(0.0)
                    + edge_scrub_rate(past.abs()) * dt;
                let steps = carry.floor();
                carry -= steps;
                let steps = steps as u16;
                // From the frame at that edge, the last one wholly in sight.
                let playhead = app.session.timeline_frame();
                let next = if past < 0.0 {
                    let edge = (scroll_x / frame_w()).ceil() as u16;
                    playhead.min(edge).saturating_sub(steps)
                } else {
                    let edge =
                        ((scroll_x + frames_right - label_edge_x) / frame_w()).floor() as u16;
                    playhead
                        .max(edge.saturating_sub(1))
                        .saturating_add(steps)
                        .min(last)
                };
                app.session.set_timeline_frame(next, frame_count);
                ui.ctx().request_repaint();
            }
        }
    }
    ui.data_mut(|data| data.insert_temp(carry_id, carry));
    // The frame under the pointer, here or over the rows below (where they
    // were last frame), gets an outlined tab that says which frame it is.
    let playhead = app.session.timeline_frame();
    let body = ui.data(|data| data.get_temp::<Rect>(body_frames_id(q0rg_id)));
    let hover_frame = ui
        .ctx()
        .pointer_hover_pos()
        .filter(|pos| {
            pos.x >= label_edge_x
                && (ui.rect_contains_pointer(frame_painter.clip_rect())
                    || body.is_some_and(|body| ui.rect_contains_pointer(body)))
        })
        .map(|pos| ((pos.x - frame_base_x) / frame_w()).floor())
        .filter(|frame| *frame >= 0.0 && *frame < f32::from(visible_frames));
    let ghost_id = egui::Id::new(("timeline_ghost_frame", q0rg_id));
    let ghost = look::fade(
        ui.ctx(),
        ghost_id,
        hover_frame.is_some_and(|frame| frame as u16 != playhead)
            && !response.dragged()
            && !app.session.playing,
        0.12,
    );
    if let Some(frame) = hover_frame {
        ui.data_mut(|data| data.insert_temp(ghost_id, frame));
    }
    if let Some(frame) = ui.data(|data| data.get_temp::<f32>(ghost_id)) {
        let x = frame_base_x + (look::glide(ui.ctx(), ghost_id, frame, 0.08) + 0.5) * frame_w();
        look::paint_ghost_head(
            &frame_painter,
            x,
            rect,
            format!("{}", frame as u32 + 1),
            (
                theme.panel.to_color32(),
                theme.text_dim.to_color32(),
                theme.text.to_color32(),
            ),
            ghost * 0.9,
        );
    }
    let playhead_x = frame_base_x + (shown_playhead(ui.ctx(), app, q0rg_id) + 0.5) * frame_w();
    look::paint_playhead_head(
        &frame_painter,
        playhead_x,
        rect,
        format!("{}", u32::from(playhead) + 1),
        theme.playhead.to_color32(),
    );
}

/// Where in a dragged block of frames it was picked up: (frames from its
/// left, rows from its top).
fn frame_grab_id(q0rg_id: u16) -> egui::Id {
    egui::Id::new(("timeline_frame_grab", q0rg_id))
}

/// Where in a dragged stretch of camera frames it was picked up.
fn camera_grab_id(q0rg_id: u16) -> egui::Id {
    egui::Id::new(("timeline_camera_grab", q0rg_id))
}

/// How fast the playhead runs on while scrubbing past an edge of the
/// frames, in frames a second, with the pointer `past` points beyond it: a
/// crawl just past the edge, quicker the further out.
fn edge_scrub_rate(past: f32) -> f32 {
    let past = past.max(0.0);
    (2.0 + 0.35 * past + 0.004 * past * past).min(200.0)
}

/// Where the rows' frame area was drawn, so the ruler can tell the pointer
/// is over it.
fn body_frames_id(q0rg_id: u16) -> egui::Id {
    egui::Id::new(("timeline_body_frames", q0rg_id))
}

/// Where the playhead shows, in frames. It glides to a new frame when
/// clicked or stepped there; while playing it keeps exact time.
fn shown_playhead(ctx: &egui::Context, app: &EditorApp, q0rg_id: u16) -> f32 {
    look::glide(
        ctx,
        egui::Id::new(("timeline_playhead_glide", q0rg_id)),
        f32::from(app.session.timeline_frame()),
        if app.session.playing { 0.0 } else { 0.12 },
    )
}

/// What the rows need to know about the playhead.
#[derive(Clone, Copy)]
struct Playhead {
    /// The frame it is on.
    frame: u16,
    /// Where it shows while it glides there, in frames.
    shown: f32,
}

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    let scrub_frame_before = app.session.timeline_frame();
    let theme = app.settings.theme.clone();
    look::set_motion(app.settings.timeline_motion);
    look::set_key_look(look::KeyLook {
        round: app.settings.timeline_key_round,
        shine: app.settings.timeline_dot_shine,
    });
    let label_width = f32::from(app.settings.timeline_layer_width).clamp(
        112.0,
        (ui.available_width() * 0.65).clamp(LAYER_LABEL_W, 520.0),
    );
    let panel = ui.available_rect_before_wrap();
    crate::select_all::note_timeline(ui.ctx(), panel, panel.left() + label_width);
    set_sizes(app, app.session.current_q0rg_id, &[], &[]);
    save_sizes_when_settled(app, ui.ctx());
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
    let row_frame_counts = crate::timeline_edit::layer_frame_counts(&app.state.project, q0rg_id);
    // Model order is back-to-front for rendering. The timeline displays the
    // conventional front-to-back order, so rows are collected in reverse.
    // Folder children live immediately before their folder in model order;
    // reverse traversal therefore shows the folder row before its children.
    let shown_now = visible_layer_indices(&app.state.project, q0rg_idx);
    let ids_now: Vec<u16> = shown_now
        .iter()
        .map(|index| app.state.project.q0rgs[q0rg_idx].layers[*index].layer_id)
        .collect();
    let rows_id = egui::Id::new(("timeline_rows", app.active_project_tab_id(), q0rg_id));
    // A new row opens like a new frame pops: it grows to its height, and
    // the rows under it slide down. Rows a folder or mask closes over
    // shrink away where they were instead of vanishing.
    let mut opening = look::arrivals(ui.ctx(), rows_id, &ids_now, ROW_OPEN_SECS, 32)
        .into_iter()
        .map(|(layer_id, age)| (layer_id, look::ease_out(age / ROW_OPEN_SECS)))
        .collect::<Vec<_>>();
    let (layer_ids, closing) = {
        let layers = &app.state.project.q0rgs[q0rg_idx].layers;
        look::with_leaving(
            ui.ctx(),
            rows_id,
            &ids_now,
            |id| layers.iter().any(|layer| layer.layer_id == id),
            ROW_OPEN_SECS,
            64,
        )
    };
    opening.extend(closing);
    // Another project's timeline in this one's place: the rows open in a
    // wave from the top.
    for (layer_id, share) in look::switch_wave(
        ui.ctx(),
        egui::Id::new("timeline_rows_switch"),
        app.active_project_tab_id().unwrap_or(0),
        &layer_ids,
    ) {
        match opening.iter_mut().find(|(id, _)| *id == layer_id) {
            Some((_, open)) => *open = open.min(share),
            None => opening.push((layer_id, share)),
        }
    }
    let visible_layer_indices: Vec<usize> = {
        let layers = &app.state.project.q0rgs[q0rg_idx].layers;
        layer_ids
            .iter()
            .filter_map(|id| layers.iter().position(|layer| layer.layer_id == *id))
            .collect()
    };
    let layer_count = visible_layer_indices.len();
    let rig_tracks = rig_timeline_tracks(&app.state.project, q0rg_id, &visible_layer_indices);
    let rig_track_count = rig_tracks.len();
    set_sizes(app, q0rg_id, &layer_ids, &opening);
    let folder_layer_ids: Vec<u16> = layer_ids
        .iter()
        .copied()
        .filter(|layer_id| app.state.project.layer_is_folder(q0rg_id, *layer_id))
        .collect();
    // Virtual frame slots past `frame_count` are rendered too so the user
    // sees the q0rg's edge and has somewhere to drop "+ Frame" hits visually.
    let visible_frames = frame_count.saturating_add(VIRTUAL_FRAMES_BUFFER);

    let camera_row_enabled = app.state.project.camera_on(q0rg_id);
    let camera_row_count = usize::from(camera_row_enabled);

    let available_frames_width = (ui.available_width() - label_width - 12.0).max(frame_w());
    let frame_strip_width = f32::from(visible_frames) * frame_w();
    let max_scroll = (frame_strip_width - available_frames_width).max(0.0);
    app.session.timeline_horizontal_scroll = app
        .session
        .timeline_horizontal_scroll
        .clamp(0.0, max_scroll);
    // The frames follow the playhead when it moves on its own (F6 on a
    // keyframe, the frame keys, playback), so a new frame never lands out
    // of sight past the edge.
    let follow_id = egui::Id::new(("timeline_followed_frame", q0rg_id));
    let playhead = app.session.timeline_frame();
    let followed = ui.data(|data| data.get_temp::<u16>(follow_id));
    if followed.is_some_and(|frame| frame != playhead)
        && app.session.timeline_scroll_to_frame.is_none()
    {
        app.session.timeline_scroll_to_frame = Some((q0rg_id, playhead));
    }
    ui.data_mut(|data| data.insert_temp(follow_id, playhead));
    if let Some((target_q0rg, target)) = app.session.timeline_scroll_to_frame.take() {
        if target_q0rg == q0rg_id {
            let x = f32::from(target.min(visible_frames.saturating_sub(1))) * frame_w();
            let scroll = &mut app.session.timeline_horizontal_scroll;
            if x < *scroll {
                *scroll = x;
            }
            if x + frame_w() > *scroll + available_frames_width {
                *scroll = x + frame_w() - available_frames_width;
            }
            *scroll = scroll.clamp(0.0, max_scroll);
        }
    }
    timeline_frame_scrollbar(
        ui,
        &theme,
        (label_width, available_frames_width, frame_strip_width),
        (
            (f32::from(app.session.timeline_frame()) + 0.5) * frame_w(),
            f32::from(frame_count) * frame_w(),
        ),
        &mut app.session.timeline_horizontal_scroll,
    );
    let scroll_x = app.session.timeline_horizontal_scroll;
    let row_spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    timeline_ruler(
        app,
        &theme,
        ui,
        q0rg_idx,
        label_width,
        scroll_x,
        visible_frames,
    );
    ui.spacing_mut().item_spacing.y = row_spacing;
    ScrollArea::vertical()
        .id_source(("timeline_layers", q0rg_id))
        .auto_shrink([false, false])
        .max_height(ui.available_height())
        .show(ui, |ui| {
            let total_h = rows_h() * (rig_track_count as f32 + camera_row_count as f32 + 1.0)
                + layer_top(layer_count);
            let (body_rect, response) = ui.allocate_exact_size(
                vec2(ui.available_width(), total_h - rows_h()),
                Sense::click_and_drag(),
            );
            // Row helpers measure from a logical header origin. Only the body
            // occupies this scroll area; the ruler above has its own layout.
            let rect = Rect::from_min_max(body_rect.min - vec2(0.0, rows_h()), body_rect.max);
            let clip = ui.clip_rect().intersect(body_rect);
            let label_edge_x = rect.min.x + label_width;
            let frame_base_x = label_edge_x - scroll_x;
            let frame_clip = Rect::from_min_max(pos2(label_edge_x, clip.top()), clip.max);
            ui.data_mut(|data| data.insert_temp(body_frames_id(q0rg_id), frame_clip));
            let painter = ui.painter_at(frame_clip);
            let label_painter = ui.painter_at(Rect::from_min_max(
                clip.min,
                pos2(label_edge_x, clip.bottom()),
            ));
            // Ctrl+wheel makes frames wider or narrower, keeping the frame
            // under the pointer where it is.
            // The width eases to where the wheel sent it: (the width aimed
            // at, the frame that was under the pointer, the pointer's
            // distance from the names), and that frame stays put under the
            // pointer all the way there.
            let zoom_id = egui::Id::new(("timeline_zoom", q0rg_id));
            let mut zooming = ui.data(|data| data.get_temp::<(f32, f32, f32)>(zoom_id));
            let zoom = ui.input(|input| input.zoom_delta());
            if zoom != 1.0 {
                if let Some(pointer) = response.hover_pos().filter(|pos| pos.x >= label_edge_x) {
                    let old = frame_w();
                    let aim = zooming.map_or(old, |(aim, _, _)| aim);
                    let new = (aim * zoom).clamp(
                        *crate::settings::TIMELINE_FRAME_WIDTHS.start(),
                        *crate::settings::TIMELINE_FRAME_WIDTHS.end(),
                    );
                    zooming = Some((
                        new,
                        (pointer.x - frame_base_x) / old,
                        pointer.x - label_edge_x,
                    ));
                }
            }
            let width = look::glide(
                ui.ctx(),
                zoom_id,
                zooming.map_or(frame_w(), |(aim, _, _)| aim),
                0.16,
            );
            if let Some((aim, frame, offset)) = zooming {
                app.settings.timeline_frame_width = width;
                app.session.timeline_horizontal_scroll = (frame * width - offset).max(0.0);
                app.session.timeline_sizes_changed = Some(std::time::Instant::now());
                if width == aim {
                    zooming = None;
                }
            }
            ui.data_mut(|data| match zooming {
                Some(zooming) => data.insert_temp(zoom_id, zooming),
                None => data.remove::<(f32, f32, f32)>(zoom_id),
            });
            if response.hovered() {
                let delta = ui.input(|input| {
                    if input.modifiers.shift {
                        input.smooth_scroll_delta.y + input.smooth_scroll_delta.x
                    } else {
                        input.smooth_scroll_delta.x
                    }
                });
                app.session.timeline_horizontal_scroll = (scroll_x - delta).clamp(0.0, max_scroll);
                ui.input_mut(|input| {
                    if input.modifiers.shift {
                        input.smooth_scroll_delta.y = 0.0;
                    }
                });
            }
            let splitter_rect = Rect::from_min_max(
                pos2(label_edge_x - 3.0, rect.top()),
                pos2(label_edge_x + 3.0, rect.bottom()),
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
            let frame_slots = visible_frame_slots(&painter, frame_base_x, visible_frames);
            let layer_rows_y = rect.min.y + rows_h() * (camera_row_count as f32 + 1.0);
            let layer_row_slots =
                visible_layer_rows(layer_rows_y, layer_count, clip.top(), clip.bottom());
            let rig_rows_y = layer_rows_y + layer_top(layer_count);
            let rig_row_slots = visible_slots(
                rig_rows_y,
                rows_h(),
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
                    let row_y = layer_rows_y + layer_top(row_index);
                    let target_rect = Rect::from_min_size(
                        pos2(rect.min.x, row_y),
                        vec2(rect.width(), layer_height(row_index)),
                    );
                    ui.scroll_to_rect(target_rect, Some(egui::Align::Center));
                    app.session.timeline_scroll_to_layer = None;
                }
            }

            // Timeline base panel — same as the editor's window colour
            // so it visually merges with the surrounding chrome.
            painter.rect_filled(rect, 0.0, theme.window.to_color32());
            label_painter.rect_filled(rect, 0.0, theme.window.to_color32());

            for f in frame_slots.clone() {
                let x = frame_base_x + f32::from(f) * frame_w();
                let color = if f % 5 == 0 {
                    if f >= frame_count {
                        theme.empty_beyond_5.to_color32()
                    } else {
                        theme.timeline_grid_5.to_color32()
                    }
                } else {
                    theme.timeline_grid.to_color32()
                };
                painter.line_segment(
                    [pos2(x, body_rect.top()), pos2(x, body_rect.bottom())],
                    Stroke::new(1.0_f32, color),
                );
            }
            // End-of-q0rg marker: dashed amber bar at the right edge of the
            // last real frame so the user can see exactly where `frame_count`
            // ends without confusing it with the solid theme-coloured playhead.
            let edge_x = frame_base_x + (frame_count as f32) * frame_w();
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

            let playhead = Playhead {
                frame: app.session.timeline_frame(),
                shown: shown_playhead(ui.ctx(), app, q0rg_id),
            };
            if camera_row_enabled {
                let row_y = rect.min.y + rows_h();
                draw_camera_layer_row(
                    app,
                    &label_painter,
                    &theme,
                    rect,
                    row_y,
                    frame_count,
                    visible_frames,
                    label_width,
                    frame_base_x,
                    &painter,
                    playhead,
                );
                // The camera's parenting square, where a layer's would be.
                let label_rect =
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(label_width, row_h()));
                let controls_left = label_rect.right() - 58.0;
                let column = Rect::from_min_max(
                    pos2(
                        controls_left - 2.0 - crate::layer_parenting_ui::column_width(app),
                        label_rect.top(),
                    ),
                    pos2(controls_left - 2.0, label_rect.bottom()),
                );
                crate::layer_parenting_ui::row(
                    app,
                    ui,
                    &label_painter,
                    q0rg_id,
                    q0s_format::parenting::CAMERA_LAYER,
                    column,
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(rect.width(), row_h())),
                    false,
                );
            }

            // The row under the pointer lights a little; the playhead is
            // drawn where it glides, and the rows' beads ride with it.
            let pointer = ui
                .ctx()
                .pointer_hover_pos()
                .filter(|pos| clip.contains(*pos) && ui.rect_contains_pointer(clip));
            let hover_row = pointer
                .map(|pos| layer_row_at(pos.y - layer_rows_y))
                .and_then(|row| usize::try_from(row).ok());

            // Layer rows
            let mut click_layer: Option<u16> = None;
            let mut click_frame: Option<u16> = None;
            let mut click_label_layer: Option<u16> = None;
            let mut open_frame_script: Option<(u16, u16, u16)> = None;
            let mut started_layer_drag: Option<(u16, TimelineLayerSelection)> = None;
            let mut flag_paints: Vec<Action> = Vec::new();
            for (li, layer_index) in visible_layer_indices
                .iter()
                .copied()
                .enumerate()
                .take(layer_row_slots.end)
                .skip(layer_row_slots.start)
            {
                let layer = &app.state.project.q0rgs[q0rg_idx].layers[layer_index];
                let row_layer_id = layer.layer_id;
                let layer_frame_count = row_frame_counts.get(&row_layer_id).copied().unwrap_or(0);
                let metadata = app.state.project.layer_metadata(q0rg_id, layer.layer_id);
                let is_folder = metadata.kind == q0s_format::v2::LayerKind::Folder;
                let is_mask = metadata.kind == q0s_format::v2::LayerKind::Mask;
                let effective_visible = app.state.project.layer_is_visible(q0rg_id, layer.layer_id);
                let effective_locked = app.state.project.layer_is_locked(q0rg_id, layer.layer_id);
                let row_y = layer_rows_y + layer_top(li);
                // A row still opening is laid out at its full height and
                // shown through the share it has opened so far.
                let full_height = app
                    .session
                    .timeline_row_heights
                    .get(&(q0rg_id, row_layer_id))
                    .copied()
                    .unwrap_or_else(rows_h);
                let opened = layer_height(li);
                ROW_NOW.with(|now| now.set(Some(full_height.max(opened))));
                let (painter, label_painter) = if opened + 0.01 < full_height {
                    let shown = Rect::from_min_max(
                        pos2(clip.min.x, row_y),
                        pos2(clip.max.x, row_y + opened),
                    );
                    (
                        painter.with_clip_rect(painter.clip_rect().intersect(shown)),
                        label_painter.with_clip_rect(label_painter.clip_rect().intersect(shown)),
                    )
                } else {
                    (painter.clone(), label_painter.clone())
                };
                let row_rect =
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(rect.width(), row_h()));
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
                    frame_base_x,
                    row_y,
                    layer_frame_count,
                    visible_frames,
                    is_current_layer,
                );

                // Label (left-side fixed column) — pull tones from the
                // panel/window pair so any theme reads as a coherent strip.
                let label_rect =
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(label_width, row_h()));
                let zebra = if li % 2 == 0 {
                    blend(theme.panel.to_color32(), theme.window.to_color32(), 0.5)
                } else {
                    theme.window.to_color32()
                };
                let row_look = egui::Id::new(("timeline_row_look", q0rg_id, row_layer_id));
                let current = look::fade(ui.ctx(), row_look, is_current_layer, 0.18);
                let hovered =
                    look::fade(ui.ctx(), row_look.with("hover"), hover_row == Some(li), 0.1);
                let accent = theme.accent.to_color32();
                let lit = look::mix(zebra, accent, 0.07 * hovered);
                if current > 0.0 {
                    label_painter.add(crate::ui::gradient_shape(
                        ui.ctx(),
                        label_rect,
                        0.0,
                        [
                            look::mix(lit, accent, 0.3 * current),
                            look::mix(lit, accent, 0.17 * current),
                        ],
                    ));
                } else {
                    label_painter.rect_filled(label_rect, 0.0, lit);
                }
                let selected =
                    look::fade(ui.ctx(), row_look.with("selected"), is_selected_layer, 0.14);
                if selected > 0.01 {
                    let band = label_rect.shrink2(vec2(1.5, 1.5));
                    label_painter.rect_filled(band, 3.0, accent.gamma_multiply(0.2 * selected));
                    label_painter.rect_stroke(
                        band,
                        3.0,
                        Stroke::new(1.0_f32, accent.gamma_multiply(selected)),
                    );
                }
                let folder_depth =
                    app.state
                        .project
                        .layer_folder_depth(q0rg_id, layer.layer_id) as f32;
                let hierarchy_indent = (folder_depth * 14.0
                    + app
                        .state
                        .project
                        .layer_mask_sources(q0rg_id, layer.layer_id)
                        .len() as f32
                        * 14.0)
                    .min((label_width - if is_mask { 100.0 } else { 78.0 }).max(0.0));
                let icon_rect = Rect::from_center_size(
                    label_rect.left_center() + vec2(11.0 + hierarchy_indent, 0.0),
                    vec2(13.0, 13.0),
                );
                if metadata.raster && metadata.kind == q0s_format::v2::LayerKind::Normal {
                    crate::raster::ui::paint_layer_icon(
                        &label_painter,
                        icon_rect,
                        theme.text.to_color32(),
                        theme.accent.to_color32(),
                    );
                } else {
                    draw_layer_icon_opening(
                        &label_painter,
                        icon_rect,
                        metadata.kind,
                        look::fade(ui.ctx(), row_look.with("open"), !metadata.collapsed, 0.14),
                        theme.text.to_color32(),
                        theme.accent.to_color32(),
                    );
                }
                let indent = hierarchy_indent;
                let visibility_rect = Rect::from_center_size(
                    label_rect.right_center() - vec2(31.0, 0.0),
                    vec2(18.0, row_h()),
                );
                let outline_rect = Rect::from_center_size(
                    label_rect.right_center() - vec2(49.0, 0.0),
                    vec2(18.0, row_h()),
                );
                let lock_rect = Rect::from_center_size(
                    label_rect.right_center() - vec2(13.0, 0.0),
                    vec2(18.0, row_h()),
                );
                let mask_art_rect = Rect::from_center_size(
                    label_rect.right_center() - vec2(72.0, 0.0),
                    vec2(18.0, row_h()),
                );
                let mask_art_hidden = app
                    .state
                    .project
                    .layer_metadata(q0rg_id, layer.layer_id)
                    .mask_preview;
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
                        } - 3.0
                            - crate::layer_parenting_ui::column_width(app),
                        label_rect.bottom(),
                    ),
                );
                if label_width > 120.0 {
                    crate::ui::clipped_label(
                        &label_painter,
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
                            "Show or hide mask and all its contents. Alt-click: hide all the others"
                        } else {
                            "Show or hide layer. Alt-click: hide all the others. Drag down the column to show or hide several"
                        },
                    ),
                    (outline_rect, "Toggle outline for this layer"),
                    (
                        lock_rect,
                        "Lock or unlock this layer. Alt-click: lock all the others. Drag down the column to lock or unlock several",
                    ),
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
                // A state that only a folder gives the layer is drawn faint:
                // the layer's own switch is still off.
                let from_folder = |own: bool, effective: bool| {
                    if effective && !own {
                        theme.text.to_color32().gamma_multiply(0.4)
                    } else {
                        theme.text.to_color32()
                    }
                };
                draw_visibility_icon(
                    &label_painter,
                    visibility_rect.shrink2(vec2(2.0, 4.0)),
                    look::fade(ui.ctx(), row_look.with("eye"), effective_visible, 0.16),
                    from_folder(metadata.hidden, !effective_visible),
                );
                if is_mask {
                    draw_mask_art_visibility_icon(
                        &label_painter,
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
                    &label_painter,
                    outline_rect.shrink2(vec2(4.0, 5.0)),
                    look::fade(ui.ctx(), row_look.with("outline"), outline_enabled, 0.16),
                    theme.text.to_color32(),
                );
                draw_lock_icon(
                    &label_painter,
                    lock_rect.shrink2(vec2(2.0, 3.0)),
                    look::fade(ui.ctx(), row_look.with("lock"), effective_locked, 0.16),
                    from_folder(
                        app.own_layer_flag(
                            q0rg_id,
                            row_layer_id,
                            crate::layer_flags::LayerFlag::Locked,
                        ),
                        effective_locked,
                    ),
                );
                {
                    let controls_left = if is_mask {
                        mask_art_rect.left()
                    } else {
                        outline_rect.left()
                    };
                    let column = Rect::from_min_max(
                        pos2(
                            controls_left - 2.0 - crate::layer_parenting_ui::column_width(app),
                            label_rect.top(),
                        ),
                        pos2(controls_left - 2.0, label_rect.bottom()),
                    );
                    crate::layer_parenting_ui::row(
                        app,
                        ui,
                        &label_painter,
                        q0rg_id,
                        row_layer_id,
                        column,
                        row_rect,
                        is_folder,
                    );
                }
                label_painter.line_segment(
                    [pos2(label_edge_x, row_y), pos2(label_edge_x, row_y + row_h())],
                    Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
                );

                // Bottom row separator — hairline in the theme's stroke colour.
                label_painter.line_segment(
                    [
                        pos2(rect.min.x, row_y + row_h()),
                        pos2(rect.min.x + rect.width(), row_y + row_h()),
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
                        frame_base_x,
                        row_y,
                        frame_count,
                        is_current_layer,
                        (q0rg_id, playhead),
                    );
                    draw_audio_waveforms(
                        &app.state.project,
                        &mut app.textures,
                        &painter,
                        &theme,
                        q0rg_id,
                        layer,
                        AudioWaveformLayout {
                            base_x: frame_base_x,
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
                            base_x: frame_base_x,
                            row_y,
                            frame_count: layer_frame_count,
                        },
                    );
                } else {
                    let folder_strip = Rect::from_min_size(
                        pos2(frame_base_x, row_y + CELL_PAD_Y),
                        vec2(
                            (layer_frame_count as f32) * frame_w(),
                            row_h() - CELL_PAD_Y * 2.0,
                        ),
                    );
                    painter.rect_filled(
                        folder_strip.shrink(0.5),
                        3.0,
                        blend(theme.panel.to_color32(), theme.window.to_color32(), 0.35),
                    );
                }
                if hovered > 0.01 {
                    painter.rect_filled(
                        row_rect,
                        0.0,
                        theme.text.to_color32().gamma_multiply(0.035 * hovered),
                    );
                }
                if !is_folder && response.double_clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        if row_rect.contains(pos) && !label_rect.contains(pos) {
                            let rel_x = pos.x - (frame_base_x);
                            if pos.x >= label_edge_x && rel_x >= 0.0 {
                                let frame = (rel_x / frame_w()).floor() as u16;
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

                // Pressing on an eye or lock and dragging paints that one new
                // state down the column, row by row.
                if response.drag_started_by(PointerButton::Primary) {
                    let pressed = ui.input(|input| input.pointer.press_origin());
                    let flag = pressed.and_then(|pos| {
                        if visibility_rect.contains(pos) {
                            Some(crate::layer_flags::LayerFlag::Hidden)
                        } else if lock_rect.contains(pos) {
                            Some(crate::layer_flags::LayerFlag::Locked)
                        } else {
                            None
                        }
                    });
                    if let Some(flag) = flag {
                        let on = !app.own_layer_flag(q0rg_id, row_layer_id, flag);
                        app.session.layer_flag_drag = Some(crate::layer_flags::LayerFlagDrag {
                            q0rg_id,
                            flag,
                            on,
                        });
                        flag_paints.push(Action::PaintLayerFlag(q0rg_id, row_layer_id, flag, on, true));
                    }
                } else if let Some(drag) = app
                    .session
                    .layer_flag_drag
                    .filter(|drag| drag.q0rg_id == q0rg_id)
                {
                    let over_row = response.dragged_by(PointerButton::Primary)
                        && ui
                            .input(|input| input.pointer.interact_pos())
                            .is_some_and(|pos| pos.y >= row_rect.top() && pos.y < row_rect.bottom());
                    if over_row && app.own_layer_flag(q0rg_id, row_layer_id, drag.flag) != drag.on {
                        flag_paints.push(Action::PaintLayerFlag(
                            q0rg_id,
                            row_layer_id,
                            drag.flag,
                            drag.on,
                            false,
                        ));
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

                // Click in row → set current_layer + current_frame
                if let Some(pos) = response.interact_pointer_pos() {
                    if row_rect.contains(pos) && response.clicked() {
                        let alt = ui.input(|input| input.modifiers.alt);
                        if visibility_rect.contains(pos) {
                            app.queue(if alt {
                                Action::ToggleOtherLayerVisibility(q0rg_id, layer.layer_id)
                            } else {
                                Action::ToggleLayerVisibility(q0rg_id, layer.layer_id)
                            });
                            continue;
                        }
                        if lock_rect.contains(pos) {
                            app.queue(if alt {
                                Action::ToggleOtherLayerLocks(q0rg_id, layer.layer_id)
                            } else {
                                Action::ToggleLayerLock(q0rg_id, layer.layer_id)
                            });
                            continue;
                        }
                        if outline_rect.contains(pos) && !is_folder {
                            app.queue(if alt {
                                Action::ToggleOtherLayerOutlines(q0rg_id, layer.layer_id)
                            } else {
                                Action::ToggleLayerOutline(q0rg_id, layer.layer_id)
                            });
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
                            let rel_x = pos.x - (frame_base_x);
                            if pos.x >= label_edge_x && rel_x >= 0.0 {
                                let f = (rel_x / frame_w()).floor() as i32;
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

            ROW_NOW.with(|now| now.set(None));
            // The current layer's mark and the current cell glide from row to
            // row and frame to frame instead of jumping.
            let on_camera = camera_row_enabled && app.camera_track_is_selected();
            let marked = if on_camera {
                // The camera row sits one row above the first layer.
                Some((-rows_h(), rows_h()))
            } else {
                layer_ids
                    .iter()
                    .position(|id| *id == app.session.current_layer_id)
                    .map(|index| (layer_top(index), layer_height(index)))
            };
            if let Some((mark_top, mark_height)) = marked {
                let accent = theme.accent.to_color32();
                let mark_id = egui::Id::new(("timeline_current_mark", q0rg_id));
                let top = look::glide(ui.ctx(), mark_id.with("top"), mark_top, 0.16);
                let height = look::glide(ui.ctx(), mark_id.with("height"), mark_height, 0.16);
                let mark = Rect::from_min_size(
                    pos2(rect.min.x + 1.5, layer_rows_y + top + 3.0),
                    vec2(3.0, (height - 6.0).max(2.0)),
                );
                label_painter.rect_filled(mark.expand(1.5), 3.0, accent.gamma_multiply(0.22));
                label_painter.rect_filled(mark, 1.5, accent);
                if !on_camera && !folder_layer_ids.contains(&app.session.current_layer_id) {
                    let frame = look::glide(
                        ui.ctx(),
                        mark_id.with("frame"),
                        f32::from(app.session.current_frame),
                        if app.session.playing { 0.0 } else { 0.12 },
                    );
                    let cell = Rect::from_min_size(
                        pos2(frame_base_x + frame * frame_w(), layer_rows_y + top),
                        vec2(frame_w(), height),
                    )
                    .shrink(0.75);
                    painter.rect_stroke(
                        cell.expand(1.0),
                        3.5,
                        Stroke::new(3.0_f32, accent.gamma_multiply(0.22)),
                    );
                    painter.rect_stroke(cell, 2.5, Stroke::new(1.5_f32, accent));
                }
            }
            // The edge between names and frames lights while it is held.
            let grip = look::fade(
                ui.ctx(),
                splitter.id,
                splitter.hovered() || splitter.dragged(),
                0.12,
            );
            if grip > 0.01 {
                ui.painter_at(clip).line_segment(
                    [
                        pos2(label_edge_x, clip.top()),
                        pos2(label_edge_x, clip.bottom()),
                    ],
                    Stroke::new(2.0_f32, theme.accent.to_color32().gamma_multiply(grip)),
                );
            }
            crate::layer_parenting_ui::finish(app, ui, q0rg_id);
            row_edges(
                app,
                ui,
                q0rg_id,
                &layer_ids,
                layer_rows_y,
                (rect.min.x, label_edge_x),
                layer_row_slots.clone(),
            );
            // Rig tracks are dedicated sublayers after the artwork rows. They
            // only exist for layers with an actual rig binding or deformer.
            for (track_index, track) in rig_tracks
                .iter()
                .enumerate()
                .take(rig_row_slots.end)
                .skip(rig_row_slots.start)
            {
                let row_y = rig_rows_y + row_h() * track_index as f32;
                let row_rect =
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(rect.width(), row_h()));
                let label_rect =
                    Rect::from_min_size(pos2(rect.min.x, row_y), vec2(label_width, row_h()));
                let selection = app.session.timeline_rig_key_selection.filter(|selection| {
                    selection.q0rg_id == q0rg_id && selection.layer_id == track.layer_id
                });
                let selected_track = selection.is_some();
                painter.rect_filled(row_rect, 0.0, theme.empty_beyond.to_color32());
                draw_empty_cells(
                    &painter,
                    &theme,
                    frame_base_x,
                    row_y,
                    frame_count,
                    visible_frames,
                    selected_track,
                );
                label_painter.rect_filled(
                    label_rect,
                    0.0,
                    blend(
                        theme.panel.to_color32(),
                        theme.accent.to_color32(),
                        if selected_track { 0.22 } else { 0.08 },
                    ),
                );
                let icon_center = label_rect.left_center() + vec2(11.0, 0.0);
                label_painter.line_segment(
                    [icon_center + vec2(-3.0, 3.0), icon_center + vec2(3.0, -3.0)],
                    Stroke::new(1.4_f32, theme.accent.to_color32()),
                );
                label_painter.circle_filled(
                    icon_center + vec2(-3.0, 3.0),
                    2.1,
                    theme.accent.to_color32(),
                );
                label_painter.circle_filled(
                    icon_center + vec2(3.0, -3.0),
                    2.1,
                    theme.accent.to_color32(),
                );
                label_painter.text(
                    label_rect.left_center() + vec2(22.0, 0.0),
                    Align2::LEFT_CENTER,
                    format!("rig: {}", truncate_label(&track.layer_name, 21)),
                    FontId::proportional(10.0),
                    theme.accent.to_color32(),
                );
                label_painter.line_segment(
                    [pos2(label_edge_x, row_y), pos2(label_edge_x, row_y + row_h())],
                    Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
                );
                label_painter.line_segment(
                    [
                        pos2(rect.min.x, row_y + row_h()),
                        pos2(rect.max.x, row_y + row_h()),
                    ],
                    Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
                );
                if let Some(selection) = selection {
                    let selected_rect = Rect::from_min_size(
                        pos2(
                            frame_base_x + f32::from(selection.frame) * frame_w(),
                            row_y + CELL_PAD_Y,
                        ),
                        vec2(frame_w(), row_h() - CELL_PAD_Y * 2.0),
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
                        frame_base_x + (f32::from(*frame) + 0.5) * frame_w(),
                        row_y + row_h() * 0.5,
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
                        Rect::from_center_size(center, vec2(frame_w(), row_h())).intersect(frame_clip),
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

            for action in flag_paints {
                app.queue(action);
            }
            if app.session.layer_flag_drag.is_some()
                && !ui.input(|input| input.pointer.primary_down())
            {
                app.session.layer_flag_drag = None;
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
                    &label_painter,
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
                        if position.x < label_edge_x {
                            return None;
                        }
                        timeline_cell_at_with_label_width(
                            rect.translate(vec2(-scroll_x, 0.0)),
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
                    crate::cursors::request(ui.ctx(), crate::cursors::CursorKind::LibraryDrop);
                    if let Some(row) = layer_ids
                        .iter()
                        .position(|candidate| *candidate == layer_id)
                    {
                        let cell = Rect::from_min_size(
                            pos2(
                                frame_base_x + frame as f32 * frame_w(),
                                layer_rows_y + layer_top(row),
                            ),
                            vec2(frame_w(), layer_height(row)),
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

            crate::online::paint_timeline_markers(
                &painter,
                &app.online.markers(app.active_project_tab_id(), q0rg_id),
                &layer_ids,
                frame_base_x,
                layer_rows_y,
                &layer_row_tops(),
                frame_w(),
            );
            draw_timeline_selection(
                &painter,
                &theme,
                app.session.timeline_selection,
                &layer_ids,
                frame_base_x,
                layer_rows_y,
                visible_frames,
            );
            if let Some((selected_q0rg, cells)) = app.session.timeline_cell_selection.as_ref() {
                if *selected_q0rg == q0rg_id {
                    draw_timeline_cell_selection(
                        &painter,
                        &theme,
                        cells,
                        &layer_ids,
                        frame_base_x,
                        layer_rows_y,
                        visible_frames,
                    );
                }
            }

            // The column under the pointer, then the playhead: a soft column
            // over its frame, the line with its glow, and while it plays a
            // trail over the frames it just left.
            if let Some(frame) = pointer
                .filter(|pos| pos.x >= label_edge_x)
                .map(|pos| ((pos.x - frame_base_x) / frame_w()).floor())
                .filter(|frame| *frame >= 0.0 && *frame < f32::from(visible_frames))
            {
                let x = frame_base_x + frame * frame_w();
                painter.rect_filled(
                    Rect::from_min_max(
                        pos2(x, rect.min.y),
                        pos2(x + frame_w(), rect.min.y + total_h),
                    ),
                    0.0,
                    theme.text.to_color32().gamma_multiply(0.03),
                );
            }
            let playhead_x = frame_base_x + (playhead.shown + 0.5) * frame_w();
            let playhead_color = theme.playhead.to_color32();
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(playhead_x - frame_w() * 0.5, rect.min.y),
                    pos2(playhead_x + frame_w() * 0.5, rect.min.y + total_h),
                ),
                0.0,
                playhead_color.gamma_multiply(0.06),
            );
            look::paint_playhead_line(
                &painter,
                playhead_x,
                (rect.min.y, rect.min.y + total_h),
                playhead_color,
                app.session.playing.then(|| frame_w() * 4.0),
            );

            let camera_row_rect = camera_row_enabled.then(|| {
                Rect::from_min_size(
                    pos2(rect.min.x, rect.min.y + rows_h()),
                    vec2(rect.width(), rows_h()),
                )
            });
            let camera_pointer_frame = response.interact_pointer_pos().and_then(|pos| {
                let row = camera_row_rect?;
                if !row.contains(pos) || pos.x < label_edge_x {
                    return None;
                }
                let frame = ((pos.x - (frame_base_x)) / frame_w()).floor() as i32;
                (frame >= 0 && frame < i32::from(visible_frames)).then_some(frame as u16)
            });
            let pointer_on_camera_row = response
                .interact_pointer_pos()
                .zip(camera_row_rect)
                .is_some_and(|(pos, row)| row.contains(pos));
            let rig_track_pointer_row = response.interact_pointer_pos().and_then(|pos| {
                let relative_y = pos.y - rig_rows_y;
                (relative_y >= 0.0 && relative_y < rows_h() * rig_track_count as f32)
                    .then_some((relative_y / rows_h()).floor() as usize)
                    .filter(|index| *index < rig_track_count)
            });
            let rig_track_pointer_cell = response.interact_pointer_pos().and_then(|pos| {
                let track_index = rig_track_pointer_row?;
                if pos.x < label_edge_x {
                    return None;
                }
                let relative_x = pos.x - frame_base_x;
                if relative_x < 0.0 {
                    return None;
                }
                let frame = (relative_x / frame_w()).floor() as i32;
                (frame >= 0 && frame < i32::from(visible_frames))
                    .then_some((track_index, frame as u16))
            });
            let rig_drop_cell = ui
                .input(|input| input.pointer.latest_pos())
                .and_then(|pos| {
                    let relative_y = pos.y - rig_rows_y;
                    if relative_y < 0.0 || relative_y >= row_h() * rig_track_count as f32 {
                        return None;
                    }
                    let track_index = (relative_y / row_h()).floor() as usize;
                    if pos.x < label_edge_x {
                        return None;
                    }
                    let relative_x = pos.x - frame_base_x;
                    if relative_x < 0.0 || track_index >= rig_track_count {
                        return None;
                    }
                    let frame = (relative_x / frame_w()).floor() as i32;
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
                            frame_base_x + f32::from(target_frame) * frame_w(),
                            rig_rows_y + row_h() * track_index as f32 + CELL_PAD_Y,
                        ),
                        vec2(frame_w(), row_h() - CELL_PAD_Y * 2.0),
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
                            app.camera().keys.iter().any(|key| {
                                (selection.first_frame()..=selection.last_frame())
                                    .contains(&key.frame)
                            })
                        });
                        let pressed = ui
                            .input(|input| input.pointer.press_origin())
                            .map(|pos| ((pos.x - frame_base_x) / frame_w()).floor().max(0.0) as u16)
                            .unwrap_or(frame);
                        let extend = ui.input(|input| input.modifiers.shift);
                        if inside_existing && existing_has_key && !extend {
                            if let Some(selection) = existing {
                                ui.data_mut(|data| {
                                    data.insert_temp(
                                        camera_grab_id(q0rg_id),
                                        pressed.saturating_sub(selection.first_frame()),
                                    )
                                });
                            }
                            app.session.camera_frame_drag =
                                existing.map(|selection| CameraFrameDrag { selection });
                        } else if extend {
                            // Shift: the stretch reaches from where it was to
                            // here, as with a layer's frames.
                            let mut selection =
                                existing.unwrap_or_else(|| CameraTimelineSelection::single(pressed));
                            selection.focus_frame = frame;
                            app.session.camera_timeline_selection = Some(selection);
                            app.session.set_timeline_frame(frame, frame_count);
                        } else {
                            app.session.camera_timeline_selection =
                                Some(CameraTimelineSelection::single(pressed));
                            app.session.set_timeline_frame(pressed, frame_count);
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

            let camera_pointer_frame = if app.session.camera_frame_drag.is_some() {
                let grab = ui
                    .data(|data| data.get_temp::<u16>(camera_grab_id(q0rg_id)))
                    .unwrap_or_default();
                camera_pointer_frame.map(|frame| frame.saturating_sub(grab))
            } else {
                camera_pointer_frame
            };
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
                        frame_base_x + f32::from(target_frame) * frame_w(),
                        rect.min.y + row_h() + CELL_PAD_Y,
                    ),
                    vec2(f32::from(width) * frame_w(), row_h() - CELL_PAD_Y * 2.0),
                );
                painter.rect_filled(preview, 3.0, theme.accent.to_color32().gamma_multiply(0.18));
                painter.rect_stroke(
                    preview,
                    3.0,
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
            let clicked_current_frame = click_frame == Some(app.session.timeline_frame())
                && app.session.pending_timeline_frame.is_none();
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
                } else if !extend && clicked_current_frame {
                    app.session.timeline_cell_selection = None;
                    app.session.timeline_selection = None;
                    app.session.selection = crate::tools::select_layer_artwork(
                        &app.state.project, q0rg_id, layer_id, frame,
                    );
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
                        if position.x < label_edge_x {
                            return None;
                        }
                        timeline_cell_at_with_label_width(
                            rect.translate(vec2(-scroll_x, 0.0)),
                            position,
                            visible_frames,
                            &layer_ids,
                            &folder_layer_ids,
                            camera_row_count,
                            label_width,
                        )
                    });

            // Sound clips are grabbed, not selected: the middle moves one,
            // its ends trim it.
            let cell_at = |position: egui::Pos2| {
                (position.x >= label_edge_x)
                    .then(|| {
                        timeline_cell_at_with_label_width(
                            rect.translate(vec2(-scroll_x, 0.0)),
                            position,
                            visible_frames,
                            &layer_ids,
                            &folder_layer_ids,
                            camera_row_count,
                            label_width,
                        )
                    })
                    .flatten()
            };
            let clip_under = |app: &crate::app::EditorApp, position: egui::Pos2| {
                let (layer_id, frame) = cell_at(position)?;
                let clip = crate::audio::audio_clip_at_frame(
                    &app.state.project,
                    q0rg_id,
                    layer_id,
                    frame,
                )?;
                let left = frame_base_x + f32::from(clip.start_frame) * frame_w();
                let right = frame_base_x + f32::from(clip.end_frame_exclusive) * frame_w();
                let edge = (frame_w() * 0.45).min((right - left) * 0.3);
                let mode = if position.x - left <= edge {
                    crate::state::AudioClipDragMode::TrimStart
                } else if right - position.x <= edge {
                    crate::state::AudioClipDragMode::TrimEnd
                } else {
                    crate::state::AudioClipDragMode::Move
                };
                Some((clip, frame, mode))
            };
            if app.session.timeline_layer_drag.is_none()
                && library_payload.is_none()
                && app.session.audio_clip_drag.is_none()
            {
                if let Some(hover) = response.hover_pos() {
                    if let Some((_, _, mode)) = clip_under(app, hover) {
                        ui.ctx().set_cursor_icon(match mode {
                            crate::state::AudioClipDragMode::Move => egui::CursorIcon::Grab,
                            _ => egui::CursorIcon::ResizeHorizontal,
                        });
                    }
                }
                if response.drag_started_by(PointerButton::Primary) {
                    if let Some((clip, frame, mode)) = ui
                        .input(|input| input.pointer.press_origin())
                        .and_then(|press| clip_under(app, press))
                    {
                        app.session.audio_clip_drag = Some(crate::state::AudioClipDrag {
                            q0rg_id,
                            clip_idx: clip.clip_idx,
                            mode,
                            grab: frame.saturating_sub(clip.start_frame),
                        });
                        app.session.timeline_selection = None;
                        app.session.timeline_cell_selection = None;
                    }
                }
            }
            // Where the clip being dragged would go: layer, start, end.
            let audio_drop = app
                .session
                .audio_clip_drag
                .filter(|drag| drag.q0rg_id == q0rg_id)
                .and_then(|drag| {
                    let clip = crate::audio::audio_clip_ref(&app.state.project, drag.clip_idx)?;
                    let source = *app.state.project.audio_clips.get(drag.clip_idx)?;
                    let pointer = ui.input(|input| input.pointer.latest_pos())?;
                    let boundary = ((pointer.x - frame_base_x) / frame_w())
                        .round()
                        .clamp(0.0, f32::from(u16::MAX)) as u16;
                    let length = clip.end_frame_exclusive - clip.start_frame;
                    Some(match drag.mode {
                        crate::state::AudioClipDragMode::Move => {
                            let (layer_id, frame) = cell_at(pointer)
                                .unwrap_or((source.layer_id, clip.start_frame + drag.grab));
                            let start = frame.saturating_sub(drag.grab);
                            (layer_id, start, start.saturating_add(length))
                        }
                        crate::state::AudioClipDragMode::TrimStart => (
                            source.layer_id,
                            boundary.min(clip.end_frame_exclusive - 1),
                            clip.end_frame_exclusive,
                        ),
                        crate::state::AudioClipDragMode::TrimEnd => (
                            source.layer_id,
                            clip.start_frame,
                            boundary.max(clip.start_frame + 1),
                        ),
                    })
                });
            if let (Some(drag), Some((layer_id, start, end))) =
                (app.session.audio_clip_drag, audio_drop)
            {
                ui.ctx().set_cursor_icon(match drag.mode {
                    crate::state::AudioClipDragMode::Move => egui::CursorIcon::Grabbing,
                    _ => egui::CursorIcon::ResizeHorizontal,
                });
                if let Some(row) = layer_ids.iter().position(|id| *id == layer_id) {
                    let preview = Rect::from_min_max(
                        pos2(
                            frame_base_x + f32::from(start) * frame_w(),
                            layer_rows_y + layer_top(row),
                        ),
                        pos2(
                            frame_base_x + f32::from(end) * frame_w(),
                            layer_rows_y + layer_top(row + 1),
                        ),
                    )
                    .shrink(1.5);
                    painter.rect_filled(preview, 3.0, theme.accent.to_color32().gamma_multiply(0.22));
                    painter.rect_stroke(preview, 3.0, Stroke::new(1.5_f32, theme.accent.to_color32()));
                }
                if response.drag_stopped_by(PointerButton::Primary) {
                    app.session.audio_clip_drag = None;
                    app.queue(Action::EditAudioClip(drag.clip_idx, layer_id, start, end));
                }
            }
            if app.session.audio_clip_drag.is_some()
                && !ui.input(|input| input.pointer.primary_down())
            {
                app.session.audio_clip_drag = None;
            }

            if app.session.timeline_layer_drag.is_none()
                && library_payload.is_none()
                && app.session.audio_clip_drag.is_none()
            {
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
                            // Where in the block it was picked up, so it moves
                            // with the pointer instead of jumping its corner
                            // there.
                            let pressed = ui
                                .input(|input| input.pointer.press_origin())
                                .and_then(|pos| {
                                    let frame = ((pos.x - frame_base_x) / frame_w()).floor();
                                    let row = layer_row_at(pos.y - layer_rows_y);
                                    (frame >= 0.0 && row >= 0)
                                        .then_some((frame as u16, row as usize))
                                })
                                .unwrap_or_else(|| {
                                    (
                                        frame,
                                        layer_ids
                                            .iter()
                                            .position(|id| *id == layer_id)
                                            .unwrap_or_default(),
                                    )
                                });
                            if let Some((first_row, _, first_frame, _)) =
                                timeline_selection_bounds(selection, &layer_ids, visible_frames)
                            {
                                ui.data_mut(|data| {
                                    data.insert_temp(
                                        frame_grab_id(q0rg_id),
                                        (
                                            pressed.0.saturating_sub(first_frame),
                                            pressed.1.saturating_sub(first_row),
                                        ),
                                    )
                                });
                            }
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

            // A dragged block lands with the cell it was picked up by under
            // the pointer.
            let frame_drop_cell = if app.session.timeline_frame_drag.is_some() {
                let (frames, rows) = ui
                    .data(|data| data.get_temp::<(u16, usize)>(frame_grab_id(q0rg_id)))
                    .unwrap_or_default();
                frame_drop_cell.and_then(|(layer_id, frame)| {
                    let row = layer_ids.iter().position(|id| *id == layer_id)?;
                    Some((layer_ids[row.saturating_sub(rows)], frame.saturating_sub(frames)))
                })
            } else {
                frame_drop_cell
            };
            if let (Some(drag), Some((target_layer_id, target_frame))) =
                (app.session.timeline_frame_drag, frame_drop_cell)
            {
                if drag.q0rg_id == q0rg_id {
                    crate::cursors::request(ui.ctx(), crate::cursors::CursorKind::FrameDrag);
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
                                frame_base_x + target_frame as f32 * frame_w(),
                                layer_rows_y + layer_top(target_row),
                            ),
                            vec2(
                                width as f32 * frame_w(),
                                layer_top(target_row + source_rows) - layer_top(target_row),
                            ),
                        )
                        .shrink(1.0);
                        painter.rect_filled(
                            preview,
                            3.0,
                            theme.accent.to_color32().gamma_multiply(0.18),
                        );
                        painter.rect_stroke(
                            preview,
                            3.0,
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
                    let rel_x = pos.x - (frame_base_x);
                    let clicked_frame = if rel_x >= 0.0 {
                        let frame = (rel_x / frame_w()).floor() as i32;
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
                        let row_idx = layer_row_at(pos.y - layer_rows_y) as i32;
                        if row_idx >= 0 {
                            if let Some(layer_index) = visible_layer_indices.get(row_idx as usize) {
                                let layer = &app.state.project.q0rgs[q0rg_idx].layers[*layer_index];
                                app.session.current_layer_id = layer.layer_id;
                                app.session.camera_track_selected = false;
                                app.session.camera_timeline_selection = None;
                                app.session.timeline_rig_key_selection = None;
                                app.session.selection = Selection::None;
                                let clicked_label = pos.x < label_edge_x;
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
            if secondary_on_rig {
                response.context_menu(|ui| rig_timeline_context_menu(app, ui));
            } else {
                response.context_menu(|ui| timeline_context_menu(app, ui));
            }
        });
    let scrub_frame = app.session.timeline_frame();
    if app.session.pending_timeline_frame.is_none() && scrub_frame != scrub_frame_before {
        app.scrub_audio_at(scrub_frame);
    }
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
    let camera_rows = usize::from(project.camera_on(q0rg_id));
    let first_row_y = rect.min.y + rows_h() * (camera_rows as f32 + 1.0);
    let raw_row = layer_row_at(pos.y - first_row_y);
    if raw_row >= row_count as isize {
        return None;
    }
    let row_index = raw_row.max(0) as usize;
    let target_id = visible_layer_ids[row_index];
    if dragged_layer_ids.is_empty()
        || dragged_layer_ids.iter().any(|dragged_layer_id| {
            project.layer_is_descendant_of(q0rg_id, target_id, *dragged_layer_id)
        })
    {
        return None;
    }
    let target_metadata = project.layer_metadata(q0rg_id, target_id);

    let row_y = first_row_y + layer_top(row_index);
    let row_fraction = ((pos.y - row_y) / layer_height(row_index)).clamp(0.0, 1.0);
    let requested_parent =
        layer_parent_for_drop_lane(project, q0rg_id, target_id, pos.x, rect.min.x);

    if dragged_layer_ids.contains(&target_id) {
        // On its own row a layer can only change lanes: dragging towards the
        // left edge outdents it from the folders it is nested in.
        if requested_parent == project.layer_parent_folder(q0rg_id, target_id) {
            return None;
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

#[allow(clippy::too_many_arguments)]
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
    let camera_rows = usize::from(project.camera_on(q0rg_id));
    match target {
        LayerDropTarget::IntoFolder(folder_id) | LayerDropTarget::IntoMask(folder_id) => {
            let Some(index) = visible_layer_ids
                .iter()
                .position(|layer_id| *layer_id == folder_id)
            else {
                return;
            };
            let row_y = rect.min.y + rows_h() * (camera_rows as f32 + 1.0) + layer_top(index);
            let highlight = Rect::from_min_size(
                pos2(rect.min.x + 1.0, row_y + 1.0),
                vec2(label_width - 2.0, layer_height(index) - 2.0),
            );
            painter.rect_filled(
                highlight,
                2.0,
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 42),
            );
            painter.rect_stroke(highlight, 2.0, Stroke::new(1.5_f32, accent));
        }
        LayerDropTarget::OutOfMask => (),
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
                + rows_h() * (camera_rows as f32 + 1.0)
                + layer_top(line_index + usize::from(line_after));
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
    if app.camera_track_is_selected() {
        camera_timeline_context_menu(app, ui);
    } else if app.session.timeline_layer_selection.is_some() {
        timeline_layer_context_menu(app, ui);
    } else {
        timeline_frame_context_menu(app, ui);
    }
}

/// Right-click on the camera row: the frame commands, and where the camera
/// ends.
fn camera_timeline_context_menu(app: &mut EditorApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Camera").strong().small());
    ui.separator();
    for (command, label, action) in [
        (
            ShortcutCommand::InsertFrame,
            "Insert frame",
            Action::InsertFrame,
        ),
        (
            ShortcutCommand::InsertKeyframe,
            "Insert keyframe",
            Action::InsertKeyframe,
        ),
        (
            ShortcutCommand::ClearKeyframe,
            "Clear keyframe",
            Action::ClearKeyframe,
        ),
        (
            ShortcutCommand::RemoveFrame,
            "Remove frames",
            Action::RemoveFrame,
        ),
        (
            ShortcutCommand::MotionTween,
            "Create motion tween",
            Action::CreateMotionTween,
        ),
    ] {
        if ui
            .add(crate::ui::shortcut_button(
                &app.settings.hotkeys,
                command,
                label,
            ))
            .clicked()
        {
            app.queue(action);
            ui.close_menu();
        }
    }
    ui.separator();
    let frame = app.session.timeline_frame();
    let length = app
        .state
        .project
        .camera_frame_count(app.session.current_q0rg_id);
    if ui
        .add_enabled(
            frame.saturating_add(1) < length,
            egui::Button::new("End camera after this frame"),
        )
        .on_hover_text("From the next frame on, the scene shows as if it had no camera")
        .clicked()
    {
        app.set_camera_length(Some(frame.saturating_add(1)));
        ui.close_menu();
    }
    if ui
        .add_enabled(
            app.camera().frame_count.is_some(),
            egui::Button::new("Run camera to the end"),
        )
        .on_hover_text("The camera runs to the end of the scene again")
        .clicked()
    {
        app.set_camera_length(None);
        ui.close_menu();
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
    if ui
        .button("New raster layer")
        .on_hover_text("A layer for frame-by-frame drawing in pixels")
        .clicked()
    {
        crate::raster::add_raster_layer(app, ui.ctx());
        ui.close_menu();
    }
    if metadata.kind != q0s_format::v2::LayerKind::Folder {
        let empty = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
            .is_some_and(|layer| layer.placements.is_empty());
        let label = if metadata.raster {
            "Make it a vector layer"
        } else {
            "Make it a raster layer"
        };
        if ui
            .add_enabled(empty, egui::Button::new(label))
            .on_disabled_hover_text("Only an empty layer changes between vector and raster")
            .clicked()
        {
            crate::raster::set_layer_raster(app, q0rg_id, layer_id, !metadata.raster);
            ui.close_menu();
        }
    }
    // whole layers through the clipboard, for hands without Ctrl+C
    let in_selection = app
        .session
        .timeline_layer_selection
        .is_some_and(|selection| {
            selection.anchor_layer_id == layer_id || selection.focus_layer_id == layer_id
        });
    let select_layers = |app: &mut EditorApp| {
        if !in_selection {
            app.session.timeline_layer_selection =
                Some(crate::state::TimelineLayerSelection::single(layer_id));
        }
        app.session.timeline_selection = None;
        app.session.timeline_cell_selection = None;
    };
    ui.horizontal(|ui| {
        if ui
            .button("Copy")
            .on_hover_text("Copy the layer with all its frames")
            .clicked()
        {
            select_layers(app);
            app.queue(Action::CopySelection);
            ui.close_menu();
        }
        if ui.button("Cut").clicked() {
            select_layers(app);
            app.queue(Action::CutSelection);
            ui.close_menu();
        }
        if ui
            .button("Paste")
            .on_hover_text("Paste copied layers above this one")
            .clicked()
        {
            app.queue(Action::Paste);
            ui.close_menu();
        }
        if ui.button("Duplicate").clicked() {
            select_layers(app);
            app.queue(Action::CopySelection);
            app.queue(Action::Paste);
            ui.close_menu();
        }
    });
    ui.separator();
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
    }
    let layer =
        crate::plugins::layer_value(app.session.current_q0rg_id, app.session.current_layer_id);
    crate::plugins::menu_items(app, ui, "layer_menu", vec![layer]);
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
    if let Some(audio) =
        crate::audio::audio_clip_at_frame(&app.state.project, q0rg_id, layer_id, zero_frame)
    {
        let locked = app.layer_refuses_timeline_edits(q0rg_id, layer_id);
        if ui
            .add_enabled(
                !locked && zero_frame > audio.start_frame,
                egui::Button::new("Split audio here"),
            )
            .clicked()
        {
            app.audio_keyframe_at(q0rg_id, layer_id, zero_frame, true);
            ui.close_menu();
            return;
        }
        if ui
            .add_enabled(!locked, egui::Button::new("Stop audio here"))
            .clicked()
        {
            app.audio_keyframe_at(q0rg_id, layer_id, zero_frame, false);
            ui.close_menu();
            return;
        }
        let muted = app.state.project.audio_clips[audio.clip_idx].muted;
        if ui
            .add_enabled(
                !locked,
                egui::Button::new(if muted { "Unmute audio" } else { "Mute audio" }),
            )
            .clicked()
        {
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
            app.state.project.audio_clips[audio.clip_idx].muted = !muted;
            app.state.mark_dirty();
            if app.session.playing {
                let _ = app.refresh_audio_playback();
            }
            ui.close_menu();
            return;
        }
        ui.separator();
    }
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
    // A frame that already belongs to a tween cannot start another one, so
    // offer to remove what is there instead.
    if let Some(label) = crate::easing::selected_tween_removal_label(app) {
        if ui.button(label).clicked() {
            crate::easing::remove_selected_tweens(app);
            ui.close_menu();
        }
        return;
    }
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
    if ui
        .add(crate::ui::shortcut_button(
            &app.settings.hotkeys,
            ShortcutCommand::ShapeTween,
            "Create shape tween",
        ))
        .on_hover_text("Morph the drawn shapes of this keyframe into those of the next one")
        .clicked()
    {
        app.queue(Action::CreateShapeTween);
        ui.close_menu();
    }
    let layer =
        crate::plugins::layer_value(app.session.current_q0rg_id, app.session.current_layer_id);
    crate::plugins::menu_items(
        app,
        ui,
        "frame_menu",
        vec![
            layer,
            q0ex::Value::Num(f64::from(app.session.current_frame) + 1.0),
        ],
    );
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

#[allow(clippy::too_many_arguments)]
fn draw_camera_layer_row(
    app: &EditorApp,
    painter: &egui::Painter,
    theme: &Theme,
    rect: Rect,
    row_y: f32,
    frame_count: u16,
    visible_frames: u16,
    label_width: f32,
    frame_base_x: f32,
    frame_painter: &egui::Painter,
    playhead: Playhead,
) {
    let selected = app.session.camera_track_selected;
    let row_rect = Rect::from_min_size(pos2(rect.min.x, row_y), vec2(rect.width(), row_h()));
    // A camera that ends early shows its frames only up to its end.
    let camera_frames = app
        .state
        .project
        .camera_frame_count(app.session.current_q0rg_id)
        .min(frame_count);
    frame_painter.rect_filled(row_rect, 0.0, theme.empty_beyond.to_color32());
    draw_empty_cells(
        frame_painter,
        theme,
        frame_base_x,
        row_y,
        camera_frames,
        visible_frames,
        selected,
    );

    let label_rect = Rect::from_min_size(pos2(rect.min.x, row_y), vec2(label_width, row_h()));
    // The camera's own row: tinted toward the accent, lit under the pointer
    // and when picked, like a layer's.
    let ctx = painter.ctx();
    let look_id = egui::Id::new(("timeline_camera_look", app.session.current_q0rg_id));
    let picked = look::fade(ctx, look_id, selected, 0.18);
    let hovered = look::fade(
        ctx,
        look_id.with("hover"),
        ctx.pointer_hover_pos().is_some_and(|pos| {
            row_rect.contains(pos)
                && (painter.clip_rect().contains(pos) || frame_painter.clip_rect().contains(pos))
        }),
        0.1,
    );
    let accent = theme.accent.to_color32();
    let base = look::mix(
        blend(theme.panel.to_color32(), theme.window.to_color32(), 0.5),
        accent,
        0.1 + 0.07 * hovered,
    );
    painter.add(crate::ui::gradient_shape(
        ctx,
        label_rect,
        0.0,
        [
            look::mix(base, accent, 0.28 * picked),
            look::mix(base, accent, 0.16 * picked),
        ],
    ));
    if picked > 0.01 {
        let band = label_rect.shrink2(vec2(1.5, 1.5));
        painter.rect_filled(band, 3.0, accent.gamma_multiply(0.18 * picked));
        painter.rect_stroke(
            band,
            3.0,
            Stroke::new(1.0_f32, accent.gamma_multiply(picked)),
        );
    }
    if hovered > 0.01 {
        frame_painter.rect_filled(
            row_rect,
            0.0,
            theme.text.to_color32().gamma_multiply(0.035 * hovered),
        );
    }
    let icon_rect =
        Rect::from_center_size(label_rect.left_center() + vec2(11.0, 0.0), vec2(13.0, 13.0));
    draw_camera_icon(painter, icon_rect, theme.text.to_color32(), accent);
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
            pos2(rect.min.x + label_width, row_y + row_h()),
        ],
        Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
    );
    painter.line_segment(
        [
            pos2(rect.min.x, row_y + row_h()),
            pos2(rect.min.x + rect.width(), row_y + row_h()),
        ],
        Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
    );

    draw_camera_spans(
        frame_painter,
        theme,
        app.camera(),
        frame_base_x,
        row_y,
        camera_frames,
        selected,
        (app.session.current_q0rg_id, playhead),
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
                    frame_base_x + f32::from(first) * frame_w(),
                    row_y + CELL_PAD_Y,
                ),
                pos2(
                    frame_base_x + (f32::from(last) + 1.0) * frame_w(),
                    row_y + row_h() - CELL_PAD_Y,
                ),
            );
            frame_painter.rect_filled(selected_rect, 3.0, fill);
            frame_painter.rect_stroke(selected_rect, 3.0, Stroke::new(1.2_f32, accent));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_camera_spans(
    painter: &egui::Painter,
    theme: &Theme,
    camera: &q0s_format::v2::CameraTrack,
    base_x: f32,
    row_y: f32,
    frame_count: u16,
    is_current_layer: bool,
    (q0rg_id, playhead): (u16, Playhead),
) {
    let keyframes = camera
        .keys
        .iter()
        .map(|key| key.frame)
        .filter(|frame| *frame < frame_count)
        .collect::<Vec<_>>();
    let pops = look::key_pops(
        painter.ctx(),
        egui::Id::new(("timeline_camera_keys", q0rg_id)),
        &keyframes,
    );
    let frame_slots = visible_frame_slots(painter, base_x, frame_count);
    if frame_slots.is_empty() {
        return;
    }
    let cell_top = row_y + CELL_PAD_Y;
    let cell_bot = row_y + row_h() - CELL_PAD_Y;
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
        let x0 = base_x + f32::from(keyframe) * frame_w();
        let x1 = base_x + (f32::from(span_end) + 1.0) * frame_w();
        look::paint_span(
            painter,
            Rect::from_min_max(pos2(x0, cell_top), pos2(x1, cell_bot)),
            if has_tween { tween_fill } else { static_fill },
            stroke_dark,
            !has_tween && span_end > keyframe,
        );
    }
    let mid_y = row_y + row_h() * 0.5;
    let bead_x = base_x + (playhead.shown + 0.5) * frame_w();
    for key in &camera.keys {
        let Some(to_frame) = key.tween.to_frame() else {
            continue;
        };
        if key.frame >= frame_slots.end || to_frame < frame_slots.start || to_frame >= frame_count {
            continue;
        }
        look::paint_tween_arrow(
            painter,
            (
                base_x + (f32::from(key.frame) + 0.5) * frame_w() + 4.0,
                base_x + (f32::from(to_frame) + 0.5) * frame_w() - 4.0,
            ),
            mid_y,
            tween_arrow,
            (playhead.frame > key.frame && playhead.frame < to_frame).then_some(bead_x),
        );
    }
    let keyframe_color = blend(theme.keyframe.to_color32(), theme.accent.to_color32(), 0.22);
    for keyframe in keyframes
        .into_iter()
        .filter(|frame| frame_slots.contains(frame))
    {
        look::paint_key(
            painter,
            &look::Key {
                center: pos2(
                    base_x + (f32::from(keyframe) + 0.5) * frame_w(),
                    key_y(row_y),
                ),
                radius: key_radius(),
                blank: false,
                color: keyframe_color,
                rim: stroke_dark.gamma_multiply(0.8),
                hole: static_fill,
                age: pops
                    .iter()
                    .find(|(frame, _)| *frame == keyframe)
                    .map(|(_, age)| *age),
                ring: (keyframe == playhead.frame)
                    .then(|| theme.playhead.to_color32().gamma_multiply(0.75)),
            },
        );
    }
}

/// How big a keyframe dot is: a little smaller in narrow frames.
fn key_radius() -> f32 {
    (frame_w() * 0.3).clamp(2.4, 3.3)
}

/// Where keyframe dots sit in a row: low in the cell, where Flash keeps
/// them, clear of the tween arrows through the middle.
fn key_y(row_y: f32) -> f32 {
    row_y + row_h() - CELL_PAD_Y - key_radius() - 2.4
}

/// The bar over the timeline, in one row like Animate's: layer and frame
/// buttons on the left, playback in the middle, the view on the right.
/// It wraps only when the panel is too narrow for one row, and every
/// button can be switched off from the bar's own menu.
fn transport_bar(app: &mut EditorApp, theme: &Theme, ui: &mut Ui) {
    const GAP: f32 = 14.0;
    let id = ui.id().with("timeline_bar_widths");
    let widths: [f32; 3] = ui.data(|data| data.get_temp(id)).unwrap_or([0.0; 3]);
    let full = ui.available_rect_before_wrap();
    let height = crate::ui::ICON_CONTROL_SIZE + 2.0;
    let known = widths.iter().all(|width| *width > 0.0);
    let one_row = known && widths.iter().sum::<f32>() + GAP * 2.0 <= full.width();
    let mut people_space = None;
    let measured = if one_row {
        let row = Rect::from_min_size(full.min, vec2(full.width(), height));
        let group = |ui: &mut Ui, rect: Rect| {
            let mut child = ui.child_ui(rect, egui::Layout::left_to_right(egui::Align::Center));
            child.set_clip_rect(ui.clip_rect().intersect(row));
            child
        };
        let mut left = group(ui, row);
        bar_layers_and_frames(app, &mut left);
        let left_width = left.min_rect().width();
        let right_x = (row.right() - widths[2]).max(row.left() + left_width + GAP);
        let mut right = group(
            ui,
            Rect::from_min_max(egui::pos2(right_x, row.top()), row.max),
        );
        bar_view(app, theme, &mut right);
        let right_width = right.min_rect().width();
        let center_x = (row.center().x - widths[1] / 2.0)
            .max(row.left() + left_width + GAP)
            .min(right_x - GAP - widths[1]);
        let mut center = group(
            ui,
            Rect::from_min_max(
                egui::pos2(center_x, row.top()),
                egui::pos2(right_x - GAP, row.bottom()),
            ),
        );
        bar_playback(app, &mut center);
        let center_width = center.min_rect().width();
        people_space = Some(Rect::from_min_max(
            egui::pos2(center.min_rect().right() + GAP * 0.5, row.top()),
            egui::pos2(right_x - GAP * 0.5, row.bottom()),
        ));
        ui.allocate_rect(row, egui::Sense::hover());
        [left_width, center_width, right_width]
    } else {
        // Too narrow, or not measured yet: the groups follow one another in
        // one row that scrolls sideways (wheel, Shift+wheel or a finger), so
        // every button stays in reach on a small screen.
        egui::ScrollArea::horizontal()
            .id_source("timeline_bar_scroll")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let left = ui.horizontal(|ui| bar_layers_and_frames(app, ui)).response;
                    ui.add_space(GAP);
                    let center = ui.horizontal(|ui| bar_playback(app, ui)).response;
                    ui.add_space(GAP);
                    let right = ui.horizontal(|ui| bar_view(app, theme, ui)).response;
                    crate::online::timeline_people(app, ui, None);
                    [left.rect.width(), center.rect.width(), right.rect.width()]
                })
                .inner
            })
            .inner
    };
    if measured != widths {
        ui.data_mut(|data| data.insert_temp(id, measured));
        if !known {
            ui.ctx().request_repaint();
        }
    }
    if people_space.is_some() {
        crate::online::timeline_people(app, ui, people_space);
    }
}

fn shows(app: &EditorApp, button: TimelineButton) -> bool {
    app.settings.timeline_bar.shows(button)
}

fn bar_layers_and_frames(app: &mut EditorApp, ui: &mut Ui) {
    if shows(app, TimelineButton::Camera) {
        // Every scene has its own camera; a symbol's timeline has none.
        let camera_available = app.state.project.is_scene_root(app.session.current_q0rg_id);
        let camera_enabled = app.camera().enabled;
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
            app.camera_mut().enabled = !camera_enabled;
            app.history.snapshot_with_session(&before, &app.session);
            app.state.mark_dirty();
            app.session.camera_track_selected = app.camera().enabled;
            app.session.timeline_selection = None;
            app.session.timeline_layer_selection = None;
            app.session.selection = Selection::None;
            app.session.status = if app.camera().enabled {
                "camera track enabled".to_string()
            } else {
                "camera track disabled".to_string()
            };
        }
    }
    if shows(app, TimelineButton::Delete) {
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
            || has_selected_frame_cells
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
    }
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let layer_buttons = [
        TimelineButton::AddLayer,
        TimelineButton::AddFolder,
        TimelineButton::AddMask,
        TimelineButton::LayerUp,
        TimelineButton::LayerDown,
        TimelineButton::Indent,
        TimelineButton::Outdent,
        TimelineButton::NewSymbol,
    ];
    if layer_buttons.iter().any(|button| shows(app, *button)) {
        ui.separator();
    }
    if shows(app, TimelineButton::AddLayer) && icon_button(ui, Icon::Layer, "Add layer").clicked() {
        app.queue(Action::AddLayer);
    }
    if shows(app, TimelineButton::AddLayer)
        && icon_button(
            ui,
            Icon::RasterLayer,
            "Add raster layer: frame-by-frame drawing in pixels, with the Raster brushes",
        )
        .clicked()
    {
        crate::raster::add_raster_layer(app, ui.ctx());
    }
    if shows(app, TimelineButton::AddFolder)
        && icon_button(ui, Icon::Folder, "Add layer folder").clicked()
    {
        app.queue(Action::AddLayerFolder);
    }
    if shows(app, TimelineButton::AddMask)
        && icon_button(ui, Icon::Mask, "Add a mask for the selected layer").clicked()
    {
        app.queue(Action::AddLayerMask);
    }
    if shows(app, TimelineButton::LayerUp)
        && layer_move_icon_button(ui, LayerMoveIcon::Up, "Move layer up").clicked()
    {
        app.queue(Action::MoveLayer(q0rg_id, layer_id, -1));
    }
    if shows(app, TimelineButton::LayerDown)
        && layer_move_icon_button(ui, LayerMoveIcon::Down, "Move layer down").clicked()
    {
        app.queue(Action::MoveLayer(q0rg_id, layer_id, 1));
    }
    if shows(app, TimelineButton::Indent)
        && layer_move_icon_button(ui, LayerMoveIcon::Indent, "Move layer into folder above")
            .clicked()
    {
        app.queue(Action::IndentLayer(q0rg_id, layer_id));
    }
    if shows(app, TimelineButton::Outdent)
        && layer_move_icon_button(ui, LayerMoveIcon::Outdent, "Move layer out of folder").clicked()
    {
        app.queue(Action::OutdentLayer(q0rg_id, layer_id));
    }
    if shows(app, TimelineButton::NewSymbol)
        && icon_button(ui, Icon::Symbol, "Create q0rg symbol").clicked()
    {
        app.queue(Action::PromptSymbol(
            crate::symbol_prompt::SymbolRequest::New,
        ));
    }
    let frame_buttons = [
        TimelineButton::InsertFrame,
        TimelineButton::Keyframe,
        TimelineButton::BlankKeyframe,
        TimelineButton::RemoveColumns,
        TimelineButton::MotionTween,
        TimelineButton::ShapeTween,
    ];
    if frame_buttons.iter().any(|button| shows(app, *button)) {
        ui.separator();
    }
    let rig_track_selected = app
        .session
        .timeline_rig_key_selection
        .is_some_and(|selection| selection.q0rg_id == q0rg_id)
        && matches!(&app.session.selection, Selection::None);
    if shows(app, TimelineButton::InsertFrame)
        && icon_button(
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
    if shows(app, TimelineButton::Keyframe)
        && icon_button_enabled(
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
    if shows(app, TimelineButton::BlankKeyframe)
        && icon_button_enabled(
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
    if shows(app, TimelineButton::RemoveColumns)
        && icon_button(
            ui,
            Icon::RemoveColumns,
            "Remove time columns across all layers and close the gap",
        )
        .clicked()
    {
        app.queue(Action::RemoveFrame);
    }
    let can_tween = (app.session.timeline_selection.is_some() || app.camera_track_is_selected())
        && app.session.pending_timeline_frame.is_none();
    if shows(app, TimelineButton::MotionTween)
        && icon_button_enabled(
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
    if shows(app, TimelineButton::ShapeTween)
        && icon_button_enabled(
            ui,
            can_tween && !app.camera_track_is_selected(),
            Icon::ShapeTween,
            &app.settings.hotkeys.hint(
                ShortcutCommand::ShapeTween,
                "Create shape tween: morph this keyframe's drawing into the next",
            ),
        )
        .clicked()
    {
        app.queue(Action::CreateShapeTween);
    }
}

fn bar_playback(app: &mut EditorApp, ui: &mut Ui) {
    if shows(app, TimelineButton::First)
        && icon_button(
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
    if shows(app, TimelineButton::Previous)
        && icon_button(
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
    if shows(app, TimelineButton::Play) {
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
    }
    if shows(app, TimelineButton::Next)
        && icon_button(
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
    if shows(app, TimelineButton::Last)
        && icon_button(
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
    if shows(app, TimelineButton::Record)
        && icon_toggle(
            ui,
            Icon::Record,
            "Record held objects: while on, dragging an object keys every frame (F6 keys one at a time)",
            app.session.record_held,
        )
        .clicked()
    {
        app.session.record_held = !app.session.record_held;
    }
    if shows(app, TimelineButton::FrameNumber) {
        let frame_count = current_frame_count(app);
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
    }
}

fn current_frame_count(app: &EditorApp) -> u16 {
    app.state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == app.session.current_q0rg_id)
        .map(|q| q.frame_count)
        .unwrap_or(1)
}

fn bar_view(app: &mut EditorApp, theme: &Theme, ui: &mut Ui) {
    let frame_count = current_frame_count(app);
    let q0rg_id = app.session.current_q0rg_id;
    if shows(app, TimelineButton::LoopRange) {
        let mut range = app
            .session
            .preview_loop_ranges
            .get(&q0rg_id)
            .copied()
            .unwrap_or((false, 0, frame_count.saturating_sub(1)));
        let range_before = range;
        ui.menu_button(
            if range.0 {
                "loop range: on"
            } else {
                "loop range"
            },
            |ui| {
                ui.checkbox(&mut range.0, "Loop selected range");
                let mut first = u32::from(range.1.min(frame_count.saturating_sub(1))) + 1;
                let mut last = u32::from(range.2.min(frame_count.saturating_sub(1))) + 1;
                ui.horizontal(|ui| {
                    ui.label("From");
                    ui.add(
                        egui::DragValue::new(&mut first)
                            .clamp_range(1..=u32::from(frame_count.max(1))),
                    );
                    ui.label("to");
                    ui.add(
                        egui::DragValue::new(&mut last)
                            .clamp_range(first..=u32::from(frame_count.max(1))),
                    );
                });
                range.1 = (first - 1) as u16;
                range.2 = (last.max(first) - 1) as u16;
                let selected = app
                    .session
                    .timeline_cell_selection
                    .as_ref()
                    .filter(|(id, _)| *id == q0rg_id)
                    .and_then(|(_, cells)| {
                        Some((
                            cells.iter().map(|cell| cell.1).min()?,
                            cells.iter().map(|cell| cell.1).max()?,
                        ))
                    })
                    .or_else(|| {
                        app.session.timeline_selection.map(|selection| {
                            (
                                selection.anchor_frame.min(selection.focus_frame),
                                selection.anchor_frame.max(selection.focus_frame),
                            )
                        })
                    })
                    .or_else(|| {
                        app.session
                            .camera_timeline_selection
                            .map(|selection| (selection.first_frame(), selection.last_frame()))
                    });
                if ui
                    .add_enabled(selected.is_some(), egui::Button::new("Use selected frames"))
                    .clicked()
                {
                    if let Some((first, last)) = selected {
                        range = (
                            true,
                            first.min(frame_count.saturating_sub(1)),
                            last.min(frame_count.saturating_sub(1)),
                        );
                    }
                }
            },
        );
        if range != range_before {
            app.session.preview_loop_ranges.insert(q0rg_id, range);
        }
    }
    if shows(app, TimelineButton::Labels) {
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
    }
    // Onion skin: its before/after counts show only while it is on, so the
    // bar stays short otherwise.
    if shows(app, TimelineButton::Onion)
        && icon_toggle(
            ui,
            Icon::Onion,
            "Onion skin: show neighbouring frames on the active layer",
            app.session.onion.enabled,
        )
        .clicked()
    {
        app.session.onion.enabled = !app.session.onion.enabled;
        if app.session.onion.enabled {
            crate::frame_edit::disable(app);
        }
    }
    if shows(app, TimelineButton::EditFrames)
        && ui
            .selectable_label(app.session.onion.edit_frames, "Edit frames")
            .on_hover_text(
                "Edit objects across a frame range. Drag the coloured range handles on the ruler.",
            )
            .clicked()
    {
        if app.session.onion.edit_frames {
            crate::frame_edit::disable(app);
        } else {
            crate::frame_edit::enable(app);
        }
    }
    if app.session.onion.enabled || app.session.onion.edit_frames {
        ui.checkbox(&mut app.session.onion.all_unlocked_layers, "all unlocked")
            .on_hover_text(
                "Off: onion skin only on the active layer. On: all visible, unlocked layers.",
            );
        if app.session.onion.enabled
            && ui
                .checkbox(&mut app.settings.onion_unlit, "no lights")
                .on_hover_text(
                    "Turns the scene's lights off while onion skin shows, so ghosts are easy to see in a dark scene. Off: everything stays lit, ghosts included.",
                )
                .changed()
        {
            app.settings.save();
        }
        ui.label(
            egui::RichText::new("Before")
                .color(theme.text_dim.to_color32())
                .small(),
        );
        ui.add(
            egui::DragValue::new(&mut app.session.onion.before)
                .clamp_range(0..=u16::MAX)
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
                .clamp_range(0..=u16::MAX)
                .speed(0.1),
        )
        .on_hover_text("Frames to ghost after current");
    }
    if shows(app, TimelineButton::Fps) {
        // The project's rate, and while playing, the rate frames really
        // show at.
        let target = app.state.project.meta.fps;
        let shown =
            crate::fps::playback_rate(ui.ctx(), app.session.playing, app.session.timeline_frame());
        let (text, color) = match shown {
            Some(rate) => (
                format!("{rate:.1} / {target} fps"),
                if rate >= f32::from(target) * 0.95 {
                    theme.text_dim.to_color32()
                } else {
                    egui::Color32::from_rgb(235, 170, 80)
                },
            ),
            None => (format!("{target} fps"), theme.text_dim.to_color32()),
        };
        ui.label(egui::RichText::new(text).color(color)).on_hover_text(
            "The project's frame rate. While it plays: how many frames a second really show, of how many it should.",
        );
    }
    bar_chooser(app, ui);
}

/// The bar's own menu: every button, to switch on and off.
fn bar_chooser(app: &mut EditorApp, ui: &mut Ui) {
    let open_id = ui.id().with("timeline_bar_chooser");
    let mut open = ui
        .data(|data| data.get_temp::<bool>(open_id))
        .unwrap_or(false);
    let button = icon_toggle(ui, Icon::Menu, "Choose the timeline's buttons", open);
    if button.clicked() {
        open = !open;
    }
    if open {
        // The timeline usually sits at the bottom: open upward there.
        let upward = button.rect.center().y > ui.ctx().screen_rect().center().y;
        let (at, pivot) = if upward {
            (
                button.rect.right_top() - vec2(0.0, 4.0),
                egui::Align2::RIGHT_BOTTOM,
            )
        } else {
            (
                button.rect.right_bottom() + vec2(0.0, 4.0),
                egui::Align2::RIGHT_TOP,
            )
        };
        let area = egui::Area::new(open_id.with("area"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .pivot(pivot)
            .constrain(true)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(330.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Timeline buttons").strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    !app.settings.timeline_bar.hidden.is_empty(),
                                    egui::Button::new("Show all"),
                                )
                                .clicked()
                            {
                                app.settings.timeline_bar.hidden.clear();
                                app.settings.save();
                            }
                        });
                    });
                    ui.label(
                        egui::RichText::new("Click one to hide or show it on the bar.")
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(4.0);
                    let groups: [&[TimelineButton]; 3] = [
                        &TimelineButton::ALL[..16],
                        &TimelineButton::ALL[16..23],
                        &TimelineButton::ALL[23..],
                    ];
                    let mut changed = None;
                    for group in groups {
                        ui.horizontal_wrapped(|ui| {
                            for button in group {
                                let visible = app.settings.timeline_bar.shows(*button);
                                let response = match chooser_icon(*button) {
                                    Some(icon) => icon_toggle(ui, icon, button.label(), visible),
                                    None => ui.selectable_label(visible, button.label()),
                                };
                                if response.clicked() {
                                    changed = Some(*button);
                                }
                            }
                        });
                        ui.add_space(2.0);
                    }
                    if let Some(button) = changed {
                        let hidden = &mut app.settings.timeline_bar.hidden;
                        if !hidden.remove(&button) {
                            hidden.insert(button);
                        }
                        app.settings.save();
                    }
                    ui.separator();
                    if ui
                        .checkbox(
                            &mut app.settings.onion_skin_during_playback,
                            "Show onion skin during playback",
                        )
                        .changed()
                    {
                        app.settings.save();
                    }
                });
            });
        let clicked_outside = ui.input(|input| {
            input.pointer.any_pressed()
                && input.pointer.interact_pos().is_some_and(|pos| {
                    !area.response.rect.contains(pos) && !button.rect.contains(pos)
                })
        });
        if clicked_outside || ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            open = false;
        }
    }
    ui.data_mut(|data| data.insert_temp(open_id, open));
}

fn chooser_icon(button: TimelineButton) -> Option<Icon> {
    Some(match button {
        TimelineButton::Camera => Icon::Camera,
        TimelineButton::Delete => Icon::Delete,
        TimelineButton::AddLayer => Icon::Layer,
        TimelineButton::AddFolder => Icon::Folder,
        TimelineButton::AddMask => Icon::Mask,
        TimelineButton::LayerUp => Icon::Up,
        TimelineButton::LayerDown => Icon::Down,
        TimelineButton::Indent => Icon::Indent,
        TimelineButton::Outdent => Icon::Outdent,
        TimelineButton::NewSymbol => Icon::Symbol,
        TimelineButton::InsertFrame => Icon::Frame,
        TimelineButton::Keyframe => Icon::Keyframe,
        TimelineButton::BlankKeyframe => Icon::BlankKeyframe,
        TimelineButton::RemoveColumns => Icon::RemoveColumns,
        TimelineButton::MotionTween => Icon::Tween,
        TimelineButton::ShapeTween => Icon::ShapeTween,
        TimelineButton::First => Icon::First,
        TimelineButton::Previous => Icon::Previous,
        TimelineButton::Play => Icon::Play,
        TimelineButton::Next => Icon::Next,
        TimelineButton::Last => Icon::Last,
        TimelineButton::Record => Icon::Record,
        TimelineButton::Onion => Icon::Onion,
        TimelineButton::FrameNumber
        | TimelineButton::LoopRange
        | TimelineButton::Labels
        | TimelineButton::EditFrames
        | TimelineButton::Fps
        | TimelineButton::Unknown => return None,
    })
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
    crate::dialog::Dialog::new("Rename layer")
        .id(egui::Id::new("timeline_layer_rename_dialog"))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .fit_to_screen(ctx)
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
    crate::dialog::Dialog::new(format!("frame {} label", u32::from(edit.frame) + 1))
        .id(egui::Id::new("timeline_frame_label_dialog"))
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .fit_to_screen(ctx)
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

/// The eye. `open` runs from 0 (shut: a lid with lashes) to 1 (open, with
/// its iris and pupil), so a toggle blinks instead of swapping pictures.
fn draw_visibility_icon(painter: &egui::Painter, rect: Rect, open: f32, color: Color32) {
    let open = open.clamp(0.0, 1.0);
    let center = rect.center();
    let half_w = (rect.width() * 0.47).min(6.0);
    let half_h = (rect.height() * 0.32).min(3.3);
    let ink = color.gamma_multiply(0.42 + 0.58 * open);
    let stroke = Stroke::new(1.15_f32, ink);
    // Across the eye: where each lid point sits open and shut.
    let xs = [-1.0_f32, -0.7, -0.35, 0.0, 0.35, 0.7, 1.0];
    let wide = [0.0_f32, -0.62, -0.92, -1.0, -0.92, -0.62, 0.0];
    let shut = [0.0_f32, 0.3, 0.52, 0.6, 0.52, 0.3, 0.0];
    let lid = |sign: f32| -> Vec<egui::Pos2> {
        xs.iter()
            .zip(wide.iter().zip(shut.iter()))
            .map(|(x, (wide, closed))| {
                pos2(
                    center.x + half_w * x,
                    center.y + half_h * (closed + (wide * sign - closed) * open),
                )
            })
            .collect()
    };
    painter.add(egui::Shape::line(lid(1.0), stroke));
    painter.add(egui::Shape::line(lid(-1.0), stroke));
    if open > 0.3 {
        let iris = (open - 0.3) / 0.7;
        painter.circle_stroke(center, 2.2 * iris, Stroke::new(1.0_f32, ink));
        painter.circle_filled(center, 1.05 * iris, ink);
        painter.circle_filled(
            center + vec2(0.9, -0.9) * iris,
            0.55 * iris,
            ink.gamma_multiply(0.8),
        );
    }
    if open < 0.6 {
        let lashes = 1.0 - open / 0.6;
        for x in [-0.55_f32, 0.0, 0.55] {
            let at = pos2(
                center.x + half_w * x,
                center.y + half_h * (0.6 - 0.3 * x.abs()),
            );
            painter.line_segment(
                [at, at + vec2(x * 1.2, 1.7) * lashes],
                Stroke::new(0.9_f32, ink.gamma_multiply(lashes)),
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

/// The outline switch: a small square, solid while the layer shows its
/// fills and hollow, bright, while it shows outlines only. `on` (0..1)
/// empties the square as it turns on.
fn draw_outline_icon(painter: &egui::Painter, rect: Rect, on: f32, color: Color32) {
    let on = on.clamp(0.0, 1.0);
    let ink = color.gamma_multiply(0.42 + 0.58 * on);
    let side = rect.width().min(rect.height()).clamp(6.0, 9.0);
    let square = Rect::from_center_size(rect.center(), vec2(side, side));
    if on < 0.99 {
        painter.rect_filled(square, 1.6, color.gamma_multiply(0.45 * (1.0 - on)));
    }
    painter.rect_stroke(square, 1.6, Stroke::new(1.2_f32, ink));
}

/// The padlock; `locked` (0..1) drops the shackle into the body and shows
/// the keyhole, so a toggle clicks shut instead of swapping pictures.
fn draw_lock_icon(painter: &egui::Painter, rect: Rect, locked: f32, color: Color32) {
    let locked = locked.clamp(0.0, 1.0);
    let center = rect.center();
    let ink = color.gamma_multiply(0.42 + 0.58 * locked);
    let stroke = Stroke::new(1.1_f32, ink);

    let body_w = (rect.width() * 0.58).clamp(6.5, 8.0);
    let body_h = (rect.height() * 0.42).clamp(4.5, 5.8);
    let body = Rect::from_center_size(pos2(center.x, center.y + 2.2), vec2(body_w, body_h));
    painter.rect_filled(body, 1.5, ink.gamma_multiply(0.12 + 0.22 * locked));
    painter.rect_stroke(body, 1.5, stroke);

    let half = body_w * 0.31;
    let open = 1.0 - locked;
    // Open, the shackle rides up and its right leg comes out of the body.
    let lift = 1.8 * open;
    let top = body.top() - 4.0 - lift;
    let bottom = body.top() + 0.6;
    let right_bottom = bottom - lift - 3.2 * open;
    let shackle = vec![
        pos2(center.x - half, bottom),
        pos2(center.x - half, top + 1.6),
        pos2(center.x - half * 0.55, top + 0.4),
        pos2(center.x, top),
        pos2(center.x + half * 0.55, top + 0.4),
        pos2(center.x + half, top + 1.6),
        pos2(center.x + half, right_bottom.max(top + 1.6)),
    ];
    painter.add(egui::Shape::line(shackle, stroke));
    if locked > 0.05 {
        let hole = pos2(center.x, body.center().y - 0.5);
        painter.circle_filled(hole, 1.0 * locked, color.gamma_multiply(locked));
        painter.line_segment(
            [hole, hole + vec2(0.0, 1.6 * locked)],
            Stroke::new(0.9_f32, color.gamma_multiply(locked)),
        );
    }
}

pub(crate) fn draw_layer_icon(
    painter: &egui::Painter,
    rect: Rect,
    kind: q0s_format::v2::LayerKind,
    collapsed: bool,
    foreground: Color32,
    accent: Color32,
) {
    draw_layer_icon_opening(
        painter,
        rect,
        kind,
        if collapsed { 0.0 } else { 1.0 },
        foreground,
        accent,
    );
}

/// A small triangle round `center` pointing right, turned a quarter down as
/// `open` goes from 0 to 1: a folder's arrow swinging open.
fn turning_arrow(center: egui::Pos2, (along, across): (f32, f32), open: f32) -> Vec<egui::Pos2> {
    let (sin, cos) = (open.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin_cos();
    [
        (-along * 0.75, -across),
        (along, 0.0),
        (-along * 0.75, across),
    ]
    .into_iter()
    .map(|(x, y)| center + vec2(x * cos - y * sin, x * sin + y * cos))
    .collect()
}

/// The layer kind's icon, with a folder's or mask's arrow `open` from 0
/// (closed, pointing right) to 1 (open, pointing down).
fn draw_layer_icon_opening(
    painter: &egui::Painter,
    rect: Rect,
    kind: q0s_format::v2::LayerKind,
    open: f32,
    foreground: Color32,
    accent: Color32,
) {
    let tint =
        |alpha: u8| Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), alpha);
    if kind == q0s_format::v2::LayerKind::Folder {
        // A folder with its tab; it fills a little more as it opens.
        let body = Rect::from_min_max(
            pos2(rect.left(), rect.top() + 3.0),
            pos2(rect.right(), rect.bottom() - 0.5),
        );
        let tab = Rect::from_min_max(
            pos2(rect.left(), rect.top() + 1.0),
            pos2(rect.left() + 5.5, rect.top() + 4.0),
        );
        painter.rect_filled(tab, 1.2, accent);
        painter.rect_filled(body, 2.0, tint(40 + (30.0 * open) as u8));
        painter.rect_stroke(body, 2.0, Stroke::new(1.1_f32, accent));
        painter.add(egui::Shape::convex_polygon(
            turning_arrow(body.center() + vec2(0.0, 0.3), (2.0, 2.4), open),
            foreground,
            Stroke::NONE,
        ));
    } else if kind == q0s_format::v2::LayerKind::Mask {
        // A stencil: a square with the shape that cuts through it.
        let frame = rect.shrink2(vec2(1.0, 1.5));
        painter.rect_filled(frame, 2.0, tint(45));
        painter.rect_stroke(frame, 2.0, Stroke::new(1.1_f32, accent));
        painter.circle_filled(
            frame.center() - vec2(1.2, 0.8),
            frame.height() * 0.28,
            accent,
        );
        painter.add(egui::Shape::convex_polygon(
            turning_arrow(
                pos2(frame.right() - 2.6, frame.bottom() - 2.4),
                (1.2, 1.5),
                open,
            ),
            foreground,
            Stroke::NONE,
        ));
    } else {
        // Two sheets, one over the other.
        let center = rect.center();
        let w = rect.width() * 0.48;
        let h = w * 0.5;
        let sheet = |dy: f32| {
            [
                center + vec2(-w, dy),
                center + vec2(0.0, dy - h),
                center + vec2(w, dy),
                center + vec2(0.0, dy + h),
            ]
        };
        let below = sheet(1.9);
        painter.add(egui::Shape::line(
            vec![below[0], below[3], below[2]],
            Stroke::new(1.1_f32, foreground.gamma_multiply(0.8)),
        ));
        let above = sheet(-1.4);
        painter.add(egui::Shape::convex_polygon(
            above.to_vec(),
            tint(70),
            Stroke::new(1.1_f32, accent),
        ));
    }
}

/// The camera row's icon: a film camera, two reels on its back.
fn draw_camera_icon(painter: &egui::Painter, rect: Rect, foreground: Color32, accent: Color32) {
    let c = rect.center();
    let body = Rect::from_min_max(
        pos2(rect.left(), c.y - 2.0),
        pos2(rect.right() - 4.0, c.y + 4.0),
    );
    painter.rect_filled(
        body,
        1.6,
        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 60),
    );
    painter.rect_stroke(body, 1.6, Stroke::new(1.1_f32, accent));
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(body.right(), c.y + 0.2),
            pos2(rect.right(), c.y - 1.8),
            pos2(rect.right(), c.y + 3.8),
            pos2(body.right(), c.y + 1.8),
        ],
        accent,
        Stroke::NONE,
    ));
    for x in [body.left() + 2.4, body.left() + 6.4] {
        painter.circle_stroke(pos2(x, c.y - 4.2), 1.9, Stroke::new(1.0_f32, foreground));
    }
}

/// Paint the empty cells under a row's spans. Inside the row's frames: one
/// strip with round ends, a hairline between frames that have room for it
/// and every fifth frame lifted a shade (the active row a touch brighter, so
/// it is clear where drawing lands). Past them the row's dark base shows,
/// every fifth frame a shade lighter: the band of frames still to come.
fn draw_empty_cells(
    painter: &egui::Painter,
    theme: &Theme,
    base_x: f32,
    row_y: f32,
    frame_count: u16,
    visible_frames: u16,
    is_current_layer: bool,
) {
    let slots = visible_frame_slots(painter, base_x, visible_frames);
    if slots.is_empty() {
        return;
    }
    let top = row_y + CELL_PAD_Y;
    let bottom = row_y + row_h() - CELL_PAD_Y;
    let fw = frame_w();
    let beyond_5 = theme.empty_beyond_5.to_color32();
    for f in slots.clone().filter(|f| *f >= frame_count && f % 5 == 4) {
        let x = base_x + f32::from(f) * fw;
        painter.rect_filled(
            Rect::from_min_max(pos2(x, top), pos2(x + fw, bottom)),
            0.0,
            beyond_5,
        );
    }
    if frame_count == 0 {
        return;
    }
    let inside = if is_current_layer {
        theme.empty_inside_active.to_color32()
    } else {
        theme.empty_inside.to_color32()
    };
    let strip = Rect::from_min_max(
        pos2(base_x, top),
        pos2(base_x + f32::from(frame_count) * fw, bottom),
    );
    painter.rect_filled(strip, (strip.height() * 0.2).clamp(1.5, 3.5), inside);
    let fifth = look::mix(inside, theme.text.to_color32(), 0.05);
    let grid = theme.timeline_grid.to_color32().gamma_multiply(0.7);
    for f in slots.filter(|f| *f < frame_count) {
        let x = base_x + f32::from(f) * fw;
        if f % 5 == 4 && f + 1 < frame_count {
            painter.rect_filled(
                Rect::from_min_max(pos2(x, top), pos2(x + fw, bottom)),
                0.0,
                fifth,
            );
        }
        if f > 0 && (fw >= 7.0 || f % 5 == 0) {
            painter.line_segment(
                [pos2(x, top + 1.0), pos2(x, bottom - 1.0)],
                Stroke::new(1.0_f32, grid),
            );
        }
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
        let x = layout.base_x + f32::from(script.frame) * frame_w() + frame_w() * 0.5;
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
    for (clip_idx, clip) in project.audio_clips.iter().enumerate() {
        if clip.q0rg_id != q0rg_id || clip.layer_id != layer.layer_id {
            continue;
        }
        let Some(reference) = crate::audio::audio_clip_ref(project, clip_idx) else {
            continue;
        };
        let end = reference.end_frame_exclusive.min(frame_count);
        if end <= clip.start_frame {
            continue;
        }
        let left = base_x + f32::from(clip.start_frame) * frame_w();
        let right = base_x + f32::from(end) * frame_w();
        let rect = Rect::from_min_max(
            pos2(left + 0.5, row_y + 2.0),
            pos2(right - 0.5, row_y + row_h() - 2.0),
        );
        let visible = rect.intersect(painter.clip_rect());
        if !visible.is_positive() {
            continue;
        }
        let p = painter.with_clip_rect(visible);
        let accent = if clip.muted {
            theme.text_dim.to_color32()
        } else {
            theme.accent.to_color32()
        };
        p.rect_filled(
            rect,
            3.0,
            blend(
                theme.panel.to_color32(),
                accent,
                if clip.muted { 0.08 } else { 0.19 },
            ),
        );
        p.rect_stroke(rect, 3.0, Stroke::new(0.8_f32, accent.gamma_multiply(0.65)));
        let name = project
            .asset_names
            .get(&clip.asset_id)
            .map(String::as_str)
            .unwrap_or("Audio");
        if rect.width() > 45.0 {
            p.text(
                pos2(left + 5.0, row_y + 3.0),
                egui::Align2::LEFT_TOP,
                if clip.muted {
                    format!("{name} · muted")
                } else {
                    name.to_string()
                },
                egui::FontId::proportional(9.0),
                theme.text.to_color32(),
            );
        }
        let Some((waveform, complete)) = textures.audio_waveform(project, clip.asset_id) else {
            continue;
        };
        if !complete {
            p.ctx()
                .request_repaint_after(std::time::Duration::from_millis(30));
        }
        if waveform.bins.is_empty() {
            p.line_segment(
                [
                    pos2(visible.left() + 2.0, row_y + 18.0),
                    pos2(visible.right() - 2.0, row_y + 18.0),
                ],
                Stroke::new(0.6_f32, accent.gamma_multiply(0.5)),
            );
            continue;
        }
        let center = row_y + 18.0;
        let amplitude = 4.0;
        let columns = (visible.width() / 1.5).ceil().clamp(1.0, 4096.0) as usize;
        let source_samples =
            waveform.samples_per_channel as f64 * 48_000.0 / f64::from(waveform.sample_rate.max(1));
        let sample_at_x = |x: f32| -> f64 {
            f64::from(((x - base_x) / frame_w() - f32::from(clip.start_frame)).max(0.0)) * 48_000.0
                / f64::from(project.meta.fps.max(1))
        };
        let length =
            u64::from(end - clip.start_frame) * 48_000 / u64::from(project.meta.fps.max(1));
        for column in 0..columns {
            let x0 = visible.left() + column as f32 * visible.width() / columns as f32;
            let x1 = visible.left() + (column + 1) as f32 * visible.width() / columns as f32;
            let local = sample_at_x((x0 + x1) * 0.5) as u64;
            let bin_at = |x| {
                ((clip.source_offset_samples as f64 + sample_at_x(x)) / source_samples.max(1.0)
                    * waveform.bins.len() as f64) as usize
            };
            let b0 = bin_at(x0).min(waveform.bins.len() - 1);
            let b1 = bin_at(x1)
                .saturating_add(1)
                .min(waveform.bins.len())
                .max(b0 + 1);
            let (mut min, mut max) = (0.0_f32, 0.0_f32);
            for &(lo, hi) in &waveform.bins[b0..b1] {
                min = min.min(lo);
                max = max.max(hi);
            }
            let gain = q0s_format::v2::audio_clip_channel_gain(*clip, local, length, 48_000, 0)
                .max(q0s_format::v2::audio_clip_channel_gain(
                    *clip, local, length, 48_000, 1,
                ));
            let gain = if clip.muted { clip.gain } else { gain };
            let x = (x0 + x1) * 0.5;
            p.line_segment(
                [
                    pos2(x, center - (max * gain).clamp(-1.0, 1.0) * amplitude),
                    pos2(x, center - (min * gain).clamp(-1.0, 1.0) * amplitude),
                ],
                Stroke::new(
                    1.0_f32,
                    accent.gamma_multiply(if clip.muted { 0.35 } else { 0.9 }),
                ),
            );
        }
        p.line_segment(
            [pos2(visible.left(), center), pos2(visible.right(), center)],
            Stroke::new(0.4_f32, accent.gamma_multiply(0.4)),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_layer_spans(
    painter: &egui::Painter,
    theme: &Theme,
    layer: &q0s_format::v2::Layer,
    base_x: f32,
    row_y: f32,
    frame_count: u16,
    is_current_layer: bool,
    (q0rg_id, playhead): (u16, Playhead),
) {
    let layer_frame_count = layer
        .timeline_frame_count
        .unwrap_or(frame_count)
        .min(frame_count);
    let keyframes = visible_keyframes(layer, layer_frame_count);
    let pops = look::key_pops(
        painter.ctx(),
        egui::Id::new(("timeline_keys", q0rg_id, layer.layer_id)),
        &keyframes,
    );
    let frame_slots = visible_frame_slots(painter, base_x, layer_frame_count);
    if frame_slots.is_empty() {
        return;
    }

    let cell_top = row_y + CELL_PAD_Y;
    let cell_bot = row_y + row_h() - CELL_PAD_Y;
    let static_fill = if is_current_layer {
        theme.extension_active.to_color32()
    } else {
        theme.extension.to_color32()
    };
    let tween_fill = theme.tween_fill.to_color32();
    let tween_arrow = theme.tween_arrow.to_color32();
    let shape_tween_fill = theme.shape_tween_fill().to_color32();
    let shape_tween_arrow = theme.shape_tween_arrow().to_color32();
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
        let has_shape_tween = layer
            .placements
            .iter()
            .any(|placement| placement.frame == keyframe && placement.tween.is_shape());
        let x0 = base_x + f32::from(keyframe) * frame_w();
        let x1 = base_x + (f32::from(span_end) + 1.0) * frame_w();
        // Each keyframe starts its own block, so two neighbouring keys never
        // fuse into one long cell.
        look::paint_span(
            painter,
            Rect::from_min_max(pos2(x0, cell_top), pos2(x1, cell_bot)),
            if has_shape_tween {
                shape_tween_fill
            } else if has_tween {
                tween_fill
            } else {
                static_fill
            },
            stroke_dark,
            !has_tween && span_end > keyframe,
        );
    }

    let mut arrows = std::collections::BTreeSet::new();
    for placement in &layer.placements {
        if let Some(to_frame) = placement.tween.to_frame() {
            if placement.frame < frame_slots.end
                && to_frame >= frame_slots.start
                && to_frame < layer_frame_count
            {
                arrows.insert((placement.frame, to_frame, placement.tween.is_shape()));
            }
        }
    }
    let mid_y = row_y + row_h() * 0.5;
    let bead_x = base_x + (playhead.shown + 0.5) * frame_w();
    for (from_frame, to_frame, is_shape) in arrows {
        look::paint_tween_arrow(
            painter,
            (
                base_x + (f32::from(from_frame) + 0.5) * frame_w() + 4.0,
                base_x + (f32::from(to_frame) + 0.5) * frame_w() - 4.0,
            ),
            mid_y,
            if is_shape {
                shape_tween_arrow
            } else {
                tween_arrow
            },
            (playhead.frame > from_frame && playhead.frame < to_frame).then_some(bead_x),
        );
    }

    let keyframe_color = theme.keyframe.to_color32();
    let hole = if is_current_layer {
        theme.empty_inside_active.to_color32()
    } else {
        theme.empty_inside.to_color32()
    };
    for keyframe in keyframes
        .into_iter()
        .filter(|frame| frame_slots.contains(frame))
    {
        look::paint_key(
            painter,
            &look::Key {
                center: pos2(
                    base_x + (f32::from(keyframe) + 0.5) * frame_w(),
                    key_y(row_y),
                ),
                radius: key_radius(),
                blank: layer.is_blank_keyframe(keyframe),
                color: keyframe_color,
                rim: stroke_dark.gamma_multiply(0.8),
                hole,
                age: pops
                    .iter()
                    .find(|(frame, _)| *frame == keyframe)
                    .map(|(_, age)| *age),
                ring: (keyframe == playhead.frame)
                    .then(|| theme.playhead.to_color32().gamma_multiply(0.75)),
            },
        );
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

#[cfg(test)]
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
    let frame = (rel_x / frame_w()).floor() as i32;
    if frame < 0 || frame >= i32::from(frame_count) {
        return None;
    }
    let row = layer_row_at(pos.y - rect.min.y - rows_h() * (camera_row_count as f32 + 1.0)) as i32;
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
    let stroke = Stroke::new(1.2_f32, accent);

    // One rounded band over every selected row, not a box per row.
    let selected = Rect::from_min_max(
        pos2(
            base_x + f32::from(first_frame) * frame_w(),
            first_row_y + layer_top(first_layer) + CELL_PAD_Y,
        ),
        pos2(
            base_x + (f32::from(last_frame) + 1.0) * frame_w(),
            first_row_y + layer_top(last_layer + 1) - CELL_PAD_Y,
        ),
    );
    painter.rect_filled(selected, 3.0, fill);
    painter.rect_stroke(selected, 3.0, stroke);
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
                base_x + f32::from(frame) * frame_w(),
                first_row_y + layer_top(row) + CELL_PAD_Y,
            ),
            vec2(frame_w(), layer_height(row) - CELL_PAD_Y * 2.0),
        );
        painter.rect_filled(rect, 2.5, fill);
        painter.rect_stroke(rect, 2.5, stroke);
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

/// Writes the timeline's sizes once they stop changing, not every frame of
/// a drag or a wheel.
fn save_sizes_when_settled(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(at) = app.session.timeline_sizes_changed else {
        return;
    };
    if at.elapsed() >= std::time::Duration::from_millis(600)
        && !ctx.input(|input| input.pointer.any_down())
    {
        app.session.timeline_sizes_changed = None;
        app.settings.save();
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(650));
    }
}

/// The bottom edge of each layer row, in the names column. Dragging it
/// makes every row taller or shorter; with Alt held, only that layer, which
/// then keeps a height of its own until its edge is double-clicked. A
/// double-click on a shared edge puts every row back to the usual height.
fn row_edges(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    layer_ids: &[u16],
    first_row_y: f32,
    (left, right): (f32, f32),
    rows: std::ops::Range<usize>,
) {
    let range = crate::settings::TIMELINE_ROW_HEIGHTS;
    for index in rows {
        let Some(&layer_id) = layer_ids.get(index) else {
            continue;
        };
        let bottom = first_row_y + layer_top(index + 1);
        let edge = Rect::from_min_max(pos2(left, bottom - 3.0), pos2(right, bottom + 2.0));
        let response = ui
            .interact(
                edge,
                ui.id().with(("timeline_row_edge", q0rg_id, layer_id)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::ResizeVertical)
            .on_hover_text(
                "Drag to make every row taller or shorter; hold Alt for this layer alone. \
                 Double-click to put it back",
            );
        let key = (q0rg_id, layer_id);
        let own = app.session.timeline_row_heights.get(&key).copied();
        if response.dragged() {
            let delta = response.drag_delta().y;
            let alone = ui.input(|input| input.modifiers.alt);
            if delta != 0.0 {
                if own.is_some() || alone {
                    let height = own.unwrap_or_else(rows_h);
                    app.session
                        .timeline_row_heights
                        .insert(key, (height + delta).clamp(*range.start(), *range.end()));
                } else {
                    app.settings.timeline_row_height =
                        (rows_h() + delta).clamp(*range.start(), *range.end());
                }
                app.session.timeline_sizes_changed = Some(std::time::Instant::now());
            }
        }
        if response.double_clicked() {
            if own.is_some() {
                app.session.timeline_row_heights.remove(&key);
            } else {
                app.settings.timeline_row_height = 26.0;
                app.session.timeline_sizes_changed = Some(std::time::Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_click_without_motion_moves_the_playhead() {
        let mut app = EditorApp::default();
        let ctx = egui::Context::default();
        let theme = app.settings.theme.clone();
        app.state.project.q0rgs[0].frame_count = 20;
        let press = pos2(140.0 + 4.5 * frame_w(), 10.0);
        let mut frame = |events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(600.0, 120.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show(ctx, |ui| {
                            timeline_ruler(&mut app, &theme, ui, 0, 140.0, 0.0, 20);
                        });
                },
            );
        };
        frame(Vec::new());
        for pressed in [true, false] {
            frame(vec![
                egui::Event::PointerMoved(press),
                egui::Event::PointerButton {
                    pos: press,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ]);
        }
        assert_eq!(app.session.current_frame, 4);
    }

    #[test]
    fn scrubbing_past_an_edge_crawls_near_it_and_speeds_up_further_out() {
        // Just past the edge: a few frames a second, not one a repaint.
        assert!(edge_scrub_rate(2.0) < 4.0);
        let mut previous = 0.0;
        for past in [5.0, 20.0, 60.0, 120.0, 200.0] {
            let rate = edge_scrub_rate(past);
            assert!(rate > previous, "{past} points out runs no faster");
            previous = rate;
        }
        assert!(edge_scrub_rate(5000.0) <= 200.0);
    }

    #[test]
    fn rows_of_their_own_height_still_line_up() {
        // Rows of 26, 60 and 26 points.
        ROW_TOPS.with(|tops| *tops.borrow_mut() = vec![0.0, 26.0, 86.0, 112.0]);
        assert_eq!(layer_top(2), 86.0);
        assert_eq!(layer_height(1), 60.0);
        assert_eq!(layer_row_at(10.0), 0);
        assert_eq!(layer_row_at(26.0), 1);
        assert_eq!(layer_row_at(85.0), 1);
        assert_eq!(layer_row_at(90.0), 2);
        assert_eq!(layer_row_at(-1.0), -1);
        // Past the table, rows have the usual height.
        assert_eq!(layer_row_at(112.0 + 30.0), 4);
        assert_eq!(layer_top(5), 112.0 + 2.0 * rows_h());
        assert_eq!(visible_layer_rows(100.0, 3, 130.0, 190.0), 1..3);
        ROW_TOPS.with(|tops| tops.borrow_mut().clear());
        assert_eq!(
            layer_row_at(53.0),
            2,
            "without a table every row is the usual height"
        );
    }
    use q0s_format::v2::{Layer, Placement, Target, Transform2D, Tween};

    #[test]
    fn viewport_slots_keep_partially_visible_frames_and_clamp_document_edges() {
        assert_eq!(visible_slots(174.0, 12.0, 65_535, 0.0, 774.0), 0..51);
        assert_eq!(visible_slots(-1200.0, 12.0, 65_535, 0.5, 600.5), 99..152);
        assert_eq!(visible_slots(174.0, 12.0, 10, 500.0, 800.0), 0..0);
        assert_eq!(visible_slots(174.0, 12.0, 10, 0.0, 170.0), 0..0);
        assert_eq!(visible_slots(0.0, row_h(), 1000, 260.0, 390.0), 9..16);
    }

    #[test]
    fn timeline_cell_paint_work_depends_on_viewport_not_movie_length() {
        fn paint_count(frames: u16, base_x: f32) -> usize {
            let ctx = egui::Context::default();
            let output = ctx.run(Default::default(), |ctx| {
                let painter = ctx
                    .layer_painter(egui::LayerId::background())
                    .with_clip_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, row_h())));
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
        // The strip, then at most a hairline and a tint per visible cell.
        assert!(short > 0 && short <= 1 + 52 * 2, "visible cells: {short}");
        assert_eq!(short, long);
        assert!(scrolled <= 1 + 53 * 2, "scrolled cells: {scrolled}");
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
                .with_clip_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, row_h())));
            draw_layer_spans(
                &painter,
                &Theme::default(),
                &layer,
                -1200.0,
                0.0,
                500,
                false,
                (
                    1,
                    Playhead {
                        frame: 0,
                        shown: 0.0,
                    },
                ),
            );
        });
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape, egui::Shape::Rect(rect) if rect.rect.left() < 0.0 && rect.rect.right() > 600.0
        )), "a held frame must fill the viewport even if its key is offscreen");
    }

    fn placement(frame: u16) -> Placement {
        Placement {
            pivot: None,
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
                    mask_preview: false,
                    raster: false,
                    fx: Default::default(),
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
                mask_preview: false,
                raster: false,
                fx: Default::default(),
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
                mask_preview: false,
                raster: false,
                fx: Default::default(),
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
                mask_preview: false,
                raster: false,
                fx: Default::default(),
            },
        );
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 120.0));
        let visible = [3, 2, 1];

        assert_eq!(
            layer_drop_target_at(&project, 1, rect, pos2(50.0, row_h() * 2.5), &visible, &[3],),
            Some(LayerDropTarget::IntoFolder(2))
        );
        assert_eq!(
            layer_drop_target_at(&project, 1, rect, pos2(8.0, row_h() * 3.75), &visible, &[1],),
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
                pos2(LAYER_LABEL_W + frame_w() * 2.5, row_h() * 1.5),
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
                pos2(LAYER_LABEL_W + 2.0, row_h() * 0.5),
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
                pos2(LAYER_LABEL_W + frame_w() * 6.0, row_h() * 1.5),
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
                pos2(LAYER_LABEL_W + frame_w() * 36.0, row_h() * 1.5),
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
                pos2(LAYER_LABEL_W + frame_w() * 2.5, row_h() * 1.5),
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
                pos2(LAYER_LABEL_W + frame_w() * 2.5, row_h() * 2.5),
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
