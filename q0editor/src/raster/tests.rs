use egui::Context;
use q0s_format::v2::{Asset, Target};

use super::*;
use crate::app::{Action, EditorApp};

fn raster_app() -> (EditorApp, Context) {
    let mut app = EditorApp::default();
    let ctx = Context::default();
    assert!(!offered(&app.state.project), "a new project has no raster");
    add_raster_layer(&mut app, &ctx);
    (app, ctx)
}

/// Paints a short hard line on the current frame, as a stroke on the stage
/// would: snapshot, canvas, stroke.
fn paint_line(app: &mut EditorApp, from: (f32, f32), to: (f32, f32)) -> CanvasRef {
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let canvas = prepare_canvas(app).expect("a raster layer can be painted");
    let drawing = raster_mut(&mut app.state.project, canvas.asset_id).unwrap();
    let settings = BrushSettings {
        size: 6.0,
        hardness: 1.0,
        pressure_size: 0.0,
        ..BrushSettings::default()
    };
    let points: Vec<_> = (0..=20)
        .map(|i| {
            let t = i as f32 / 20.0;
            (
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
                None,
            )
        })
        .collect();
    engine::paint_path(drawing, settings, [0, 0, 0], &points);
    app.state.mark_dirty();
    canvas
}

fn placements_on(app: &EditorApp, frame: u16) -> Vec<u16> {
    let q0rg = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == app.session.current_q0rg_id)
        .unwrap();
    let layer = q0rg
        .layers
        .iter()
        .find(|l| l.layer_id == app.session.current_layer_id)
        .unwrap();
    layer
        .placements
        .iter()
        .filter(|p| p.frame == frame)
        .filter_map(|p| match p.target {
            Target::Asset(id) => Some(id),
            Target::Q0rg(_) => None,
        })
        .collect()
}

#[test]
fn a_raster_layer_turns_the_raster_tools_on() {
    let (app, _) = raster_app();
    assert!(offered(&app.state.project));
    assert!(is_raster_layer(
        &app.state.project,
        app.session.current_q0rg_id,
        app.session.current_layer_id
    ));
    assert_eq!(app.session.raster.tool, Some(RasterTool::Brush));
    assert!(!app.session.tools3d.active());
}

#[test]
fn picking_a_2d_tool_leaves_the_raster_tools() {
    let (mut app, ctx) = raster_app();
    app.handle(&ctx, Action::SelectTool(crate::state::Tool::Select));
    assert!(!app.session.raster.active());
}

#[test]
fn vector_layers_refuse_raster_paint() {
    let mut app = EditorApp::default();
    assert_eq!(prepare_canvas(&mut app), Err(NoCanvas::NotRaster));
}

#[test]
fn painting_makes_a_drawing_on_the_keyframe_and_undo_takes_it_back() {
    let (mut app, ctx) = raster_app();
    let before = app.state.project.clone();
    let canvas = paint_line(&mut app, (10.0, 10.0), (60.0, 10.0));
    assert_eq!(placements_on(&app, 0), vec![canvas.asset_id]);
    let drawing = raster_ref(&app.state.project, canvas.asset_id).unwrap();
    assert_eq!(drawing.pixel(30, 10), [0, 0, 0, 255]);
    // Painting again on the same frame uses the same drawing.
    let again = paint_line(&mut app, (10.0, 30.0), (60.0, 30.0));
    assert_eq!(again.asset_id, canvas.asset_id);
    app.handle(&ctx, Action::Undo);
    app.handle(&ctx, Action::Undo);
    assert!(raster_ref(&app.state.project, canvas.asset_id).is_none_or(RasterAsset::is_empty));
    assert_eq!(placements_on(&app, 0), placements_on_project(&before, &app));
}

fn placements_on_project(project: &ProjectV2, app: &EditorApp) -> Vec<u16> {
    project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == app.session.current_q0rg_id)
        .and_then(|q| {
            q.layers
                .iter()
                .find(|l| l.layer_id == app.session.current_layer_id)
        })
        .map(|layer| {
            layer
                .placements
                .iter()
                .filter_map(|p| match p.target {
                    Target::Asset(id) => Some(id),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_held_frame_paints_its_keyframe_and_a_blank_keyframe_gets_its_own_drawing() {
    let (mut app, ctx) = raster_app();
    let first = paint_line(&mut app, (10.0, 10.0), (60.0, 10.0));
    // Frame 3 holds frame 0's drawing: painting there changes that drawing.
    app.session.current_frame = 3;
    app.handle(&ctx, Action::InsertFrame);
    let held = paint_line(&mut app, (10.0, 40.0), (60.0, 40.0));
    assert_eq!(held.asset_id, first.asset_id);
    // A blank keyframe on frame 5 gets a new drawing of its own.
    app.session.current_frame = 5;
    app.handle(&ctx, Action::InsertBlankKeyframe);
    let blank = paint_line(&mut app, (10.0, 60.0), (60.0, 60.0));
    assert_ne!(blank.asset_id, first.asset_id);
    assert_eq!(placements_on(&app, 5), vec![blank.asset_id]);
}

#[test]
fn a_copied_keyframe_shares_tiles_until_painted() {
    let (mut app, ctx) = raster_app();
    let first = paint_line(&mut app, (10.0, 10.0), (60.0, 10.0));
    app.session.current_frame = 4;
    app.handle(&ctx, Action::InsertKeyframe);
    let shared: Vec<u16> = placements_on(&app, 4);
    assert_eq!(shared.len(), 1);
    let copy = paint_line(&mut app, (10.0, 300.0), (60.0, 300.0));
    // Frame 0 keeps its drawing; frame 4 now has its own.
    assert_eq!(placements_on(&app, 0), vec![first.asset_id]);
    assert_eq!(placements_on(&app, 4), vec![copy.asset_id]);
    let a = raster_ref(&app.state.project, first.asset_id).unwrap();
    let b = raster_ref(&app.state.project, copy.asset_id).unwrap();
    assert!(a.pixel(30, 300)[3] == 0 && b.pixel(30, 300)[3] == 255);
    // The line both started with is the same tile, not a copy.
    assert!(std::sync::Arc::ptr_eq(&a.tiles[&(0, 0)], &b.tiles[&(0, 0)]));
}

#[test]
fn raster_frames_survive_a_save_and_load() {
    let (mut app, _) = raster_app();
    paint_line(&mut app, (10.0, 10.0), (60.0, 10.0));
    let bytes = q0s_format::v2::write_packed(&app.state.project).unwrap();
    let back = q0s_format::v2::parse(&bytes).unwrap();
    assert!(offered(&back));
    let drawings = back
        .assets
        .iter()
        .filter(|asset| matches!(asset, Asset::Raster(r) if !r.is_empty()))
        .count();
    assert_eq!(drawings, 1);
}

#[test]
fn raster_assets_go_through_json_for_rooms_and_plugins() {
    let (mut app, _) = raster_app();
    let canvas = paint_line(&mut app, (10.0, 10.0), (60.0, 10.0));
    let asset = Asset::Raster(
        raster_ref(&app.state.project, canvas.asset_id)
            .unwrap()
            .clone(),
    );
    let text = serde_json::to_string(&asset).unwrap();
    let back: Asset = serde_json::from_str(&text).unwrap();
    assert_eq!(back, asset);
}

#[test]
fn a_locked_raster_layer_is_left_alone() {
    let (mut app, _) = raster_app();
    let key = LayerKey::new(app.session.current_q0rg_id, app.session.current_layer_id);
    app.state
        .project
        .layer_metadata
        .get_mut(&key)
        .unwrap()
        .locked = true;
    assert_eq!(prepare_canvas(&mut app), Err(NoCanvas::Locked));
}

#[test]
fn prefs_keep_changes_per_brush() {
    let mut prefs = RasterPrefs::default();
    let (pencil, mut settings) = prefs.preset(Family::Paint);
    assert_eq!(pencil.key, "pencil");
    settings.size = 40.0;
    prefs.set_settings(Family::Paint, settings);
    assert_eq!(prefs.preset(Family::Paint).1.size, 40.0);
    prefs.choose(Family::Paint, "ink");
    assert_ne!(prefs.preset(Family::Paint).1.size, 40.0);
    prefs.choose(Family::Paint, "pencil");
    assert_eq!(prefs.preset(Family::Paint).1.size, 40.0);
    // Back to the preset's own value: nothing kept.
    prefs.set_settings(Family::Paint, pencil.settings);
    assert!(prefs.changed.is_empty());
}

#[test]
fn drawing_tools_mean_their_raster_kin_on_a_raster_layer() {
    let (mut app, ctx) = raster_app();
    app.handle(&ctx, Action::SelectTool(crate::state::Tool::Eraser));
    assert_eq!(app.session.raster.tool, Some(RasterTool::Eraser));
    app.handle(&ctx, Action::SelectTool(crate::state::Tool::Bucket));
    assert_eq!(app.session.raster.tool, Some(RasterTool::Fill));
    // The Select tool stays itself, for moving drawings and symbols whole.
    app.handle(&ctx, Action::SelectTool(crate::state::Tool::Select));
    assert!(!app.session.raster.active());
    assert_eq!(app.session.current_tool, crate::state::Tool::Select);
}

#[test]
fn copied_pixels_paste_onto_another_frame_where_they_were() {
    let (mut app, ctx) = raster_app();
    let first = paint_line(&mut app, (10.0, 10.0), (60.0, 10.0));
    pick_tool(&mut app, RasterTool::Marquee);
    app.handle(&ctx, Action::SelectAll(crate::select_all::Scope::Stage));
    app.handle(&ctx, Action::CopySelection);
    assert!(app.session.raster.clip_newest);
    // A blank keyframe later on gets the same line, in the same place.
    app.session.current_frame = 6;
    app.handle(&ctx, Action::InsertBlankKeyframe);
    app.handle(&ctx, Action::Paste);
    select::let_go(&mut app);
    let pasted = placements_on(&app, 6);
    assert_eq!(pasted.len(), 1);
    assert_ne!(pasted[0], first.asset_id);
    let drawing = raster_ref(&app.state.project, pasted[0]).unwrap();
    assert_eq!(drawing.pixel(30, 10), [0, 0, 0, 255]);
}

#[test]
fn the_picker_reads_the_drawings_shown_without_rendering_the_frame() {
    let (mut app, ctx) = raster_app();
    paint_line(&mut app, (10.0, 10.0), (60.0, 10.0));
    let stage = q0s_format::v2::Vec2::new(30.5, 10.5);
    assert_eq!(sample_raster(&app, stage), Some([0, 0, 0, 255]));
    // An empty raster layer on top changes nothing; the paint below shows.
    add_raster_layer(&mut app, &ctx);
    assert_eq!(shown_drawings(&app).len(), 1);
    assert_eq!(sample_raster(&app, stage), Some([0, 0, 0, 255]));
    assert_eq!(
        sample_raster(&app, q0s_format::v2::Vec2::new(30.5, 90.5)),
        None
    );
}
