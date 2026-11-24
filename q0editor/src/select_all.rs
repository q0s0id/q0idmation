//! Select All (Ctrl+A), for whatever is being worked on: the frames on the
//! timeline, its layers, the points of a path being reshaped, the frames
//! being edited together, or everything on the stage at this frame.

use crate::app::EditorApp;
use crate::state::{TimelineLayerSelection, TimelineSelection, Tool};

/// What Select All takes in, worked out from where the person is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every drawing and object on the stage at this frame, on the layers
    /// that are shown and unlocked.
    Stage,
    /// Every frame of the current layer; again, every frame of every layer.
    Frames,
    /// Every layer.
    Layers,
    /// Every point of the path being reshaped.
    Points,
    /// Every frame Edit Frames shows.
    FrameEdit,
}

const TIMELINE_AREA_ID: &str = "q0editor.select_all.timeline";
const STAGE_AREA_ID: &str = "q0editor.select_all.stage";

/// Notes where the stage is this frame: Select All over it takes the art.
pub(crate) fn note_stage(ctx: &egui::Context, panel: egui::Rect) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(STAGE_AREA_ID), panel));
}

/// Notes where the timeline is this frame, and where its layer names end,
/// so Select All can tell what the pointer is over.
pub(crate) fn note_timeline(ctx: &egui::Context, panel: egui::Rect, names_end: f32) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(TIMELINE_AREA_ID), (panel, names_end)));
}

/// What Select All means right now.
pub(crate) fn scope(app: &EditorApp, ctx: &egui::Context) -> Scope {
    if app.session.onion.edit_frames {
        return Scope::FrameEdit;
    }
    let timeline: Option<(egui::Rect, f32)> =
        ctx.data(|data| data.get_temp(egui::Id::new(TIMELINE_AREA_ID)));
    let pointer = ctx.input(|input| input.pointer.latest_pos());
    if let (Some((panel, names_end)), Some(pointer)) = (timeline, pointer) {
        if panel.contains(pointer) {
            return if pointer.x < names_end {
                Scope::Layers
            } else {
                Scope::Frames
            };
        }
    }
    if app.session.current_tool == Tool::Subselect
        && matches!(
            app.session.selection,
            crate::state::Selection::SubselectPoints { .. } | crate::state::Selection::Path { .. }
        )
    {
        return Scope::Points;
    }
    // Over the stage it is the art, even with a frame picked in the
    // timeline: picking a frame is how one gets to it.
    let stage: Option<egui::Rect> = ctx.data(|data| data.get_temp(egui::Id::new(STAGE_AREA_ID)));
    if let (Some(stage), Some(pointer)) = (stage, pointer) {
        if stage.contains(pointer) {
            return Scope::Stage;
        }
    }
    // Elsewhere, a selection on the timeline still says where the work is.
    if app.session.timeline_layer_selection.is_some() {
        return Scope::Layers;
    }
    if app.session.timeline_selection.is_some() || app.session.timeline_cell_selection.is_some() {
        return Scope::Frames;
    }
    Scope::Stage
}

/// Selects all of `scope`.
pub(crate) fn apply(app: &mut EditorApp, scope: Scope) {
    match scope {
        Scope::FrameEdit => {
            crate::frame_edit::select_all(app);
            app.session.status = "All frames being edited selected".to_string();
        }
        Scope::Points => {
            if crate::tools::select_every_point(app) {
                app.session.status = "Every point selected".to_string();
            }
        }
        Scope::Stage => {
            app.session.timeline_selection = None;
            app.session.timeline_layer_selection = None;
            app.session.timeline_cell_selection = None;
            app.session.status = if crate::tools::select_everything(app) {
                "Everything on this frame selected".to_string()
            } else {
                "Nothing to select on this frame".to_string()
            };
        }
        Scope::Frames => select_all_frames(app),
        Scope::Layers => {
            let Some((first, last)) = layer_span(app) else {
                return;
            };
            app.session.timeline_selection = None;
            app.session.timeline_cell_selection = None;
            app.session.timeline_layer_selection = Some(TimelineLayerSelection {
                anchor_layer_id: first,
                focus_layer_id: last,
            });
            app.session.status = "All layers selected".to_string();
        }
    }
}

/// The first and last layer of the symbol being edited.
fn layer_span(app: &EditorApp) -> Option<(u16, u16)> {
    let q0rg = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == app.session.current_q0rg_id)?;
    Some((q0rg.layers.first()?.layer_id, q0rg.layers.last()?.layer_id))
}

/// Every frame of the current layer; when those are already selected,
/// every frame of every layer.
fn select_all_frames(app: &mut EditorApp) {
    let Some((first, last)) = layer_span(app) else {
        return;
    };
    let frame_count = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == app.session.current_q0rg_id)
        .map_or(1, |q0rg| q0rg.frame_count.max(1));
    let end = frame_count - 1;
    let layer = app.session.current_layer_id;
    let this_layer = TimelineSelection {
        anchor_layer_id: layer,
        anchor_frame: 0,
        focus_layer_id: layer,
        focus_frame: end,
    };
    let (selection, status) = if app.session.timeline_selection == Some(this_layer) {
        (
            TimelineSelection {
                anchor_layer_id: first,
                anchor_frame: 0,
                focus_layer_id: last,
                focus_frame: end,
            },
            "Every frame of every layer selected",
        )
    } else {
        (
            this_layer,
            "Every frame of this layer selected (again for all layers)",
        )
    };
    app.session.selection = crate::state::Selection::None;
    app.session.timeline_layer_selection = None;
    app.session.timeline_cell_selection = None;
    app.session.timeline_selection = Some(selection);
    app.session.status = status.to_string();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_all_on_the_timeline_takes_the_layer_then_every_layer() {
        let mut app = EditorApp::default();
        app.session.current_q0rg_id = app.state.project.q0rgs[0].q0rg_id;
        app.session.current_layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
        apply(&mut app, Scope::Frames);
        let first = app.session.timeline_selection.expect("a frame selection");
        assert_eq!(first.anchor_layer_id, first.focus_layer_id);
        assert_eq!(first.anchor_frame, 0);
        apply(&mut app, Scope::Frames);
        let second = app.session.timeline_selection.expect("a frame selection");
        let (top, bottom) = layer_span(&app).expect("layers");
        assert_eq!(
            (second.anchor_layer_id, second.focus_layer_id),
            (top, bottom)
        );
    }

    #[test]
    fn select_all_on_the_stage_takes_what_is_drawn() {
        let mut app = EditorApp::default();
        app.session.current_q0rg_id = app.state.project.q0rgs[0].q0rg_id;
        app.session.current_layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
        app.state.project.q0rgs[0].layers[0].placements.clear();
        apply(&mut app, Scope::Stage);
        assert!(matches!(
            app.session.selection,
            crate::state::Selection::None
        ));
        crate::tools::draw_test_rectangle(&mut app);
        apply(&mut app, Scope::Stage);
        assert!(!matches!(
            app.session.selection,
            crate::state::Selection::None
        ));
    }

    #[test]
    fn select_all_takes_a_filled_shape_without_an_outline() {
        let mut app = EditorApp::default();
        app.session.current_q0rg_id = app.state.project.q0rgs[0].q0rg_id;
        app.session.current_layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
        app.state.project.q0rgs[0].layers[0].placements.clear();
        crate::tools::draw_test_rectangle(&mut app);
        let mut filled = false;
        for asset in &mut app.state.project.assets {
            if let q0s_format::v2::Asset::Vector(vector) = asset {
                vector.stroke = None;
                filled |= vector.fill.is_some();
            }
        }
        assert!(filled, "the test rectangle is painted");
        app.session.selection = crate::state::Selection::None;
        apply(&mut app, Scope::Stage);
        assert!(
            !matches!(app.session.selection, crate::state::Selection::None),
            "a fill with no outline is selected too"
        );
    }

    #[test]
    fn over_the_stage_select_all_takes_the_art_even_with_a_frame_picked() {
        let mut app = EditorApp::default();
        app.session.timeline_selection = Some(TimelineSelection::single(1, 0));
        let ctx = egui::Context::default();
        let scope_at = |at: egui::Pos2| {
            let mut seen = None;
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::PointerMoved(at)],
                    ..Default::default()
                },
                |ctx| {
                    note_stage(
                        ctx,
                        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)),
                    );
                    seen = Some(scope(&app, ctx));
                },
            );
            seen
        };
        assert_eq!(scope_at(egui::pos2(100.0, 100.0)), Some(Scope::Stage));
        assert_eq!(scope_at(egui::pos2(600.0, 600.0)), Some(Scope::Frames));
    }
}
