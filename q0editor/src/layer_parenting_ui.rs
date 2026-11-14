//! The timeline's parenting view, after Animate's: a column with a square in
//! each layer's outline colour. A child's square sits a step to the right of
//! its parent's and a curve in the child's colour joins the two. Drag a
//! square onto another layer to hang it there, or click it for a menu.
//! The main timeline's camera row has a square too: layers hung from the
//! camera stay put in its view, and a camera hung from a layer follows it.

use egui::epaint::CubicBezierShape;
use egui::{pos2, vec2, Color32, Id, Painter, Rect, Sense, Stroke, Ui};

use crate::app::{Action, EditorApp};
use q0s_format::parenting::CAMERA_LAYER;

/// Width the view takes from the layer names while it is shown.
pub(crate) const COLUMN_WIDTH: f32 = 46.0;
/// How far right each generation's square sits.
const STEP: f32 = 10.0;

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
    color: Color32,
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
    let generation = q0s_format::parenting::depth(project, q0rg_id, layer_id).min(3) as f32;
    let color = square_color(app, layer_id);
    let square = Rect::from_center_size(
        pos2(column.left() + 8.0 + generation * STEP, column.center().y),
        vec2(8.0, 10.0),
    );
    let response = ui
        .interact(
            square.expand(3.0),
            Id::new(("layer_parent_square", q0rg_id, layer_id)),
            Sense::click_and_drag(),
        )
        .on_hover_text(if layer_id == CAMERA_LAYER {
            "Drag onto a layer to make the camera follow it. Click for a menu."
        } else {
            "Drag onto another layer to hang this layer from it. Click for a menu."
        })
        .on_hover_cursor(egui::CursorIcon::Grab);
    painter.rect_filled(square, 1.5, color);
    if response.hovered() || response.dragged() {
        painter.rect_stroke(
            square.expand(1.5),
            2.0,
            Stroke::new(1.0_f32, ui.visuals().strong_text_color()),
        );
    }
    ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<Row>>(rows_id(q0rg_id))
            .push(Row {
                layer_id,
                square,
                row,
                color,
            });
    });
    if response.drag_started() {
        ui.ctx()
            .data_mut(|data| data.insert_temp(drag_id(q0rg_id), layer_id));
    }

    let popup = Id::new(("layer_parent_menu", q0rg_id, layer_id));
    if response.clicked() {
        ui.memory_mut(|memory| memory.toggle_popup(popup));
    }
    let parent = q0s_format::parenting::parent_of(project, q0rg_id, layer_id);
    let mut choice: Option<Option<u16>> = None;
    egui::popup::popup_below_widget(ui, popup, &response, |ui| {
        ui.set_min_width(170.0);
        ui.label(egui::RichText::new("Parent layer").small().weak());
        if ui
            .add_enabled(
                parent.is_some(),
                egui::SelectableLabel::new(parent.is_none(), "None"),
            )
            .clicked()
        {
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
        {
            ui.horizontal(|ui| {
                let (swatch, _) = ui.allocate_exact_size(vec2(8.0, 10.0), Sense::hover());
                ui.painter()
                    .rect_filled(swatch, 1.5, square_color(app, CAMERA_LAYER));
                if ui
                    .selectable_label(parent == Some(CAMERA_LAYER), "Camera")
                    .clicked()
                {
                    choice = Some(Some(CAMERA_LAYER));
                }
            });
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
            ui.horizontal(|ui| {
                let (swatch, _) = ui.allocate_exact_size(vec2(8.0, 10.0), Sense::hover());
                ui.painter().rect_filled(
                    swatch,
                    1.5,
                    crate::render::outline_color_for_layer(candidate),
                );
                if ui
                    .selectable_label(parent == Some(candidate), &layer.name)
                    .clicked()
                {
                    choice = Some(Some(candidate));
                }
            });
        }
    });
    if let Some(choice) = choice {
        ui.memory_mut(|memory| memory.close_popup());
        if choice != parent {
            ui.ctx()
                .data_mut(|data| data.insert_temp(pending_id(q0rg_id), (layer_id, choice)));
        }
    }
}

/// Whether the timeline shows a camera row (the main one, camera on).
fn camera_row_shown(app: &EditorApp, q0rg_id: u16) -> bool {
    q0rg_id == app.state.project.meta.entry_q0rg_id && app.state.project.runtime.camera.enabled
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

/// After every row: the curves between children and parents, the drag line,
/// and the drop.
pub(crate) fn finish(app: &mut EditorApp, ui: &Ui, q0rg_id: u16) {
    let rows: Vec<Row> = ui
        .ctx()
        .data_mut(|data| data.remove_temp(rows_id(q0rg_id)))
        .unwrap_or_default();
    let dragging: Option<u16> = ui.ctx().data(|data| data.get_temp(drag_id(q0rg_id)));
    let pending: Option<(u16, Option<u16>)> = ui
        .ctx()
        .data_mut(|data| data.remove_temp(pending_id(q0rg_id)));
    if let Some((layer_id, parent)) = pending {
        app.queue(Action::SetLayerParent(q0rg_id, layer_id, parent));
    }
    if !shown(app) || rows.is_empty() {
        ui.ctx()
            .data_mut(|data| data.remove::<u16>(drag_id(q0rg_id)));
        return;
    }
    let painter = ui.painter();
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
    for child in &rows {
        let Some(parent_id) = q0s_format::parenting::parent_of(project, q0rg_id, child.layer_id)
        else {
            continue;
        };
        let start = child.square.left_center() - vec2(1.0, 0.0);
        let stroke = Stroke::new(1.6_f32, child.color);
        match rows.iter().find(|row| row.layer_id == parent_id) {
            Some(parent) => {
                let above = parent.square.center().y < child.square.center().y;
                let end = if above {
                    parent.square.center_bottom()
                } else {
                    parent.square.center_top()
                };
                let bend = if above { STEP } else { -STEP };
                painter.add(CubicBezierShape::from_points_stroke(
                    [
                        start,
                        pos2(end.x, start.y),
                        pos2(end.x, end.y + bend * 0.5),
                        end,
                    ],
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ));
            }
            None => {
                // The parent is scrolled out of view: a short lead its way.
                let up = order(parent_id) < order(child.layer_id);
                let end = start - vec2(STEP * 0.6, 0.0) + vec2(0.0, if up { -8.0 } else { 8.0 });
                painter.line_segment([start, pos2(end.x, start.y)], stroke);
                painter.line_segment([pos2(end.x, start.y), end], stroke);
            }
        }
    }

    let Some(source) = dragging else {
        return;
    };
    let pointer = ui.ctx().pointer_latest_pos();
    let source_row = rows.iter().find(|row| row.layer_id == source);
    let target = pointer.and_then(|pointer| {
        rows.iter()
            .find(|row| row.row.contains(pointer) && row.layer_id != source)
    });
    let allowed = target.is_some_and(|target| {
        !q0s_format::parenting::would_loop(&app.state.project, q0rg_id, source, target.layer_id)
    });
    if let (Some(from), Some(pointer)) = (source_row, pointer) {
        let color = if target.is_some() && !allowed {
            ui.visuals().error_fg_color
        } else {
            from.color
        };
        painter.line_segment([from.square.center(), pointer], Stroke::new(1.6_f32, color));
        painter.circle_filled(pointer, 3.0, color);
    }
    if let Some(target) = target {
        painter.rect_stroke(
            target.square.expand(3.0),
            2.0,
            Stroke::new(
                1.5_f32,
                if allowed {
                    target.color
                } else {
                    ui.visuals().error_fg_color
                },
            ),
        );
    }
    if ui.input(|input| input.pointer.any_released()) {
        ui.ctx()
            .data_mut(|data| data.remove::<u16>(drag_id(q0rg_id)));
        if let Some(target) = target.filter(|_| allowed) {
            app.queue(Action::SetLayerParent(
                q0rg_id,
                source,
                Some(target.layer_id),
            ));
        } else if pointer.zip(source_row).is_some_and(|(pointer, row)| {
            !row.row.contains(pointer) && !rows.iter().any(|row| row.row.contains(pointer))
        }) {
            // Dropped clear of every layer: no parent.
            if q0s_format::parenting::parent_of(&app.state.project, q0rg_id, source).is_some() {
                app.queue(Action::SetLayerParent(q0rg_id, source, None));
            }
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
}
