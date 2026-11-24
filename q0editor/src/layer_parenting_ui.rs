//! The timeline's parenting view, after Animate's: a column with a square in
//! each layer's outline colour, a child's a step to the right of its
//! parent's. Each parent owns a lane on the left of the squares; its
//! children's links run along it, so a link never crosses another layer's
//! square, and links that do cross each other break where they pass.
//!
//! Drag a square onto another layer to hang it there: the link runs out
//! like a rope and the parent pulses as it takes it. Drag it clear of every
//! layer to let it go, or click it for a menu. Hovering a square lights its
//! whole family and sends a spark up to its parent. The main timeline's
//! camera row has a square too: layers hung from the camera stay put in its
//! view, and a camera hung from a layer follows it.

use egui::epaint::CubicBezierShape;
use egui::{pos2, vec2, Color32, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui};

use crate::app::{Action, EditorApp};
use q0s_format::parenting::CAMERA_LAYER;

/// Width the view takes from the layer names while it is shown.
pub(crate) const COLUMN_WIDTH: f32 = 46.0;
/// The strip on the left where links run, one lane per parent.
const LANES: f32 = 17.0;
/// Between two lanes.
const LANE_STEP: f32 = 4.0;
/// How far right each generation's square sits.
const STEP: f32 = 4.0;
/// A link running out to its parent, or back in.
const LINK_SECS: f32 = 0.32;

fn rows_id(q0rg_id: u16) -> Id {
    Id::new(("timeline_parenting_rows", q0rg_id))
}

/// A menu choice made while the rows were drawn, queued by `finish`.
fn pending_id(q0rg_id: u16) -> Id {
    Id::new(("timeline_parenting_pending", q0rg_id))
}

fn drag_id(q0rg_id: u16) -> Id {
    Id::new(("timeline_parenting_drag", q0rg_id))
}

fn links_id(q0rg_id: u16) -> Id {
    Id::new(("timeline_parenting_links", q0rg_id))
}

fn pulse_id(q0rg_id: u16) -> Id {
    Id::new(("timeline_parenting_pulse", q0rg_id))
}

fn hover_id(q0rg_id: u16) -> Id {
    Id::new(("timeline_parenting_hover", q0rg_id))
}

pub(crate) fn shown(app: &EditorApp) -> bool {
    app.settings.timeline_parenting_view
}

/// Shows or hides the view; the choice is kept with the editor's settings.
pub(crate) fn toggle(app: &mut EditorApp) {
    app.settings.timeline_parenting_view = !app.settings.timeline_parenting_view;
    app.settings.save();
}

/// The room the view needs in a layer row, 0 when it is hidden.
pub(crate) fn column_width(app: &EditorApp) -> f32 {
    if shown(app) {
        COLUMN_WIDTH
    } else {
        0.0
    }
}

#[derive(Clone, Copy)]
struct Row {
    layer_id: u16,
    square: Rect,
    row: Rect,
    column: Rect,
    color: Color32,
}

/// The links seen last frame and the ones on their way in or out.
#[derive(Clone, Default)]
struct Links {
    /// (child, parent) as drawn last frame.
    seen: Vec<(u16, u16)>,
    /// Newly made: (child, parent, since).
    made: Vec<(u16, u16, f64)>,
    /// Just undone, still drawing back in: (child, parent, since).
    undone: Vec<(u16, u16, f64)>,
    /// Seen once: the links there at first sight do not run out.
    ready: bool,
}

fn now(ui: &Ui) -> f64 {
    ui.input(|input| input.time)
}

fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// The family of `layer_id`: itself, its ancestors and its descendants.
fn family(app: &EditorApp, q0rg_id: u16, layer_id: u16) -> Vec<u16> {
    let project = &app.state.project;
    let mut out = vec![layer_id];
    let mut up = q0s_format::parenting::parent_of(project, q0rg_id, layer_id);
    while let Some(id) = up.filter(|id| !out.contains(id)) {
        out.push(id);
        up = q0s_format::parenting::parent_of(project, q0rg_id, id);
    }
    let mut frontier = vec![layer_id];
    while let Some(parent) = frontier.pop() {
        for (key, &of) in &project.runtime.layer_parents {
            if key.q0rg_id == q0rg_id && of == parent && !out.contains(&key.layer_id) {
                out.push(key.layer_id);
                frontier.push(key.layer_id);
            }
        }
    }
    out
}

/// One layer's square. `column` is the view's strip in the row and `row` the
/// whole row, which takes drops.
#[allow(clippy::too_many_arguments)]
pub(crate) fn row(
    app: &EditorApp,
    ui: &Ui,
    painter: &Painter,
    q0rg_id: u16,
    layer_id: u16,
    column: Rect,
    row: Rect,
    folder: bool,
) {
    if !shown(app) || folder {
        return;
    }
    let project = &app.state.project;
    let ctx = ui.ctx();
    let generation = q0s_format::parenting::depth(project, q0rg_id, layer_id).min(3) as f32;
    let color = square_color(app, layer_id);
    // A square glides to its new step when it is hung or let go.
    let x = ctx.animate_value_with_time(
        Id::new(("layer_parent_square_x", q0rg_id, layer_id)),
        column.left() + LANES + 8.0 + generation * STEP,
        0.22,
    );
    let id = Id::new(("layer_parent_square", q0rg_id, layer_id));
    let base = Rect::from_center_size(pos2(x, column.center().y), vec2(8.0, 10.0));
    let response = ui
        .interact(base.expand(3.0), id, Sense::click_and_drag())
        .on_hover_text(if layer_id == CAMERA_LAYER {
            "Drag onto a layer to make the camera follow it. Click for a menu."
        } else {
            "Drag onto another layer to hang this layer from it, or clear of every layer to let it go. Click for a menu."
        })
        .on_hover_cursor(egui::CursorIcon::Grab);
    if response.hovered() {
        // Read outside the data lock: asking the context anything while it
        // holds that lock waits on itself forever.
        let frame = ctx.frame_nr();
        ctx.data_mut(|data| data.insert_temp(hover_id(q0rg_id), (layer_id, frame)));
    }
    let lit = ctx.animate_bool_with_time(
        id.with("lit"),
        response.hovered() || response.dragged(),
        0.12,
    );
    // Out of the hovered family, a square steps back.
    let hovered: Option<(u16, u64)> = ctx.data(|data| data.get_temp(hover_id(q0rg_id)));
    let dimmed = hovered
        .filter(|(_, frame)| frame + 1 >= ctx.frame_nr())
        .is_some_and(|(hovered, _)| !family(app, q0rg_id, hovered).contains(&layer_id));
    let dim = ctx.animate_bool_with_time(id.with("dim"), dimmed, 0.15);
    let pulse = ctx
        .data(|data| data.get_temp::<(u16, f64)>(pulse_id(q0rg_id)))
        .filter(|(layer, _)| *layer == layer_id)
        .map(|(_, since)| ((now(ui) - since) as f32 / 0.45).clamp(0.0, 1.0));
    let swell = lit * 1.5 + pulse.map_or(0.0, |t| 2.0 * (1.0 - t) * (t * 9.0).sin().abs());
    let square = base.expand(swell * 0.5);
    let shown_color = color.gamma_multiply(1.0 - 0.6 * dim);
    painter.rect_filled(
        square.translate(vec2(0.0, 1.0)),
        2.5,
        Color32::from_black_alpha((50.0 * (1.0 - dim)) as u8),
    );
    painter.rect_filled(square, 2.5, shown_color);
    // A sheen across the top, so it reads as a little block.
    painter.rect_filled(
        Rect::from_min_max(
            square.min + vec2(1.0, 1.0),
            pos2(square.max.x - 1.0, square.center().y - 1.0),
        ),
        1.5,
        Color32::from_white_alpha((40.0 * (1.0 - dim)) as u8),
    );
    if lit > 0.01 {
        painter.rect_stroke(
            square.expand(1.5),
            3.5,
            Stroke::new(
                1.2_f32,
                ui.visuals().strong_text_color().gamma_multiply(lit),
            ),
        );
    }
    if let Some(t) = pulse {
        if t < 1.0 {
            painter.rect_stroke(
                square.expand(2.0 + 7.0 * ease_out(t)),
                4.0,
                Stroke::new(1.5_f32 * (1.0 - t), color.gamma_multiply(1.0 - t)),
            );
            ctx.request_repaint();
        }
    }
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<Row>>(rows_id(q0rg_id))
            .push(Row {
                layer_id,
                square,
                row,
                column,
                color,
            });
    });
    if response.drag_started() {
        ctx.data_mut(|data| data.insert_temp(drag_id(q0rg_id), layer_id));
    }

    let popup = Id::new(("layer_parent_menu", q0rg_id, layer_id));
    if response.clicked() {
        ui.memory_mut(|memory| memory.toggle_popup(popup));
    }
    let parent = q0s_format::parenting::parent_of(project, q0rg_id, layer_id);
    let mut choice: Option<Option<u16>> = None;
    egui::popup::popup_below_widget(ui, popup, &response, |ui| {
        ui.set_min_width(180.0);
        ui.label(egui::RichText::new("Follows").small().weak());
        let entry = |ui: &mut Ui, swatch: Option<Color32>, on: bool, name: &str| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(8.0, 10.0), Sense::hover());
                match swatch {
                    Some(color) => {
                        ui.painter().rect_filled(rect, 2.0, color);
                    }
                    None => {
                        ui.painter().rect_stroke(
                            rect.shrink(0.5),
                            2.0,
                            Stroke::new(1.0_f32, ui.visuals().weak_text_color()),
                        );
                    }
                }
                ui.selectable_label(on, name).clicked()
            })
            .inner
        };
        if parent.is_some() && entry(ui, None, false, "Nothing (let go)") {
            choice = Some(None);
        }
        let Some(q0rg) = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        // The camera, for a layer of the main timeline that should stay put
        // in its view.
        if layer_id != CAMERA_LAYER
            && camera_row_shown(app, q0rg_id)
            && !q0s_format::parenting::would_loop(
                &app.state.project,
                q0rg_id,
                layer_id,
                CAMERA_LAYER,
            )
            && entry(
                ui,
                Some(square_color(app, CAMERA_LAYER)),
                parent == Some(CAMERA_LAYER),
                "Camera",
            )
        {
            choice = Some(Some(CAMERA_LAYER));
        }
        for layer in &q0rg.layers {
            let candidate = layer.layer_id;
            if candidate == layer_id
                || app.state.project.layer_is_folder(q0rg_id, candidate)
                || q0s_format::parenting::would_loop(
                    &app.state.project,
                    q0rg_id,
                    layer_id,
                    candidate,
                )
            {
                continue;
            }
            if entry(
                ui,
                Some(crate::render::outline_color_for_layer(candidate)),
                parent == Some(candidate),
                &layer.name,
            ) {
                choice = Some(Some(candidate));
            }
        }
    });
    if let Some(choice) = choice {
        ui.memory_mut(|memory| memory.close_popup());
        if choice != parent {
            ctx.data_mut(|data| data.insert_temp(pending_id(q0rg_id), (layer_id, choice)));
        }
    }
}

/// Whether the timeline shows a camera row (the main one, camera on).
fn camera_row_shown(app: &EditorApp, q0rg_id: u16) -> bool {
    app.state.project.camera_on(q0rg_id)
}

/// A layer's outline colour; the camera's is the theme's accent, like its
/// frame on the stage.
fn square_color(app: &EditorApp, layer_id: u16) -> Color32 {
    if layer_id == CAMERA_LAYER {
        app.settings.theme.accent.to_color32()
    } else {
        crate::render::outline_color_for_layer(layer_id)
    }
}

/// A link's way from the child's square to its parent's: left to the
/// parent's lane, along it, and right into the parent, the corners rounded.
/// `to` is None when the parent is out of view: the link then runs along
/// the lane to `edge`, the top or bottom of the rows.
fn link_path(from: Pos2, lane: f32, to: Option<Pos2>, edge: f32) -> Vec<Pos2> {
    let end_y = to.map_or(edge, |to| to.y);
    let down = if end_y >= from.y { 1.0 } else { -1.0 };
    let bend = 3.0_f32.min((end_y - from.y).abs() * 0.5);
    let mut points = vec![from, pos2(lane + bend, from.y)];
    // The first corner.
    for k in 1..=4 {
        let a = std::f32::consts::FRAC_PI_2 * k as f32 / 4.0;
        points.push(pos2(
            lane + bend * a.cos(),
            from.y + down * bend * (1.0 - a.sin()),
        ));
    }
    match to {
        Some(to) => {
            points.push(pos2(lane, to.y - down * bend));
            for k in 1..=4 {
                let a = std::f32::consts::FRAC_PI_2 * k as f32 / 4.0;
                points.push(pos2(
                    lane + bend * (1.0 - a.cos()),
                    to.y - down * bend * (1.0 - a.sin()),
                ));
            }
            points.push(pos2(to.x, to.y));
        }
        None => points.push(pos2(lane, edge)),
    }
    points
}

/// The first `share` of `points` by length.
fn part_of(points: &[Pos2], share: f32) -> Vec<Pos2> {
    if share >= 1.0 {
        return points.to_vec();
    }
    let total: f32 = points
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum();
    let mut left = total * share.max(0.0);
    let mut out = vec![points[0]];
    for pair in points.windows(2) {
        let length = pair[0].distance(pair[1]);
        if length >= left {
            if length > 1.0e-3 {
                out.push(pair[0] + (pair[1] - pair[0]) * (left / length));
            }
            break;
        }
        left -= length;
        out.push(pair[1]);
    }
    out
}

/// The point `share` of the way along `points`.
fn point_along(points: &[Pos2], share: f32) -> Pos2 {
    *part_of(points, share).last().unwrap_or(&points[0])
}

/// After every row: the links between children and parents on their lanes,
/// the drag rope, and the drop.
pub(crate) fn finish(app: &mut EditorApp, ui: &Ui, q0rg_id: u16) {
    let ctx = ui.ctx().clone();
    let rows: Vec<Row> = ctx
        .data_mut(|data| data.remove_temp(rows_id(q0rg_id)))
        .unwrap_or_default();
    let dragging: Option<u16> = ctx.data(|data| data.get_temp(drag_id(q0rg_id)));
    let pending: Option<(u16, Option<u16>)> =
        ctx.data_mut(|data| data.remove_temp(pending_id(q0rg_id)));
    if let Some((layer_id, parent)) = pending {
        app.queue(Action::SetLayerParent(q0rg_id, layer_id, parent));
        if let Some(parent) = parent {
            let since = now(ui);
            ctx.data_mut(|data| data.insert_temp(pulse_id(q0rg_id), (parent, since)));
        }
    }
    if !shown(app) || rows.is_empty() {
        ctx.data_mut(|data| data.remove::<u16>(drag_id(q0rg_id)));
        return;
    }
    let time = now(ui);
    let project = &app.state.project;
    let order = |layer_id: u16| {
        project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .position(|layer| layer.layer_id == layer_id)
            })
    };
    let top = rows
        .iter()
        .map(|row| row.row.top())
        .fold(f32::MAX, f32::min);
    let bottom = rows
        .iter()
        .map(|row| row.row.bottom())
        .fold(f32::MIN, f32::max);
    let column = rows[0].column;

    // The links now, and the ones just made or undone.
    let links: Vec<(u16, u16)> = rows
        .iter()
        .filter_map(|row| {
            q0s_format::parenting::parent_of(project, q0rg_id, row.layer_id)
                .map(|parent| (row.layer_id, parent))
        })
        .collect();
    let mut memory: Links = ctx
        .data(|data| data.get_temp(links_id(q0rg_id)))
        .unwrap_or_default();
    if memory.ready {
        for link in &links {
            if !memory.seen.contains(link) {
                memory.made.push((link.0, link.1, time));
                memory.undone.retain(|(child, _, _)| *child != link.0);
            }
        }
        for link in &memory.seen {
            if !links.contains(link) && rows.iter().any(|row| row.layer_id == link.0) {
                memory.undone.push((link.0, link.1, time));
            }
        }
    }
    memory
        .made
        .retain(|(_, _, since)| time - since < f64::from(LINK_SECS));
    memory
        .undone
        .retain(|(_, _, since)| time - since < f64::from(LINK_SECS));
    memory.seen = links.clone();
    memory.ready = true;
    let settling = !memory.made.is_empty() || !memory.undone.is_empty();

    // Lanes: each parent with children in view takes the lane nearest the
    // squares that is free over the rows its family spans. Shorter spans go
    // nearer, so long links wrap round short ones instead of crossing them.
    let y_of = |layer: u16| {
        rows.iter()
            .find(|row| row.layer_id == layer)
            .map(|row| row.square.center().y)
    };
    let mut spans: Vec<(u16, f32, f32)> = Vec::new();
    let every: Vec<(u16, u16)> = links
        .iter()
        .copied()
        .chain(
            memory
                .undone
                .iter()
                .map(|(child, parent, _)| (*child, *parent)),
        )
        .collect();
    for (child, parent) in every {
        let Some(child_y) = y_of(child) else {
            continue;
        };
        let parent_y = y_of(parent).unwrap_or(if order(parent) < order(child) {
            bottom
        } else {
            top
        });
        let (lo, hi) = (child_y.min(parent_y), child_y.max(parent_y));
        match spans.iter_mut().find(|(id, _, _)| *id == parent) {
            Some(span) => {
                span.1 = span.1.min(lo);
                span.2 = span.2.max(hi);
            }
            None => spans.push((parent, lo, hi)),
        }
    }
    spans.sort_by(|a, b| (a.2 - a.1).total_cmp(&(b.2 - b.1)));
    let mut lanes: Vec<(u16, usize)> = Vec::new();
    let mut taken: Vec<Vec<(f32, f32)>> = Vec::new();
    for (parent, lo, hi) in &spans {
        let lane = (0..)
            .find(|lane: &usize| {
                taken
                    .get(*lane)
                    .is_none_or(|spans| spans.iter().all(|(a, b)| hi + 2.0 < *a || lo - 2.0 > *b))
            })
            .unwrap_or(0);
        if taken.len() <= lane {
            taken.resize(lane + 1, Vec::new());
        }
        taken[lane].push((*lo, *hi));
        lanes.push((*parent, lane));
    }
    let lane_x = |parent: u16| {
        let lane = lanes
            .iter()
            .find(|(id, _)| *id == parent)
            .map_or(0, |(_, lane)| *lane);
        (column.left() + LANES - 1.0 - lane as f32 * LANE_STEP).max(column.left() + 1.5)
    };

    let painter = ui.painter().with_clip_rect(
        Rect::from_min_max(pos2(column.left(), top), pos2(column.right(), bottom))
            .intersect(ui.clip_rect()),
    );
    let halo = ui.visuals().panel_fill;
    let hovered: Option<u16> = ctx
        .data(|data| data.get_temp::<(u16, u64)>(hover_id(q0rg_id)))
        .filter(|(_, frame)| frame + 1 >= ctx.frame_nr())
        .map(|(layer, _)| layer);
    let lit_family = hovered.map(|layer| family(app, q0rg_id, layer));
    let mut sparks = false;
    let mut draw_link = |child: u16, parent: u16, share: f32| {
        let Some(child_row) = rows.iter().find(|row| row.layer_id == child) else {
            return;
        };
        let parent_row = rows.iter().find(|row| row.layer_id == parent);
        let lane = lane_x(parent);
        let from = child_row.square.left_center() - vec2(1.0, 0.0);
        let to = parent_row.map(|row| row.square.left_center() - vec2(1.0, 0.0));
        let edge = if order(parent) < order(child) {
            bottom + 6.0
        } else {
            top - 6.0
        };
        let points = part_of(&link_path(from, lane, to, edge), share);
        if points.len() < 2 {
            return;
        }
        let color = parent_row.map_or(child_row.color, |row| row.color);
        let in_family = lit_family
            .as_ref()
            .is_none_or(|family| family.contains(&child) && family.contains(&parent));
        let strength = if in_family { 1.0 } else { 0.3 };
        let width: f32 = if hovered.is_some() && in_family {
            2.0
        } else {
            1.5
        };
        painter.add(Shape::line(points.clone(), Stroke::new(width + 2.4, halo)));
        painter.add(Shape::line(
            points.clone(),
            Stroke::new(width, color.gamma_multiply(strength)),
        ));
        // Where it plugs into the parent, a dot; where it leaves the child, a
        // short tick in the child's colour.
        if share >= 1.0 {
            if let Some(to) = to {
                painter.circle_filled(to, 2.0, color.gamma_multiply(strength));
            }
            painter.line_segment(
                [from, from - vec2(3.0, 0.0)],
                Stroke::new(width + 0.5, child_row.color.gamma_multiply(strength)),
            );
        } else if let Some(head) = points.last() {
            painter.circle_filled(*head, 2.2, color);
        }
        // Hovered: a spark runs from the child up to the parent.
        if hovered.is_some() && in_family && share >= 1.0 {
            let full = link_path(from, lane, to, edge);
            let t = ((time * 0.9) % 1.0) as f32;
            let at = point_along(&full, ease_out(t));
            painter.circle_filled(at, 2.6, color.gamma_multiply(0.35 * (1.0 - t)));
            painter.circle_filled(at, 1.4, Color32::WHITE.gamma_multiply(1.0 - t));
            sparks = true;
        }
    };
    // Long links first, so short ones lie over them.
    let mut ordered = links.clone();
    ordered.sort_by_key(|(child, parent)| {
        let a = order(*child).unwrap_or(0) as i64;
        let b = order(*parent).unwrap_or(0) as i64;
        std::cmp::Reverse((a - b).abs())
    });
    for (child, parent) in ordered {
        let share = memory
            .made
            .iter()
            .find(|(c, p, _)| *c == child && *p == parent)
            .map_or(1.0, |(_, _, since)| {
                ease_out((time - since) as f32 / LINK_SECS)
            });
        draw_link(child, parent, share);
    }
    for (child, parent, since) in memory.undone.clone() {
        let t = ((time - since) as f32 / LINK_SECS).clamp(0.0, 1.0);
        draw_link(child, parent, 1.0 - ease_out(t));
    }
    ctx.data_mut(|data| data.insert_temp(links_id(q0rg_id), memory));
    if settling || sparks {
        ctx.request_repaint();
    }

    let Some(source) = dragging else {
        return;
    };
    let pointer = ctx.pointer_latest_pos();
    let source_row = rows.iter().find(|row| row.layer_id == source);
    let target = pointer.and_then(|pointer| {
        rows.iter()
            .find(|row| row.row.contains(pointer) && row.layer_id != source)
    });
    let allowed = target.is_some_and(|target| {
        !q0s_format::parenting::would_loop(&app.state.project, q0rg_id, source, target.layer_id)
    });
    let clear = pointer.is_some_and(|pointer| !rows.iter().any(|row| row.row.contains(pointer)));
    let has_parent =
        q0s_format::parenting::parent_of(&app.state.project, q0rg_id, source).is_some();
    let lets_go = clear && has_parent;
    crate::cursors::request(
        &ctx,
        if lets_go {
            crate::cursors::CursorKind::ParentUnlink
        } else {
            crate::cursors::CursorKind::ParentLink
        },
    );
    let free = ui.painter();
    if let Some(target) = target {
        let color = if allowed {
            target.color
        } else {
            ui.visuals().error_fg_color
        };
        let breathe = 0.5 + 0.5 * ((time * 6.0) as f32).sin();
        free.rect_filled(target.row, 0.0, color.gamma_multiply(0.07 + 0.04 * breathe));
        free.rect_stroke(
            target.square.expand(2.5 + breathe),
            3.5,
            Stroke::new(1.5_f32, color),
        );
    }
    if let (Some(from), Some(pointer)) = (source_row, pointer) {
        let color = if target.is_some() && !allowed {
            ui.visuals().error_fg_color
        } else {
            from.color
        };
        // Refused: the rope shudders.
        let shake = if target.is_some() && !allowed {
            vec2(((time * 55.0).sin() * 1.5) as f32, 0.0)
        } else {
            egui::Vec2::ZERO
        };
        let start = from.square.center();
        let end = pointer + shake;
        // A rope: it sags under its own weight, less the tauter it is pulled.
        let span = start.distance(end);
        let sag = (span * 0.22).min(26.0);
        let rope = CubicBezierShape::from_points_stroke(
            [
                start,
                start + (end - start) * 0.3 + vec2(0.0, sag),
                start + (end - start) * 0.7 + vec2(0.0, sag),
                end,
            ],
            false,
            Color32::TRANSPARENT,
            Stroke::NONE,
        );
        let points = rope.flatten(Some(0.5));
        let faded = if lets_go { 0.55 } else { 1.0 };
        free.add(Shape::line(
            points.clone(),
            Stroke::new(3.6_f32, halo.gamma_multiply(0.8)),
        ));
        // Dashes run along it toward the pointer.
        let dash = 6.0;
        let flow = ((time * 28.0) as f32).rem_euclid(dash * 2.0);
        let mut along = 0.0;
        for pair in points.windows(2) {
            let length = pair[0].distance(pair[1]);
            let phase = (along - flow).rem_euclid(dash * 2.0);
            let on = phase < dash || !lets_go;
            if on {
                free.line_segment(
                    [pair[0], pair[1]],
                    Stroke::new(1.7_f32, color.gamma_multiply(faded)),
                );
            }
            along += length;
        }
        if lets_go {
            // Let go here and the link breaks: a gap with a spark.
            let mid = point_along(&points, 0.5);
            free.circle_filled(mid, 3.5, halo);
            let spark = (((time * 2.0) % 1.0) as f32 * 4.0).min(1.0);
            free.circle_stroke(
                mid,
                2.0 + 3.0 * spark,
                Stroke::new(1.0_f32, color.gamma_multiply(1.0 - spark)),
            );
        } else {
            free.circle_filled(end, 3.2, color);
            free.circle_filled(end, 1.4, Color32::WHITE.gamma_multiply(0.8));
        }
        ctx.request_repaint();
    }
    if ui.input(|input| input.pointer.any_released()) {
        ctx.data_mut(|data| data.remove::<u16>(drag_id(q0rg_id)));
        if let Some(target) = target.filter(|_| allowed) {
            app.queue(Action::SetLayerParent(
                q0rg_id,
                source,
                Some(target.layer_id),
            ));
            ctx.data_mut(|data| data.insert_temp(pulse_id(q0rg_id), (target.layer_id, time)));
        } else if lets_go {
            // Dropped clear of every layer: no parent.
            app.queue(Action::SetLayerParent(q0rg_id, source, None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{
        Asset, Layer, Placement, PlacementFx, Target, Transform2D, Tween, VectorAsset,
    };

    fn placement(tx: f32, rotation: f32) -> Placement {
        Placement {
            pivot: None,
            instance_id: 0,
            frame: 0,
            target: Target::Asset(900),
            transform: Transform2D {
                tx,
                ty: 100.0,
                rotation,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: PlacementFx::default(),
        }
    }

    #[test]
    fn hanging_a_layer_is_undoable_and_stage_edits_land_in_the_parents_space() {
        let ctx = egui::Context::default();
        let mut app = EditorApp::default();
        let q0rg_id = app.session.current_q0rg_id;
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 900,
            paths: Vec::new(),
            fill: None,
            stroke: None,
        }));
        let q0rg = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .expect("current timeline");
        q0rg.layers = [
            (1, "arm", placement(100.0, std::f32::consts::FRAC_PI_2)),
            (2, "hand", placement(150.0, 0.0)),
        ]
        .into_iter()
        .map(|(layer_id, name, placement)| Layer {
            layer_id,
            timeline_frame_count: None,
            name: name.into(),
            explicit_keyframes: vec![0],
            placements: vec![placement],
        })
        .collect();

        app.handle(&ctx, Action::SetLayerParent(q0rg_id, 2, Some(1)));
        assert_eq!(
            q0s_format::parenting::parent_of(&app.state.project, q0rg_id, 2),
            Some(1)
        );
        let (shown, rig_bound) =
            crate::tools::placement_transform_for_edit(&app.state.project, q0rg_id, 2, 0, 0)
                .expect("hand on the stage");
        assert!(!rig_bound, "a parent is not a bone");
        assert!((shown.tx - 150.0).abs() < 1e-3 && (shown.ty - 100.0).abs() < 1e-3);

        // Dragged 10 px right on the stage, it is 10 px right on the stage.
        let moved = Transform2D {
            tx: shown.tx + 10.0,
            ..shown
        };
        assert!(crate::tools::set_placement_transform_for_edit(
            &mut app, q0rg_id, 2, 0, moved
        ));
        let (again, _) =
            crate::tools::placement_transform_for_edit(&app.state.project, q0rg_id, 2, 0, 0)
                .expect("hand on the stage");
        assert!(
            (again.tx - 160.0).abs() < 1e-3 && (again.ty - 100.0).abs() < 1e-3,
            "{again:?}"
        );

        app.handle(&ctx, Action::Undo);
        assert_eq!(
            q0s_format::parenting::parent_of(&app.state.project, q0rg_id, 2),
            None,
            "one undo frees it again"
        );
    }

    #[test]
    fn something_drawn_on_a_parented_layer_stays_where_it_was_drawn() {
        let ctx = egui::Context::default();
        let mut app = EditorApp::default();
        let q0rg_id = app.session.current_q0rg_id;
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 900,
            paths: Vec::new(),
            fill: None,
            stroke: None,
        }));
        let q0rg = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .expect("current timeline");
        q0rg.layers = vec![
            Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "body".into(),
                explicit_keyframes: vec![0],
                placements: vec![placement(100.0, 0.0)],
            },
            Layer {
                layer_id: 2,
                timeline_frame_count: None,
                name: "hat".into(),
                explicit_keyframes: vec![0],
                placements: Vec::new(),
            },
        ];
        app.handle(&ctx, Action::SetLayerParent(q0rg_id, 2, Some(1)));
        // Drawn at (130, 110) on the stage, with no id yet.
        let mut drawn = placement(130.0, 0.0);
        drawn.transform.ty = 110.0;
        app.state.project.q0rgs[0]
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == 2)
            .unwrap()
            .placements
            .push(drawn);
        assert!(crate::app::place_new_objects_under_parents(
            &mut app.state.project,
            q0rg_id,
            0
        ));
        q0s_format::parenting::tidy(&mut app.state.project);
        let (shown, _) =
            crate::tools::placement_transform_for_edit(&app.state.project, q0rg_id, 2, 0, 0)
                .expect("the hat on the stage");
        assert!(
            (shown.tx - 130.0).abs() < 1e-3 && (shown.ty - 110.0).abs() < 1e-3,
            "{shown:?}"
        );
    }

    #[test]
    fn hovering_a_square_does_not_lock_the_editor_up() {
        // Run on a thread: a lock taken twice never returns, and the test
        // should fail rather than hang with it.
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut app = EditorApp::default();
            app.settings.timeline_parenting_view = true;
            let q0rg_id = app.session.current_q0rg_id;
            let ctx = egui::Context::default();
            let column = Rect::from_min_size(pos2(100.0, 100.0), vec2(COLUMN_WIDTH, 20.0));
            let whole_row = Rect::from_min_size(pos2(0.0, 100.0), vec2(400.0, 20.0));
            let square = pos2(column.left() + LANES + 8.0, column.center().y);
            for _ in 0..3 {
                let input = egui::RawInput {
                    events: vec![egui::Event::PointerMoved(square)],
                    ..Default::default()
                };
                let _ = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let painter = ui.painter().clone();
                        row(&app, ui, &painter, q0rg_id, 1, column, whole_row, false);
                        finish(&mut app, ui, q0rg_id);
                    });
                });
            }
            let hovered = ctx.data(|data| data.get_temp::<(u16, u64)>(hover_id(q0rg_id)));
            let _ = done.send(hovered.map(|(layer, _)| layer));
        });
        let hovered = finished
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("hovering a parenting square locked the editor up");
        assert_eq!(hovered, Some(1), "the square was hovered");
    }
}
