use egui::epaint::{PathShape, Vertex};
use egui::{
    pos2, Color32, ColorImage, Context, Key, Mesh, Painter, PointerButton, Pos2, Response, Shape,
    Stroke, TextureHandle, TextureOptions,
};
use geo::{
    Area, BooleanOps, BoundingRect, Contains, Coord, LineString, MultiPolygon, Point, Polygon,
};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Anchor, Asset, Path as VPath, Placement, ProjectV2, Rgba, Stroke as VStroke, Target,
    Transform2D, Tween, Vec2, VectorAsset,
};

use crate::app::EditorApp;
use crate::render::{
    flatten_path, flatten_path_for_stroke, paint_complex_fill, paint_round_stroke_preview,
    placement_bbox, placement_local_bbox, StageView,
};
use crate::state::{
    GroupTransformOperation, Handle, PathRef, PlacementRef, Selection, Tool, ToolState,
    TransformEdge, TransformPivot,
};

const HIT_RADIUS: f32 = 6.0;
// Sample spacing for raw brush points. We commit fewer than this many anchors
// (Catmull-Rom smoothing turns each segment into a cubic bezier), so going too
// dense just inflates the asset without visual gain. 2.5 is the sweet spot.
const PENCIL_MIN_DIST: f32 = 2.5;

pub fn handle(
    app: &mut EditorApp,
    response: &Response,
    painter: &Painter,
    view: &StageView,
    ctx: &Context,
) {
    // Temporary H/Space hand and MMB pan own the gesture completely. Do not
    // let the underlying brush/select/etc. observe the same primary events.
    if app.session.viewport.hand_active || app.session.viewport.panning {
        if response.hovered() {
            ctx.set_cursor_icon(if app.session.viewport.panning {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        return;
    }

    let current_layer_is_folder = app
        .state
        .project
        .layer_is_folder(app.session.current_q0rg_id, app.session.current_layer_id);
    if current_layer_is_folder
        && !matches!(
            app.session.current_tool,
            Tool::Select | Tool::Subselect | Tool::Hand
        )
    {
        if response.clicked() || response.drag_started() {
            app.session.status = "folders cannot contain artwork or keyframes".to_string();
        }
        return;
    }

    let cursor_stage = response
        .hover_pos()
        .map(|p| screen_to_stage(p, view))
        .filter(|p| {
            // Allow cursor while in-stage; tools clamp themselves to stage rect.
            p.x.is_finite() && p.y.is_finite()
        });

    match app.session.current_tool {
        Tool::Hand => {}
        Tool::Pen => pen(app, response, cursor_stage, painter, view, ctx),
        Tool::Pencil => pencil_freehand(app, response, cursor_stage),
        Tool::Brush => brush_tool(app, response, view, ctx),
        Tool::Eraser => eraser(app, response, view, ctx),
        Tool::Select => select(app, response, cursor_stage, view),
        Tool::Subselect => subselect(app, response, cursor_stage),
        Tool::Line => line(app, response, cursor_stage, painter, view),
        Tool::Rectangle => primitive_rect(app, response, cursor_stage, painter, view, false),
        Tool::Oval => primitive_rect(app, response, cursor_stage, painter, view, true),
        Tool::Bucket => bucket(app, response, cursor_stage),
        Tool::Eyedropper => eyedropper(app, response, cursor_stage, view),
    }

    // Cursor hint: only when the pointer is actually over the canvas.
    // (so menubar / panels keep their default cursor).
    if response.hovered() {
        if let Some(icon) = pick_cursor(app, response, view) {
            ctx.set_cursor_icon(icon);
        }
    }

    draw_in_progress_overlay(app, painter, view);
    draw_brush_cursor(app, painter, view);
    draw_eraser_cursor(app, painter, view);
    draw_transform_cursor(app, response, painter, view);
}

fn subselect(app: &mut EditorApp, response: &Response, cursor: Option<Vec2>) {
    if !response.clicked_by(PointerButton::Primary) {
        return;
    }
    let Some(cursor) = cursor else {
        return;
    };
    if let Some(hit) = hit_test_raw_path(
        &app.state.project,
        app.session.current_q0rg_id,
        app.session.current_frame,
        cursor,
    ) {
        app.session.selection = Selection::Path {
            q0rg_id: app.session.current_q0rg_id,
            layer_id: hit.layer_id,
            placement_idx: hit.placement_idx,
            path_idx: hit.path_idx,
        };
        app.session.status = "Subselect: contour points".to_string();
    } else {
        app.session.selection = Selection::None;
    }
}

/// Decide what cursor the OS should show for the current tool + cursor
/// position. The Select-tool branch additionally hit-tests transform
/// handles vs. placement bodies so the user sees exactly what the next
/// click will do: corners resize both axes, edge handles resize one axis,
/// and the body moves the selection.
fn pick_cursor(app: &EditorApp, response: &Response, view: &StageView) -> Option<egui::CursorIcon> {
    use egui::CursorIcon;
    // Mid-drag: the cursor freezes on whatever icon was shown when the
    // drag started, otherwise it flickers between Move/Resize as the
    // pointer crosses handle boundaries. Also force `Grabbing` while
    // the user is actively dragging a placement body.
    if let ToolState::DraggingRawHandle { handle, .. } | ToolState::DraggingHandle { handle, .. } =
        app.session.tool_state
    {
        return Some(resize_cursor_for_handle(handle));
    }
    if let ToolState::DraggingGroup {
        operation: GroupTransformOperation::Scale { handle, .. },
        ..
    } = app.session.tool_state
    {
        return Some(resize_cursor_for_handle(handle));
    }
    if matches!(
        app.session.tool_state,
        ToolState::DraggingRawRotate { .. }
            | ToolState::DraggingPlacementRotate { .. }
            | ToolState::DraggingRawSkew { .. }
            | ToolState::DraggingPlacementSkew { .. }
            | ToolState::DraggingGroup {
                operation: GroupTransformOperation::Rotate { .. }
                    | GroupTransformOperation::Skew { .. },
                ..
            }
    ) {
        return Some(CursorIcon::None);
    }
    if matches!(
        app.session.tool_state,
        ToolState::DraggingPlacement { .. }
            | ToolState::DraggingPath { .. }
            | ToolState::DraggingPaths { .. }
            | ToolState::DraggingPathPoints { .. }
            | ToolState::DraggingGroup {
                operation: GroupTransformOperation::Move { .. },
                ..
            }
    ) {
        return Some(CursorIcon::Grabbing);
    }
    // Keep the closed-hand cursor for the whole body drag even on the threshold
    // frame where egui has started moving but the semantic drag state has not
    // yet been installed by Select.
    if app.session.current_tool == Tool::Select
        && !matches!(app.session.selection, Selection::None)
        && response.dragged_by(PointerButton::Primary)
    {
        return Some(CursorIcon::Grabbing);
    }

    Some(match app.session.current_tool {
        Tool::Hand => CursorIcon::Grab,
        Tool::Pen | Tool::Line | Tool::Rectangle | Tool::Oval => CursorIcon::Crosshair,
        Tool::Pencil => CursorIcon::Crosshair,
        // The brush paints its own exact nib-size ring.
        Tool::Brush => CursorIcon::None,
        // We draw our own ring overlay for the eraser; the OS pointer
        // would just clutter the area, so hide it.
        Tool::Eraser => CursorIcon::None,
        Tool::Bucket | Tool::Eyedropper => CursorIcon::Crosshair,
        Tool::Subselect => CursorIcon::Default,
        Tool::Select => select_cursor(app, response, view).unwrap_or(CursorIcon::Default),
    })
}

fn resize_cursor_for_handle(handle: Handle) -> egui::CursorIcon {
    use egui::CursorIcon;
    match handle {
        Handle::TopLeft | Handle::BottomRight => CursorIcon::ResizeNwSe,
        Handle::TopRight | Handle::BottomLeft => CursorIcon::ResizeNeSw,
        Handle::MidTop | Handle::MidBottom => CursorIcon::ResizeVertical,
        Handle::MidLeft | Handle::MidRight => CursorIcon::ResizeHorizontal,
    }
}

fn selected_transform_hit(
    app: &EditorApp,
    view: &StageView,
    cursor_screen: Pos2,
) -> Option<TransformHit> {
    if matches!(
        app.session.selection,
        Selection::RawArea { .. } | Selection::Mixed { .. } | Selection::Multi(_)
    ) {
        if let Some(bounds) = selection_transform_bounds(app) {
            if let Some(hit) = hit_test_raw_transform(bounds, view, cursor_screen) {
                return Some(hit);
            }
        }
    }
    if let Some(refs) = selection_raw_path_refs(&app.session.selection) {
        if let Some(bounds) = raw_path_refs_ui_bounds(&app.state.project, &refs) {
            if let Some(hit) = hit_test_raw_transform(bounds, view, cursor_screen) {
                return Some(hit);
            }
        }
    }
    if let Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = app.session.selection.clone()
    {
        if let Some((_, local_bbox, transform)) = drag_start_data(
            &app.state.project,
            q0rg_id,
            layer_id,
            placement_idx,
            app.session.current_frame,
        ) {
            if let Some(hit) =
                hit_test_placement_transform(local_bbox, transform, view, cursor_screen)
            {
                return Some(hit);
            }
        }
    }
    None
}

fn select_cursor(
    app: &EditorApp,
    response: &Response,
    view: &StageView,
) -> Option<egui::CursorIcon> {
    use egui::CursorIcon;
    let cursor_screen = response.hover_pos()?;

    if selected_pivot_hit(app, view, cursor_screen) {
        return Some(CursorIcon::Move);
    }
    // Transform handles take priority over body hit so the user can grab
    // a handle that overlaps a body pixel without surprise.
    if let Some(hit) = selected_transform_hit(app, view, cursor_screen) {
        return Some(cursor_for_transform_hit(hit));
    }

    // Raw graphics are selected by their actual fill/stroke geometry, never
    // by the shared drawing Placement's bounding box. Display objects (q0rg,
    // bitmap, transformed vector instances) keep normal object hit-testing.
    let cursor_stage = screen_to_stage(cursor_screen, view);
    if selected_raw_body_contains_point(app, cursor_stage)
        || hit_test_raw_selection(
            &app.state.project,
            app.session.current_q0rg_id,
            app.session.current_frame,
            cursor_stage,
        )
        .is_some()
        || hit_test_selectable_placement(
            &app.state.project,
            app.session.current_q0rg_id,
            app.session.current_frame,
            cursor_stage,
        )
        .is_some()
    {
        Some(CursorIcon::Grab)
    } else {
        Some(CursorIcon::Default)
    }
}

fn screen_to_stage(pos: Pos2, view: &StageView) -> Vec2 {
    Vec2::new(
        (pos.x - view.origin.x) / view.scale,
        (pos.y - view.origin.y) / view.scale,
    )
}

fn stage_to_screen(p: Vec2, view: &StageView) -> Pos2 {
    pos2(
        view.origin.x + p.x * view.scale,
        view.origin.y + p.y * view.scale,
    )
}

fn selection_color(app: &EditorApp) -> Color32 {
    app.settings.theme.accent.to_color32()
}

fn selection_fill_color(app: &EditorApp, alpha: u8) -> Color32 {
    let color = selection_color(app);
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn union_bounds(
    left: Option<(f32, f32, f32, f32)>,
    right: Option<(f32, f32, f32, f32)>,
) -> Option<(f32, f32, f32, f32)> {
    match (left, right) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))),
        (Some(bounds), None) | (None, Some(bounds)) => Some(bounds),
        (None, None) => None,
    }
}

fn placement_ref_world_bounds(
    project: &ProjectV2,
    reference: PlacementRef,
    frame: u16,
) -> Option<(f32, f32, f32, f32)> {
    let layer = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == reference.layer_id)?;
    let placement = layer.placements.get(reference.placement_idx)?;
    let transform =
        crate::render::active_transform_for_placement(layer, reference.placement_idx, frame)?;
    let mut visual = placement.clone();
    visual.transform = transform;
    placement_bbox(project, &visual)
}

fn placement_refs_world_bounds(
    project: &ProjectV2,
    references: &[PlacementRef],
    frame: u16,
) -> Option<(f32, f32, f32, f32)> {
    references.iter().copied().fold(None, |bounds, reference| {
        union_bounds(
            bounds,
            placement_ref_world_bounds(project, reference, frame),
        )
    })
}

fn selection_transform_bounds(app: &EditorApp) -> Option<(f32, f32, f32, f32)> {
    match &app.session.selection {
        Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } => placement_ref_world_bounds(
            &app.state.project,
            PlacementRef {
                q0rg_id: *q0rg_id,
                layer_id: *layer_id,
                placement_idx: *placement_idx,
            },
            app.session.current_frame,
        ),
        Selection::Path {
            q0rg_id,
            layer_id,
            placement_idx,
            path_idx,
        } => raw_path_refs_ui_bounds(
            &app.state.project,
            &[PathRef {
                q0rg_id: *q0rg_id,
                layer_id: *layer_id,
                placement_idx: *placement_idx,
                path_idx: *path_idx,
            }],
        ),
        Selection::Paths(paths) => raw_path_refs_ui_bounds(&app.state.project, paths),
        Selection::RawArea {
            objects,
            bounds_min,
            bounds_max,
            ..
        } => union_bounds(
            Some((bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y)),
            placement_refs_world_bounds(&app.state.project, objects, app.session.current_frame),
        ),
        Selection::Mixed { paths, objects } => union_bounds(
            raw_path_refs_ui_bounds(&app.state.project, paths),
            placement_refs_world_bounds(&app.state.project, objects, app.session.current_frame),
        ),
        Selection::Multi(objects) => {
            placement_refs_world_bounds(&app.state.project, objects, app.session.current_frame)
        }
        Selection::None
        | Selection::Asset(_)
        | Selection::Q0rg(_)
        | Selection::PathPoints { .. } => None,
    }
}

fn selection_default_pivot(app: &EditorApp) -> Option<Vec2> {
    if let Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = app.session.selection
    {
        let (_, local_bbox, transform) = drag_start_data(
            &app.state.project,
            q0rg_id,
            layer_id,
            placement_idx,
            app.session.current_frame,
        )?;
        let local_center = Vec2::new(
            (local_bbox.0 + local_bbox.2) * 0.5,
            (local_bbox.1 + local_bbox.3) * 0.5,
        );
        return Some(Affine::from_transform(transform).apply(local_center));
    }
    selection_transform_bounds(app)
        .map(|bounds| Vec2::new((bounds.0 + bounds.2) * 0.5, (bounds.1 + bounds.3) * 0.5))
}

fn selection_transform_pivot(app: &EditorApp) -> Option<Vec2> {
    app.session
        .transform_pivot
        .as_ref()
        .filter(|pivot| pivot.selection == app.session.selection)
        .map(|pivot| pivot.point)
        .or_else(|| selection_default_pivot(app))
}

fn set_selection_transform_pivot(app: &mut EditorApp, point: Vec2) {
    app.session.transform_pivot = Some(TransformPivot {
        selection: app.session.selection.clone(),
        point,
    });
}

fn selected_pivot_hit(app: &EditorApp, view: &StageView, cursor_screen: Pos2) -> bool {
    selection_transform_pivot(app).is_some_and(|pivot| {
        screen_distance_sq(stage_to_screen(pivot, view), cursor_screen)
            <= PIVOT_HIT_RADIUS_PX * PIVOT_HIT_RADIUS_PX
    })
}

// ---------------- Pen tool ----------------

fn pen(
    app: &mut EditorApp,
    response: &Response,
    cursor: Option<Vec2>,
    _painter: &Painter,
    _view: &StageView,
    ctx: &Context,
) {
    let in_progress = matches!(app.session.tool_state, ToolState::PenDrawing { .. });
    if !in_progress && !matches!(app.session.tool_state, ToolState::Idle) {
        // Other tool was active; reset.
        app.session.tool_state = ToolState::Idle;
    }

    let commit_close = response.double_clicked_by(PointerButton::Primary);
    let escape = ctx.input(|i| i.key_pressed(Key::Escape));
    let enter = ctx.input(|i| i.key_pressed(Key::Enter));

    if response.clicked_by(PointerButton::Primary)
        && !response.double_clicked_by(PointerButton::Primary)
    {
        if let Some(p) = cursor {
            let mut anchors = match std::mem::replace(&mut app.session.tool_state, ToolState::Idle)
            {
                ToolState::PenDrawing { anchors } => anchors,
                _ => Vec::new(),
            };
            // Clicking very close to the first anchor closes the path.
            if anchors.len() >= 2 {
                let first = anchors[0].point;
                let dx = (first.x - p.x) * 1.0;
                let dy = (first.y - p.y) * 1.0;
                if (dx * dx + dy * dy).sqrt() < HIT_RADIUS {
                    commit_path(app, anchors, true, DrawingMode::Merge);
                    return;
                }
            }
            anchors.push(Anchor {
                point: p,
                in_handle: None,
                out_handle: None,
            });
            app.session.tool_state = ToolState::PenDrawing { anchors };
            app.session.status = "Pen: anchor added (Enter closes, Esc cancels)".to_string();
        }
    }

    if commit_close {
        if let ToolState::PenDrawing { anchors } =
            std::mem::replace(&mut app.session.tool_state, ToolState::Idle)
        {
            if anchors.len() >= 2 {
                commit_path(app, anchors, true, DrawingMode::Merge);
            }
        }
    } else if enter {
        if let ToolState::PenDrawing { anchors } =
            std::mem::replace(&mut app.session.tool_state, ToolState::Idle)
        {
            if anchors.len() >= 2 {
                commit_path(app, anchors, false, DrawingMode::Merge);
            }
        }
    } else if escape {
        app.session.tool_state = ToolState::Idle;
        app.session.status = "Pen: cancelled".to_string();
    }
}

// ---------------- Freehand Pencil + classic fill Brush ----------------

fn pencil_freehand(app: &mut EditorApp, response: &Response, cursor: Option<Vec2>) {
    if response.drag_started_by(PointerButton::Primary) {
        if let Some(point) = cursor {
            app.session.tool_state = ToolState::FreehandDrawing {
                points: vec![point],
            };
        }
    }

    if response.dragged_by(PointerButton::Primary) {
        if let ToolState::FreehandDrawing { points } = &mut app.session.tool_state {
            if let Some(point) = cursor {
                let add = points
                    .last()
                    .map(|last| {
                        let dx = last.x - point.x;
                        let dy = last.y - point.y;
                        (dx * dx + dy * dy).sqrt() >= PENCIL_MIN_DIST
                    })
                    .unwrap_or(true);
                if add {
                    points.push(point);
                }
            }
        }
    }

    if response.drag_stopped_by(PointerButton::Primary) {
        if let ToolState::FreehandDrawing { points } =
            std::mem::replace(&mut app.session.tool_state, ToolState::Idle)
        {
            if points.len() >= 2 {
                commit_path(
                    app,
                    catmull_rom_anchors(&points),
                    false,
                    DrawingMode::Object,
                );
            }
        }
    }
}

fn brush_settings_for_view(app: &EditorApp, view_scale: f32) -> crate::brush::BrushSettings {
    let mut settings = app.session.brush;
    if !settings.scale_with_stage {
        settings.size /= view_scale.max(1.0e-4);
    }
    settings
}

fn advanced_brush_settings_for_view(
    app: &EditorApp,
    view_scale: f32,
) -> crate::advanced_brush::AdvancedBrushSettings {
    let mut settings = app.session.advanced_brush.sanitized();
    if !settings.scale_with_stage {
        settings.size /= view_scale.max(1.0e-4);
        settings.glow_radius /= view_scale.max(1.0e-4);
    }
    settings
}

fn brush_tool(app: &mut EditorApp, response: &Response, view: &StageView, ctx: &Context) {
    match app.session.brush_mode {
        crate::advanced_brush::BrushMode::Classic => classic_brush(app, response, view, ctx),
        crate::advanced_brush::BrushMode::Advanced => advanced_brush(app, response, view, ctx),
    }
}

fn advanced_material(
    settings: crate::advanced_brush::AdvancedBrushSettings,
) -> Option<q0s_format::v2::VectorMaterial> {
    #[cfg(feature = "appearance-mask-eraser")]
    {
        settings.material()
    }
    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        let _ = settings;
        None
    }
}

fn advanced_input_samples(
    ctx: &Context,
    response: &Response,
    view: &StageView,
) -> Vec<crate::advanced_brush::AdvancedBrushSample> {
    let (now, dt, events) =
        ctx.input(|input| (input.time, input.unstable_dt, input.events.clone()));
    let mut touch = Vec::new();
    for event in &events {
        if let egui::Event::Touch {
            phase, pos, force, ..
        } = event
        {
            if matches!(phase, egui::TouchPhase::Start | egui::TouchPhase::Move)
                && response.rect.contains(*pos)
            {
                touch.push((*pos, *force));
            }
        }
    }
    let mut raw = if touch.is_empty() {
        events
            .iter()
            .filter_map(|event| match event {
                egui::Event::PointerMoved(pos) if response.rect.contains(*pos) => {
                    Some((*pos, None))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    } else {
        touch
    };
    if raw.is_empty() {
        if let Some(pos) = response
            .interact_pointer_pos()
            .filter(|pos| response.rect.contains(*pos))
        {
            raw.push((pos, None));
        }
    }
    let count = raw.len().max(1) as f64;
    let span = f64::from(dt.max(1.0 / 1000.0));
    raw.into_iter()
        .enumerate()
        .map(
            |(index, (screen, pressure))| crate::advanced_brush::AdvancedBrushSample {
                position: screen_to_stage(screen, view),
                pressure,
                time_seconds: now - span + span * (index as f64 + 1.0) / count,
            },
        )
        .collect()
}

fn advanced_brush(app: &mut EditorApp, response: &Response, view: &StageView, ctx: &Context) {
    let settings = advanced_brush_settings_for_view(app, view.scale);
    if response.drag_started_by(PointerButton::Primary) {
        if let Some(screen) = response.interact_pointer_pos() {
            let time = ctx.input(|input| input.time);
            let pressure = ctx.input(|input| {
                input.events.iter().rev().find_map(|event| match event {
                    egui::Event::Touch { pos, force, .. } if (*pos - screen).length_sq() <= 9.0 => {
                        *force
                    }
                    _ => None,
                })
            });
            app.session.tool_state = ToolState::AdvancedBrushDrawing {
                stroke: crate::advanced_brush::advanced_begin(
                    settings,
                    crate::advanced_brush::AdvancedBrushSample {
                        position: screen_to_stage(screen, view),
                        pressure,
                        time_seconds: time,
                    },
                ),
            };
            app.session.status = "Advanced Brush: drawing (GPU preview)".to_string();
        }
    }

    if response.drag_started_by(PointerButton::Primary)
        || response.dragged_by(PointerButton::Primary)
        || response.drag_stopped_by(PointerButton::Primary)
    {
        let samples = advanced_input_samples(ctx, response, view);
        if let ToolState::AdvancedBrushDrawing { stroke } = &mut app.session.tool_state {
            for sample in samples {
                crate::advanced_brush::advanced_add_sample(stroke, settings, sample);
            }
        }
    }

    if response.drag_stopped_by(PointerButton::Primary) {
        if let ToolState::AdvancedBrushDrawing { stroke } =
            std::mem::replace(&mut app.session.tool_state, ToolState::Idle)
        {
            let region = crate::advanced_brush::advanced_finish(stroke);
            let classic_bridge = crate::brush::BrushSettings {
                color: settings.color,
                size: settings.size,
                smoothing: 0,
                nib: crate::brush::BrushNib::Circle,
                scale_with_stage: settings.scale_with_stage,
                sync_with_eraser: app.session.brush.sync_with_eraser,
            };
            crate::brush::commit_brush_region_with_material(
                app,
                region,
                classic_bridge,
                advanced_material(settings),
            );
        }
        return;
    }

    if response.clicked_by(PointerButton::Primary)
        && matches!(app.session.tool_state, ToolState::Idle)
    {
        if let Some(screen) = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos())
        {
            let time = ctx.input(|input| input.time);
            let stroke = crate::advanced_brush::advanced_begin(
                settings,
                crate::advanced_brush::AdvancedBrushSample::mouse(
                    screen_to_stage(screen, view),
                    time,
                ),
            );
            let region = crate::advanced_brush::advanced_finish(stroke);
            let classic_bridge = crate::brush::BrushSettings {
                color: settings.color,
                size: settings.size,
                smoothing: 0,
                nib: crate::brush::BrushNib::Circle,
                scale_with_stage: settings.scale_with_stage,
                sync_with_eraser: app.session.brush.sync_with_eraser,
            };
            crate::brush::commit_brush_region_with_material(
                app,
                region,
                classic_bridge,
                advanced_material(settings),
            );
        }
    }
}

fn brush_cursor_radius_px(settings: crate::brush::BrushSettings, view_scale: f32) -> f32 {
    let stage_size = if settings.scale_with_stage {
        settings.size
    } else {
        settings.size / view_scale.max(1.0e-4)
    };
    stage_size.max(0.1) * view_scale * 0.5
}

fn classic_brush(app: &mut EditorApp, response: &Response, view: &StageView, ctx: &Context) {
    let settings = brush_settings_for_view(app, view.scale);

    if response.drag_started_by(PointerButton::Primary) {
        if let Some(screen) = response.interact_pointer_pos() {
            let sample = crate::brush::BrushSample::mouse(screen_to_stage(screen, view));
            app.session.tool_state = ToolState::BrushDrawing {
                stroke: crate::brush::brush_begin(settings, sample),
            };
            app.session.status = "Brush: drawing".to_string();
        }
    }

    if response.drag_started_by(PointerButton::Primary)
        || response.dragged_by(PointerButton::Primary)
        || response.drag_stopped_by(PointerButton::Primary)
    {
        let event_positions: Vec<Pos2> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::PointerMoved(position) => Some(*position),
                    _ => None,
                })
                .collect()
        });

        if let ToolState::BrushDrawing { stroke } = &mut app.session.tool_state {
            for screen in event_positions {
                if response.rect.contains(screen) {
                    crate::brush::brush_add_sample(
                        stroke,
                        settings,
                        crate::brush::BrushSample::mouse(screen_to_stage(screen, view)),
                    );
                }
            }
            if let Some(screen) = response.interact_pointer_pos() {
                if response.rect.contains(screen) {
                    let point = screen_to_stage(screen, view);
                    if stroke
                        .samples
                        .last()
                        .is_none_or(|sample| sample.position != point)
                    {
                        crate::brush::brush_add_sample(
                            stroke,
                            settings,
                            crate::brush::BrushSample::mouse(point),
                        );
                    }
                }
            }
        }
    }

    if response.drag_stopped_by(PointerButton::Primary) {
        if let ToolState::BrushDrawing { stroke } =
            std::mem::replace(&mut app.session.tool_state, ToolState::Idle)
        {
            let region = crate::brush::brush_finish(stroke, settings);
            crate::brush::commit_brush_region(app, region, settings);
        }
        return;
    }

    // A click without a drag is still a real brush gesture: one nib imprint.
    if response.clicked_by(PointerButton::Primary)
        && matches!(app.session.tool_state, ToolState::Idle)
    {
        if let Some(screen) = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos())
        {
            let stroke = crate::brush::brush_begin(
                settings,
                crate::brush::BrushSample::mouse(screen_to_stage(screen, view)),
            );
            let region = crate::brush::brush_finish(stroke, settings);
            crate::brush::commit_brush_region(app, region, settings);
        }
    }
}
fn raw_path_to_geo_polygon(path: &VPath) -> Option<Polygon<f64>> {
    let points = flatten_path(path);
    if points.len() < 3 {
        return None;
    }
    let mut coords: Vec<Coord<f64>> = points
        .into_iter()
        .map(|point| Coord {
            x: point.x as f64,
            y: point.y as f64,
        })
        .collect();
    if coords.first() != coords.last() {
        coords.push(coords[0]);
    }
    Some(Polygon::new(LineString::new(coords), Vec::new()))
}

pub(crate) fn rect_polygon(rect: (f32, f32, f32, f32)) -> Polygon<f64> {
    Polygon::new(
        LineString::new(vec![
            Coord {
                x: rect.0 as f64,
                y: rect.1 as f64,
            },
            Coord {
                x: rect.2 as f64,
                y: rect.1 as f64,
            },
            Coord {
                x: rect.2 as f64,
                y: rect.3 as f64,
            },
            Coord {
                x: rect.0 as f64,
                y: rect.3 as f64,
            },
            Coord {
                x: rect.0 as f64,
                y: rect.1 as f64,
            },
        ]),
        Vec::new(),
    )
}

fn signed_path_area(path: &VPath) -> f64 {
    let points = flatten_path(path);
    if points.len() < 3 {
        return 0.0;
    }
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(a, b)| (a.x as f64 * b.y as f64) - (b.x as f64 * a.y as f64))
        .sum::<f64>()
        * 0.5
}

/// Reconstruct the final NonZero-filled surface, rather than treating each
/// contour as an independent polygon. Oppositely-wound contours are holes.
pub(crate) fn vector_fill_geometry(vector: &VectorAsset) -> MultiPolygon<f64> {
    let mut rings: Vec<(&VPath, Polygon<f64>, f64)> = vector
        .paths
        .iter()
        .filter(|path| path.closed)
        .filter_map(|path| {
            let polygon = raw_path_to_geo_polygon(path)?;
            let area = signed_path_area(path);
            (area.abs() > 1.0e-4).then_some((path, polygon, area))
        })
        .collect();
    rings.sort_by(|left, right| right.2.abs().total_cmp(&left.2.abs()));

    let mut surface = MultiPolygon(Vec::new());
    let mut inside_windings: Vec<i32> = Vec::with_capacity(rings.len());
    for index in 0..rings.len() {
        let (_, polygon, area) = &rings[index];
        let sample = polygon
            .exterior()
            .0
            .first()
            .map(|coord| Point::new(coord.x, coord.y));
        let parent = sample.and_then(|point| {
            (0..index)
                .rev()
                .find(|candidate| rings[*candidate].1.contains(&point))
        });
        let outside_winding = parent.map(|parent| inside_windings[parent]).unwrap_or(0);
        let inside_winding = outside_winding + if *area > 0.0 { 1 } else { -1 };

        if outside_winding == 0 && inside_winding != 0 {
            surface = surface.union(polygon);
        } else if outside_winding != 0 && inside_winding == 0 {
            surface = surface.difference(polygon);
        }
        inside_windings.push(inside_winding);
    }
    surface
}

pub(crate) fn raw_selectable_fill_surface(
    project: &ProjectV2,
    asset_id: u16,
    vector: &VectorAsset,
) -> MultiPolygon<f64> {
    #[cfg(feature = "appearance-mask-eraser")]
    {
        crate::appearance::visible_material_surface_for_vector(
            vector,
            project.asset_appearances.get(&asset_id),
        )
    }
    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        let _ = (project, asset_id);
        vector_fill_geometry(vector)
    }
}

fn raw_selectable_paths_surface(
    project: &ProjectV2,
    asset_id: u16,
    vector: &VectorAsset,
    path_indices: &[usize],
) -> MultiPolygon<f64> {
    #[cfg(feature = "appearance-mask-eraser")]
    {
        crate::appearance::visible_material_surface_for_paths(
            vector,
            project.asset_appearances.get(&asset_id),
            path_indices,
        )
    }
    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        let _ = (project, asset_id);
        vector_fill_geometry(&VectorAsset {
            asset_id: vector.asset_id,
            paths: path_indices
                .iter()
                .filter_map(|index| vector.paths.get(*index).cloned())
                .collect(),
            fill: vector.fill,
            stroke: None,
        })
    }
}

fn selectable_component_at_cursor(
    project: &ProjectV2,
    asset_id: u16,
    vector: &VectorAsset,
    cursor: Vec2,
) -> Option<Polygon<f64>> {
    #[cfg(not(feature = "appearance-mask-eraser"))]
    let _ = (project, asset_id);
    #[cfg(not(feature = "appearance-mask-eraser"))]
    let point = Point::new(cursor.x as f64, cursor.y as f64);
    let source = vector_fill_geometry(vector);

    #[cfg(feature = "appearance-mask-eraser")]
    {
        let appearance = project.asset_appearances.get(&asset_id);
        if !crate::appearance::visible_material_contains_point(vector, appearance, cursor, 2.0) {
            return None;
        }

        for component in source.0 {
            let support = MultiPolygon(vec![component.clone()]);
            let hits_component = if let Some(appearance) = appearance {
                let Some(inverse_field) = appearance.field_transform.inverse() else {
                    continue;
                };
                let canonical_cursor = inverse_field.apply(cursor);
                let canonical_support =
                    crate::appearance::transform_surface(&support, inverse_field);
                crate::appearance::material_support_contains_point(
                    &canonical_support,
                    appearance.material,
                    canonical_cursor,
                    2.0,
                )
            } else {
                crate::appearance::surface_contains_or_near(&support, cursor, 2.0)
            };
            if hits_component {
                return Some(component);
            }
        }
        None
    }

    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        for component in source.0 {
            if component.contains(&point) || polygon_boundary_near_cursor(&component, cursor, 2.0) {
                return Some(component);
            }
        }
        None
    }
}

fn surface_to_screen_contours(surface: &MultiPolygon<f64>, view: &StageView) -> Vec<Vec<Pos2>> {
    crate::brush::coverage_to_paths(surface)
        .iter()
        .map(|path| {
            flatten_path(path)
                .into_iter()
                .map(|point| stage_to_screen(point, view))
                .collect()
        })
        .collect()
}

pub(crate) fn geo_multi_polygon_to_linear_paths(geometry: &MultiPolygon<f64>) -> Vec<VPath> {
    fn ring_path(ring: &LineString<f64>) -> Option<VPath> {
        let mut points: Vec<Vec2> = ring
            .0
            .iter()
            .map(|coord| Vec2::new(coord.x as f32, coord.y as f32))
            .collect();
        if points.len() > 1 && distance_sq(points[0], points[points.len() - 1]) <= 1.0e-5 {
            points.pop();
        }
        let mut looped = points;
        if looped.len() < 3 {
            return None;
        }
        looped.push(looped[0]);
        let mut simplified = rdp_simplify(&looped, 0.06);
        if simplified.len() > 1
            && distance_sq(simplified[0], simplified[simplified.len() - 1]) <= 1.0e-5
        {
            simplified.pop();
        }
        (simplified.len() >= 3).then(|| VPath {
            anchors: simplified.into_iter().map(anchor).collect(),
            closed: true,
        })
    }

    let mut paths = Vec::new();
    for polygon in &geometry.0 {
        if polygon.unsigned_area() < 0.05 {
            continue;
        }
        if let Some(path) = ring_path(polygon.exterior()) {
            paths.push(path);
        }
        for hole in polygon.interiors() {
            if let Some(path) = ring_path(hole) {
                paths.push(path);
            }
        }
    }
    paths
}

/// Build a V marquee without touching the project. The selection records the
/// drawing surfaces intersected by the rectangle; rendering clips those same
/// surfaces, so holes never become selectable phantom fills.
fn select_raw_area_by_rect(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    rect: (f32, f32, f32, f32),
) -> Option<Selection> {
    let clip = rect_polygon(rect);
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    let mut placements = Vec::new();
    let mut actual_min = Vec2::new(f32::INFINITY, f32::INFINITY);
    let mut actual_max = Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut found_bounds = false;
    for layer in &q.layers {
        for placement_idx in active_raw_placement_indices(project, layer, frame) {
            let placement = &layer.placements[placement_idx];
            let Target::Asset(asset_id) = placement.target else {
                continue;
            };
            let Some(Asset::Vector(vector)) = project.assets.iter().find(|a| a.id() == asset_id)
            else {
                continue;
            };
            if vector.fill.is_none() {
                continue;
            }
            let selected =
                raw_selectable_fill_surface(project, asset_id, vector).intersection(&clip);
            if selected.unsigned_area() > 0.05 {
                if let Some(bounds) = selected.bounding_rect() {
                    actual_min.x = actual_min.x.min(bounds.min().x as f32);
                    actual_min.y = actual_min.y.min(bounds.min().y as f32);
                    actual_max.x = actual_max.x.max(bounds.max().x as f32);
                    actual_max.y = actual_max.y.max(bounds.max().y as f32);
                    found_bounds = true;
                }
                placements.push(PlacementRef {
                    q0rg_id,
                    layer_id: layer.layer_id,
                    placement_idx,
                });
            }
        }
    }
    (!placements.is_empty() && found_bounds).then_some(Selection::RawArea {
        placements,
        objects: Vec::new(),
        bounds_min: actual_min,
        bounds_max: actual_max,
    })
}

fn marquee_selection(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    rect: (f32, f32, f32, f32),
) -> Selection {
    let objects = collect_placements_in_rect(project, q0rg_id, frame, rect);
    if let Some(mut selection) = select_raw_area_by_rect(project, q0rg_id, frame, rect) {
        if let Selection::RawArea {
            objects: selected_objects,
            ..
        } = &mut selection
        {
            *selected_objects = objects;
        }
        return selection;
    }

    let raw_hits = collect_raw_paths_in_rect(project, q0rg_id, frame, rect);
    if !raw_hits.is_empty() {
        return match raw_hits.len() {
            1 => Selection::Path {
                q0rg_id: raw_hits[0].q0rg_id,
                layer_id: raw_hits[0].layer_id,
                placement_idx: raw_hits[0].placement_idx,
                path_idx: raw_hits[0].path_idx,
            },
            _ => Selection::Paths(raw_hits),
        };
    }

    match objects.len() {
        0 => Selection::None,
        1 => Selection::Placement {
            q0rg_id: objects[0].q0rg_id,
            layer_id: objects[0].layer_id,
            placement_idx: objects[0].placement_idx,
        },
        _ => Selection::Multi(objects),
    }
}

/// Materialise a non-destructive V selection only when a drag starts. The
/// boolean result is stored as accurate linear boundaries: no Bezier refit,
/// no rubber-band handles, and no geometry mutation merely from selecting.
fn prepare_writable_raw_references(
    project: &mut ProjectV2,
    paths: &[PathRef],
    placements: &[PlacementRef],
    frame: u16,
) {
    let mut groups: std::collections::BTreeMap<(u16, u16), std::collections::BTreeSet<u16>> =
        std::collections::BTreeMap::new();
    for reference in placements
        .iter()
        .copied()
        .chain(paths.iter().map(|path| PlacementRef {
            q0rg_id: path.q0rg_id,
            layer_id: path.layer_id,
            placement_idx: path.placement_idx,
        }))
    {
        let Some(asset_id) = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.layer_id == reference.layer_id)
            })
            .and_then(|layer| layer.placements.get(reference.placement_idx))
            .and_then(|placement| match placement.target {
                Target::Asset(asset_id) => Some(asset_id),
                Target::Q0rg(_) => None,
            })
        else {
            continue;
        };
        groups
            .entry((reference.q0rg_id, reference.layer_id))
            .or_default()
            .insert(asset_id);
    }
    for ((q0rg_id, layer_id), asset_ids) in groups {
        crate::brush::prepare_writable_raw_assets(project, q0rg_id, layer_id, frame, &asset_ids);
    }
}

fn remap_group_references_for_edit(
    project: &mut ProjectV2,
    paths: &[PathRef],
    raw_placements: &[PlacementRef],
    objects: &[PlacementRef],
    frame: u16,
) -> Option<(Vec<PathRef>, Vec<PlacementRef>, Vec<PlacementRef>)> {
    let mut all = Vec::new();
    for reference in paths
        .iter()
        .map(|path| PlacementRef {
            q0rg_id: path.q0rg_id,
            layer_id: path.layer_id,
            placement_idx: path.placement_idx,
        })
        .chain(raw_placements.iter().copied())
        .chain(objects.iter().copied())
    {
        if !all.contains(&reference) {
            all.push(reference);
        }
    }
    let mapped = materialize_placement_refs_for_edit(project, &all, frame)?;
    let remap = |reference: PlacementRef| -> Option<PlacementRef> {
        let index = all.iter().position(|candidate| *candidate == reference)?;
        mapped.get(index).copied()
    };
    let mapped_paths: Vec<PathRef> = paths
        .iter()
        .map(|path| {
            let placement = remap(PlacementRef {
                q0rg_id: path.q0rg_id,
                layer_id: path.layer_id,
                placement_idx: path.placement_idx,
            })?;
            Some(PathRef {
                q0rg_id: placement.q0rg_id,
                layer_id: placement.layer_id,
                placement_idx: placement.placement_idx,
                path_idx: path.path_idx,
            })
        })
        .collect::<Option<_>>()?;
    let mapped_raw: Vec<PlacementRef> = raw_placements
        .iter()
        .copied()
        .map(remap)
        .collect::<Option<_>>()?;
    let mapped_objects: Vec<PlacementRef> =
        objects.iter().copied().map(remap).collect::<Option<_>>()?;
    prepare_writable_raw_references(project, &mapped_paths, &mapped_raw, frame);
    Some((mapped_paths, mapped_raw, mapped_objects))
}

fn cut_raw_areas_for_drag_prepared(
    app: &mut EditorApp,
    placements: &[PlacementRef],
    rect: (f32, f32, f32, f32),
) -> Vec<PathRef> {
    prepare_writable_raw_references(
        &mut app.state.project,
        &[],
        placements,
        app.session.current_frame,
    );
    let clip_polygon = rect_polygon(rect);
    let clip = MultiPolygon(vec![clip_polygon.clone()]);
    let mut jobs = Vec::new();
    let mut seen_assets = std::collections::BTreeSet::new();
    for r in placements {
        let Some(placement) = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == r.q0rg_id)
            .and_then(|q| q.layers.iter().find(|l| l.layer_id == r.layer_id))
            .and_then(|l| l.placements.get(r.placement_idx))
        else {
            continue;
        };
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        if !seen_assets.insert(asset_id) {
            continue;
        }
        let Some(Asset::Vector(vector)) =
            app.state.project.assets.iter().find(|a| a.id() == asset_id)
        else {
            continue;
        };
        let source = vector_fill_geometry(vector);
        #[cfg(feature = "appearance-mask-eraser")]
        let appearance_enabled = app.state.project.asset_appearances.contains_key(&asset_id);
        #[cfg(not(feature = "appearance-mask-eraser"))]
        let appearance_enabled = false;

        if appearance_enabled {
            // Raw-area selection is based on what is actually visible. A marquee
            // may contain only a piece of halo and no source fill at all; that is
            // still a real post-material fragment and must be draggable without
            // falling through to a whole-source click drag.
            let visible = raw_selectable_fill_surface(&app.state.project, asset_id, vector);
            let selected_visible = visible.intersection(&clip);
            if selected_visible.unsigned_area() <= 0.05 {
                continue;
            }
            let visible_remainder = visible.difference(&clip);
            jobs.push((
                *r,
                asset_id,
                source.intersection(&clip),
                source.difference(&clip),
                true,
                visible_remainder.unsigned_area() <= 0.05,
            ));
            continue;
        }

        let selected = source.intersection(&clip);
        if selected.unsigned_area() <= 0.05 {
            continue;
        }
        let remainder = source.difference(&clip);
        jobs.push((*r, asset_id, selected, remainder, false, false));
    }
    if jobs.is_empty() {
        return Vec::new();
    }
    // Inserting a selected fragment immediately after its source placement must
    // not invalidate placement indices of jobs we have not processed yet.
    jobs.sort_by(|left, right| {
        right
            .0
            .q0rg_id
            .cmp(&left.0.q0rg_id)
            .then(right.0.layer_id.cmp(&left.0.layer_id))
            .then(right.0.placement_idx.cmp(&left.0.placement_idx))
    });

    let mut refs = Vec::new();
    let mut appearance_changed = false;
    for (r, asset_id, selected, remainder, appearance_enabled, whole_visual) in jobs {
        if appearance_enabled {
            let Some(Asset::Vector(original)) = app
                .state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == asset_id)
                .cloned()
            else {
                continue;
            };

            if whole_visual {
                // The marquee already contains the complete resolved fragment;
                // no split is needed. Return its real source refs so the ordinary
                // whole-asset move path can carry vector + appearance together.
                refs.extend(
                    original
                        .paths
                        .iter()
                        .enumerate()
                        .filter_map(|(path_idx, path)| {
                            path.closed.then_some(PathRef {
                                q0rg_id: r.q0rg_id,
                                layer_id: r.layer_id,
                                placement_idx: r.placement_idx,
                                path_idx,
                            })
                        }),
                );
                continue;
            }

            let selected_has_body = selected.unsigned_area() > 0.05;
            let remainder_has_body = remainder.unsigned_area() > 0.05;
            let carrier_paths: Vec<VPath> = original
                .paths
                .iter()
                .filter(|path| path.closed)
                .cloned()
                .collect();
            if carrier_paths.is_empty() {
                continue;
            }

            // When the marquee cuts real fill we keep the compact physical body
            // split. If it catches halo only (or all body but not all halo), the
            // frozen source is duplicated only as an internal carrier; the
            // post-material clip is authoritative for rendering/hit-testing.
            let selected_paths = if selected_has_body {
                let paths = geo_multi_polygon_to_linear_paths(&selected);
                if paths.is_empty() {
                    carrier_paths.clone()
                } else {
                    paths
                }
            } else {
                carrier_paths.clone()
            };
            if selected_has_body && remainder_has_body {
                if let Some(Asset::Vector(vector)) = app
                    .state
                    .project
                    .assets
                    .iter_mut()
                    .find(|asset| asset.id() == asset_id)
                {
                    let mut paths: Vec<VPath> = vector
                        .paths
                        .iter()
                        .filter(|path| !path.closed)
                        .cloned()
                        .collect();
                    paths.extend(geo_multi_polygon_to_linear_paths(&remainder));
                    vector.paths = paths;
                }
            }

            let selected_asset_id = next_asset_id(&app.state.project);
            app.state.project.assets.push(Asset::Vector(VectorAsset {
                asset_id: selected_asset_id,
                paths: selected_paths,
                fill: original.fill,
                stroke: original.stroke,
            }));
            #[cfg(feature = "appearance-mask-eraser")]
            crate::appearance::split_asset_appearance(
                &mut app.state.project,
                asset_id,
                selected_asset_id,
                &original.paths,
                &clip,
                false,
            );

            let Some(layer) = app
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q| q.q0rg_id == r.q0rg_id)
                .and_then(|q| q.layers.iter_mut().find(|l| l.layer_id == r.layer_id))
            else {
                continue;
            };
            let insertion = (r.placement_idx + 1).min(layer.placements.len());
            layer.placements.insert(
                insertion,
                Placement {
                    frame: app.session.current_frame,
                    target: Target::Asset(selected_asset_id),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                },
            );
            let selected_len = app
                .state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == selected_asset_id)
                .and_then(|asset| match asset {
                    Asset::Vector(vector) => Some(vector.paths.len()),
                    _ => None,
                })
                .unwrap_or(0);
            refs.extend((0..selected_len).map(|path_idx| PathRef {
                q0rg_id: r.q0rg_id,
                layer_id: r.layer_id,
                placement_idx: insertion,
                path_idx,
            }));
            appearance_changed = true;
            continue;
        }

        let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|a| a.id() == asset_id)
        else {
            continue;
        };
        let mut paths: Vec<VPath> = vector
            .paths
            .iter()
            .filter(|path| !path.closed)
            .cloned()
            .collect();
        paths.extend(geo_multi_polygon_to_linear_paths(&remainder));
        let selected_paths = geo_multi_polygon_to_linear_paths(&selected);
        let selected_start = paths.len();
        paths.extend(selected_paths);
        vector.paths = paths;
        refs.extend(
            (selected_start..vector.paths.len()).map(|path_idx| PathRef {
                q0rg_id: r.q0rg_id,
                layer_id: r.layer_id,
                placement_idx: r.placement_idx,
                path_idx,
            }),
        );
    }
    if !refs.is_empty() {
        app.state.dirty = true;
    }
    if appearance_changed {
        app.textures.invalidate();
    }
    refs
}

fn cut_raw_areas_for_drag(
    app: &mut EditorApp,
    placements: &[PlacementRef],
    rect: (f32, f32, f32, f32),
) -> Vec<PathRef> {
    app.history.snapshot(&app.state.project);
    let Some((_, placements, _)) = remap_group_references_for_edit(
        &mut app.state.project,
        &[],
        placements,
        &[],
        app.session.current_frame,
    ) else {
        return Vec::new();
    };
    cut_raw_areas_for_drag_prepared(app, &placements, rect)
}

#[derive(Debug, Clone, Copy)]
enum GroupTransformIntent {
    Move,
    Hit(TransformHit),
}

struct GroupTransformData<'a> {
    refs: &'a [PathRef],
    start_paths: &'a [VPath],
    start_appearances: &'a [(u16, q0s_format::v2::VectorAppearance)],
    objects: &'a [PlacementRef],
    start_transforms: &'a [Transform2D],
    operation: GroupTransformOperation,
    start_pivot: Vec2,
}

fn placement_ref_transform(project: &ProjectV2, reference: PlacementRef) -> Option<Transform2D> {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == reference.layer_id)?
        .placements
        .get(reference.placement_idx)
        .map(|placement| placement.transform)
}

fn selection_from_group_parts(paths: Vec<PathRef>, objects: Vec<PlacementRef>) -> Selection {
    if !paths.is_empty() && !objects.is_empty() {
        return Selection::Mixed { paths, objects };
    }
    if !paths.is_empty() {
        return match paths.len() {
            1 => Selection::Path {
                q0rg_id: paths[0].q0rg_id,
                layer_id: paths[0].layer_id,
                placement_idx: paths[0].placement_idx,
                path_idx: paths[0].path_idx,
            },
            _ => Selection::Paths(paths),
        };
    }
    match objects.len() {
        0 => Selection::None,
        1 => Selection::Placement {
            q0rg_id: objects[0].q0rg_id,
            layer_id: objects[0].layer_id,
            placement_idx: objects[0].placement_idx,
        },
        _ => Selection::Multi(objects),
    }
}

fn begin_group_transform(
    app: &mut EditorApp,
    selection: Selection,
    intent: GroupTransformIntent,
    start_cursor: Vec2,
) -> bool {
    let Some(bounds) = selection_transform_bounds(app) else {
        return false;
    };
    if !raw_selection_supports_axis_resize(bounds) {
        return false;
    }
    let pivot = selection_transform_pivot(app)
        .unwrap_or_else(|| Vec2::new((bounds.0 + bounds.2) * 0.5, (bounds.1 + bounds.3) * 0.5));
    let (paths, raw_placements, objects, raw_rect) = match selection {
        Selection::RawArea {
            placements,
            objects,
            bounds_min,
            bounds_max,
        } => (
            Vec::new(),
            placements,
            objects,
            Some((bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y)),
        ),
        Selection::Mixed { paths, objects } => (paths, Vec::new(), objects, None),
        Selection::Multi(objects) => (Vec::new(), Vec::new(), objects, None),
        _ => return false,
    };
    app.history.snapshot(&app.state.project);
    let Some((mut paths, raw_placements, objects)) = remap_group_references_for_edit(
        &mut app.state.project,
        &paths,
        &raw_placements,
        &objects,
        app.session.current_frame,
    ) else {
        return false;
    };
    if let Some(rect) = raw_rect {
        paths.extend(cut_raw_areas_for_drag_prepared(app, &raw_placements, rect));
    }
    if paths.is_empty() && objects.is_empty() {
        return false;
    }
    let start_paths: Vec<VPath> = paths
        .iter()
        .filter_map(|reference| {
            raw_path_clone(
                &app.state.project,
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
                reference.path_idx,
            )
        })
        .collect();
    let start_transforms: Vec<Transform2D> = objects
        .iter()
        .copied()
        .filter_map(|reference| placement_ref_transform(&app.state.project, reference))
        .collect();
    if start_paths.len() != paths.len() || start_transforms.len() != objects.len() {
        return false;
    }
    let start_appearances =
        capture_whole_asset_appearances_for_raw_refs(&app.state.project, &paths);
    app.session.selection = selection_from_group_parts(paths.clone(), objects.clone());
    set_selection_transform_pivot(app, pivot);
    let operation = match intent {
        GroupTransformIntent::Move => GroupTransformOperation::Move { start_cursor },
        GroupTransformIntent::Hit(TransformHit::Scale(handle)) => GroupTransformOperation::Scale {
            handle,
            start_bounds: bounds,
        },
        GroupTransformIntent::Hit(TransformHit::Rotate(_)) => GroupTransformOperation::Rotate {
            center: pivot,
            start_angle: (start_cursor.y - pivot.y).atan2(start_cursor.x - pivot.x),
        },
        GroupTransformIntent::Hit(TransformHit::Skew(edge)) => GroupTransformOperation::Skew {
            edge,
            start_bounds: bounds,
            start_cursor,
        },
    };
    app.session.tool_state = ToolState::DraggingGroup {
        refs: paths,
        start_paths,
        start_appearances,
        objects,
        start_transforms,
        operation,
        start_pivot: pivot,
    };
    app.session.status = match operation {
        GroupTransformOperation::Move { .. } => "Moving mixed selection",
        GroupTransformOperation::Scale { .. } => "Resizing mixed selection",
        GroupTransformOperation::Rotate { .. } => "Rotating mixed selection",
        GroupTransformOperation::Skew { .. } => "Skewing mixed selection",
    }
    .to_string();
    true
}

fn group_transform_affine(operation: GroupTransformOperation, cursor: Vec2) -> Option<Affine> {
    match operation {
        GroupTransformOperation::Move { start_cursor } => Some(Affine {
            tx: cursor.x - start_cursor.x,
            ty: cursor.y - start_cursor.y,
            ..Affine::IDENTITY
        }),
        GroupTransformOperation::Scale {
            handle,
            start_bounds,
        } => {
            let (anchor, scale_x, scale_y) = raw_handle_scale(start_bounds, handle, cursor)?;
            Some(Affine {
                a11: scale_x,
                a12: 0.0,
                a21: 0.0,
                a22: scale_y,
                tx: anchor.x * (1.0 - scale_x),
                ty: anchor.y * (1.0 - scale_y),
            })
        }
        GroupTransformOperation::Rotate {
            center,
            start_angle,
        } => {
            let angle = (cursor.y - center.y).atan2(cursor.x - center.x) - start_angle;
            let (sin, cos) = angle.sin_cos();
            Some(Affine {
                a11: cos,
                a12: -sin,
                a21: sin,
                a22: cos,
                tx: center.x - cos * center.x + sin * center.y,
                ty: center.y - sin * center.x - cos * center.y,
            })
        }
        GroupTransformOperation::Skew {
            edge,
            start_bounds,
            start_cursor,
        } => {
            let delta = Vec2::new(cursor.x - start_cursor.x, cursor.y - start_cursor.y);
            let (min_x, min_y, max_x, max_y) = start_bounds;
            let mut affine = Affine::IDENTITY;
            match edge {
                TransformEdge::Top => {
                    let shear = (delta.x / (min_y - max_y)).clamp(-8.0, 8.0);
                    affine.a12 = shear;
                    affine.tx = -max_y * shear;
                }
                TransformEdge::Bottom => {
                    let shear = (delta.x / (max_y - min_y)).clamp(-8.0, 8.0);
                    affine.a12 = shear;
                    affine.tx = -min_y * shear;
                }
                TransformEdge::Left => {
                    let shear = (delta.y / (min_x - max_x)).clamp(-8.0, 8.0);
                    affine.a21 = shear;
                    affine.ty = -max_x * shear;
                }
                TransformEdge::Right => {
                    let shear = (delta.y / (max_x - min_x)).clamp(-8.0, 8.0);
                    affine.a21 = shear;
                    affine.ty = -min_x * shear;
                }
            }
            Some(affine)
        }
    }
}

fn apply_group_transform(app: &mut EditorApp, data: GroupTransformData<'_>, cursor: Vec2) -> bool {
    let Some(transform) = group_transform_affine(data.operation, cursor) else {
        return false;
    };
    let mut changed = apply_raw_affine_snapshot(
        &mut app.state.project,
        data.refs,
        data.start_paths,
        data.start_appearances,
        transform,
    );
    for (reference, source) in data
        .objects
        .iter()
        .copied()
        .zip(data.start_transforms.iter().copied())
    {
        let composed = Affine::compose(transform, Affine::from_transform(source));
        let Some(next) = crate::app::affine_to_transform(composed) else {
            continue;
        };
        if let Some(placement) = placement_mut(
            app,
            reference.q0rg_id,
            reference.layer_id,
            reference.placement_idx,
        ) {
            placement.transform = next;
            changed = true;
        }
    }
    if changed {
        app.state.dirty = true;
        set_selection_transform_pivot(app, transform.apply(data.start_pivot));
    }
    changed
}

fn begin_transforming_raw_area(
    app: &mut EditorApp,
    placements: &[PlacementRef],
    bounds: (f32, f32, f32, f32),
    pivot: Vec2,
    hit: TransformHit,
    start_cursor: Vec2,
) -> bool {
    if placements.is_empty() || !raw_selection_supports_axis_resize(bounds) {
        return false;
    }
    // Cutting is deferred until the user actually grabs a transform zone.
    // From this frame onward the clipped piece is ordinary path-granular raw
    // graphics, so scale/rotate/skew can start without a fake move first.
    let refs = cut_raw_areas_for_drag(app, placements, bounds);
    if refs.is_empty() {
        return false;
    }
    let start_paths: Vec<VPath> = refs
        .iter()
        .filter_map(|reference| {
            raw_path_clone(
                &app.state.project,
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
                reference.path_idx,
            )
        })
        .collect();
    if start_paths.len() != refs.len() {
        return false;
    }
    app.session.selection = selection_from_group_parts(refs.clone(), Vec::new());
    set_selection_transform_pivot(app, pivot);
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(&app.state.project, &refs);
    match hit {
        TransformHit::Scale(handle) => {
            app.session.tool_state = ToolState::DraggingRawHandle {
                refs,
                start_paths,
                start_appearances,
                handle,
                start_bounds: bounds,
                start_pivot: pivot,
            };
            app.session.status = "Resizing selected fill area".to_string();
        }
        TransformHit::Rotate(_) => {
            app.session.tool_state = ToolState::DraggingRawRotate {
                refs,
                start_paths,
                start_appearances,
                center: pivot,
                start_angle: (start_cursor.y - pivot.y).atan2(start_cursor.x - pivot.x),
            };
            app.session.status = "Rotating selected fill area".to_string();
        }
        TransformHit::Skew(edge) => {
            app.session.tool_state = ToolState::DraggingRawSkew {
                refs,
                start_paths,
                start_appearances,
                edge,
                start_bounds: bounds,
                start_cursor,
                start_pivot: pivot,
            };
            app.session.status = "Skewing selected fill area".to_string();
        }
    }
    true
}

fn rdp_simplify(points: &[Vec2], epsilon: f32) -> Vec<Vec2> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let first = points[0];
    let last = points[points.len() - 1];
    let mut max_distance = 0.0;
    let mut split = 0;
    for (index, point) in points.iter().enumerate().take(points.len() - 1).skip(1) {
        let distance = point_segment_distance(*point, first, last);
        if distance > max_distance {
            max_distance = distance;
            split = index;
        }
    }
    if max_distance <= epsilon {
        return vec![first, last];
    }
    let mut left = rdp_simplify(&points[..=split], epsilon);
    let right = rdp_simplify(&points[split..], epsilon);
    left.pop();
    left.extend(right);
    left
}

fn point_segment_distance(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq <= 1.0e-8 {
        0.0
    } else {
        (((point.x - a.x) * dx + (point.y - a.y) * dy) / length_sq).clamp(0.0, 1.0)
    };
    let closest = Vec2::new(a.x + dx * t, a.y + dy * t);
    ((point.x - closest.x).powi(2) + (point.y - closest.y).powi(2)).sqrt()
}

fn distance_sq(a: Vec2, b: Vec2) -> f32 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
}

/// Re-export of the canonical `brush_outline` so editor call sites that
/// already say `crate::tools::brush_outline` keep working; the actual
/// implementation lives in `q0s_format::geom` so the player can use it
/// at playback without depending on the editor crate.
pub fn brush_outline(centerline: &[Vec2], half_width: f32, cap_steps: usize) -> Vec<Vec2> {
    q0s_format::geom::brush_outline(centerline, half_width, cap_steps)
}

pub fn next_asset_id_pub(project: &ProjectV2) -> u16 {
    next_asset_id(project)
}

/// Convert a polyline of sampled mouse positions into bezier anchors so the
/// resulting path renders as a smooth curve through every sample, not as a
/// chain of straight segments. Uses Catmull-Rom-to-Bezier conversion: for
/// each anchor at p[i], handles point along the chord (p[i+1] - p[i-1]).
fn catmull_rom_anchors(points: &[Vec2]) -> Vec<Anchor> {
    let n = points.len();
    let tension: f32 = 0.5; // 0.5 = standard centripetal-ish smoothness
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p = points[i];
        let prev = if i > 0 { points[i - 1] } else { p };
        let next = if i + 1 < n { points[i + 1] } else { p };
        let dx = (next.x - prev.x) * tension / 3.0;
        let dy = (next.y - prev.y) * tension / 3.0;
        let in_handle = if i > 0 {
            Some(Vec2::new(p.x - dx, p.y - dy))
        } else {
            None
        };
        let out_handle = if i + 1 < n {
            Some(Vec2::new(p.x + dx, p.y + dy))
        } else {
            None
        };
        out.push(Anchor {
            point: p,
            in_handle,
            out_handle,
        });
    }
    out
}

// ---------------- Eraser tool ----------------

fn eraser_settings(app: &EditorApp, view_scale: f32) -> crate::brush::BrushSettings {
    let mut settings = app.session.brush;
    if settings.sync_with_eraser {
        if !settings.scale_with_stage {
            settings.size /= view_scale.max(1.0e-4);
        }
    } else {
        settings.size = app.session.eraser_size.max(0.1);
        if !settings.scale_with_stage {
            settings.size /= view_scale.max(1.0e-4);
        }
        settings.nib = crate::brush::BrushNib::Circle;
    }
    settings.smoothing = 0;
    settings
}

/// Classic area eraser. It records the same raw pointer samples as the brush,
/// builds one unioned nib coverage, and subtracts that coverage from raw fills
/// once per gesture. Open strokes and display objects keep their legacy
/// fallback semantics when no raw fill was touched.
fn eraser(app: &mut EditorApp, response: &Response, view: &StageView, ctx: &Context) {
    let settings = eraser_settings(app, view.scale);

    if response.drag_started_by(PointerButton::Primary) {
        if let Some(screen) = response.interact_pointer_pos() {
            let sample = crate::brush::BrushSample::mouse(screen_to_stage(screen, view));
            app.session.tool_state = ToolState::EraserDrawing {
                stroke: crate::brush::brush_begin(settings, sample),
            };
            app.session.status = "Eraser: drawing area".to_string();
        }
    }

    if response.drag_started_by(PointerButton::Primary)
        || response.dragged_by(PointerButton::Primary)
        || response.drag_stopped_by(PointerButton::Primary)
    {
        let event_positions: Vec<Pos2> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::PointerMoved(position) => Some(*position),
                    _ => None,
                })
                .collect()
        });

        if let ToolState::EraserDrawing { stroke } = &mut app.session.tool_state {
            for screen in event_positions {
                if response.rect.contains(screen) {
                    crate::brush::brush_add_sample(
                        stroke,
                        settings,
                        crate::brush::BrushSample::mouse(screen_to_stage(screen, view)),
                    );
                }
            }
            if let Some(screen) = response.interact_pointer_pos() {
                if response.rect.contains(screen) {
                    let point = screen_to_stage(screen, view);
                    if stroke
                        .samples
                        .last()
                        .is_none_or(|sample| sample.position != point)
                    {
                        crate::brush::brush_add_sample(
                            stroke,
                            settings,
                            crate::brush::BrushSample::mouse(point),
                        );
                    }
                }
            }
        }
    }

    if response.drag_stopped_by(PointerButton::Primary) {
        if let ToolState::EraserDrawing { stroke } =
            std::mem::replace(&mut app.session.tool_state, ToolState::Idle)
        {
            let fallback_point = stroke.samples.last().map(|sample| sample.position);
            let region = crate::brush::brush_finish(stroke, settings);
            #[cfg(feature = "appearance-mask-eraser")]
            let appearance_erased = crate::appearance::erase_visible_region(app, region.clone());
            #[cfg(not(feature = "appearance-mask-eraser"))]
            let appearance_erased = false;
            let geometry_erased = if appearance_erased {
                crate::brush::erase_brush_region_without_snapshot(app, region)
            } else {
                crate::brush::erase_brush_region(app, region)
            };
            if !appearance_erased && !geometry_erased {
                if let Some(point) = fallback_point {
                    erase_fallback_at(app, point, settings.size.max(0.1) * 0.5);
                }
            }
        }
        return;
    }

    // A click without a drag is one circular subtraction dab.
    if response.clicked_by(PointerButton::Primary)
        && matches!(app.session.tool_state, ToolState::Idle)
    {
        if let Some(screen) = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos())
        {
            let point = screen_to_stage(screen, view);
            let stroke =
                crate::brush::brush_begin(settings, crate::brush::BrushSample::mouse(point));
            let region = crate::brush::brush_finish(stroke, settings);
            #[cfg(feature = "appearance-mask-eraser")]
            let appearance_erased = crate::appearance::erase_visible_region(app, region.clone());
            #[cfg(not(feature = "appearance-mask-eraser"))]
            let appearance_erased = false;
            let geometry_erased = if appearance_erased {
                crate::brush::erase_brush_region_without_snapshot(app, region)
            } else {
                crate::brush::erase_brush_region(app, region)
            };
            if !appearance_erased && !geometry_erased {
                erase_fallback_at(app, point, settings.size.max(0.1) * 0.5);
            }
        }
    }
}

fn erase_fallback_at(app: &mut EditorApp, point: Vec2, eraser_radius: f32) {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;

    if let Some(hit) =
        find_path_under_cursor(&app.state.project, q0rg_id, frame, point, eraser_radius)
    {
        app.history.snapshot(&app.state.project);
        let asset_emptied =
            remove_path_from_asset(&mut app.state.project, hit.asset_id, hit.path_idx);
        if asset_emptied {
            if let Some(q0rg) = app
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            {
                for layer in &mut q0rg.layers {
                    layer.placements.retain(
                        |placement| !matches!(placement.target, Target::Asset(id) if id == hit.asset_id),
                    );
                }
            }
            app.state
                .project
                .assets
                .retain(|asset| asset.id() != hit.asset_id);
        }
        app.session.selection = Selection::None;
        app.state.dirty = true;
        app.session.status = if asset_emptied {
            "Drawing cleared".to_string()
        } else {
            "Stroke path erased".to_string()
        };
        return;
    }

    let Some((layer_id, placement_idx)) =
        hit_test_selectable_placement(&app.state.project, q0rg_id, frame, point)
    else {
        return;
    };
    app.history.snapshot(&app.state.project);
    if let Some(layer) = app
        .state
        .project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)
        })
    {
        if placement_idx < layer.placements.len() {
            layer.placements.remove(placement_idx);
            app.session.selection = Selection::None;
            app.state.dirty = true;
            app.session.status = "Instance erased".to_string();
        }
    }
}
/// One match record for the merge-drawing eraser hit-test.
struct PathHit {
    asset_id: u16,
    path_idx: usize,
}

/// Walk every merge-drawing asset on `q0rg_id` at `frame` (top-most layer
/// first, latest path within it first) and return the first path that
/// the cursor falls inside (closed paths) or close enough to its
/// centerline (open paths, scaled by stroke width).
fn find_path_under_cursor(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    cursor: Vec2,
    eraser_radius: f32,
) -> Option<PathHit> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    for layer in q.layers.iter().rev() {
        for placement_idx in active_raw_placement_indices(project, layer, frame)
            .into_iter()
            .rev()
        {
            let placement = &layer.placements[placement_idx];
            let Target::Asset(asset_id) = placement.target else {
                continue;
            };
            let Some(Asset::Vector(v)) = project.assets.iter().find(|a| a.id() == asset_id) else {
                continue;
            };
            let stroke_w = v.stroke.as_ref().map(|s| s.width.max(1.0)).unwrap_or(1.0);
            for (idx, path) in v.paths.iter().enumerate().rev() {
                // Filled raw geometry is handled by boolean subtraction above.
                // This fallback is only for ordinary line/stroke paths.
                if path.closed && v.fill.is_some() && v.stroke.is_none() {
                    continue;
                }
                let pts = flatten_path(path);
                if pts.len() < 2 {
                    continue;
                }
                let hit = if path.closed {
                    point_in_polygon(&pts, cursor)
                        || nearest_segment_distance(&pts, cursor) <= stroke_w * 0.5 + 2.0
                } else {
                    nearest_segment_distance(&pts, cursor) <= stroke_w * 0.5 + eraser_radius * 0.5
                };
                if hit {
                    return Some(PathHit {
                        asset_id,
                        path_idx: idx,
                    });
                }
            }
        }
    }
    None
}

/// Removes `path_idx` from the vector asset with the given id. Returns
/// `true` if the asset is now empty (so the caller should also drop the
/// asset + its placement).
fn remove_path_from_asset(project: &mut ProjectV2, asset_id: u16, path_idx: usize) -> bool {
    if let Some(Asset::Vector(v)) = project.assets.iter_mut().find(|a| a.id() == asset_id) {
        if path_idx < v.paths.len() {
            v.paths.remove(path_idx);
        }
        return v.paths.is_empty();
    }
    false
}

fn point_in_polygon(poly: &[Vec2], p: Vec2) -> bool {
    // Standard ray-casting (Jordan). Handles concave polygons; we don't
    // care about perfect edge semantics; a 1-pixel error is invisible
    // for an eraser hit.
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let pi = poly[i];
        let pj = poly[j];
        let crosses = (pi.y > p.y) != (pj.y > p.y)
            && p.x < (pj.x - pi.x) * (p.y - pi.y) / (pj.y - pi.y + 1e-9) + pi.x;
        if crosses {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn nearest_segment_distance(pts: &[Vec2], p: Vec2) -> f32 {
    let mut min_d = f32::INFINITY;
    for w in pts.windows(2) {
        let a = w[0];
        let b = w[1];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len2 = dx * dx + dy * dy;
        let t = if len2 < 1e-6 {
            0.0
        } else {
            (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
        };
        let cx = a.x + t * dx;
        let cy = a.y + t * dy;
        let d = ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt();
        if d < min_d {
            min_d = d;
        }
    }
    min_d
}

/// Draw the exact static nib footprint before and during a brush gesture.
/// The ring sits just outside the painted area, so it never hides the edge.
fn draw_brush_cursor(app: &EditorApp, painter: &Painter, view: &StageView) {
    if app.session.current_tool != Tool::Brush {
        return;
    }
    let Some(center) = painter.ctx().pointer_hover_pos() else {
        return;
    };
    if !painter.clip_rect().contains(center) {
        return;
    }
    match app.session.brush_mode {
        crate::advanced_brush::BrushMode::Classic => {
            let size_px = brush_cursor_radius_px(app.session.brush, view.scale) * 2.0 + 2.5;
            draw_nib_cursor_outline(painter, center, app.session.brush.nib, size_px);
        }
        crate::advanced_brush::BrushMode::Advanced => {
            let settings = advanced_brush_settings_for_view(app, view.scale);
            let major = settings.size * view.scale * 0.5 + 1.25;
            let minor = major * settings.roundness;
            let angle = settings.angle_degrees.to_radians();
            let cos_a = angle.cos();
            let sin_a = angle.sin();
            let points = (0..=40)
                .map(|index| {
                    let phase = std::f32::consts::TAU * index as f32 / 40.0;
                    let x = phase.cos() * major;
                    let y = phase.sin() * minor;
                    pos2(
                        center.x + x * cos_a - y * sin_a,
                        center.y + x * sin_a + y * cos_a,
                    )
                })
                .collect();
            painter.add(Shape::Path(PathShape {
                points,
                closed: true,
                fill: Color32::TRANSPARENT,
                stroke: Stroke::new(1.0_f32, Color32::from_white_alpha(210)),
            }));
        }
    }
}

/// Draw the active eraser footprint. When brush/eraser sync is enabled this
/// deliberately mirrors the selected nib instead of lying with a round cursor.
fn draw_eraser_cursor(app: &EditorApp, painter: &Painter, view: &StageView) {
    if app.session.current_tool != Tool::Eraser {
        return;
    }
    let Some(center) = painter.ctx().pointer_hover_pos() else {
        return;
    };
    if !painter.clip_rect().contains(center) {
        return;
    }
    let settings = eraser_settings(app, view.scale);
    draw_nib_cursor_outline(
        painter,
        center,
        settings.nib,
        settings.size * view.scale + 2.0,
    );
}

fn draw_nib_cursor_outline(
    painter: &Painter,
    center: Pos2,
    nib: crate::brush::BrushNib,
    size_px: f32,
) {
    let points: Vec<Pos2> = crate::brush::nib_outline(nib, size_px, Vec2::new(center.x, center.y))
        .into_iter()
        .map(|point| pos2(point.x, point.y))
        .collect();
    if points.len() < 3 {
        return;
    }
    painter.add(Shape::Path(PathShape {
        points: points.clone(),
        closed: true,
        fill: Color32::TRANSPARENT,
        stroke: Stroke::new(2.5_f32, Color32::from_black_alpha(210)),
    }));
    painter.add(Shape::Path(PathShape {
        points,
        closed: true,
        fill: Color32::TRANSPARENT,
        stroke: Stroke::new(1.0_f32, Color32::WHITE),
    }));
}
fn active_custom_transform_cursor(
    app: &EditorApp,
    response: &Response,
    view: &StageView,
) -> Option<CustomTransformCursor> {
    match app.session.tool_state {
        ToolState::DraggingRawRotate { .. } | ToolState::DraggingPlacementRotate { .. } => {
            return Some(CustomTransformCursor::Rotate);
        }
        ToolState::DraggingRawSkew { edge, .. } | ToolState::DraggingPlacementSkew { edge, .. } => {
            return Some(CustomTransformCursor::Skew(edge));
        }
        _ => {}
    }

    if app.session.current_tool != Tool::Select {
        return None;
    }
    let cursor_screen = response.hover_pos()?;
    selected_transform_hit(app, view, cursor_screen).and_then(custom_transform_cursor_for_hit)
}

fn draw_transform_cursor(
    app: &EditorApp,
    response: &Response,
    painter: &Painter,
    view: &StageView,
) {
    let Some(cursor_kind) = active_custom_transform_cursor(app, response, view) else {
        return;
    };
    let Some(center) = painter.ctx().pointer_hover_pos() else {
        return;
    };
    if !painter.clip_rect().contains(center) {
        return;
    }

    let accent = selection_color(app);
    match cursor_kind {
        CustomTransformCursor::Rotate => draw_rotate_cursor(painter, center, accent),
        CustomTransformCursor::Skew(edge) => draw_skew_cursor(painter, center, edge, accent),
    }
}

fn draw_rotate_cursor(painter: &Painter, center: Pos2, accent: Color32) {
    let radius = 8.0;
    let start_angle = -0.65 * std::f32::consts::PI;
    let end_angle = 0.95 * std::f32::consts::PI;
    let steps = 22;
    let points: Vec<Pos2> = (0..=steps)
        .map(|step| {
            let t = step as f32 / steps as f32;
            let angle = start_angle + (end_angle - start_angle) * t;
            center + egui::vec2(angle.cos() * radius, angle.sin() * radius)
        })
        .collect();
    draw_cursor_polyline(painter, &points, accent);
    if points.len() >= 2 {
        draw_cursor_arrowhead(painter, points[0], points[0] - points[1], accent);
        let last = points.len() - 1;
        draw_cursor_arrowhead(
            painter,
            points[last],
            points[last] - points[last - 1],
            accent,
        );
    }
}

fn draw_skew_cursor(painter: &Painter, center: Pos2, edge: TransformEdge, accent: Color32) {
    let (start, end) = match edge {
        TransformEdge::Top => (
            center + egui::vec2(-8.0, 3.0),
            center + egui::vec2(8.0, -3.0),
        ),
        TransformEdge::Bottom => (
            center + egui::vec2(-8.0, -3.0),
            center + egui::vec2(8.0, 3.0),
        ),
        TransformEdge::Left => (
            center + egui::vec2(3.0, -8.0),
            center + egui::vec2(-3.0, 8.0),
        ),
        TransformEdge::Right => (
            center + egui::vec2(-3.0, -8.0),
            center + egui::vec2(3.0, 8.0),
        ),
    };
    draw_cursor_segment(painter, start, end, accent);
    draw_cursor_arrowhead(painter, start, start - end, accent);
    draw_cursor_arrowhead(painter, end, end - start, accent);
}

fn draw_cursor_polyline(painter: &Painter, points: &[Pos2], accent: Color32) {
    if points.len() < 2 {
        return;
    }
    for stroke in cursor_strokes(accent) {
        painter.add(Shape::line(points.to_vec(), stroke));
    }
}

fn draw_cursor_segment(painter: &Painter, start: Pos2, end: Pos2, accent: Color32) {
    for stroke in cursor_strokes(accent) {
        painter.line_segment([start, end], stroke);
    }
}

fn draw_cursor_arrowhead(painter: &Painter, tip: Pos2, direction: egui::Vec2, accent: Color32) {
    if direction.length_sq() <= 1.0e-6 {
        return;
    }
    let direction = direction.normalized();
    let perpendicular = egui::vec2(-direction.y, direction.x);
    let base = tip - direction * 4.6;
    let left = base + perpendicular * 2.8;
    let right = base - perpendicular * 2.8;
    draw_cursor_segment(painter, tip, left, accent);
    draw_cursor_segment(painter, tip, right, accent);
}

fn cursor_strokes(accent: Color32) -> [Stroke; 3] {
    [
        Stroke::new(4.2_f32, Color32::from_black_alpha(235)),
        Stroke::new(2.5_f32, Color32::WHITE),
        Stroke::new(1.15_f32, accent),
    ]
}

// ---------------- Selection tool (move placement) ----------------

fn select(app: &mut EditorApp, response: &Response, cursor: Option<Vec2>, view: &StageView) {
    // Double-click a q0rg-instance placement to enter that q0rg (Flash semantics).
    if response.double_clicked_by(PointerButton::Primary) {
        if let Some(p) = cursor {
            if let Some((layer_id, idx)) = hit_test_placement(
                &app.state.project,
                app.session.current_q0rg_id,
                app.session.current_frame,
                p,
            ) {
                if let Some(Target::Q0rg(child)) = app
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q| q.q0rg_id == app.session.current_q0rg_id)
                    .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
                    .and_then(|l| l.placements.get(idx))
                    .map(|pl| pl.target)
                {
                    app.queue(crate::app::Action::EnterQ0rg(child));
                    return;
                }
            }
        }
    }

    // Raw graphics never become a placement-sized object selection. A fill
    // click selects only the connected filled region under the pointer
    // (including its hole contours); an open stroke selects that one path.
    // q0rg, bitmap and transformed vector instances remain display objects.
    if response.clicked_by(PointerButton::Primary) {
        if let Some(p) = cursor {
            if let Some(hit) = hit_test_raw_selection(
                &app.state.project,
                app.session.current_q0rg_id,
                app.session.current_frame,
                p,
            ) {
                app.session.selection = match hit {
                    RawSelectionHit::Fill(refs) => {
                        app.session.status = "Fill region selected".to_string();
                        Selection::Paths(refs)
                    }
                    RawSelectionHit::Path(path) => {
                        app.session.status = "Contour selected".to_string();
                        Selection::Path {
                            q0rg_id: path.q0rg_id,
                            layer_id: path.layer_id,
                            placement_idx: path.placement_idx,
                            path_idx: path.path_idx,
                        }
                    }
                };
                return;
            }
            match hit_test_selectable_placement(
                &app.state.project,
                app.session.current_q0rg_id,
                app.session.current_frame,
                p,
            ) {
                Some((layer_id, idx)) => {
                    app.session.selection = Selection::Placement {
                        q0rg_id: app.session.current_q0rg_id,
                        layer_id,
                        placement_idx: idx,
                    };
                    app.session.status = "Object selected".to_string();
                }
                None => {
                    app.session.selection = Selection::None;
                }
            }
        }
    }

    // Drag start priority: if a placement is selected, first check transform
    // handles around its bbox. If a supported handle is hit, start axis scaling.
    // Otherwise hit-test the placement body for move, or clear the selection.
    if response.drag_started_by(PointerButton::Primary) {
        if let Some(current_cursor) = cursor {
            // egui reports drag_started after crossing its drag threshold. Use
            // the reconstructed press origin so tiny handles remain grabbable.
            let cursor_screen = response
                .interact_pointer_pos()
                .map(|screen| screen - response.drag_delta());
            let p = cursor_screen
                .map(|screen| screen_to_stage(screen, view))
                .unwrap_or(current_cursor);
            if let Some(screen_pos) = cursor_screen {
                if selected_pivot_hit(app, view, screen_pos) {
                    let selection = app.session.selection.clone();
                    set_selection_transform_pivot(app, p);
                    app.session.tool_state = ToolState::DraggingTransformPivot { selection };
                    app.session.status = "Moving transform anchor".to_string();
                    return;
                }
            }
            let group_selection = match &app.session.selection {
                Selection::RawArea { objects, .. } if !objects.is_empty() => {
                    Some(app.session.selection.clone())
                }
                Selection::Mixed { .. } | Selection::Multi(_) => {
                    Some(app.session.selection.clone())
                }
                _ => None,
            };
            if let (Some(selection), Some(screen_pos)) = (group_selection, cursor_screen) {
                if let Some(bounds) = selection_transform_bounds(app) {
                    if let Some(hit) = hit_test_raw_transform(bounds, view, screen_pos) {
                        if begin_group_transform(app, selection, GroupTransformIntent::Hit(hit), p)
                        {
                            return;
                        }
                    }
                }
            }
            if let (Some(refs), Some(screen_pos)) = (
                selection_raw_path_refs(&app.session.selection),
                cursor_screen,
            ) {
                if let Some(bounds) = raw_path_refs_ui_bounds(&app.state.project, &refs) {
                    if let Some(hit) = hit_test_raw_transform(bounds, view, screen_pos) {
                        let started = match hit {
                            TransformHit::Scale(handle) => {
                                begin_scaling_raw_paths(app, refs, handle, bounds)
                            }
                            TransformHit::Rotate(_) => {
                                let center = selection_transform_pivot(app).unwrap_or_else(|| {
                                    Vec2::new(
                                        (bounds.0 + bounds.2) * 0.5,
                                        (bounds.1 + bounds.3) * 0.5,
                                    )
                                });
                                begin_rotating_raw_paths(app, refs, bounds, center, p)
                            }
                            TransformHit::Skew(edge) => {
                                begin_skewing_raw_paths(app, refs, edge, bounds, p)
                            }
                        };
                        if started {
                            return;
                        }
                    }
                }
            }
            if let (
                Selection::RawArea {
                    placements,
                    objects,
                    bounds_min,
                    bounds_max,
                },
                Some(screen_pos),
            ) = (app.session.selection.clone(), cursor_screen)
            {
                if objects.is_empty() {
                    let bounds = (bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y);
                    if let Some(hit) = hit_test_raw_transform(bounds, view, screen_pos) {
                        let pivot = selection_transform_pivot(app).unwrap_or_else(|| {
                            Vec2::new((bounds.0 + bounds.2) * 0.5, (bounds.1 + bounds.3) * 0.5)
                        });
                        if begin_transforming_raw_area(app, &placements, bounds, pivot, hit, p) {
                            return;
                        }
                    }
                }
            }
            let group_selection = match &app.session.selection {
                Selection::RawArea { objects, .. } if !objects.is_empty() => {
                    Some(app.session.selection.clone())
                }
                Selection::Mixed { .. } | Selection::Multi(_) => {
                    Some(app.session.selection.clone())
                }
                _ => None,
            };
            if let Some(selection) = group_selection {
                if let Some(bounds) = selection_transform_bounds(app) {
                    if p.x >= bounds.0
                        && p.x <= bounds.2
                        && p.y >= bounds.1
                        && p.y <= bounds.3
                        && begin_group_transform(app, selection, GroupTransformIntent::Move, p)
                    {
                        return;
                    }
                }
            }
            if let Selection::RawArea {
                placements,
                objects,
                bounds_min,
                bounds_max,
            } = app.session.selection.clone()
            {
                if objects.is_empty()
                    && raw_area_selection_contains_point(
                        &app.state.project,
                        &placements,
                        bounds_min,
                        bounds_max,
                        p,
                    )
                {
                    let start_pivot = selection_transform_pivot(app);
                    let refs = cut_raw_areas_for_drag(
                        app,
                        &placements,
                        (bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y),
                    );
                    let start_paths: Vec<VPath> = refs
                        .iter()
                        .filter_map(|r| {
                            raw_path_clone(
                                &app.state.project,
                                r.q0rg_id,
                                r.layer_id,
                                r.placement_idx,
                                r.path_idx,
                            )
                        })
                        .collect();
                    if !refs.is_empty() && start_paths.len() == refs.len() {
                        app.session.selection =
                            selection_from_group_parts(refs.clone(), Vec::new());
                        if let Some(pivot) = start_pivot {
                            set_selection_transform_pivot(app, pivot);
                        }
                        let start_appearances =
                            capture_whole_asset_appearances_for_raw_refs(&app.state.project, &refs);
                        app.session.tool_state = ToolState::DraggingPaths {
                            refs,
                            start_cursor: p,
                            start_paths,
                            start_appearances,
                            start_pivot,
                        };
                        app.session.status = "Moving selected fill".to_string();
                        return;
                    }
                }
            }
            if let Some(refs) = selection_raw_path_refs(&app.session.selection) {
                if point_hits_selected_raw_paths(&app.state.project, &refs, p)
                    && begin_dragging_raw_paths(app, refs, p, "Moving selected raw graphics")
                {
                    return;
                }
            }
            // Existing V-area selection: dragging inside its marquee moves
            // only the selected boundary points and reshapes the raw fill.
            if let Selection::PathPoints {
                path,
                anchor_indices,
                bounds_min,
                bounds_max,
            } = app.session.selection.clone()
            {
                if p.x >= bounds_min.x
                    && p.x <= bounds_max.x
                    && p.y >= bounds_min.y
                    && p.y <= bounds_max.y
                {
                    app.history.snapshot(&app.state.project);
                    let Some(mut mapped) = prepare_raw_path_refs_for_edit(
                        &mut app.state.project,
                        &[path],
                        app.session.current_frame,
                    ) else {
                        return;
                    };
                    let mapped_path = mapped.remove(0);
                    if let Some(start_path) = raw_path_clone(
                        &app.state.project,
                        mapped_path.q0rg_id,
                        mapped_path.layer_id,
                        mapped_path.placement_idx,
                        mapped_path.path_idx,
                    ) {
                        app.session.selection = Selection::PathPoints {
                            path: mapped_path,
                            anchor_indices: anchor_indices.clone(),
                            bounds_min,
                            bounds_max,
                        };
                        app.session.tool_state = ToolState::DraggingPathPoints {
                            path: mapped_path,
                            anchor_indices,
                            start_cursor: p,
                            start_path,
                            start_bounds_min: bounds_min,
                            start_bounds_max: bounds_max,
                        };
                        app.session.status = "Reshaping fill selection".to_string();
                        return;
                    }
                }
            }
            // 1. Try handle hit-test against current selection
            if let (
                Selection::Placement {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                },
                Some(screen_pos),
            ) = (app.session.selection.clone(), cursor_screen)
            {
                if let Some((world_bbox, local_bbox, start_t)) = drag_start_data(
                    &app.state.project,
                    q0rg_id,
                    layer_id,
                    placement_idx,
                    app.session.current_frame,
                ) {
                    if let Some(hit) =
                        hit_test_placement_transform(local_bbox, start_t, view, screen_pos)
                    {
                        let transform_pivot = selection_transform_pivot(app);
                        let pivot_local = transform_pivot.and_then(|point| {
                            Affine::from_transform(start_t)
                                .inverse()
                                .map(|inverse| inverse.apply(point))
                        });
                        app.history.snapshot(&app.state.project);
                        let Some(placement_idx) = materialize_placement_keyframe_for_edit(
                            &mut app.state.project,
                            q0rg_id,
                            layer_id,
                            placement_idx,
                            app.session.current_frame,
                        ) else {
                            return;
                        };
                        app.session.selection = Selection::Placement {
                            q0rg_id,
                            layer_id,
                            placement_idx,
                        };
                        if let Some(point) = transform_pivot {
                            set_selection_transform_pivot(app, point);
                        }
                        match hit {
                            TransformHit::Scale(handle) => {
                                app.session.tool_state = ToolState::DraggingHandle {
                                    q0rg_id,
                                    layer_id,
                                    placement_idx,
                                    handle,
                                    start_transform: start_t,
                                    start_local_bbox: local_bbox,
                                    start_world_bbox: world_bbox,
                                    pivot_local,
                                };
                                app.session.status = "Resizing selection".to_string();
                            }
                            TransformHit::Rotate(_) => {
                                let default_center_local = Vec2::new(
                                    (local_bbox.0 + local_bbox.2) * 0.5,
                                    (local_bbox.1 + local_bbox.3) * 0.5,
                                );
                                let affine = Affine::from_transform(start_t);
                                let center_world = transform_pivot
                                    .unwrap_or_else(|| affine.apply(default_center_local));
                                let center_local = affine
                                    .inverse()
                                    .map(|inverse| inverse.apply(center_world))
                                    .unwrap_or(default_center_local);
                                app.session.tool_state = ToolState::DraggingPlacementRotate {
                                    q0rg_id,
                                    layer_id,
                                    placement_idx,
                                    start_transform: start_t,
                                    center_local,
                                    center_world,
                                    start_angle: (p.y - center_world.y).atan2(p.x - center_world.x),
                                };
                                app.session.status = "Rotating selection".to_string();
                            }
                            TransformHit::Skew(edge) => {
                                let Some(inverse) = Affine::from_transform(start_t).inverse()
                                else {
                                    return;
                                };
                                app.session.tool_state = ToolState::DraggingPlacementSkew {
                                    q0rg_id,
                                    layer_id,
                                    placement_idx,
                                    edge,
                                    start_transform: start_t,
                                    start_local_bbox: local_bbox,
                                    start_cursor_local: inverse.apply(p),
                                    pivot_local,
                                };
                                app.session.status = "Skewing selection".to_string();
                            }
                        }
                        return;
                    }
                }
            }
            // 2. Raw fill dragging moves only the connected region under the
            // pointer. Open strokes still move as individual contours.
            if let Some(hit) = hit_test_raw_selection(
                &app.state.project,
                app.session.current_q0rg_id,
                app.session.current_frame,
                p,
            ) {
                match hit {
                    RawSelectionHit::Fill(refs) => {
                        if begin_dragging_raw_paths(app, refs, p, "Moving fill region") {
                            return;
                        }
                    }
                    RawSelectionHit::Path(path) => {
                        if begin_dragging_raw_paths(app, vec![path], p, "Moving contour") {
                            return;
                        }
                    }
                }
            }

            // 3. A display-object body hit selects and moves the placement.
            // Raw graphics are deliberately excluded here.
            if let Some((layer_id, idx)) = hit_test_selectable_placement(
                &app.state.project,
                app.session.current_q0rg_id,
                app.session.current_frame,
                p,
            ) {
                let q0rg_id = app.session.current_q0rg_id;
                let previous_selection = app.session.selection.clone();
                let custom_pivot = app
                    .session
                    .transform_pivot
                    .as_ref()
                    .filter(|pivot| pivot.selection == previous_selection)
                    .filter(|_| {
                        matches!(
                            previous_selection,
                            Selection::Placement {
                                q0rg_id: selected_q0rg,
                                layer_id: selected_layer,
                                placement_idx: selected_index,
                            } if selected_q0rg == q0rg_id
                                && selected_layer == layer_id
                                && selected_index == idx
                        )
                    })
                    .map(|pivot| pivot.point);
                app.session.selection = Selection::Placement {
                    q0rg_id,
                    layer_id,
                    placement_idx: idx,
                };
                if let Some(t) = placement_transform(
                    &app.state.project,
                    q0rg_id,
                    layer_id,
                    idx,
                    app.session.current_frame,
                ) {
                    app.history.snapshot(&app.state.project);
                    let Some(placement_idx) = materialize_placement_keyframe_for_edit(
                        &mut app.state.project,
                        q0rg_id,
                        layer_id,
                        idx,
                        app.session.current_frame,
                    ) else {
                        return;
                    };
                    app.session.selection = Selection::Placement {
                        q0rg_id,
                        layer_id,
                        placement_idx,
                    };
                    if let Some(pivot) = custom_pivot {
                        set_selection_transform_pivot(app, pivot);
                    }
                    let cursor_offset = Vec2::new(t.tx - p.x, t.ty - p.y);
                    let pivot_cursor_offset =
                        custom_pivot.map(|pivot| Vec2::new(pivot.x - p.x, pivot.y - p.y));
                    app.session.tool_state = ToolState::DraggingPlacement {
                        q0rg_id,
                        layer_id,
                        placement_idx,
                        cursor_offset,
                        pivot_cursor_offset,
                    };
                    app.session.status = "Object keyframed and selected".to_string();
                }
            } else {
                // 4. An empty stage starts a rubber-band marquee. Selection is
                // cleared so the user starts from a clean state, and the final
                // selection lands on drag_stopped.
                app.session.selection = Selection::None;
                app.session.tool_state = ToolState::Marquee { start: p };
                app.session.status = "Drag to select".to_string();
            }
        }
    }

    if response.dragged_by(PointerButton::Primary) {
        match app.session.tool_state.clone() {
            ToolState::DraggingPlacement {
                q0rg_id,
                layer_id,
                placement_idx,
                cursor_offset,
                pivot_cursor_offset,
            } => {
                if let Some(p) = cursor {
                    let mut moved = false;
                    if let Some(pl) = placement_mut(app, q0rg_id, layer_id, placement_idx) {
                        pl.transform.tx = p.x + cursor_offset.x;
                        pl.transform.ty = p.y + cursor_offset.y;
                        moved = true;
                    }
                    if moved {
                        app.state.dirty = true;
                        if let Some(offset) = pivot_cursor_offset {
                            set_selection_transform_pivot(
                                app,
                                Vec2::new(p.x + offset.x, p.y + offset.y),
                            );
                        }
                    }
                }
            }
            ToolState::DraggingPath {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
                start_cursor,
                start_path,
            } => {
                if let Some(p) = cursor {
                    let delta = Vec2::new(p.x - start_cursor.x, p.y - start_cursor.y);
                    if replace_raw_path_translated(
                        &mut app.state.project,
                        q0rg_id,
                        layer_id,
                        placement_idx,
                        path_idx,
                        &start_path,
                        delta,
                    ) {
                        app.state.dirty = true;
                    }
                }
            }
            ToolState::DraggingPaths {
                refs,
                start_cursor,
                start_paths,
                start_appearances,
                start_pivot,
            } => {
                if let Some(p) = cursor {
                    let delta = Vec2::new(p.x - start_cursor.x, p.y - start_cursor.y);
                    let mut changed = false;
                    for (r, source) in refs.iter().zip(start_paths.iter()) {
                        changed |= replace_raw_path_translated(
                            &mut app.state.project,
                            r.q0rg_id,
                            r.layer_id,
                            r.placement_idx,
                            r.path_idx,
                            source,
                            delta,
                        );
                    }
                    changed |= translate_captured_appearances(
                        &mut app.state.project,
                        &start_appearances,
                        delta,
                    );
                    if changed {
                        app.state.dirty = true;
                        if let Some(pivot) = start_pivot {
                            set_selection_transform_pivot(
                                app,
                                Vec2::new(pivot.x + delta.x, pivot.y + delta.y),
                            );
                        }
                    }
                }
            }
            ToolState::DraggingRawHandle {
                refs,
                start_paths,
                start_appearances,
                handle,
                start_bounds,
                start_pivot,
            } => {
                if let Some(p) = cursor {
                    if let Some(transform) = group_transform_affine(
                        GroupTransformOperation::Scale {
                            handle,
                            start_bounds,
                        },
                        p,
                    ) {
                        if apply_raw_affine_snapshot(
                            &mut app.state.project,
                            &refs,
                            &start_paths,
                            &start_appearances,
                            transform,
                        ) {
                            app.state.dirty = true;
                            set_selection_transform_pivot(app, transform.apply(start_pivot));
                        }
                    }
                }
            }
            ToolState::DraggingRawRotate {
                refs,
                start_paths,
                start_appearances,
                center,
                start_angle,
            } => {
                if let Some(p) = cursor {
                    if let Some(transform) = group_transform_affine(
                        GroupTransformOperation::Rotate {
                            center,
                            start_angle,
                        },
                        p,
                    ) {
                        if apply_raw_affine_snapshot(
                            &mut app.state.project,
                            &refs,
                            &start_paths,
                            &start_appearances,
                            transform,
                        ) {
                            app.state.dirty = true;
                        }
                    }
                }
            }
            ToolState::DraggingRawSkew {
                refs,
                start_paths,
                start_appearances,
                edge,
                start_bounds,
                start_cursor,
                start_pivot,
            } => {
                if let Some(p) = cursor {
                    if let Some(transform) = group_transform_affine(
                        GroupTransformOperation::Skew {
                            edge,
                            start_bounds,
                            start_cursor,
                        },
                        p,
                    ) {
                        if apply_raw_affine_snapshot(
                            &mut app.state.project,
                            &refs,
                            &start_paths,
                            &start_appearances,
                            transform,
                        ) {
                            app.state.dirty = true;
                            set_selection_transform_pivot(app, transform.apply(start_pivot));
                        }
                    }
                }
            }
            ToolState::DraggingPathPoints {
                path,
                anchor_indices,
                start_cursor,
                start_path,
                start_bounds_min,
                start_bounds_max,
            } => {
                if let Some(p) = cursor {
                    let delta = Vec2::new(p.x - start_cursor.x, p.y - start_cursor.y);
                    if replace_raw_path_points_translated(
                        &mut app.state.project,
                        path,
                        &anchor_indices,
                        &start_path,
                        delta,
                    ) {
                        app.state.dirty = true;
                        app.session.selection = Selection::PathPoints {
                            path,
                            anchor_indices,
                            bounds_min: Vec2::new(
                                start_bounds_min.x + delta.x,
                                start_bounds_min.y + delta.y,
                            ),
                            bounds_max: Vec2::new(
                                start_bounds_max.x + delta.x,
                                start_bounds_max.y + delta.y,
                            ),
                        };
                    }
                }
            }
            ToolState::DraggingHandle {
                q0rg_id,
                layer_id,
                placement_idx,
                handle,
                start_transform,
                start_local_bbox,
                start_world_bbox,
                pivot_local,
            } => {
                if let Some(p) = cursor {
                    let mut next_pivot = None;
                    let mut changed = false;
                    if let Some(pl) = placement_mut(app, q0rg_id, layer_id, placement_idx) {
                        if apply_handle_drag(
                            &mut pl.transform,
                            start_transform,
                            start_local_bbox,
                            start_world_bbox,
                            handle,
                            p,
                        ) {
                            changed = true;
                            next_pivot = pivot_local
                                .map(|local| Affine::from_transform(pl.transform).apply(local));
                        }
                    }
                    if changed {
                        app.state.dirty = true;
                        if let Some(pivot) = next_pivot {
                            set_selection_transform_pivot(app, pivot);
                        }
                    }
                }
            }
            ToolState::DraggingPlacementRotate {
                q0rg_id,
                layer_id,
                placement_idx,
                start_transform,
                center_local,
                center_world,
                start_angle,
            } => {
                if let Some(p) = cursor {
                    let delta = (p.y - center_world.y).atan2(p.x - center_world.x) - start_angle;
                    if let Some(pl) = placement_mut(app, q0rg_id, layer_id, placement_idx) {
                        pl.transform = rotate_placement_about_local_point(
                            start_transform,
                            center_local,
                            center_world,
                            delta,
                        );
                        app.state.dirty = true;
                    }
                }
            }
            ToolState::DraggingPlacementSkew {
                q0rg_id,
                layer_id,
                placement_idx,
                edge,
                start_transform,
                start_local_bbox,
                start_cursor_local,
                pivot_local,
            } => {
                if let Some(p) = cursor {
                    if let Some(next) = skew_placement_from_cursor(
                        start_transform,
                        start_local_bbox,
                        edge,
                        start_cursor_local,
                        p,
                    ) {
                        let mut changed = false;
                        if let Some(pl) = placement_mut(app, q0rg_id, layer_id, placement_idx) {
                            pl.transform = next;
                            changed = true;
                        }
                        if changed {
                            app.state.dirty = true;
                            if let Some(local) = pivot_local {
                                set_selection_transform_pivot(
                                    app,
                                    Affine::from_transform(next).apply(local),
                                );
                            }
                        }
                    }
                }
            }
            ToolState::DraggingGroup {
                refs,
                start_paths,
                start_appearances,
                objects,
                start_transforms,
                operation,
                start_pivot,
            } => {
                if let Some(p) = cursor {
                    apply_group_transform(
                        app,
                        GroupTransformData {
                            refs: &refs,
                            start_paths: &start_paths,
                            start_appearances: &start_appearances,
                            objects: &objects,
                            start_transforms: &start_transforms,
                            operation,
                            start_pivot,
                        },
                        p,
                    );
                }
            }
            ToolState::DraggingTransformPivot { selection }
                if selection == app.session.selection =>
            {
                if let Some(p) = cursor {
                    set_selection_transform_pivot(app, p);
                }
            }
            _ => {}
        }
    }

    if response.drag_stopped_by(PointerButton::Primary) {
        // Marquee finalisation: collect every placement on the current q0rg
        // whose bbox intersects the rubber-band rect.
        if let ToolState::Marquee { start } = app.session.tool_state.clone() {
            if let Some(end) = cursor {
                let min = Vec2::new(start.x.min(end.x), start.y.min(end.y));
                let max = Vec2::new(start.x.max(end.x), start.y.max(end.y));
                let selection = marquee_selection(
                    &app.state.project,
                    app.session.current_q0rg_id,
                    app.session.current_frame,
                    (min.x, min.y, max.x, max.y),
                );
                app.session.status = match &selection {
                    Selection::RawArea {
                        placements,
                        objects,
                        ..
                    } => format!(
                        "Selected fill area across {} drawing(s) and {} object(s)",
                        placements.len(),
                        objects.len()
                    ),
                    Selection::Paths(items) => format!("Selected {} contour(s)", items.len()),
                    Selection::Path { .. } => "Selected contour".to_string(),
                    Selection::Multi(items) => format!("Selected {} object(s)", items.len()),
                    Selection::Placement { .. } => "Selected object".to_string(),
                    _ => "Selection cleared".to_string(),
                };
                app.session.selection = selection;
            }
            app.session.tool_state = ToolState::Idle;
            return;
        }
        let finished_state = app.session.tool_state.clone();
        let raw_layers = raw_edit_layers(&finished_state);
        let finished_drag = matches!(
            finished_state,
            ToolState::DraggingPlacement { .. }
                | ToolState::DraggingPath { .. }
                | ToolState::DraggingPaths { .. }
                | ToolState::DraggingRawHandle { .. }
                | ToolState::DraggingRawRotate { .. }
                | ToolState::DraggingRawSkew { .. }
                | ToolState::DraggingPathPoints { .. }
                | ToolState::DraggingHandle { .. }
                | ToolState::DraggingPlacementRotate { .. }
                | ToolState::DraggingPlacementSkew { .. }
                | ToolState::DraggingGroup { .. }
                | ToolState::DraggingTransformPivot { .. }
        );
        let mut merged = false;
        for (q0rg_id, layer_id) in raw_layers {
            merged |= crate::brush::merge_touching_raw_fills_after_edit(
                &mut app.state.project,
                q0rg_id,
                layer_id,
                app.session.current_frame,
            );
        }
        if merged {
            app.session.selection = cursor
                .and_then(|point| {
                    selection_at_point_pub(
                        &app.state.project,
                        app.session.current_q0rg_id,
                        app.session.current_frame,
                        point,
                    )
                })
                .unwrap_or(Selection::None);
            app.session.status = "Touching raw fills merged".to_string();
            app.state.dirty = true;
        }
        if finished_drag {
            app.session.tool_state = ToolState::Idle;
        }
    }
}

fn raw_edit_layers(state: &ToolState) -> std::collections::BTreeSet<(u16, u16)> {
    let mut layers = std::collections::BTreeSet::new();
    match state {
        ToolState::DraggingPath {
            q0rg_id, layer_id, ..
        } => {
            layers.insert((*q0rg_id, *layer_id));
        }
        ToolState::DraggingPaths { refs, .. }
        | ToolState::DraggingRawHandle { refs, .. }
        | ToolState::DraggingRawRotate { refs, .. }
        | ToolState::DraggingRawSkew { refs, .. }
        | ToolState::DraggingGroup { refs, .. } => {
            layers.extend(
                refs.iter()
                    .map(|reference| (reference.q0rg_id, reference.layer_id)),
            );
        }
        ToolState::DraggingPathPoints { path, .. } => {
            layers.insert((path.q0rg_id, path.layer_id));
        }
        _ => {}
    }
    layers
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawSelectionHit {
    /// One connected filled surface. The refs include every source contour
    /// that contributes to that surface, including hole contours, but exclude
    /// disconnected fills that merely share the same VectorAsset/Placement.
    Fill(Vec<PathRef>),
    /// One open or stroke-only raw path.
    Path(PathRef),
}

/// Hit-test raw graphics with Flash-like semantics: a click inside a fill
/// selects the connected filled region, not the asset/placement that happens
/// to store it. This is intentionally separate from `hit_test_raw_path`, which
/// remains contour-oriented for Subselect and low-level editing tools.
fn hit_test_raw_selection(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    cursor: Vec2,
) -> Option<RawSelectionHit> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    for layer in q.layers.iter().rev() {
        for placement_idx in active_raw_placement_indices(project, layer, frame)
            .into_iter()
            .rev()
        {
            let placement = &layer.placements[placement_idx];
            let Target::Asset(asset_id) = placement.target else {
                continue;
            };
            let Some(Asset::Vector(vector)) =
                project.assets.iter().find(|asset| asset.id() == asset_id)
            else {
                continue;
            };

            if vector.fill.is_some() {
                if let Some(component) =
                    selectable_component_at_cursor(project, asset_id, vector, cursor)
                {
                    let refs: Vec<PathRef> = vector
                        .paths
                        .iter()
                        .enumerate()
                        .filter(|(_, path)| path.closed)
                        .filter_map(|(path_idx, path)| {
                            // Refs stay tied to the source vector component even
                            // when the click lands in its halo. This preserves
                            // independent raw regions while making the material
                            // support participate in normal Select semantics.
                            path.anchors
                                .iter()
                                .any(|anchor| {
                                    polygon_boundary_near_cursor(&component, anchor.point, 0.5)
                                })
                                .then_some(PathRef {
                                    q0rg_id,
                                    layer_id: layer.layer_id,
                                    placement_idx,
                                    path_idx,
                                })
                        })
                        .collect();
                    if !refs.is_empty() {
                        return Some(RawSelectionHit::Fill(refs));
                    }
                }
            }

            let stroke_radius = vector
                .stroke
                .as_ref()
                .map(|stroke| stroke.width.max(1.0) * 0.5 + 3.0)
                .unwrap_or(3.0);
            for (path_idx, path) in vector.paths.iter().enumerate().rev() {
                if path.closed && vector.fill.is_some() {
                    continue;
                }
                let points = flatten_path(path);
                if points.len() >= 2 && nearest_segment_distance(&points, cursor) <= stroke_radius {
                    return Some(RawSelectionHit::Path(PathRef {
                        q0rg_id,
                        layer_id: layer.layer_id,
                        placement_idx,
                        path_idx,
                    }));
                }
            }
        }
    }
    None
}

fn polygon_boundary_near_cursor(polygon: &Polygon<f64>, cursor: Vec2, radius: f32) -> bool {
    let ring_near = |ring: &LineString<f64>| {
        let points: Vec<Vec2> = ring
            .0
            .iter()
            .map(|coord| Vec2::new(coord.x as f32, coord.y as f32))
            .collect();
        points.len() >= 2 && nearest_segment_distance(&points, cursor) <= radius
    };
    ring_near(polygon.exterior()) || polygon.interiors().iter().any(ring_near)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawPathHit {
    layer_id: u16,
    placement_idx: usize,
    path_idx: usize,
}

/// Hit-test the actual raw vector geometry instead of its bounding box.
/// Only identity, non-tweened placements are raw-edit surfaces; transformed
/// placements are display objects and intentionally fall back to object
/// selection.
fn hit_test_raw_path(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    cursor: Vec2,
) -> Option<RawPathHit> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    for layer in q.layers.iter().rev() {
        for placement_idx in active_raw_placement_indices(project, layer, frame)
            .into_iter()
            .rev()
        {
            let placement = &layer.placements[placement_idx];
            let Target::Asset(asset_id) = placement.target else {
                continue;
            };
            let Some(Asset::Vector(vector)) = project.assets.iter().find(|a| a.id() == asset_id)
            else {
                continue;
            };
            let stroke_radius = vector
                .stroke
                .as_ref()
                .map(|stroke| stroke.width.max(1.0) * 0.5 + 3.0)
                .unwrap_or(3.0);
            if vector.fill.is_some() {
                let surface = vector_fill_geometry(vector);
                let point = Point::new(cursor.x as f64, cursor.y as f64);
                let on_boundary = vector
                    .paths
                    .iter()
                    .filter(|path| path.closed)
                    .map(flatten_path)
                    .any(|points| nearest_segment_distance(&points, cursor) <= 2.0);
                if surface.contains(&point) || on_boundary {
                    let dominant_sign = vector
                        .paths
                        .iter()
                        .filter(|path| path.closed)
                        .map(signed_path_area)
                        .max_by(|a, b| a.abs().total_cmp(&b.abs()))
                        .map(f64::signum)
                        .unwrap_or(1.0);
                    let best = vector
                        .paths
                        .iter()
                        .enumerate()
                        .filter(|(_, path)| {
                            path.closed && signed_path_area(path).signum() == dominant_sign
                        })
                        .filter_map(|(path_idx, path)| {
                            let points = flatten_path(path);
                            (point_in_polygon(&points, cursor)
                                || nearest_segment_distance(&points, cursor) <= 2.0)
                                .then_some((path_idx, signed_path_area(path).abs()))
                        })
                        .min_by(|a, b| a.1.total_cmp(&b.1));
                    if let Some((path_idx, _)) = best {
                        return Some(RawPathHit {
                            layer_id: layer.layer_id,
                            placement_idx,
                            path_idx,
                        });
                    }
                }
            }
            for (path_idx, path) in vector.paths.iter().enumerate().rev() {
                let points = flatten_path(path);
                if points.len() < 2 {
                    continue;
                }
                if path.closed && vector.fill.is_some() {
                    continue;
                }
                let hit = nearest_segment_distance(&points, cursor) <= stroke_radius;
                if hit {
                    return Some(RawPathHit {
                        layer_id: layer.layer_id,
                        placement_idx,
                        path_idx,
                    });
                }
            }
        }
    }
    None
}

fn selection_raw_path_refs(selection: &Selection) -> Option<Vec<PathRef>> {
    match selection {
        Selection::Paths(refs) if !refs.is_empty() => Some(refs.clone()),
        Selection::Path {
            q0rg_id,
            layer_id,
            placement_idx,
            path_idx,
        } => Some(vec![PathRef {
            q0rg_id: *q0rg_id,
            layer_id: *layer_id,
            placement_idx: *placement_idx,
            path_idx: *path_idx,
        }]),
        _ => None,
    }
}

pub(crate) fn materialize_raw_paths_as_placements(
    app: &mut EditorApp,
    refs: &[PathRef],
) -> Option<Vec<PlacementRef>> {
    if refs.is_empty() {
        return None;
    }
    let frame = app.session.current_frame;
    let refs = prepare_raw_path_refs_for_edit(&mut app.state.project, refs, frame)?;
    let mut groups: std::collections::BTreeMap<(u16, u16, usize), Vec<usize>> =
        std::collections::BTreeMap::new();
    for reference in &refs {
        groups
            .entry((
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
            ))
            .or_default()
            .push(reference.path_idx);
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by(|left, right| {
        let (left_q0rg, left_layer, left_placement) = left.0;
        let (right_q0rg, right_layer, right_placement) = right.0;
        right_q0rg
            .cmp(&left_q0rg)
            .then(right_layer.cmp(&left_layer))
            .then(right_placement.cmp(&left_placement))
    });

    let mut created_assets: Vec<(u16, u16, u16)> = Vec::new();
    let mut emptied_assets = std::collections::BTreeSet::new();
    for ((q0rg_id, layer_id, placement_idx), mut path_indices) in groups {
        let placement = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?
            .placements
            .get(placement_idx)?;
        if placement.frame != frame
            || placement.transform != Transform2D::IDENTITY
            || !matches!(placement.tween, Tween::None)
        {
            return None;
        }
        let Target::Asset(original_asset_id) = placement.target else {
            return None;
        };

        // Raw editing must never mutate another frame or transformed instance
        // that happens to reference the same vector asset.
        let reference_count = app
            .state
            .project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .flat_map(|layer| &layer.placements)
            .filter(|placement| {
                matches!(placement.target, Target::Asset(id) if id == original_asset_id)
            })
            .count();
        let writable_asset_id = if reference_count > 1 {
            let Asset::Vector(mut clone) = app
                .state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == original_asset_id)?
                .clone()
            else {
                return None;
            };
            let new_id = next_asset_id(&app.state.project);
            clone.asset_id = new_id;
            app.state.project.assets.push(Asset::Vector(clone));
            #[cfg(feature = "appearance-mask-eraser")]
            crate::appearance::clone_asset_appearance(
                &mut app.state.project,
                original_asset_id,
                new_id,
            );
            let target = app
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
                .layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)?
                .placements
                .get_mut(placement_idx)?;
            target.target = Target::Asset(new_id);
            new_id
        } else {
            original_asset_id
        };

        path_indices.sort_unstable();
        path_indices.dedup();
        let (selected_paths, fill, stroke, source_empty, original_paths) = {
            let Asset::Vector(vector) = app
                .state
                .project
                .assets
                .iter_mut()
                .find(|asset| asset.id() == writable_asset_id)?
            else {
                return None;
            };
            let original_paths = vector.paths.clone();
            if path_indices
                .iter()
                .any(|path_idx| *path_idx >= vector.paths.len())
            {
                return None;
            }
            let selected_paths = path_indices
                .iter()
                .map(|path_idx| vector.paths[*path_idx].clone())
                .collect::<Vec<_>>();
            for path_idx in path_indices.into_iter().rev() {
                vector.paths.remove(path_idx);
            }
            (
                selected_paths,
                vector.fill,
                vector.stroke,
                vector.paths.is_empty(),
                original_paths,
            )
        };
        if selected_paths.is_empty() {
            continue;
        }

        let selected_asset_id = next_asset_id(&app.state.project);
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: selected_asset_id,
            paths: selected_paths,
            fill,
            stroke,
        }));
        #[cfg(not(feature = "appearance-mask-eraser"))]
        let _ = &original_paths;
        #[cfg(feature = "appearance-mask-eraser")]
        {
            let selected_geometry = app
                .state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == selected_asset_id)
                .and_then(|asset| match asset {
                    Asset::Vector(vector) => Some(vector_fill_geometry(vector)),
                    _ => None,
                })
                .unwrap_or_else(|| MultiPolygon(Vec::new()));
            let partition = app
                .state
                .project
                .asset_appearances
                .get(&writable_asset_id)
                .map(|appearance| {
                    crate::appearance::material_support(&selected_geometry, appearance.material)
                })
                .unwrap_or(selected_geometry);
            crate::appearance::split_asset_appearance(
                &mut app.state.project,
                writable_asset_id,
                selected_asset_id,
                &original_paths,
                &partition,
                source_empty,
            );
        }
        let layer = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)?;
        if source_empty {
            layer.placements.get_mut(placement_idx)?.target = Target::Asset(selected_asset_id);
            emptied_assets.insert(writable_asset_id);
        } else {
            let insertion = (placement_idx + 1).min(layer.placements.len());
            layer.placements.insert(
                insertion,
                Placement {
                    frame,
                    target: Target::Asset(selected_asset_id),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                },
            );
        }
        created_assets.push((q0rg_id, layer_id, selected_asset_id));
    }

    if !emptied_assets.is_empty() {
        app.state
            .project
            .assets
            .retain(|asset| !emptied_assets.contains(&asset.id()));
        for asset_id in &emptied_assets {
            app.state.project.asset_names.remove(asset_id);
            app.state.project.asset_appearances.remove(asset_id);
        }
    }

    let mut placements = Vec::new();
    for (q0rg_id, layer_id, asset_id) in created_assets {
        let layer = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?;
        let placement_idx = layer.placements.iter().position(|placement| {
            placement.frame == frame
                && placement.transform == Transform2D::IDENTITY
                && matches!(placement.target, Target::Asset(id) if id == asset_id)
        })?;
        placements.push(PlacementRef {
            q0rg_id,
            layer_id,
            placement_idx,
        });
    }
    if placements.is_empty() {
        return None;
    }
    app.state.dirty = true;
    app.textures.invalidate();
    Some(placements)
}

#[cfg(test)]
fn raw_path_refs_bounds(project: &ProjectV2, refs: &[PathRef]) -> Option<(f32, f32, f32, f32)> {
    let mut grouped: std::collections::BTreeMap<(u16, u16, usize), Vec<usize>> =
        std::collections::BTreeMap::new();
    for reference in refs {
        grouped
            .entry((
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
            ))
            .or_default()
            .push(reference.path_idx);
    }

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut found = false;
    for ((q0rg_id, layer_id, placement_idx), mut path_indices) in grouped {
        path_indices.sort_unstable();
        path_indices.dedup();
        let Some(placement) = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| layer.placements.get(placement_idx))
        else {
            continue;
        };
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(Asset::Vector(vector)) =
            project.assets.iter().find(|asset| asset.id() == asset_id)
        else {
            continue;
        };
        let surface = raw_selectable_paths_surface(project, asset_id, vector, &path_indices);
        if let Some(bounds) = surface.bounding_rect() {
            min_x = min_x.min(bounds.min().x as f32);
            min_y = min_y.min(bounds.min().y as f32);
            max_x = max_x.max(bounds.max().x as f32);
            max_y = max_y.max(bounds.max().y as f32);
            found = true;
            continue;
        }
        #[cfg(feature = "appearance-mask-eraser")]
        if project.asset_appearances.contains_key(&asset_id) {
            // An appearance asset with no visible material surface is fully
            // erased. Never resurrect its source paths as selection bounds.
            continue;
        }
        for path_idx in path_indices {
            let Some(path) = vector.paths.get(path_idx) else {
                continue;
            };
            for point in flatten_path(path) {
                if !point.x.is_finite() || !point.y.is_finite() {
                    continue;
                }
                min_x = min_x.min(point.x);
                min_y = min_y.min(point.y);
                max_x = max_x.max(point.x);
                max_y = max_y.max(point.y);
                found = true;
            }
        }
    }
    found.then_some((min_x, min_y, max_x, max_y))
}

fn raw_path_refs_ui_bounds(project: &ProjectV2, refs: &[PathRef]) -> Option<(f32, f32, f32, f32)> {
    let mut grouped: std::collections::BTreeMap<(u16, u16, usize), Vec<usize>> =
        std::collections::BTreeMap::new();
    for reference in refs {
        grouped
            .entry((
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
            ))
            .or_default()
            .push(reference.path_idx);
    }

    let mut result: Option<(f32, f32, f32, f32)> = None;
    for ((q0rg_id, layer_id, placement_idx), mut path_indices) in grouped {
        path_indices.sort_unstable();
        path_indices.dedup();
        let Some(placement) = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| layer.placements.get(placement_idx))
        else {
            continue;
        };
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(Asset::Vector(vector)) =
            project.assets.iter().find(|asset| asset.id() == asset_id)
        else {
            continue;
        };

        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = project.asset_appearances.get(&asset_id) {
            if let Some(bounds) = crate::appearance::fast_visible_material_bounds_for_paths(
                vector,
                Some(appearance),
                &path_indices,
            ) {
                result = union_bounds(result, Some(bounds));
            }
            // Appearance geometry may keep a hidden carrier after erase/split.
            // Never fall back to that carrier for the interactive selection box.
            continue;
        }

        let mut bounds: Option<(f32, f32, f32, f32)> = None;
        for path_idx in path_indices {
            let Some(path) = vector.paths.get(path_idx) else {
                continue;
            };
            for point in flatten_path(path) {
                if !point.x.is_finite() || !point.y.is_finite() {
                    continue;
                }
                bounds = Some(match bounds {
                    Some((min_x, min_y, max_x, max_y)) => (
                        min_x.min(point.x),
                        min_y.min(point.y),
                        max_x.max(point.x),
                        max_y.max(point.y),
                    ),
                    None => (point.x, point.y, point.x, point.y),
                });
            }
        }
        result = union_bounds(result, bounds);
    }
    result
}
fn raw_selection_supports_axis_resize(bounds: (f32, f32, f32, f32)) -> bool {
    let (min_x, min_y, max_x, max_y) = bounds;
    [min_x, min_y, max_x, max_y].into_iter().all(f32::is_finite)
        && max_x - min_x > 1.0e-3
        && max_y - min_y > 1.0e-3
}

fn point_hits_selected_raw_paths(project: &ProjectV2, refs: &[PathRef], point: Vec2) -> bool {
    if refs.is_empty() {
        return false;
    }
    let mut grouped: std::collections::BTreeMap<(u16, u16, usize), Vec<usize>> =
        std::collections::BTreeMap::new();
    for reference in refs {
        grouped
            .entry((
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
            ))
            .or_default()
            .push(reference.path_idx);
    }

    for ((q0rg_id, layer_id, placement_idx), mut path_indices) in grouped {
        path_indices.sort_unstable();
        path_indices.dedup();
        let Some(placement) = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| layer.placements.get(placement_idx))
        else {
            continue;
        };
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(Asset::Vector(vector)) =
            project.assets.iter().find(|asset| asset.id() == asset_id)
        else {
            continue;
        };

        let closed_indices: Vec<usize> = path_indices
            .iter()
            .copied()
            .filter(|index| vector.paths.get(*index).is_some_and(|path| path.closed))
            .collect();
        if vector.fill.is_some() && !closed_indices.is_empty() {
            #[cfg(feature = "appearance-mask-eraser")]
            if let Some(appearance) = project.asset_appearances.get(&asset_id) {
                let Some(tester) = crate::appearance::prepare_visible_material_hit_tester(
                    vector,
                    Some(appearance),
                ) else {
                    continue;
                };
                if tester.contains(point, 2.0) {
                    let all_closed_selected = vector
                        .paths
                        .iter()
                        .enumerate()
                        .filter(|(_, path)| path.closed)
                        .all(|(index, _)| closed_indices.contains(&index));
                    if all_closed_selected {
                        return true;
                    }
                    let subset = VectorAsset {
                        asset_id: vector.asset_id,
                        paths: closed_indices
                            .iter()
                            .filter_map(|index| vector.paths.get(*index).cloned())
                            .collect(),
                        fill: vector.fill,
                        stroke: None,
                    };
                    let subset_surface = vector_fill_geometry(&subset);
                    if let Some(inverse_field) = appearance.field_transform.inverse() {
                        let canonical_subset =
                            crate::appearance::transform_surface(&subset_surface, inverse_field);
                        if crate::appearance::material_support_contains_point(
                            &canonical_subset,
                            appearance.material,
                            tester.canonical_point(point),
                            2.0,
                        ) {
                            return true;
                        }
                    }
                }
                // Appearance owns the visible body. Never fall through to the
                // hidden source vector for cursor/drag hit-testing.
                continue;
            }

            let subset = VectorAsset {
                asset_id: vector.asset_id,
                paths: closed_indices
                    .iter()
                    .filter_map(|index| vector.paths.get(*index).cloned())
                    .collect(),
                fill: vector.fill,
                stroke: None,
            };
            let surface = vector_fill_geometry(&subset);
            let geo_point = Point::new(f64::from(point.x), f64::from(point.y));
            if surface.contains(&geo_point)
                || surface
                    .0
                    .iter()
                    .any(|polygon| polygon_boundary_near_cursor(polygon, point, 2.0))
            {
                return true;
            }
        }

        for path_idx in path_indices {
            let Some(path) = vector.paths.get(path_idx) else {
                continue;
            };
            if path.closed && vector.fill.is_some() {
                continue;
            }
            let points = flatten_path(path);
            let radius = vector
                .stroke
                .as_ref()
                .map(|stroke| stroke.width.max(1.0) * 0.5 + 3.0)
                .unwrap_or(3.0);
            if points.len() >= 2 && nearest_segment_distance(&points, point) <= radius {
                return true;
            }
        }
    }
    false
}

fn raw_area_selection_contains_point(
    project: &ProjectV2,
    placements: &[PlacementRef],
    bounds_min: Vec2,
    bounds_max: Vec2,
    point: Vec2,
) -> bool {
    if point.x < bounds_min.x
        || point.x > bounds_max.x
        || point.y < bounds_min.y
        || point.y > bounds_max.y
    {
        return false;
    }
    placements.iter().any(|reference| {
        let Some(placement) = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.layer_id == reference.layer_id)
            })
            .and_then(|layer| layer.placements.get(reference.placement_idx))
        else {
            return false;
        };
        let Target::Asset(asset_id) = placement.target else {
            return false;
        };
        let Some(Asset::Vector(vector)) =
            project.assets.iter().find(|asset| asset.id() == asset_id)
        else {
            return false;
        };
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = project.asset_appearances.get(&asset_id) {
            return crate::appearance::visible_material_contains_point(
                vector,
                Some(appearance),
                point,
                2.0,
            );
        }
        let surface = vector_fill_geometry(vector);
        let geo_point = Point::new(f64::from(point.x), f64::from(point.y));
        surface.contains(&geo_point)
            || surface
                .0
                .iter()
                .any(|polygon| polygon_boundary_near_cursor(polygon, point, 2.0))
    })
}

fn selected_raw_body_contains_point(app: &EditorApp, point: Vec2) -> bool {
    match &app.session.selection {
        Selection::Path {
            q0rg_id,
            layer_id,
            placement_idx,
            path_idx,
        } => point_hits_selected_raw_paths(
            &app.state.project,
            &[PathRef {
                q0rg_id: *q0rg_id,
                layer_id: *layer_id,
                placement_idx: *placement_idx,
                path_idx: *path_idx,
            }],
            point,
        ),
        Selection::Paths(refs) => point_hits_selected_raw_paths(&app.state.project, refs, point),
        Selection::RawArea {
            placements,
            bounds_min,
            bounds_max,
            ..
        } => raw_area_selection_contains_point(
            &app.state.project,
            placements,
            *bounds_min,
            *bounds_max,
            point,
        ),
        Selection::Mixed { paths, .. } => {
            point_hits_selected_raw_paths(&app.state.project, paths, point)
        }
        _ => false,
    }
}

pub(crate) fn prepare_raw_path_refs_for_edit(
    project: &mut ProjectV2,
    refs: &[PathRef],
    frame: u16,
) -> Option<Vec<PathRef>> {
    let mut mappings = std::collections::BTreeMap::new();
    for reference in refs {
        mappings
            .entry((reference.q0rg_id, reference.layer_id))
            .or_insert_with(|| {
                materialize_layer_keyframe_for_edit(
                    project,
                    reference.q0rg_id,
                    reference.layer_id,
                    frame,
                )
            });
    }

    let mut mapped = Vec::with_capacity(refs.len());
    for reference in refs {
        let mapping = mappings
            .get(&(reference.q0rg_id, reference.layer_id))?
            .as_ref()?;
        mapped.push(PathRef {
            q0rg_id: reference.q0rg_id,
            layer_id: reference.layer_id,
            placement_idx: *mapping.get(&reference.placement_idx)?,
            path_idx: reference.path_idx,
        });
    }

    let mut groups: std::collections::BTreeMap<(u16, u16), std::collections::BTreeSet<u16>> =
        std::collections::BTreeMap::new();
    for reference in &mapped {
        let asset_id = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.layer_id == reference.layer_id)
            })
            .and_then(|layer| layer.placements.get(reference.placement_idx))
            .and_then(|placement| match placement.target {
                Target::Asset(asset_id) => Some(asset_id),
                Target::Q0rg(_) => None,
            })?;
        groups
            .entry((reference.q0rg_id, reference.layer_id))
            .or_default()
            .insert(asset_id);
    }
    for ((q0rg_id, layer_id), asset_ids) in groups {
        crate::brush::prepare_writable_raw_assets(project, q0rg_id, layer_id, frame, &asset_ids);
    }
    Some(mapped)
}

fn capture_whole_asset_appearances_for_raw_refs(
    project: &ProjectV2,
    refs: &[PathRef],
) -> Vec<(u16, q0s_format::v2::VectorAppearance)> {
    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        let _ = (project, refs);
        return Vec::new();
    }
    #[cfg(feature = "appearance-mask-eraser")]
    {
        let mut grouped: std::collections::BTreeMap<u16, std::collections::BTreeSet<usize>> =
            std::collections::BTreeMap::new();
        for reference in refs {
            let Some(asset_id) = project
                .q0rgs
                .iter()
                .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
                .and_then(|q0rg| {
                    q0rg.layers
                        .iter()
                        .find(|layer| layer.layer_id == reference.layer_id)
                })
                .and_then(|layer| layer.placements.get(reference.placement_idx))
                .and_then(|placement| match placement.target {
                    Target::Asset(asset_id) => Some(asset_id),
                    Target::Q0rg(_) => None,
                })
            else {
                continue;
            };
            grouped
                .entry(asset_id)
                .or_default()
                .insert(reference.path_idx);
        }
        grouped
            .into_iter()
            .filter_map(|(asset_id, selected)| {
                let mut appearance = project.asset_appearances.get(&asset_id)?.clone();
                let Asset::Vector(vector) =
                    project.assets.iter().find(|asset| asset.id() == asset_id)?
                else {
                    return None;
                };
                if appearance.material_source.is_empty() {
                    // The first affine edit freezes the pre-transform material source.
                    // From now on field_transform moves the already-resolved glow.
                    appearance.material_source = vector.paths.clone();
                }
                let editable: std::collections::BTreeSet<usize> = vector
                    .paths
                    .iter()
                    .enumerate()
                    .filter_map(|(index, path)| path.closed.then_some(index))
                    .collect();
                (selected == editable).then_some((asset_id, appearance))
            })
            .collect()
    }
}

fn transform_captured_appearances(
    project: &mut ProjectV2,
    start_appearances: &[(u16, q0s_format::v2::VectorAppearance)],
    transform: Affine,
) -> bool {
    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        let _ = (project, start_appearances, transform);
        false
    }
    #[cfg(feature = "appearance-mask-eraser")]
    {
        let mut changed = false;
        for (asset_id, source) in start_appearances {
            let transformed = crate::appearance::transform_appearance(source, transform);
            if project.asset_appearances.get(asset_id) != Some(&transformed) {
                project.asset_appearances.insert(*asset_id, transformed);
                changed = true;
            }
        }
        changed
    }
}

fn translate_captured_appearances(
    project: &mut ProjectV2,
    start_appearances: &[(u16, q0s_format::v2::VectorAppearance)],
    delta: Vec2,
) -> bool {
    transform_captured_appearances(
        project,
        start_appearances,
        Affine {
            tx: delta.x,
            ty: delta.y,
            ..Affine::IDENTITY
        },
    )
}

fn apply_raw_affine_snapshot(
    project: &mut ProjectV2,
    refs: &[PathRef],
    start_paths: &[VPath],
    start_appearances: &[(u16, q0s_format::v2::VectorAppearance)],
    transform: Affine,
) -> bool {
    let mut changed = false;
    for (reference, source) in refs.iter().copied().zip(start_paths) {
        changed |=
            replace_raw_path_mapped(project, reference, source, |point| transform.apply(point));
    }
    changed | transform_captured_appearances(project, start_appearances, transform)
}

fn begin_dragging_raw_paths(
    app: &mut EditorApp,
    refs: Vec<PathRef>,
    start_cursor: Vec2,
    status: &str,
) -> bool {
    if refs.is_empty() {
        return false;
    }
    let start_pivot = selection_transform_pivot(app);
    app.history.snapshot(&app.state.project);
    let Some(refs) =
        prepare_raw_path_refs_for_edit(&mut app.state.project, &refs, app.session.current_frame)
    else {
        return false;
    };
    let start_paths: Vec<VPath> = refs
        .iter()
        .filter_map(|r| {
            raw_path_clone(
                &app.state.project,
                r.q0rg_id,
                r.layer_id,
                r.placement_idx,
                r.path_idx,
            )
        })
        .collect();
    if start_paths.len() != refs.len() {
        return false;
    }
    app.session.selection = selection_from_group_parts(refs.clone(), Vec::new());
    if let Some(pivot) = start_pivot {
        set_selection_transform_pivot(app, pivot);
    }
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(&app.state.project, &refs);
    app.session.tool_state = ToolState::DraggingPaths {
        refs,
        start_cursor,
        start_paths,
        start_appearances,
        start_pivot,
    };
    app.session.status = status.to_string();
    true
}

fn begin_scaling_raw_paths(
    app: &mut EditorApp,
    refs: Vec<PathRef>,
    handle: Handle,
    start_bounds: (f32, f32, f32, f32),
) -> bool {
    if refs.is_empty() || !raw_selection_supports_axis_resize(start_bounds) {
        return false;
    }
    let start_pivot = selection_transform_pivot(app).unwrap_or_else(|| {
        Vec2::new(
            (start_bounds.0 + start_bounds.2) * 0.5,
            (start_bounds.1 + start_bounds.3) * 0.5,
        )
    });
    app.history.snapshot(&app.state.project);
    let Some(refs) =
        prepare_raw_path_refs_for_edit(&mut app.state.project, &refs, app.session.current_frame)
    else {
        return false;
    };
    let start_paths: Vec<VPath> = refs
        .iter()
        .filter_map(|r| {
            raw_path_clone(
                &app.state.project,
                r.q0rg_id,
                r.layer_id,
                r.placement_idx,
                r.path_idx,
            )
        })
        .collect();
    if start_paths.len() != refs.len() {
        return false;
    }
    app.session.selection = selection_from_group_parts(refs.clone(), Vec::new());
    set_selection_transform_pivot(app, start_pivot);
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(&app.state.project, &refs);
    app.session.tool_state = ToolState::DraggingRawHandle {
        refs,
        start_paths,
        start_appearances,
        handle,
        start_bounds,
        start_pivot,
    };
    app.session.status = "Resizing raw graphics".to_string();
    true
}

fn begin_rotating_raw_paths(
    app: &mut EditorApp,
    refs: Vec<PathRef>,
    bounds: (f32, f32, f32, f32),
    center: Vec2,
    start_cursor: Vec2,
) -> bool {
    if refs.is_empty() || !raw_selection_supports_axis_resize(bounds) {
        return false;
    }
    app.history.snapshot(&app.state.project);
    let Some(refs) =
        prepare_raw_path_refs_for_edit(&mut app.state.project, &refs, app.session.current_frame)
    else {
        return false;
    };
    let start_paths: Vec<VPath> = refs
        .iter()
        .filter_map(|r| {
            raw_path_clone(
                &app.state.project,
                r.q0rg_id,
                r.layer_id,
                r.placement_idx,
                r.path_idx,
            )
        })
        .collect();
    if start_paths.len() != refs.len() {
        return false;
    }
    app.session.selection = selection_from_group_parts(refs.clone(), Vec::new());
    set_selection_transform_pivot(app, center);
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(&app.state.project, &refs);
    app.session.tool_state = ToolState::DraggingRawRotate {
        refs,
        start_paths,
        start_appearances,
        center,
        start_angle: (start_cursor.y - center.y).atan2(start_cursor.x - center.x),
    };
    app.session.status = "Rotating raw graphics".to_string();
    true
}

fn begin_skewing_raw_paths(
    app: &mut EditorApp,
    refs: Vec<PathRef>,
    edge: TransformEdge,
    bounds: (f32, f32, f32, f32),
    start_cursor: Vec2,
) -> bool {
    if refs.is_empty() || !raw_selection_supports_axis_resize(bounds) {
        return false;
    }
    let start_pivot = selection_transform_pivot(app)
        .unwrap_or_else(|| Vec2::new((bounds.0 + bounds.2) * 0.5, (bounds.1 + bounds.3) * 0.5));
    app.history.snapshot(&app.state.project);
    let Some(refs) =
        prepare_raw_path_refs_for_edit(&mut app.state.project, &refs, app.session.current_frame)
    else {
        return false;
    };
    let start_paths: Vec<VPath> = refs
        .iter()
        .filter_map(|r| {
            raw_path_clone(
                &app.state.project,
                r.q0rg_id,
                r.layer_id,
                r.placement_idx,
                r.path_idx,
            )
        })
        .collect();
    if start_paths.len() != refs.len() {
        return false;
    }
    app.session.selection = selection_from_group_parts(refs.clone(), Vec::new());
    set_selection_transform_pivot(app, start_pivot);
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(&app.state.project, &refs);
    app.session.tool_state = ToolState::DraggingRawSkew {
        refs,
        start_paths,
        start_appearances,
        edge,
        start_bounds: bounds,
        start_cursor,
        start_pivot,
    };
    app.session.status = "Skewing raw graphics".to_string();
    true
}

fn raw_handle_scale(
    bounds: (f32, f32, f32, f32),
    handle: Handle,
    cursor: Vec2,
) -> Option<(Vec2, f32, f32)> {
    let (min_x, min_y, max_x, max_y) = bounds;
    let width = max_x - min_x;
    let height = max_y - min_y;
    if width <= 1.0e-3 || height <= 1.0e-3 {
        return None;
    }

    let (anchor, mut scale_x, mut scale_y) = match handle {
        Handle::TopLeft => (
            Vec2::new(max_x, max_y),
            (cursor.x - max_x) / (min_x - max_x),
            (cursor.y - max_y) / (min_y - max_y),
        ),
        Handle::TopRight => (
            Vec2::new(min_x, max_y),
            (cursor.x - min_x) / (max_x - min_x),
            (cursor.y - max_y) / (min_y - max_y),
        ),
        Handle::BottomRight => (
            Vec2::new(min_x, min_y),
            (cursor.x - min_x) / (max_x - min_x),
            (cursor.y - min_y) / (max_y - min_y),
        ),
        Handle::BottomLeft => (
            Vec2::new(max_x, min_y),
            (cursor.x - max_x) / (min_x - max_x),
            (cursor.y - min_y) / (max_y - min_y),
        ),
        Handle::MidTop => (
            Vec2::new((min_x + max_x) * 0.5, max_y),
            1.0,
            (cursor.y - max_y) / (min_y - max_y),
        ),
        Handle::MidRight => (
            Vec2::new(min_x, (min_y + max_y) * 0.5),
            (cursor.x - min_x) / (max_x - min_x),
            1.0,
        ),
        Handle::MidBottom => (
            Vec2::new((min_x + max_x) * 0.5, min_y),
            1.0,
            (cursor.y - min_y) / (max_y - min_y),
        ),
        Handle::MidLeft => (
            Vec2::new(max_x, (min_y + max_y) * 0.5),
            (cursor.x - max_x) / (min_x - max_x),
            1.0,
        ),
    };
    scale_x = valid_scale(scale_x);
    scale_y = valid_scale(scale_y);
    Some((anchor, scale_x, scale_y))
}

fn replace_raw_path_mapped<F>(project: &mut ProjectV2, r: PathRef, source: &VPath, map: F) -> bool
where
    F: Fn(Vec2) -> Vec2,
{
    let asset_id = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == r.q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .find(|layer| layer.layer_id == r.layer_id)
        })
        .and_then(|layer| layer.placements.get(r.placement_idx))
        .and_then(|placement| match placement.target {
            Target::Asset(asset_id) => Some(asset_id),
            Target::Q0rg(_) => None,
        });
    let Some(asset_id) = asset_id else {
        return false;
    };
    let Some(Asset::Vector(vector)) = project
        .assets
        .iter_mut()
        .find(|asset| asset.id() == asset_id)
    else {
        return false;
    };
    let Some(target) = vector.paths.get_mut(r.path_idx) else {
        return false;
    };
    *target = source.clone();
    for anchor in &mut target.anchors {
        anchor.point = map(anchor.point);
        if let Some(handle) = &mut anchor.in_handle {
            *handle = map(*handle);
        }
        if let Some(handle) = &mut anchor.out_handle {
            *handle = map(*handle);
        }
    }
    true
}

fn raw_path_clone(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    path_idx: usize,
) -> Option<VPath> {
    let placement = project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)?
        .placements
        .get(placement_idx)?;
    let Target::Asset(asset_id) = placement.target else {
        return None;
    };
    let Asset::Vector(vector) = project.assets.iter().find(|asset| asset.id() == asset_id)? else {
        return None;
    };
    vector.paths.get(path_idx).cloned()
}

fn replace_raw_path_translated(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    path_idx: usize,
    source: &VPath,
    delta: Vec2,
) -> bool {
    let asset_id = project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
        .and_then(|layer| layer.placements.get(placement_idx))
        .and_then(|placement| match placement.target {
            Target::Asset(asset_id) => Some(asset_id),
            Target::Q0rg(_) => None,
        });
    let Some(asset_id) = asset_id else {
        return false;
    };
    let Some(Asset::Vector(vector)) = project
        .assets
        .iter_mut()
        .find(|asset| asset.id() == asset_id)
    else {
        return false;
    };
    let Some(target) = vector.paths.get_mut(path_idx) else {
        return false;
    };
    *target = source.clone();
    for anchor in &mut target.anchors {
        anchor.point.x += delta.x;
        anchor.point.y += delta.y;
        if let Some(handle) = &mut anchor.in_handle {
            handle.x += delta.x;
            handle.y += delta.y;
        }
        if let Some(handle) = &mut anchor.out_handle {
            handle.x += delta.x;
            handle.y += delta.y;
        }
    }
    true
}

fn replace_raw_path_points_translated(
    project: &mut ProjectV2,
    r: PathRef,
    selected: &[usize],
    source: &VPath,
    delta: Vec2,
) -> bool {
    let asset_id = project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == r.q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == r.layer_id))
        .and_then(|layer| layer.placements.get(r.placement_idx))
        .and_then(|placement| match placement.target {
            Target::Asset(asset_id) => Some(asset_id),
            Target::Q0rg(_) => None,
        });
    let Some(asset_id) = asset_id else {
        return false;
    };
    let Some(Asset::Vector(vector)) = project
        .assets
        .iter_mut()
        .find(|asset| asset.id() == asset_id)
    else {
        return false;
    };
    let Some(target) = vector.paths.get_mut(r.path_idx) else {
        return false;
    };
    *target = source.clone();
    for index in selected.iter().copied() {
        let Some(anchor) = target.anchors.get_mut(index) else {
            continue;
        };
        anchor.point.x += delta.x;
        anchor.point.y += delta.y;
        if let Some(handle) = &mut anchor.in_handle {
            handle.x += delta.x;
            handle.y += delta.y;
        }
        if let Some(handle) = &mut anchor.out_handle {
            handle.x += delta.x;
            handle.y += delta.y;
        }
    }
    true
}

type Aabb = (f32, f32, f32, f32);
type DragStartData = (Aabb, Aabb, Transform2D);

/// Pull together the snapshot data needed at drag-start: world AABB, local
/// AABB, and the transform itself.  Returns None if any of these can't be
/// computed (e.g. empty asset / orphaned q0rg).
fn drag_start_data(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    frame: u16,
) -> Option<DragStartData> {
    let layer = project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))?;
    let pl = layer.placements.get(placement_idx)?;
    let active_transform =
        crate::render::active_transform_for_placement(layer, placement_idx, frame)?;
    let mut visual = pl.clone();
    visual.transform = active_transform;
    let world = placement_bbox(project, &visual)?;
    let local = placement_local_bbox(project, &visual)?;
    Some((world, local, active_transform))
}

/// Materialize the complete visible layer state at `frame` before an edit.
/// A keyframe owns the whole layer contents, so cloning only the selected
/// placement would make every unselected object disappear at the new key.
pub(crate) fn materialize_layer_keyframe_for_edit(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Option<std::collections::BTreeMap<usize, usize>> {
    if project.layer_is_folder(q0rg_id, layer_id) {
        return None;
    }
    let active = {
        let layer = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?;
        crate::render::active_placements_at(layer, frame)
    };
    let mut mapping = std::collections::BTreeMap::new();
    if active.is_empty() {
        return Some(mapping);
    }

    let already_keyframed = {
        let layer = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?;
        active
            .iter()
            .all(|(index, _)| layer.placements[*index].frame == frame)
    };
    if already_keyframed {
        mapping.extend(active.into_iter().map(|(index, _)| (index, index)));
        return Some(mapping);
    }

    let clones: Vec<(usize, Placement)> = {
        let layer = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == layer_id)?;
        active
            .into_iter()
            .map(|(source_index, transform)| {
                let source = &layer.placements[source_index];
                (
                    source_index,
                    Placement {
                        frame,
                        target: source.target,
                        transform,
                        tween: Tween::None,
                    },
                )
            })
            .collect()
    };
    let layer = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == layer_id)?;
    let insert_at = layer.placements.len();
    for (offset, (source_index, placement)) in clones.into_iter().enumerate() {
        let new_index = insert_at + offset;
        layer.placements.push(placement);
        mapping.insert(source_index, new_index);
    }
    Some(mapping)
}

/// Materialize every referenced display object on `frame` and remap the
/// selection to the freshly baked placement indices. This is the multi-object
/// counterpart of `materialize_placement_keyframe_for_edit`; without it,
/// deleting a marquee selection on a held frame mutates the source keyframe.
pub(crate) fn materialize_placement_refs_for_edit(
    project: &mut ProjectV2,
    refs: &[PlacementRef],
    frame: u16,
) -> Option<Vec<PlacementRef>> {
    let mut mappings = std::collections::BTreeMap::new();
    for reference in refs {
        let key = (reference.q0rg_id, reference.layer_id);
        if mappings.contains_key(&key) {
            continue;
        }
        let mapping = materialize_layer_keyframe_for_edit(
            project,
            reference.q0rg_id,
            reference.layer_id,
            frame,
        )?;
        mappings.insert(key, mapping);
    }

    refs.iter()
        .map(|reference| {
            let mapping = mappings.get(&(reference.q0rg_id, reference.layer_id))?;
            Some(PlacementRef {
                q0rg_id: reference.q0rg_id,
                layer_id: reference.layer_id,
                placement_idx: *mapping.get(&reference.placement_idx)?,
            })
        })
        .collect()
}

/// A content keyframe is represented implicitly by placements. If a stage edit
/// removes the final placement, preserve the frame explicitly so it becomes a
/// real blank keyframe instead of disappearing and revealing the previous hold.
pub(crate) fn preserve_blank_keyframe_after_content_delete(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> bool {
    if project.layer_is_folder(q0rg_id, layer_id) {
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
    if layer
        .placements
        .iter()
        .any(|placement| placement.frame == frame)
    {
        return false;
    }

    let mut changed = !layer.explicit_keyframes.contains(&frame);
    layer.ensure_explicit_keyframe(frame);
    for placement in &mut layer.placements {
        if placement.tween.to_frame() == Some(frame) {
            placement.tween = Tween::None;
            changed = true;
        }
    }
    changed
}

/// Bake a held display object by materializing its complete layer keyframe and
/// return the selected placement's remapped index.
pub(crate) fn materialize_placement_keyframe_for_edit(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    frame: u16,
) -> Option<usize> {
    materialize_layer_keyframe_for_edit(project, q0rg_id, layer_id, frame)?
        .get(&placement_idx)
        .copied()
}

fn placement_mut(
    app: &mut EditorApp,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
) -> Option<&mut Placement> {
    app.state
        .project
        .q0rgs
        .iter_mut()
        .find(|q| q.q0rg_id == q0rg_id)?
        .layers
        .iter_mut()
        .find(|l| l.layer_id == layer_id)?
        .placements
        .get_mut(placement_idx)
}

/// Screen-space transform zones. The visible squares stay compact, but their
/// invisible hit targets are deliberately generous so free transform is usable
/// with a mouse at low zoom.
const HANDLE_HIT_RADIUS_PX: f32 = 14.0;
const RAW_HANDLE_HIT_RADIUS_PX: f32 = 10.0;
const ROTATE_HIT_RADIUS_PX: f32 = 34.0;
const PIVOT_HIT_RADIUS_PX: f32 = 10.0;
const SKEW_EDGE_HIT_RADIUS_PX: f32 = 7.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransformHit {
    Scale(Handle),
    Rotate(Handle),
    Skew(TransformEdge),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustomTransformCursor {
    Rotate,
    Skew(TransformEdge),
}

fn custom_transform_cursor_for_hit(hit: TransformHit) -> Option<CustomTransformCursor> {
    match hit {
        TransformHit::Scale(_) => None,
        TransformHit::Rotate(_) => Some(CustomTransformCursor::Rotate),
        TransformHit::Skew(edge) => Some(CustomTransformCursor::Skew(edge)),
    }
}

#[derive(Debug, Clone, Copy)]
struct TransformFrame {
    corners: [Vec2; 4], // TL, TR, BR, BL
    mids: [Vec2; 4],    // top, right, bottom, left
    center: Vec2,
}

fn cursor_for_transform_hit(hit: TransformHit) -> egui::CursorIcon {
    match hit {
        TransformHit::Scale(handle) => resize_cursor_for_handle(handle),
        TransformHit::Rotate(_) | TransformHit::Skew(_) => egui::CursorIcon::None,
    }
}

/// Any finite non-degenerate placement can use the oriented free-transform
/// frame. Rotation/skew no longer disable handles after the first edit.
fn supports_axis_resize(transform: Transform2D) -> bool {
    transform.sx.is_finite()
        && transform.sy.is_finite()
        && transform.rotation.is_finite()
        && transform.skew_x.is_finite()
        && transform.skew_y.is_finite()
        && transform.sx.abs() > 1.0e-5
        && transform.sy.abs() > 1.0e-5
}

fn axis_aligned_transform_frame(bbox: (f32, f32, f32, f32)) -> TransformFrame {
    let (min_x, min_y, max_x, max_y) = bbox;
    let corners = [
        Vec2::new(min_x, min_y),
        Vec2::new(max_x, min_y),
        Vec2::new(max_x, max_y),
        Vec2::new(min_x, max_y),
    ];
    transform_frame_from_corners(corners)
}

fn placement_transform_frame(
    local_bbox: (f32, f32, f32, f32),
    transform: Transform2D,
) -> TransformFrame {
    let affine = Affine::from_transform(transform);
    let (min_x, min_y, max_x, max_y) = local_bbox;
    transform_frame_from_corners([
        affine.apply(Vec2::new(min_x, min_y)),
        affine.apply(Vec2::new(max_x, min_y)),
        affine.apply(Vec2::new(max_x, max_y)),
        affine.apply(Vec2::new(min_x, max_y)),
    ])
}

fn transform_frame_from_corners(corners: [Vec2; 4]) -> TransformFrame {
    let midpoint = |a: Vec2, b: Vec2| Vec2::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
    let mids = [
        midpoint(corners[0], corners[1]),
        midpoint(corners[1], corners[2]),
        midpoint(corners[2], corners[3]),
        midpoint(corners[3], corners[0]),
    ];
    TransformFrame {
        corners,
        mids,
        center: midpoint(corners[0], corners[2]),
    }
}

fn frame_handle_positions(frame: TransformFrame) -> [(Handle, Vec2); 8] {
    [
        (Handle::TopLeft, frame.corners[0]),
        (Handle::TopRight, frame.corners[1]),
        (Handle::BottomRight, frame.corners[2]),
        (Handle::BottomLeft, frame.corners[3]),
        (Handle::MidTop, frame.mids[0]),
        (Handle::MidRight, frame.mids[1]),
        (Handle::MidBottom, frame.mids[2]),
        (Handle::MidLeft, frame.mids[3]),
    ]
}

fn screen_distance_sq(a: Pos2, b: Pos2) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy
}

fn screen_segment_distance(point: Pos2, start: Pos2, end: Pos2) -> f32 {
    let segment = end - start;
    let length_sq = segment.length_sq();
    if length_sq <= 1.0e-6 {
        return point.distance(start);
    }
    let t = ((point - start).dot(segment) / length_sq).clamp(0.0, 1.0);
    point.distance(start + segment * t)
}

fn hit_test_transform_frame(
    frame: TransformFrame,
    view: &StageView,
    screen_pos: Pos2,
    handle_radius: f32,
    suppress_thin_mids: bool,
) -> Option<TransformHit> {
    let corner_screen = frame.corners.map(|point| stage_to_screen(point, view));
    let center_screen = stage_to_screen(frame.center, view);
    let width = corner_screen[0].distance(corner_screen[1]);
    let height = corner_screen[0].distance(corner_screen[3]);
    let min_body_zone = 10.0;
    let suppress_vertical_mids = suppress_thin_mids && height < handle_radius * 2.0 + min_body_zone;
    let suppress_horizontal_mids =
        suppress_thin_mids && width < handle_radius * 2.0 + min_body_zone;

    for (handle, world) in frame_handle_positions(frame) {
        if suppress_vertical_mids && matches!(handle, Handle::MidTop | Handle::MidBottom) {
            continue;
        }
        if suppress_horizontal_mids && matches!(handle, Handle::MidLeft | Handle::MidRight) {
            continue;
        }
        let screen = stage_to_screen(world, view);
        if screen_distance_sq(screen, screen_pos) <= handle_radius * handle_radius {
            return Some(TransformHit::Scale(handle));
        }
    }

    for (index, handle) in [
        Handle::TopLeft,
        Handle::TopRight,
        Handle::BottomRight,
        Handle::BottomLeft,
    ]
    .into_iter()
    .enumerate()
    {
        let corner = corner_screen[index];
        let outward = corner - center_screen;
        let from_corner = screen_pos - corner;
        let distance_sq = from_corner.length_sq();
        let inner_radius = handle_radius + 1.0;
        if outward.length_sq() > 1.0e-6
            && from_corner.dot(outward) > 0.0
            && distance_sq >= inner_radius * inner_radius
            && distance_sq <= ROTATE_HIT_RADIUS_PX * ROTATE_HIT_RADIUS_PX
        {
            return Some(TransformHit::Rotate(handle));
        }
    }

    // Keep a real body-drag zone even on very thin selections. When opposite
    // edge hitboxes would overlap, shrink them symmetrically instead of making
    // the entire shape behave like a skew handle.
    let horizontal_edge_radius =
        SKEW_EDGE_HIT_RADIUS_PX.min(((height - min_body_zone) * 0.5).max(2.0));
    let vertical_edge_radius =
        SKEW_EDGE_HIT_RADIUS_PX.min(((width - min_body_zone) * 0.5).max(2.0));
    let edges = [
        (
            TransformEdge::Top,
            corner_screen[0],
            corner_screen[1],
            horizontal_edge_radius,
        ),
        (
            TransformEdge::Right,
            corner_screen[1],
            corner_screen[2],
            vertical_edge_radius,
        ),
        (
            TransformEdge::Bottom,
            corner_screen[2],
            corner_screen[3],
            horizontal_edge_radius,
        ),
        (
            TransformEdge::Left,
            corner_screen[3],
            corner_screen[0],
            vertical_edge_radius,
        ),
    ];
    for (edge, start, end, radius) in edges {
        if screen_segment_distance(screen_pos, start, end) <= radius {
            return Some(TransformHit::Skew(edge));
        }
    }

    None
}

fn hit_test_raw_transform(
    bbox: (f32, f32, f32, f32),
    view: &StageView,
    screen_pos: Pos2,
) -> Option<TransformHit> {
    hit_test_transform_frame(
        axis_aligned_transform_frame(bbox),
        view,
        screen_pos,
        RAW_HANDLE_HIT_RADIUS_PX,
        true,
    )
}

fn hit_test_placement_transform(
    local_bbox: (f32, f32, f32, f32),
    transform: Transform2D,
    view: &StageView,
    screen_pos: Pos2,
) -> Option<TransformHit> {
    supports_axis_resize(transform).then(|| {
        hit_test_transform_frame(
            placement_transform_frame(local_bbox, transform),
            view,
            screen_pos,
            HANDLE_HIT_RADIUS_PX,
            false,
        )
    })?
}

/// Resize from a corner or edge while keeping the opposite handle anchored.
/// Corner handles change both axes; edge handles change one axis.
fn apply_handle_drag(
    t: &mut Transform2D,
    start_t: Transform2D,
    local_bbox: (f32, f32, f32, f32),
    _world_bbox: (f32, f32, f32, f32),
    handle: Handle,
    cursor_world: Vec2,
) -> bool {
    if !supports_axis_resize(start_t) {
        *t = start_t;
        return false;
    }

    let (min_x, min_y, max_x, max_y) = local_bbox;
    let width = max_x - min_x;
    let height = max_y - min_y;
    if width.abs() < 1.0e-3 || height.abs() < 1.0e-3 {
        return false;
    }
    let start_affine = Affine::from_transform(start_t);
    let Some(inverse) = start_affine.inverse() else {
        return false;
    };
    let cursor_local = inverse.apply(cursor_world);

    let (anchor_local, dragged_local, affect_x, affect_y) = match handle {
        Handle::TopLeft => (Vec2::new(max_x, max_y), Vec2::new(min_x, min_y), true, true),
        Handle::TopRight => (Vec2::new(min_x, max_y), Vec2::new(max_x, min_y), true, true),
        Handle::BottomRight => (Vec2::new(min_x, min_y), Vec2::new(max_x, max_y), true, true),
        Handle::BottomLeft => (Vec2::new(max_x, min_y), Vec2::new(min_x, max_y), true, true),
        Handle::MidTop => (
            Vec2::new((min_x + max_x) * 0.5, max_y),
            Vec2::new((min_x + max_x) * 0.5, min_y),
            false,
            true,
        ),
        Handle::MidRight => (
            Vec2::new(min_x, (min_y + max_y) * 0.5),
            Vec2::new(max_x, (min_y + max_y) * 0.5),
            true,
            false,
        ),
        Handle::MidBottom => (
            Vec2::new((min_x + max_x) * 0.5, min_y),
            Vec2::new((min_x + max_x) * 0.5, max_y),
            false,
            true,
        ),
        Handle::MidLeft => (
            Vec2::new(max_x, (min_y + max_y) * 0.5),
            Vec2::new(min_x, (min_y + max_y) * 0.5),
            true,
            false,
        ),
    };

    let ratio_x = if affect_x {
        (cursor_local.x - anchor_local.x) / (dragged_local.x - anchor_local.x)
    } else {
        1.0
    };
    let ratio_y = if affect_y {
        (cursor_local.y - anchor_local.y) / (dragged_local.y - anchor_local.y)
    } else {
        1.0
    };

    *t = start_t;
    t.sx = valid_scale(start_t.sx * ratio_x);
    t.sy = valid_scale(start_t.sy * ratio_y);
    let anchor_world = start_affine.apply(anchor_local);
    let after = apply_no_translate(*t, anchor_local);
    t.tx = anchor_world.x - after.x;
    t.ty = anchor_world.y - after.y;
    true
}

fn rotate_placement_about_local_point(
    start: Transform2D,
    center_local: Vec2,
    center_world: Vec2,
    delta: f32,
) -> Transform2D {
    let mut next = start;
    next.rotation = start.rotation + delta;
    let after = apply_no_translate(next, center_local);
    next.tx = center_world.x - after.x;
    next.ty = center_world.y - after.y;
    next
}

fn skew_placement_from_cursor(
    start: Transform2D,
    local_bbox: (f32, f32, f32, f32),
    edge: TransformEdge,
    start_cursor_local: Vec2,
    cursor_world: Vec2,
) -> Option<Transform2D> {
    let start_affine = Affine::from_transform(start);
    let cursor_local = start_affine.inverse()?.apply(cursor_world);
    let (min_x, min_y, max_x, max_y) = local_bbox;
    let center_x = (min_x + max_x) * 0.5;
    let center_y = (min_y + max_y) * 0.5;
    let mut next = start;

    let anchor_local = match edge {
        TransformEdge::Top => {
            let denom = (min_y - max_y).abs().max(1.0e-4);
            let delta_tan = (cursor_local.x - start_cursor_local.x) / -denom;
            next.skew_x = (start.skew_x.tan() + delta_tan).clamp(-8.0, 8.0).atan();
            Vec2::new(center_x, max_y)
        }
        TransformEdge::Bottom => {
            let denom = (max_y - min_y).abs().max(1.0e-4);
            let delta_tan = (cursor_local.x - start_cursor_local.x) / denom;
            next.skew_x = (start.skew_x.tan() + delta_tan).clamp(-8.0, 8.0).atan();
            Vec2::new(center_x, min_y)
        }
        TransformEdge::Left => {
            let denom = (min_x - max_x).abs().max(1.0e-4);
            let delta_tan = (cursor_local.y - start_cursor_local.y) / -denom;
            next.skew_y = (start.skew_y.tan() + delta_tan).clamp(-8.0, 8.0).atan();
            Vec2::new(max_x, center_y)
        }
        TransformEdge::Right => {
            let denom = (max_x - min_x).abs().max(1.0e-4);
            let delta_tan = (cursor_local.y - start_cursor_local.y) / denom;
            next.skew_y = (start.skew_y.tan() + delta_tan).clamp(-8.0, 8.0).atan();
            Vec2::new(min_x, center_y)
        }
    };

    let anchor_world = start_affine.apply(anchor_local);
    let after = apply_no_translate(next, anchor_local);
    next.tx = anchor_world.x - after.x;
    next.ty = anchor_world.y - after.y;
    Some(next)
}

fn valid_scale(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.01)
    } else {
        1.0
    }
}

fn apply_no_translate(t: Transform2D, p: Vec2) -> Vec2 {
    let mut x = p.x * t.sx;
    let mut y = p.y * t.sy;
    if t.skew_x != 0.0 {
        x += y * t.skew_x.tan();
    }
    if t.skew_y != 0.0 {
        y += x * t.skew_y.tan();
    }
    let (sin, cos) = t.rotation.sin_cos();
    Vec2::new(x * cos - y * sin, x * sin + y * cos)
}

// ---------------- Line / Rectangle / Oval primitives ----------------

fn line(
    app: &mut EditorApp,
    response: &Response,
    cursor: Option<Vec2>,
    _painter: &Painter,
    view: &StageView,
) {
    primitive_drag(app, response, cursor, view, PrimitiveKind::Line);
}

fn primitive_rect(
    app: &mut EditorApp,
    response: &Response,
    cursor: Option<Vec2>,
    _painter: &Painter,
    view: &StageView,
    oval: bool,
) {
    primitive_drag(
        app,
        response,
        cursor,
        view,
        if oval {
            PrimitiveKind::Oval
        } else {
            PrimitiveKind::Rect
        },
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrimitiveKind {
    Line,
    Rect,
    Oval,
}

fn primitive_kind_for_tool(tool: Tool) -> Option<PrimitiveKind> {
    match tool {
        Tool::Line => Some(PrimitiveKind::Line),
        Tool::Rectangle => Some(PrimitiveKind::Rect),
        Tool::Oval => Some(PrimitiveKind::Oval),
        _ => None,
    }
}

fn primitive_pointer(response: &Response, cursor: Option<Vec2>, view: &StageView) -> Option<Vec2> {
    response
        .interact_pointer_pos()
        .map(|screen| screen_to_stage(screen, view))
        .or(cursor)
}

fn primitive_point_is_on_stage(app: &EditorApp, point: Vec2) -> bool {
    point.x >= 0.0
        && point.y >= 0.0
        && point.x <= app.state.project.meta.stage_width as f32
        && point.y <= app.state.project.meta.stage_height as f32
}

fn constrain_primitive_end(kind: PrimitiveKind, start: Vec2, end: Vec2, shift: bool) -> Vec2 {
    if !shift {
        return end;
    }

    let delta = Vec2::new(end.x - start.x, end.y - start.y);
    match kind {
        PrimitiveKind::Line => {
            let length = (delta.x * delta.x + delta.y * delta.y).sqrt();
            if length <= 1.0e-6 {
                return end;
            }
            let step = std::f32::consts::FRAC_PI_4;
            let angle = (delta.y.atan2(delta.x) / step).round() * step;
            Vec2::new(
                start.x + angle.cos() * length,
                start.y + angle.sin() * length,
            )
        }
        PrimitiveKind::Rect | PrimitiveKind::Oval => {
            let size = delta.x.abs().max(delta.y.abs());
            let sign_x = if delta.x < 0.0 { -1.0 } else { 1.0 };
            let sign_y = if delta.y < 0.0 { -1.0 } else { 1.0 };
            Vec2::new(start.x + sign_x * size, start.y + sign_y * size)
        }
    }
}

fn primitive_path(kind: PrimitiveKind, a: Vec2, b: Vec2) -> Option<VPath> {
    const MIN_EXTENT: f32 = 1.0e-3;

    match kind {
        PrimitiveKind::Line => {
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            ((dx * dx + dy * dy).sqrt() >= MIN_EXTENT).then(|| VPath {
                anchors: vec![anchor(a), anchor(b)],
                closed: false,
            })
        }
        PrimitiveKind::Rect => {
            let min_x = a.x.min(b.x);
            let min_y = a.y.min(b.y);
            let max_x = a.x.max(b.x);
            let max_y = a.y.max(b.y);
            if max_x - min_x < MIN_EXTENT || max_y - min_y < MIN_EXTENT {
                return None;
            }
            Some(VPath {
                // Always use one canonical winding. Drag direction must never
                // turn an ordinary rectangle into a hole contour.
                anchors: vec![
                    anchor(Vec2::new(min_x, min_y)),
                    anchor(Vec2::new(max_x, min_y)),
                    anchor(Vec2::new(max_x, max_y)),
                    anchor(Vec2::new(min_x, max_y)),
                ],
                closed: true,
            })
        }
        PrimitiveKind::Oval => {
            let min_x = a.x.min(b.x);
            let min_y = a.y.min(b.y);
            let max_x = a.x.max(b.x);
            let max_y = a.y.max(b.y);
            let rx = (max_x - min_x) * 0.5;
            let ry = (max_y - min_y) * 0.5;
            if rx * 2.0 < MIN_EXTENT || ry * 2.0 < MIN_EXTENT {
                return None;
            }

            let cx = (min_x + max_x) * 0.5;
            let cy = (min_y + max_y) * 0.5;
            let k = 0.5522848_f32; // 4*(sqrt(2)-1)/3
            let kx = rx * k;
            let ky = ry * k;
            Some(VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(cx, cy - ry),
                        in_handle: Some(Vec2::new(cx - kx, cy - ry)),
                        out_handle: Some(Vec2::new(cx + kx, cy - ry)),
                    },
                    Anchor {
                        point: Vec2::new(cx + rx, cy),
                        in_handle: Some(Vec2::new(cx + rx, cy - ky)),
                        out_handle: Some(Vec2::new(cx + rx, cy + ky)),
                    },
                    Anchor {
                        point: Vec2::new(cx, cy + ry),
                        in_handle: Some(Vec2::new(cx + kx, cy + ry)),
                        out_handle: Some(Vec2::new(cx - kx, cy + ry)),
                    },
                    Anchor {
                        point: Vec2::new(cx - rx, cy),
                        in_handle: Some(Vec2::new(cx - rx, cy + ky)),
                        out_handle: Some(Vec2::new(cx - rx, cy - ky)),
                    },
                ],
                closed: true,
            })
        }
    }
}

fn primitive_drag(
    app: &mut EditorApp,
    response: &Response,
    cursor: Option<Vec2>,
    view: &StageView,
    kind: PrimitiveKind,
) {
    let pointer = primitive_pointer(response, cursor, view);
    let shift = response.ctx.input(|input| input.modifiers.shift);

    if response.drag_started_by(PointerButton::Primary) {
        if let Some(point) = pointer.filter(|point| primitive_point_is_on_stage(app, *point)) {
            app.session.tool_state = ToolState::PrimitiveDrawing {
                start: point,
                end: point,
            };
            app.session.status = match kind {
                PrimitiveKind::Line => "Line: drawing",
                PrimitiveKind::Rect => "Rectangle: drawing",
                PrimitiveKind::Oval => "Oval: drawing",
            }
            .to_string();
        }
    }

    if response.dragged_by(PointerButton::Primary)
        || response.drag_stopped_by(PointerButton::Primary)
    {
        let stage_width = app.state.project.meta.stage_width as f32;
        let stage_height = app.state.project.meta.stage_height as f32;
        if let ToolState::PrimitiveDrawing { start, end } = &mut app.session.tool_state {
            if let Some(point) = pointer {
                let point = Vec2::new(
                    point.x.clamp(0.0, stage_width),
                    point.y.clamp(0.0, stage_height),
                );
                let constrained = constrain_primitive_end(kind, *start, point, shift);
                *end = Vec2::new(
                    constrained.x.clamp(0.0, stage_width),
                    constrained.y.clamp(0.0, stage_height),
                );
            }
        }
    }

    if response.drag_stopped_by(PointerButton::Primary) {
        if let ToolState::PrimitiveDrawing { start, end } =
            std::mem::replace(&mut app.session.tool_state, ToolState::Idle)
        {
            if let Some(path) = primitive_path(kind, start, end) {
                commit_vector_path(app, path, DrawingMode::Merge);
            } else {
                app.session.status = "Shape too small".to_string();
            }
        }
    }
}

fn anchor(p: Vec2) -> Anchor {
    Anchor {
        point: p,
        in_handle: None,
        out_handle: None,
    }
}

// ---------------- Commit & helpers ----------------

/// Drawing commit mode.
///
/// `Object` is the discrete-object behaviour: every stroke spawns its own
/// VectorAsset and Placement. The user can select, move, transform each
/// stroke independently (Pencil tool, also legacy default).
///
/// `Merge` is touch-based merge drawing. The new stroke's bbox is checked
/// against existing same-style drawings on this (q0rg, layer, frame). If
/// any of their paths lies within `MERGE_TOUCH_TOLERANCE` of the new
/// path's bbox, the stroke is appended into that asset. Strokes that
/// don't touch any existing drawing get their own asset, so two pen
/// strokes on opposite ends of the stage stay separate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrawingMode {
    Object,
    Merge,
}

/// How close two paths' bboxes have to be (in stage units) to count as
/// "touching" for the purposes of merge drawing. Empirically: a few px
/// past the average stroke half-width, which is small enough that strokes drawn
/// far apart never glue, large enough that grazing touches do.
const MERGE_TOUCH_TOLERANCE: f32 = 6.0;

fn commit_path(app: &mut EditorApp, anchors: Vec<Anchor>, closed: bool, mode: DrawingMode) {
    commit_vector_path(app, VPath { anchors, closed }, mode);
}

fn commit_vector_path(app: &mut EditorApp, new_path: VPath, mode: DrawingMode) {
    app.history.snapshot(&app.state.project);

    let fill = if new_path.closed {
        app.session.fill_color
    } else {
        None
    };
    let stroke = Some(VStroke {
        color: app.session.stroke_color,
        width: app.session.stroke_width.max(0.1),
        cap: app.session.brush_cap,
    });
    let new_bbox = match path_bbox(&new_path) {
        Some(b) => b,
        None => return, // degenerate path with no geometry
    };

    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    if materialize_layer_keyframe_for_edit(&mut app.state.project, q0rg_id, layer_id, frame)
        .is_none()
    {
        return;
    }

    let merge_asset_id = match mode {
        DrawingMode::Object => None,
        DrawingMode::Merge => find_touching_drawing(
            &app.state.project,
            q0rg_id,
            layer_id,
            frame,
            fill,
            stroke,
            new_bbox,
        ),
    };
    if let Some(asset_id) = merge_asset_id {
        let mut assets = std::collections::BTreeSet::new();
        assets.insert(asset_id);
        let writable = crate::brush::prepare_writable_raw_assets(
            &mut app.state.project,
            q0rg_id,
            layer_id,
            frame,
            &assets,
        );
        let asset_id = *writable.get(&asset_id).unwrap_or(&asset_id);
        if let Some(Asset::Vector(v)) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|a| a.id() == asset_id)
        {
            v.paths.push(new_path);
            app.state.dirty = true;
            app.session.status = match mode {
                DrawingMode::Merge => format!("Shape updated ({} paths)", v.paths.len()),
                DrawingMode::Object => unreachable!(),
            };
            return;
        }
    }

    // Object mode, or merge mode with no touching drawing, creates a fresh asset.
    let asset_id = next_asset_id(&app.state.project);
    app.state.project.assets.push(Asset::Vector(VectorAsset {
        asset_id,
        paths: vec![new_path],
        fill,
        stroke,
    }));

    if let Some(q) = app
        .state
        .project
        .q0rgs
        .iter_mut()
        .find(|q| q.q0rg_id == q0rg_id)
    {
        if let Some(layer) = q.layers.iter_mut().find(|l| l.layer_id == layer_id) {
            layer.placements.push(Placement {
                frame,
                target: Target::Asset(asset_id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            });
            app.state.dirty = true;
            app.session.status = match mode {
                DrawingMode::Object => "New object".to_string(),
                DrawingMode::Merge => "New drawing".to_string(),
            };
        }
    }
}

/// Touch-based merge: look at every same-style drawing on (q0rg, layer,
/// frame) and return the first one with at least one path whose bbox
/// (slightly inflated) overlaps the new path's bbox. Returns `None` if
/// the new stroke is far enough from existing drawings to start its own.
fn find_touching_drawing(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    fill: Option<Rgba>,
    stroke: Option<VStroke>,
    new_bbox: (f32, f32, f32, f32),
) -> Option<u16> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    let layer = q.layers.iter().find(|l| l.layer_id == layer_id)?;
    for placement in &layer.placements {
        if placement.frame != frame {
            continue;
        }
        if placement.transform != Transform2D::IDENTITY {
            continue;
        }
        if !matches!(placement.tween, Tween::None) {
            continue;
        }
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(Asset::Vector(v)) = project.assets.iter().find(|a| a.id() == asset_id) else {
            continue;
        };
        if v.fill != fill || v.stroke != stroke {
            continue;
        }
        for existing in &v.paths {
            if let Some(eb) = path_bbox(existing) {
                if bboxes_touch(eb, new_bbox, MERGE_TOUCH_TOLERANCE) {
                    return Some(asset_id);
                }
            }
        }
    }
    None
}

fn path_bbox(path: &VPath) -> Option<(f32, f32, f32, f32)> {
    let pts = flatten_path(path);
    if pts.is_empty() {
        return None;
    }
    let mut min_x = pts[0].x;
    let mut min_y = pts[0].y;
    let mut max_x = pts[0].x;
    let mut max_y = pts[0].y;
    for p in &pts[1..] {
        if p.x < min_x {
            min_x = p.x;
        }
        if p.y < min_y {
            min_y = p.y;
        }
        if p.x > max_x {
            max_x = p.x;
        }
        if p.y > max_y {
            max_y = p.y;
        }
    }
    Some((min_x, min_y, max_x, max_y))
}

fn bboxes_touch(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32), pad: f32) -> bool {
    !(a.2 + pad < b.0 || b.2 + pad < a.0 || a.3 + pad < b.1 || b.3 + pad < a.1)
}

fn next_asset_id(project: &ProjectV2) -> u16 {
    project
        .assets
        .iter()
        .map(|a| a.id())
        .max()
        .unwrap_or(0)
        .saturating_add(1)
        .max(1)
}

fn placement_transform(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    idx: usize,
    frame: u16,
) -> Option<Transform2D> {
    project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
        .and_then(|layer| crate::render::active_transform_for_placement(layer, idx, frame))
}

pub fn hit_test_placement_pub(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    p: Vec2,
) -> Option<(u16, usize)> {
    hit_test_selectable_placement(project, q0rg_id, frame, p)
}

/// Shared canvas/context-menu selection semantics. Raw graphics resolve to a
/// connected fill region or individual stroke path; only actual display
/// objects resolve to `Selection::Placement`.
pub fn selection_at_point_pub(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    p: Vec2,
) -> Option<Selection> {
    if let Some(hit) = hit_test_raw_selection(project, q0rg_id, frame, p) {
        return Some(match hit {
            RawSelectionHit::Fill(refs) => Selection::Paths(refs),
            RawSelectionHit::Path(path) => Selection::Path {
                q0rg_id: path.q0rg_id,
                layer_id: path.layer_id,
                placement_idx: path.placement_idx,
                path_idx: path.path_idx,
            },
        });
    }
    hit_test_selectable_placement(project, q0rg_id, frame, p).map(|(layer_id, placement_idx)| {
        Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        }
    })
}

/// Walk every placement on `q0rg_id` and collect those whose bbox intersects
/// the marquee rect. Order: top-most layer & last placement first, matching
/// hit-test ordering; useful when the caller wants the visually-frontmost
/// hit at index 0.
fn collect_placements_in_rect(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    rect: (f32, f32, f32, f32),
) -> Vec<PlacementRef> {
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for layer in q.layers.iter().rev() {
        for (idx, transform) in crate::render::active_placements_at(layer, frame)
            .into_iter()
            .rev()
        {
            let Some(placement) = layer.placements.get(idx) else {
                continue;
            };
            // Identity vector placements are raw drawing surfaces. Their
            // individual paths were considered by `collect_raw_paths_in_rect`;
            // never fall back to selecting their whole asset-sized bbox.
            if is_raw_graphics_placement(project, placement) {
                continue;
            }
            let mut visual = placement.clone();
            visual.transform = transform;
            let Some(bbox) = placement_bbox(project, &visual) else {
                continue;
            };
            // AABB-vs-AABB overlap.
            if bbox.0 > rect.2 || bbox.2 < rect.0 || bbox.1 > rect.3 || bbox.3 < rect.1 {
                continue;
            }
            out.push(PlacementRef {
                q0rg_id,
                layer_id: layer.layer_id,
                placement_idx: idx,
            });
        }
    }
    out
}

fn collect_raw_paths_in_rect(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    rect: (f32, f32, f32, f32),
) -> Vec<PathRef> {
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for layer in q.layers.iter().rev() {
        for placement_idx in active_raw_placement_indices(project, layer, frame)
            .into_iter()
            .rev()
        {
            let placement = &layer.placements[placement_idx];
            let Target::Asset(asset_id) = placement.target else {
                continue;
            };
            let Some(Asset::Vector(vector)) = project.assets.iter().find(|a| a.id() == asset_id)
            else {
                continue;
            };
            for (path_idx, path) in vector.paths.iter().enumerate().rev() {
                // Closed fills are selected by their combined NonZero surface
                // above. Handling their rings individually would make a hole
                // look like a selectable filled object.
                if path.closed && vector.fill.is_some() {
                    continue;
                }
                let points = flatten_path(path);
                if path_intersects_rect(&points, path.closed, rect) {
                    hits.push(PathRef {
                        q0rg_id,
                        layer_id: layer.layer_id,
                        placement_idx,
                        path_idx,
                    });
                }
            }
        }
    }
    hits
}

fn path_intersects_rect(points: &[Vec2], closed: bool, rect: (f32, f32, f32, f32)) -> bool {
    if points.is_empty() {
        return false;
    }
    let inside_rect = |point: Vec2| {
        point.x >= rect.0 && point.x <= rect.2 && point.y >= rect.1 && point.y <= rect.3
    };
    if points.iter().copied().any(inside_rect) {
        return true;
    }
    if closed {
        let corners = [
            Vec2::new(rect.0, rect.1),
            Vec2::new(rect.2, rect.1),
            Vec2::new(rect.2, rect.3),
            Vec2::new(rect.0, rect.3),
        ];
        if corners
            .iter()
            .copied()
            .any(|corner| point_in_polygon(points, corner))
        {
            return true;
        }
    }

    let edges = [
        (Vec2::new(rect.0, rect.1), Vec2::new(rect.2, rect.1)),
        (Vec2::new(rect.2, rect.1), Vec2::new(rect.2, rect.3)),
        (Vec2::new(rect.2, rect.3), Vec2::new(rect.0, rect.3)),
        (Vec2::new(rect.0, rect.3), Vec2::new(rect.0, rect.1)),
    ];
    for segment in points.windows(2) {
        if edges
            .iter()
            .any(|(a, b)| segments_intersect(segment[0], segment[1], *a, *b))
        {
            return true;
        }
    }
    if closed && points.len() > 2 {
        return edges
            .iter()
            .any(|(a, b)| segments_intersect(points[points.len() - 1], points[0], *a, *b));
    }
    false
}

fn segments_intersect(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    const EPSILON: f32 = 1.0e-5;
    fn orientation(a: Vec2, b: Vec2, c: Vec2) -> f32 {
        (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    }
    fn on_segment(a: Vec2, b: Vec2, p: Vec2) -> bool {
        const EPSILON: f32 = 1.0e-5;
        p.x >= a.x.min(b.x) - EPSILON
            && p.x <= a.x.max(b.x) + EPSILON
            && p.y >= a.y.min(b.y) - EPSILON
            && p.y <= a.y.max(b.y) + EPSILON
    }
    let ab_c = orientation(a, b, c);
    let ab_d = orientation(a, b, d);
    let cd_a = orientation(c, d, a);
    let cd_b = orientation(c, d, b);
    if ab_c.abs() <= EPSILON && on_segment(a, b, c) {
        return true;
    }
    if ab_d.abs() <= EPSILON && on_segment(a, b, d) {
        return true;
    }
    if cd_a.abs() <= EPSILON && on_segment(c, d, a) {
        return true;
    }
    if cd_b.abs() <= EPSILON && on_segment(c, d, b) {
        return true;
    }
    ab_c.signum() != ab_d.signum() && cd_a.signum() != cd_b.signum()
}

fn hit_test_placement(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    p: Vec2,
) -> Option<(u16, usize)> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    // Iterate layers in reverse (top layer first) for hit priority. Resolve
    // held/tweened spans exactly like the renderer instead of requiring an
    // explicit keyframe under the playhead.
    for layer in q.layers.iter().rev() {
        for (idx, transform) in crate::render::active_placements_at(layer, frame)
            .into_iter()
            .rev()
        {
            let Some(placement) = layer.placements.get(idx) else {
                continue;
            };
            let mut visual = placement.clone();
            visual.transform = transform;
            if let Some(bbox) = placement_bbox(project, &visual) {
                if p.x >= bbox.0 && p.x <= bbox.2 && p.y >= bbox.1 && p.y <= bbox.3 {
                    return Some((layer.layer_id, idx));
                }
            }
        }
    }
    None
}

/// Identity, non-tweened vector placements are implementation containers for
/// raw drawing geometry. They must never behave like q0rg/display objects in
/// the Selection tool, even when several disconnected fills share one asset.
fn active_raw_placement_indices(
    project: &ProjectV2,
    layer: &q0s_format::v2::Layer,
    frame: u16,
) -> Vec<usize> {
    crate::render::active_placements_at(layer, frame)
        .into_iter()
        .filter_map(|(index, _)| {
            layer
                .placements
                .get(index)
                .is_some_and(|placement| is_raw_graphics_placement(project, placement))
                .then_some(index)
        })
        .collect()
}

fn is_raw_graphics_placement(project: &ProjectV2, placement: &Placement) -> bool {
    if placement.transform != Transform2D::IDENTITY || !matches!(placement.tween, Tween::None) {
        return false;
    }
    matches!(
        placement.target,
        Target::Asset(asset_id)
            if matches!(
                project.assets.iter().find(|asset| asset.id() == asset_id),
                Some(Asset::Vector(_))
            )
    )
}

/// Object hit-test used by the Selection tool. q0rg instances, bitmaps and
/// transformed vector instances retain their placement bbox/handles; raw
/// graphics are reachable only through fill/path hit-testing.
fn hit_test_selectable_placement(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    p: Vec2,
) -> Option<(u16, usize)> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    for layer in q.layers.iter().rev() {
        for (idx, transform) in crate::render::active_placements_at(layer, frame)
            .into_iter()
            .rev()
        {
            let Some(placement) = layer.placements.get(idx) else {
                continue;
            };
            if is_raw_graphics_placement(project, placement) {
                continue;
            }
            let mut visual = placement.clone();
            visual.transform = transform;
            if let Some(bbox) = placement_bbox(project, &visual) {
                if p.x >= bbox.0 && p.x <= bbox.2 && p.y >= bbox.1 && p.y <= bbox.3 {
                    return Some((layer.layer_id, idx));
                }
            }
        }
    }
    None
}

fn paint_classic_nib_preview(
    painter: &Painter,
    stroke: &crate::brush::BrushStroke,
    view: &StageView,
    fill: Color32,
    outline: Option<Color32>,
) {
    if crate::brush::brush_preview_nib(stroke) == crate::brush::BrushNib::Circle {
        let points: Vec<Pos2> = crate::brush::brush_preview_trajectory(stroke)
            .into_iter()
            .map(|point| stage_to_screen(point, view))
            .collect();
        let width = crate::brush::brush_preview_size(stroke) * view.scale;
        if let Some(outline) = outline {
            paint_round_stroke_preview(painter, &points, width + 2.0, outline);
        }
        paint_round_stroke_preview(painter, &points, width, fill);
        return;
    }

    let contours: Vec<Vec<Pos2>> = crate::brush::brush_preview_paths_for_render(stroke)
        .iter()
        .map(|path| {
            flatten_path(path)
                .into_iter()
                .map(|point| stage_to_screen(point, view))
                .collect()
        })
        .filter(|points: &Vec<Pos2>| points.len() >= 3)
        .collect();
    paint_complex_fill(painter, &contours, fill);
    if let Some(outline) = outline {
        for points in contours {
            painter.add(Shape::Path(PathShape {
                points,
                closed: true,
                fill: Color32::TRANSPARENT,
                stroke: Stroke::new(1.5_f32, outline),
            }));
        }
    }
}
fn advanced_preview_mesh(
    dabs: &[crate::advanced_brush::AdvancedDab],
    view: &StageView,
    color: Color32,
    expansion: f32,
) -> Mesh {
    const SEGMENTS: usize = 18;
    let mut mesh = Mesh::default();
    let mut rings: Vec<[Pos2; SEGMENTS]> = Vec::with_capacity(dabs.len());

    for dab in dabs {
        let center = stage_to_screen(dab.center, view);
        let major = (dab.major_radius + expansion).max(0.05) * view.scale;
        let minor = (dab.minor_radius + expansion).max(0.05) * view.scale;
        let cos_a = dab.angle_radians.cos();
        let sin_a = dab.angle_radians.sin();
        let ring = std::array::from_fn(|index| {
            let phase = std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
            let x = phase.cos() * major;
            let y = phase.sin() * minor;
            pos2(
                center.x + x * cos_a - y * sin_a,
                center.y + x * sin_a + y * cos_a,
            )
        });

        let center_index = mesh.vertices.len() as u32;
        mesh.colored_vertex(center, color);
        let ring_start = mesh.vertices.len() as u32;
        for point in ring {
            mesh.colored_vertex(point, color);
        }
        for index in 0..SEGMENTS {
            mesh.add_triangle(
                center_index,
                ring_start + index as u32,
                ring_start + ((index + 1) % SEGMENTS) as u32,
            );
        }
        rings.push(ring);
    }

    // Morph one oriented nib ring into the next with a GPU triangle strip.
    // This tracks roundness/angle/taper much more closely than a centre-line
    // ribbon, while still avoiding all boolean geometry during live input.
    for pair in rings.windows(2) {
        let first = &pair[0];
        let second = &pair[1];
        for index in 0..SEGMENTS {
            let next = (index + 1) % SEGMENTS;
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(first[index], color);
            mesh.colored_vertex(first[next], color);
            mesh.colored_vertex(second[next], color);
            mesh.colored_vertex(second[index], color);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
    }
    mesh
}

fn paint_advanced_gpu_preview(
    painter: &Painter,
    stroke: &crate::advanced_brush::AdvancedBrushStroke,
    view: &StageView,
) {
    let dabs = crate::advanced_brush::advanced_dabs(stroke);
    if dabs.is_empty() {
        return;
    }
    let settings = stroke.settings.sanitized();
    if settings.glow {
        // Preview-only soft halo approximation: a handful of translucent GPU
        // mesh passes. Commit/export use the canonical material renderer.
        for layer in (1..=5).rev() {
            let t = layer as f32 / 5.0;
            let alpha = (settings.glow_opacity * (1.0 - t).powi(2) * 90.0).clamp(0.0, 80.0) as u8;
            if alpha == 0 {
                continue;
            }
            let glow = Color32::from_rgba_unmultiplied(
                settings.color.r,
                settings.color.g,
                settings.color.b,
                alpha,
            );
            painter.add(Shape::Mesh(advanced_preview_mesh(
                &dabs,
                view,
                glow,
                settings.glow_radius * t,
            )));
        }
    }
    let body = Color32::from_rgba_unmultiplied(
        settings.color.r,
        settings.color.g,
        settings.color.b,
        settings.color.a,
    );
    painter.add(Shape::Mesh(advanced_preview_mesh(&dabs, view, body, 0.0)));
}

fn draw_in_progress_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    let red = Color32::from_rgb(0xCC, 0x33, 0x33);
    let cursor = painter.ctx().pointer_hover_pos();

    match &app.session.tool_state {
        ToolState::PenDrawing { anchors } => {
            if anchors.len() >= 2 {
                let pts: Vec<Pos2> = anchors
                    .iter()
                    .map(|a| stage_to_screen(a.point, view))
                    .collect();
                painter.add(Shape::Path(PathShape {
                    points: pts,
                    closed: false,
                    fill: Color32::TRANSPARENT,
                    stroke: Stroke::new(1.0_f32, red),
                }));
            }
            if let (Some(last), Some(c)) = (anchors.last(), cursor) {
                let from = stage_to_screen(last.point, view);
                painter.add(Shape::Path(PathShape {
                    points: vec![from, c],
                    closed: false,
                    fill: Color32::TRANSPARENT,
                    stroke: Stroke::new(1.0_f32, Color32::from_rgb(0xCC, 0x66, 0x66)),
                }));
            }
            for a in anchors {
                let s = stage_to_screen(a.point, view);
                painter.circle_filled(s, 3.5, red);
                painter.circle_stroke(s, 3.5, Stroke::new(1.0_f32, Color32::WHITE));
            }
        }
        ToolState::FreehandDrawing { points } => {
            if points.len() >= 2 {
                let stroke_color = Color32::from_rgba_unmultiplied(
                    app.session.stroke_color.r,
                    app.session.stroke_color.g,
                    app.session.stroke_color.b,
                    app.session.stroke_color.a,
                );
                let points: Vec<Pos2> = points.iter().map(|p| stage_to_screen(*p, view)).collect();
                painter.add(Shape::Path(PathShape {
                    points,
                    closed: false,
                    fill: Color32::TRANSPARENT,
                    stroke: Stroke::new(
                        app.session.stroke_width.max(0.5) * view.scale,
                        stroke_color,
                    ),
                }));
            }
        }
        ToolState::BrushDrawing { stroke } => {
            let color = Color32::from_rgba_unmultiplied(
                app.session.brush.color.r,
                app.session.brush.color.g,
                app.session.brush.color.b,
                app.session.brush.color.a,
            );
            paint_classic_nib_preview(painter, stroke, view, color, None);
        }
        ToolState::AdvancedBrushDrawing { stroke } => {
            paint_advanced_gpu_preview(painter, stroke, view);
        }
        ToolState::EraserDrawing { stroke } => {
            let fill = Color32::from_rgba_unmultiplied(0xCC, 0x22, 0x22, 40);
            let outline = Color32::from_rgba_unmultiplied(0xCC, 0x22, 0x22, 220);
            paint_classic_nib_preview(painter, stroke, view, fill, Some(outline));
        }
        ToolState::PrimitiveDrawing { start, end } => {
            if let Some(kind) = primitive_kind_for_tool(app.session.current_tool) {
                if let Some(path) = primitive_path(kind, *start, *end) {
                    let points: Vec<Pos2> = flatten_path_for_stroke(&path)
                        .into_iter()
                        .map(|point| stage_to_screen(point, view))
                        .collect();
                    let fill = if path.closed {
                        app.session
                            .fill_color
                            .map_or(Color32::TRANSPARENT, |color| {
                                Color32::from_rgba_unmultiplied(
                                    color.r,
                                    color.g,
                                    color.b,
                                    ((color.a as u16 * 3) / 8) as u8,
                                )
                            })
                    } else {
                        Color32::TRANSPARENT
                    };
                    painter.add(Shape::Path(PathShape {
                        points,
                        closed: path.closed,
                        fill,
                        stroke: Stroke::new(
                            app.session.stroke_width.max(0.5) * view.scale,
                            Color32::from_rgba_unmultiplied(
                                app.session.stroke_color.r,
                                app.session.stroke_color.g,
                                app.session.stroke_color.b,
                                app.session.stroke_color.a,
                            ),
                        ),
                    }));
                }
            }
        }
        ToolState::Marquee { start } => {
            if let Some(c) = cursor {
                let a = stage_to_screen(*start, view);
                let b = c;
                let r = egui::Rect::from_two_pos(a, b);
                let fill = selection_fill_color(app, 48);
                let border = selection_color(app);
                painter.rect_filled(r, 0.0, fill);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, border));
            }
        }
        ToolState::DraggingPlacement { .. }
        | ToolState::DraggingPath { .. }
        | ToolState::DraggingPaths { .. }
        | ToolState::DraggingRawHandle { .. }
        | ToolState::DraggingRawRotate { .. }
        | ToolState::DraggingRawSkew { .. }
        | ToolState::DraggingPathPoints { .. }
        | ToolState::DraggingHandle { .. }
        | ToolState::DraggingPlacementRotate { .. }
        | ToolState::DraggingPlacementSkew { .. }
        | ToolState::DraggingGroup { .. }
        | ToolState::DraggingTransformPivot { .. }
        | ToolState::Idle => {}
    }
}

/// Draw selection bounds using the current theme accent.
pub fn draw_selection_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    draw_selection_content_overlay(app, painter, view);
    draw_group_transform_frame(app, painter, view);
    draw_transform_pivot_overlay(app, painter, view);
}

fn draw_selection_content_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    let accent = selection_color(app);
    if let Selection::Multi(refs) = &app.session.selection {
        for r in refs {
            let Some(q) = app
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == r.q0rg_id)
            else {
                continue;
            };
            let Some(layer) = q.layers.iter().find(|l| l.layer_id == r.layer_id) else {
                continue;
            };
            let Some(p) = layer.placements.get(r.placement_idx) else {
                continue;
            };
            let Some(transform) = crate::render::active_transform_for_placement(
                layer,
                r.placement_idx,
                app.session.current_frame,
            ) else {
                continue;
            };
            let mut visual = p.clone();
            visual.transform = transform;
            let Some((min_x, min_y, max_x, max_y)) = placement_bbox(&app.state.project, &visual)
            else {
                continue;
            };
            let mut rect = egui::Rect::from_min_max(
                stage_to_screen(Vec2::new(min_x, min_y), view),
                stage_to_screen(Vec2::new(max_x, max_y), view),
            );
            rect = rect.expand(2.0);
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(2.0_f32, Color32::from_black_alpha(180)),
            );
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, accent));
        }
        return;
    }
    if let Selection::RawArea {
        ref placements,
        ref objects,
        bounds_min,
        bounds_max,
    } = app.session.selection
    {
        draw_raw_area_selection(
            app,
            painter,
            view,
            placements,
            bounds_min,
            bounds_max,
            objects.is_empty(),
        );
        for reference in objects {
            let Some(layer) = app
                .state
                .project
                .q0rgs
                .iter()
                .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
                .and_then(|q0rg| {
                    q0rg.layers
                        .iter()
                        .find(|layer| layer.layer_id == reference.layer_id)
                })
            else {
                continue;
            };
            let Some(placement) = layer.placements.get(reference.placement_idx) else {
                continue;
            };
            let Some(transform) = crate::render::active_transform_for_placement(
                layer,
                reference.placement_idx,
                app.session.current_frame,
            ) else {
                continue;
            };
            let mut visual = placement.clone();
            visual.transform = transform;
            let Some((min_x, min_y, max_x, max_y)) = placement_bbox(&app.state.project, &visual)
            else {
                continue;
            };
            let rect = egui::Rect::from_min_max(
                stage_to_screen(Vec2::new(min_x, min_y), view),
                stage_to_screen(Vec2::new(max_x, max_y), view),
            )
            .expand(2.0);
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(2.0_f32, Color32::from_black_alpha(180)),
            );
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, accent));
        }
        return;
    }
    if let Selection::Mixed {
        ref paths,
        ref objects,
    } = app.session.selection
    {
        draw_raw_paths_overlay(app, painter, view, paths, false);
        draw_object_reference_outlines(app, painter, view, objects);
        return;
    }
    if let Selection::PathPoints {
        path,
        ref anchor_indices,
        bounds_min,
        bounds_max,
    } = app.session.selection
    {
        draw_partial_raw_selection(
            app,
            painter,
            view,
            path,
            anchor_indices,
            bounds_min,
            bounds_max,
        );
        return;
    }
    if let Selection::Paths(ref refs) = app.session.selection {
        draw_raw_paths_overlay(app, painter, view, refs, true);
        return;
    }
    if let Selection::Path {
        q0rg_id,
        layer_id,
        placement_idx,
        path_idx,
    } = app.session.selection
    {
        if app.session.current_tool != Tool::Subselect {
            draw_whole_raw_fill_overlay(
                app,
                painter,
                view,
                PathRef {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                    path_idx,
                },
            );
            return;
        }
        let Some(path) = raw_path_clone(
            &app.state.project,
            q0rg_id,
            layer_id,
            placement_idx,
            path_idx,
        ) else {
            return;
        };
        let raw_points: Vec<Pos2> = flatten_path(&path)
            .into_iter()
            .map(|point| stage_to_screen(point, view))
            .collect();
        let points = crate::render::sanitize_display_polyline(&raw_points, 2.0, path.closed);
        if points.len() >= 2 {
            let closed = path.closed;
            painter.add(Shape::Path(PathShape {
                points,
                closed,
                fill: Color32::TRANSPARENT,
                stroke: Stroke::new(3.0_f32, Color32::from_black_alpha(210)),
            }));
            painter.add(Shape::Path(PathShape {
                points: flatten_path(&path)
                    .into_iter()
                    .map(|point| stage_to_screen(point, view))
                    .collect(),
                closed,
                fill: Color32::TRANSPARENT,
                stroke: Stroke::new(1.5_f32, accent),
            }));

            // Cap the anchor count so dense contours remain legible.
            let step = (path.anchors.len() / 64).max(1);
            for anchor in path.anchors.iter().step_by(step) {
                let point = stage_to_screen(anchor.point, view);
                painter.circle_filled(point, 2.8, Color32::BLACK);
                painter.circle_filled(point, 1.7, accent);
            }
        }
        return;
    }
    if let Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = app.session.selection
    {
        let Some(q) = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
        else {
            return;
        };
        let Some(layer) = q.layers.iter().find(|l| l.layer_id == layer_id) else {
            return;
        };
        let Some(p) = layer.placements.get(placement_idx) else {
            return;
        };
        let Some(active_transform) = crate::render::active_transform_for_placement(
            layer,
            placement_idx,
            app.session.current_frame,
        ) else {
            return;
        };
        let mut visual = p.clone();
        visual.transform = active_transform;
        let Some(local_bbox) = placement_local_bbox(&app.state.project, &visual) else {
            // Empty q0rg (no children with bbox): show a tiny marker at the
            // placement origin so something is still visible.
            let center = stage_to_screen(Vec2::new(active_transform.tx, active_transform.ty), view);
            painter.circle_stroke(center, 8.0, Stroke::new(2.0_f32, Color32::BLACK));
            painter.circle_stroke(center, 8.0, Stroke::new(1.0_f32, accent));
            return;
        };
        let frame = placement_transform_frame(local_bbox, active_transform);
        let outline: Vec<Pos2> = frame
            .corners
            .iter()
            .map(|point| stage_to_screen(*point, view))
            .collect();

        // Use the same oriented frame for rendering, hit-testing and drag
        // math. Rotated/skewed q0rg, bitmap and vector instances therefore no
        // longer show a misleading axis-aligned box.
        painter.add(Shape::Path(PathShape {
            points: outline.clone(),
            closed: true,
            fill: Color32::TRANSPARENT,
            stroke: Stroke::new(3.0_f32, Color32::from_black_alpha(220)),
        }));
        painter.add(Shape::Path(PathShape {
            points: outline,
            closed: true,
            fill: Color32::TRANSPARENT,
            stroke: Stroke::new(1.5_f32, accent),
        }));

        if supports_axis_resize(active_transform) {
            for (_, world) in frame_handle_positions(frame) {
                let point = stage_to_screen(world, view);
                let handle = egui::Rect::from_center_size(point, egui::vec2(9.0, 9.0));
                painter.rect_filled(handle, 0.0, Color32::WHITE);
                painter.rect_stroke(handle, 0.0, Stroke::new(1.5_f32, Color32::BLACK));
            }
        }
    }
}

fn draw_object_reference_outlines(
    app: &EditorApp,
    painter: &Painter,
    view: &StageView,
    objects: &[PlacementRef],
) {
    let accent = selection_color(app);
    for reference in objects {
        let Some(layer) = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.layer_id == reference.layer_id)
            })
        else {
            continue;
        };
        let Some(placement) = layer.placements.get(reference.placement_idx) else {
            continue;
        };
        let Some(transform) = crate::render::active_transform_for_placement(
            layer,
            reference.placement_idx,
            app.session.current_frame,
        ) else {
            continue;
        };
        let mut visual = placement.clone();
        visual.transform = transform;
        let Some((min_x, min_y, max_x, max_y)) = placement_bbox(&app.state.project, &visual) else {
            continue;
        };
        let rect = egui::Rect::from_min_max(
            stage_to_screen(Vec2::new(min_x, min_y), view),
            stage_to_screen(Vec2::new(max_x, max_y), view),
        )
        .expand(2.0);
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(2.0_f32, Color32::from_black_alpha(180)),
        );
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, accent));
    }
}

fn draw_group_transform_frame(app: &EditorApp, painter: &Painter, view: &StageView) {
    let needs_group_frame = match &app.session.selection {
        Selection::RawArea { objects, .. } => !objects.is_empty(),
        Selection::Mixed { .. } | Selection::Multi(_) => true,
        _ => false,
    };
    if !needs_group_frame {
        return;
    }
    let Some(bounds) = selection_transform_bounds(app) else {
        return;
    };
    let rect = egui::Rect::from_min_max(
        stage_to_screen(Vec2::new(bounds.0, bounds.1), view),
        stage_to_screen(Vec2::new(bounds.2, bounds.3), view),
    );
    draw_flash_selection_box(painter, rect, selection_color(app));
}

fn draw_transform_pivot_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    if app.session.current_tool != Tool::Select || selection_transform_bounds(app).is_none() {
        return;
    }
    let Some(pivot) = selection_transform_pivot(app) else {
        return;
    };
    let point = stage_to_screen(pivot, view);
    let accent = selection_color(app);
    painter.circle_filled(point, 5.0, Color32::WHITE);
    painter.circle_stroke(point, 6.0, Stroke::new(2.0_f32, Color32::BLACK));
    painter.circle_stroke(point, 5.0, Stroke::new(1.5_f32, accent));
    painter.line_segment(
        [
            Pos2::new(point.x - 8.0, point.y),
            Pos2::new(point.x + 8.0, point.y),
        ],
        Stroke::new(1.0_f32, accent),
    );
    painter.line_segment(
        [
            Pos2::new(point.x, point.y - 8.0),
            Pos2::new(point.x, point.y + 8.0),
        ],
        Stroke::new(1.0_f32, accent),
    );
}

const SELECTION_STIPPLE_SPACING_PX: f32 = 4.0;
const SELECTION_CONTOUR_SPACING_PX: f32 = 1.0;

fn clip_segment_to_rect(a: Pos2, b: Pos2, rect: egui::Rect) -> Option<(Pos2, Pos2)> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t0 = 0.0_f32;
    let mut t1 = 1.0_f32;
    for (p, q) in [
        (-dx, a.x - rect.left()),
        (dx, rect.right() - a.x),
        (-dy, a.y - rect.top()),
        (dy, rect.bottom() - a.y),
    ] {
        if p.abs() <= f32::EPSILON {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            t0 = t0.max(r);
        } else {
            t1 = t1.min(r);
        }
        if t0 > t1 {
            return None;
        }
    }
    Some((a + (b - a) * t0, a + (b - a) * t1))
}

fn dense_selection_contour_points(contours: &[Vec<Pos2>], clip_rect: egui::Rect) -> Vec<Pos2> {
    let mut points = Vec::new();
    for contour in contours {
        if contour.len() < 2 {
            continue;
        }
        for (a, b) in contour
            .iter()
            .copied()
            .zip(contour.iter().copied().cycle().skip(1))
            .take(contour.len())
        {
            let Some((visible_a, visible_b)) = clip_segment_to_rect(a, b, clip_rect) else {
                continue;
            };
            let delta = visible_b - visible_a;
            let length = delta.length();
            if length <= f32::EPSILON {
                continue;
            }
            let direction = delta / length;
            // Screen-space spacing is intentionally fixed. This mimics the
            // dense Flash/Animate selection cue even through smooth zoom.
            let mut distance = 0.0_f32;
            while distance <= length {
                points.push(visible_a + direction * distance);
                distance += SELECTION_CONTOUR_SPACING_PX;
            }
        }
    }
    points
}

fn draw_dense_selection_contour(painter: &Painter, contours: &[Vec<Pos2>], accent: Color32) {
    let points = dense_selection_contour_points(contours, painter.clip_rect());
    // One mesh per colour instead of one epaint shape per dot. At Animate-like
    // density a viewport can contain tens of thousands of contour samples.
    draw_stipple_batch(painter, &points, 0.45, Color32::from_black_alpha(220));
    draw_stipple_batch(painter, &points, 0.24, accent);
}

#[cfg(feature = "appearance-mask-eraser")]
fn fixed_selection_grid_start(min: f32) -> f32 {
    (min / SELECTION_STIPPLE_SPACING_PX).ceil() * SELECTION_STIPPLE_SPACING_PX
}

fn selection_stipple_texture(painter: &Painter) -> TextureHandle {
    let id = egui::Id::new("q0editor.selection-stipple-texture.v1");
    if let Some(handle) = painter
        .ctx()
        .data(|data| data.get_temp::<TextureHandle>(id))
    {
        return handle;
    }
    let side = SELECTION_STIPPLE_SPACING_PX.round().max(2.0) as usize;
    let mut rgba = vec![0_u8; side * side * 4];
    rgba[0..4].copy_from_slice(&[255, 255, 255, 255]);
    let image = ColorImage::from_rgba_unmultiplied([side, side], &rgba);
    let handle = painter.ctx().load_texture(
        "q0editor-selection-stipple",
        image,
        TextureOptions::NEAREST_REPEAT,
    );
    painter
        .ctx()
        .data_mut(|data| data.insert_temp(id, handle.clone()));
    handle
}

fn paint_selection_stipple_pattern(painter: &Painter, contours: &[Vec<Pos2>]) {
    if contours.is_empty() {
        return;
    }
    let texture = selection_stipple_texture(painter);
    crate::render::paint_complex_fill_pattern(
        painter,
        contours,
        texture.id(),
        SELECTION_STIPPLE_SPACING_PX,
    );
}

fn stipple_mesh(points: &[Pos2], half_size: f32, color: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(points.len().saturating_mul(4));
    mesh.indices.reserve(points.len().saturating_mul(6));
    for point in points {
        let base = mesh.vertices.len() as u32;
        let min = Pos2::new(point.x - half_size, point.y - half_size);
        let max = Pos2::new(point.x + half_size, point.y + half_size);
        for pos in [
            Pos2::new(min.x, min.y),
            Pos2::new(max.x, min.y),
            Pos2::new(max.x, max.y),
            Pos2::new(min.x, max.y),
        ] {
            mesh.vertices.push(Vertex {
                pos,
                uv: Pos2::ZERO,
                color,
            });
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    mesh
}

fn draw_stipple_batch(painter: &Painter, points: &[Pos2], half_size: f32, color: Color32) {
    if points.is_empty() {
        return;
    }
    painter.add(Shape::Mesh(stipple_mesh(points, half_size, color)));
}

#[cfg(feature = "appearance-mask-eraser")]
fn selection_stipple_step() -> f32 {
    SELECTION_STIPPLE_SPACING_PX
}

#[cfg(feature = "appearance-mask-eraser")]
fn appearance_selection_stipple_points(
    view: &StageView,
    rect: egui::Rect,
    tester: &crate::appearance::VisibleMaterialHitTester,
    subset_support: Option<(&MultiPolygon<f64>, q0s_format::v2::VectorMaterial)>,
) -> Vec<Pos2> {
    let step = selection_stipple_step();
    let mut points = Vec::new();
    let mut y = fixed_selection_grid_start(rect.top());
    while y <= rect.bottom() {
        let mut x = fixed_selection_grid_start(rect.left());
        while x <= rect.right() {
            let world = Vec2::new(
                (x - view.origin.x) / view.scale,
                (y - view.origin.y) / view.scale,
            );
            let subset_hit = subset_support.is_none_or(|(surface, material)| {
                crate::appearance::material_support_contains_point(
                    surface,
                    material,
                    tester.canonical_point(world),
                    0.0,
                )
            });
            if subset_hit && tester.contains(world, 0.0) {
                points.push(Pos2::new(x, y));
            }
            x += step;
        }
        y += step;
    }
    points
}

#[cfg(feature = "appearance-mask-eraser")]
fn draw_appearance_selection_stipple(
    painter: &Painter,
    view: &StageView,
    rect: egui::Rect,
    tester: &crate::appearance::VisibleMaterialHitTester,
    subset_support: Option<(&MultiPolygon<f64>, q0s_format::v2::VectorMaterial)>,
) {
    let sample_rect = rect.intersect(painter.clip_rect());
    if !sample_rect.is_positive() {
        return;
    }
    let clipped = painter.with_clip_rect(sample_rect);
    let points = appearance_selection_stipple_points(view, sample_rect, tester, subset_support);
    draw_stipple_batch(&clipped, &points, 0.38, Color32::WHITE);
}

fn draw_raw_area_selection(
    app: &EditorApp,
    painter: &Painter,
    view: &StageView,
    placements: &[PlacementRef],
    bounds_min: Vec2,
    bounds_max: Vec2,
    draw_box: bool,
) {
    let selection_rect = egui::Rect::from_two_pos(
        stage_to_screen(bounds_min, view),
        stage_to_screen(bounds_max, view),
    );
    let visible_rect = selection_rect.intersect(painter.clip_rect());
    let clipped = painter.with_clip_rect(visible_rect);
    let mut surfaces = Vec::new();
    for r in placements {
        let Some(placement) = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == r.q0rg_id)
            .and_then(|q| q.layers.iter().find(|l| l.layer_id == r.layer_id))
            .and_then(|l| l.placements.get(r.placement_idx))
        else {
            continue;
        };
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(Asset::Vector(vector)) =
            app.state.project.assets.iter().find(|a| a.id() == asset_id)
        else {
            continue;
        };
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = app.state.project.asset_appearances.get(&asset_id) {
            if let Some(tester) =
                crate::appearance::prepare_visible_material_hit_tester(vector, Some(appearance))
            {
                draw_appearance_selection_stipple(painter, view, selection_rect, &tester, None);
            }
            let body =
                crate::appearance::visible_source_surface_for_vector(vector, Some(appearance));
            let body_contours = surface_to_screen_contours(&body, view);
            draw_dense_selection_contour(&clipped, &body_contours, selection_color(app));
            // Do not add the appearance to `surfaces`: doing so would rebuild
            // the expensive buffered glow for the legacy stipple pass below.
            continue;
        }
        let surface = raw_selectable_fill_surface(&app.state.project, asset_id, vector);
        let contours = surface_to_screen_contours(&surface, view);
        crate::render::paint_complex_fill(&clipped, &contours, selection_fill_color(app, 64));
        draw_dense_selection_contour(&clipped, &contours, selection_color(app));
        surfaces.push(surface);
    }

    // Animate-style stipple is evaluated against one unioned surface, so holes
    // remain empty. Density is fixed in screen space at every zoom; only the
    // visible viewport is sampled so off-screen geometry does not create work.
    let surface = geo::unary_union(surfaces.iter());
    let stipple_contours = surface_to_screen_contours(&surface, view);
    paint_selection_stipple_pattern(&clipped, &stipple_contours);
    if draw_box {
        draw_flash_selection_box(painter, selection_rect, selection_color(app));
    }
}

fn draw_raw_paths_overlay(
    app: &EditorApp,
    painter: &Painter,
    view: &StageView,
    refs: &[PathRef],
    draw_boxes: bool,
) {
    // Path refs may span multiple internal carrier placements after appearance
    // splitting. That is an implementation detail: one logical selection gets
    // exactly one transform frame. Per-carrier boxes make a single glow look
    // like two independent selections.
    let selection_frame = draw_boxes
        .then(|| raw_path_refs_ui_bounds(&app.state.project, refs))
        .flatten();
    let mut grouped: std::collections::BTreeMap<(u16, u16, usize), Vec<usize>> =
        std::collections::BTreeMap::new();
    for reference in refs {
        grouped
            .entry((
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
            ))
            .or_default()
            .push(reference.path_idx);
    }

    for ((q0rg_id, layer_id, placement_idx), mut path_indices) in grouped {
        path_indices.sort_unstable();
        path_indices.dedup();
        let Some(placement) = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| layer.placements.get(placement_idx))
        else {
            continue;
        };
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
        else {
            continue;
        };

        // Open paths remain ordinary path overlays. Closed fill refs use the
        // resolved visual material surface, so erased mask regions disappear
        // from the stipple/outline while soft halo becomes selectable artwork.
        for path_idx in &path_indices {
            let Some(path) = vector.paths.get(*path_idx) else {
                continue;
            };
            if path.closed && vector.fill.is_some() {
                continue;
            }
            let raw_points: Vec<Pos2> = flatten_path(path)
                .into_iter()
                .map(|point| stage_to_screen(point, view))
                .collect();
            let points = crate::render::sanitize_display_polyline(&raw_points, 2.0, path.closed);
            if points.len() >= 2 {
                painter.add(Shape::Path(PathShape {
                    points: points.clone(),
                    closed: path.closed,
                    fill: Color32::TRANSPARENT,
                    stroke: Stroke::new(3.0_f32, Color32::from_black_alpha(210)),
                }));
                painter.add(Shape::Path(PathShape {
                    points,
                    closed: path.closed,
                    fill: Color32::TRANSPARENT,
                    stroke: Stroke::new(1.5_f32, selection_color(app)),
                }));
            }
        }

        let closed_indices: Vec<usize> = path_indices
            .iter()
            .copied()
            .filter(|index| {
                vector
                    .paths
                    .get(*index)
                    .is_some_and(|path| path.closed && vector.fill.is_some())
            })
            .collect();
        if closed_indices.is_empty() {
            continue;
        }
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = app.state.project.asset_appearances.get(&asset_id) {
            let fast_bounds = crate::appearance::fast_visible_material_bounds_for_paths(
                vector,
                Some(appearance),
                &closed_indices,
            );
            let fallback_bounds = || {
                let mut bounds: Option<(f32, f32, f32, f32)> = None;
                for index in &closed_indices {
                    let Some(path) = vector.paths.get(*index) else {
                        continue;
                    };
                    for anchor in &path.anchors {
                        let point = anchor.point;
                        bounds = Some(match bounds {
                            Some((min_x, min_y, max_x, max_y)) => (
                                min_x.min(point.x),
                                min_y.min(point.y),
                                max_x.max(point.x),
                                max_y.max(point.y),
                            ),
                            None => (point.x, point.y, point.x, point.y),
                        });
                    }
                }
                bounds
            };
            if let Some((min_x, min_y, max_x, max_y)) = fast_bounds.or_else(fallback_bounds) {
                let rect = egui::Rect::from_min_max(
                    stage_to_screen(Vec2::new(min_x, min_y), view),
                    stage_to_screen(Vec2::new(max_x, max_y), view),
                );
                let subset = VectorAsset {
                    asset_id: vector.asset_id,
                    paths: closed_indices
                        .iter()
                        .filter_map(|index| vector.paths.get(*index).cloned())
                        .collect(),
                    fill: vector.fill,
                    stroke: None,
                };
                let body =
                    crate::appearance::visible_source_surface_for_vector(&subset, Some(appearance));
                let body_contours = surface_to_screen_contours(&body, view);
                draw_dense_selection_contour(painter, &body_contours, selection_color(app));
                if let Some(tester) =
                    crate::appearance::prepare_visible_material_hit_tester(vector, Some(appearance))
                {
                    let subset_surface = vector_fill_geometry(&subset);
                    let canonical_subset = appearance.field_transform.inverse().map(|inverse| {
                        crate::appearance::transform_surface(&subset_surface, inverse)
                    });
                    let subset_support = canonical_subset
                        .as_ref()
                        .map(|surface| (surface, appearance.material));
                    draw_appearance_selection_stipple(painter, view, rect, &tester, subset_support);
                }
            }
            continue;
        }

        let surface =
            raw_selectable_paths_surface(&app.state.project, asset_id, vector, &closed_indices);
        let contours = surface_to_screen_contours(&surface, view);
        if contours.is_empty() {
            continue;
        }
        crate::render::paint_complex_fill(painter, &contours, selection_fill_color(app, 64));
        draw_dense_selection_contour(painter, &contours, selection_color(app));
        for contour in &contours {
            let outline = crate::render::sanitize_display_polyline(contour, 2.0, true);
            if outline.len() >= 3 {
                painter.add(Shape::Path(PathShape {
                    points: outline,
                    closed: true,
                    fill: Color32::TRANSPARENT,
                    stroke: Stroke::new(1.5_f32, selection_color(app)),
                }));
            }
        }
    }

    if let Some((min_x, min_y, max_x, max_y)) = selection_frame {
        let rect = egui::Rect::from_min_max(
            stage_to_screen(Vec2::new(min_x, min_y), view),
            stage_to_screen(Vec2::new(max_x, max_y), view),
        );
        draw_flash_selection_box(painter, rect, selection_color(app));
    }
}

fn draw_whole_raw_fill_overlay(app: &EditorApp, painter: &Painter, view: &StageView, r: PathRef) {
    draw_raw_paths_overlay(app, painter, view, &[r], true);
}

fn draw_partial_raw_selection(
    app: &EditorApp,
    painter: &Painter,
    view: &StageView,
    r: PathRef,
    _anchor_indices: &[usize],
    bounds_min: Vec2,
    bounds_max: Vec2,
) {
    let Some(path) = raw_path_clone(
        &app.state.project,
        r.q0rg_id,
        r.layer_id,
        r.placement_idx,
        r.path_idx,
    ) else {
        return;
    };
    let local = flatten_path(&path);
    let screen: Vec<Pos2> = local
        .iter()
        .copied()
        .map(|point| stage_to_screen(point, view))
        .collect();
    let selection_rect = egui::Rect::from_two_pos(
        stage_to_screen(bounds_min, view),
        stage_to_screen(bounds_max, view),
    );
    let visible_rect = selection_rect.intersect(painter.clip_rect());
    let clipped = painter.with_clip_rect(visible_rect);
    crate::render::paint_concave_fill(&clipped, &screen, selection_fill_color(app, 64));
    if path.closed && screen.len() >= 3 {
        draw_dense_selection_contour(
            &clipped,
            std::slice::from_ref(&screen),
            selection_color(app),
        );
    }

    // Keep the dotted raw-area cue at one fixed screen-space density. Work is
    // clipped to the viewport rather than thinning the pattern on large areas.
    paint_selection_stipple_pattern(&clipped, std::slice::from_ref(&screen));
    draw_flash_selection_box(painter, selection_rect, selection_color(app));
}

fn draw_flash_selection_box(painter: &Painter, rect: egui::Rect, accent: Color32) {
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, accent));
    for point in [
        rect.left_top(),
        rect.center_top(),
        rect.right_top(),
        rect.right_center(),
        rect.right_bottom(),
        rect.center_bottom(),
        rect.left_bottom(),
        rect.left_center(),
    ] {
        let handle = egui::Rect::from_center_size(point, egui::vec2(7.0, 7.0));
        painter.rect_filled(handle, 0.0, Color32::BLACK);
        painter.rect_stroke(handle, 0.0, Stroke::new(1.0_f32, accent));
    }
}

// ---------------- Eyedropper ----------------

#[derive(Clone, Copy)]
enum SampledPaint {
    Fill(Rgba),
    Stroke(VStroke),
}

fn eyedropper(app: &mut EditorApp, response: &Response, cursor: Option<Vec2>, view: &StageView) {
    if !response.clicked_by(PointerButton::Primary) {
        return;
    }
    let Some(cursor) = cursor else {
        return;
    };
    let tolerance = 4.0 / view.scale.max(0.001);
    let sampled = sample_paint_in_q0rg(
        &app.state.project,
        app.session.current_q0rg_id,
        app.session.current_frame,
        Affine::IDENTITY,
        cursor,
        tolerance,
        0,
    );
    match sampled {
        Some(SampledPaint::Fill(color)) => {
            app.session.fill_color = Some(color);
            app.session.status = "Fill sampled".to_string();
        }
        Some(SampledPaint::Stroke(stroke)) => {
            app.session.stroke_color = stroke.color;
            app.session.stroke_width = stroke.width.max(0.1);
            app.session.brush_cap = stroke.cap;
            app.session.status = "Stroke sampled".to_string();
        }
        None => {
            app.session.status = "No vector paint under pointer".to_string();
        }
    }
}

fn sample_paint_in_q0rg(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    cursor: Vec2,
    tolerance: f32,
    depth: u8,
) -> Option<SampledPaint> {
    const RECURSION_LIMIT: u8 = 16;
    if depth > RECURSION_LIMIT {
        return None;
    }
    let q0rg = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };

    for layer in q0rg.layers.iter().rev() {
        let active = crate::render::active_placements_at(layer, local_frame);
        for (placement_idx, transform) in active.into_iter().rev() {
            let Some(placement) = layer.placements.get(placement_idx) else {
                continue;
            };
            let composed = Affine::compose(parent, Affine::from_transform(transform));
            match placement.target {
                Target::Asset(asset_id) => {
                    let Some(Asset::Vector(vector)) =
                        project.assets.iter().find(|asset| asset.id() == asset_id)
                    else {
                        continue;
                    };
                    if let Some(sampled) = sample_vector_paint(vector, composed, cursor, tolerance)
                    {
                        return Some(sampled);
                    }
                }
                Target::Q0rg(child_id) if child_id != q0rg_id => {
                    if let Some(sampled) = sample_paint_in_q0rg(
                        project,
                        child_id,
                        local_frame,
                        composed,
                        cursor,
                        tolerance,
                        depth + 1,
                    ) {
                        return Some(sampled);
                    }
                }
                Target::Q0rg(_) => {}
            }
        }
    }
    None
}

fn sample_vector_paint(
    vector: &VectorAsset,
    transform: Affine,
    cursor: Vec2,
    tolerance: f32,
) -> Option<SampledPaint> {
    let local_cursor = transform.inverse()?.apply(cursor);
    let local_tolerance = tolerance / transform.uniform_scale().max(0.0001);

    if let Some(stroke) = vector.stroke.filter(|stroke| stroke.color.a > 0) {
        let hit_radius = stroke.width.max(0.5) * 0.5 + local_tolerance;
        let hit = vector.paths.iter().rev().any(|path| {
            let points = flatten_path(path);
            points.len() >= 2 && nearest_segment_distance(&points, local_cursor) <= hit_radius
        });
        if hit {
            return Some(SampledPaint::Stroke(stroke));
        }
    }

    if let Some(fill) = vector.fill.filter(|fill| fill.a > 0) {
        let point = Point::new(local_cursor.x as f64, local_cursor.y as f64);
        let boundary_hit = vector.paths.iter().filter(|path| path.closed).any(|path| {
            let points = flatten_path(path);
            nearest_segment_distance(&points, local_cursor) <= local_tolerance
        });
        if vector_fill_geometry(vector).contains(&point) || boundary_hit {
            return Some(SampledPaint::Fill(fill));
        }
    }

    None
}

// ---------------- Paint Bucket ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BucketTargetKind {
    ExistingFill,
    EmptyBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BucketTarget {
    layer_id: u16,
    placement_idx: usize,
    asset_id: u16,
    path_indices: Vec<usize>,
    kind: BucketTargetKind,
}

fn bucket(app: &mut EditorApp, response: &Response, cursor: Option<Vec2>) {
    if !response.clicked_by(PointerButton::Primary) {
        return;
    }
    let Some(point) = cursor else {
        return;
    };
    if !bucket_fill_at(app, point) {
        app.session.status = "No enclosed region under pointer".to_string();
    }
}

fn raw_fill_surface_on_layer(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> MultiPolygon<f64> {
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return MultiPolygon(Vec::new());
    };
    let surfaces: Vec<MultiPolygon<f64>> = active_raw_placement_indices(project, layer, frame)
        .into_iter()
        .filter_map(|placement_idx| {
            let placement = layer.placements.get(placement_idx)?;
            let Target::Asset(asset_id) = placement.target else {
                return None;
            };
            let Asset::Vector(vector) =
                project.assets.iter().find(|asset| asset.id() == asset_id)?
            else {
                return None;
            };
            vector.fill.map(|_| vector_fill_geometry(vector))
        })
        .collect();
    geo::unary_union(surfaces.iter())
}

/// Fill exactly one connected raw region. A bucket click never recolours every
/// contour merely because several drawings share one VectorAsset. The selected
/// contours are split into their own fill-only/style-preserving raw asset when
/// neighbouring contours must keep their old paint.
fn bucket_fill_at(app: &mut EditorApp, point: Vec2) -> bool {
    let q0rg_id = app.session.current_q0rg_id;
    let layer_frame = app.session.current_frame;
    let Some(target) = find_bucket_target(&app.state.project, q0rg_id, layer_frame, point) else {
        return false;
    };
    let color = app.session.fill_color.unwrap_or(Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    });

    app.history.snapshot(&app.state.project);
    let Some(mapping) = materialize_layer_keyframe_for_edit(
        &mut app.state.project,
        q0rg_id,
        target.layer_id,
        layer_frame,
    ) else {
        return false;
    };
    let Some(&placement_idx) = mapping.get(&target.placement_idx) else {
        return false;
    };
    let current_asset_id = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .find(|layer| layer.layer_id == target.layer_id)
        })
        .and_then(|layer| layer.placements.get(placement_idx))
        .and_then(|placement| match placement.target {
            Target::Asset(asset_id) => Some(asset_id),
            Target::Q0rg(_) => None,
        });
    let Some(current_asset_id) = current_asset_id else {
        return false;
    };
    let writable = crate::brush::prepare_writable_raw_assets(
        &mut app.state.project,
        q0rg_id,
        target.layer_id,
        layer_frame,
        &std::collections::BTreeSet::from([current_asset_id]),
    );
    let asset_id = writable
        .get(&current_asset_id)
        .copied()
        .unwrap_or(current_asset_id);

    let Some(source) = app
        .state
        .project
        .assets
        .iter()
        .find(|asset| asset.id() == asset_id)
        .and_then(|asset| match asset {
            Asset::Vector(vector) => Some(vector.clone()),
            Asset::Bitmap(_) | Asset::Q0v(_) => None,
        })
    else {
        return false;
    };

    let mut indices = target.path_indices.clone();
    indices.sort_unstable();
    indices.dedup();
    if indices.is_empty() || indices.iter().any(|index| *index >= source.paths.len()) {
        return false;
    }
    let selected_paths: Vec<VPath> = indices
        .iter()
        .map(|index| source.paths[*index].clone())
        .collect();
    let selected_surface = vector_fill_geometry(&VectorAsset {
        asset_id: 0,
        paths: selected_paths.clone(),
        fill: Some(color),
        stroke: None,
    });
    if selected_surface.0.is_empty() {
        return false;
    }

    let open_paths: Vec<VPath> = source
        .paths
        .iter()
        .filter(|path| !path.closed)
        .cloned()
        .collect();
    let old_surface = source
        .fill
        .map(|_| vector_fill_geometry(&source))
        .unwrap_or_else(|| MultiPolygon(Vec::new()));
    let bucket_surface = match target.kind {
        BucketTargetKind::ExistingFill => selected_surface.clone(),
        BucketTargetKind::EmptyBoundary => {
            // Filling an empty region must not erase or cover artwork that was
            // already drawn inside it. Cut the new colour around every existing
            // raw fill on this layer; stroke-only boundaries stay visible because
            // the new placement is inserted underneath all raw graphics.
            let occupied = raw_fill_surface_on_layer(
                &app.state.project,
                q0rg_id,
                target.layer_id,
                layer_frame,
            );
            selected_surface.difference(&occupied)
        }
    };
    if bucket_surface.unsigned_area() <= 0.05 {
        return false;
    }
    let remaining_surface = match target.kind {
        BucketTargetKind::ExistingFill => old_surface.difference(&selected_surface),
        BucketTargetKind::EmptyBoundary => old_surface.clone(),
    };
    let remaining_fill_paths = geo_multi_polygon_to_linear_paths(&remaining_surface);
    let selected_fill_paths = geo_multi_polygon_to_linear_paths(&bucket_surface);
    if selected_fill_paths.is_empty() {
        return false;
    }

    // Recolouring an existing connected fill may reuse its source asset. Filling
    // an empty boundary is non-destructive: the source artwork remains intact.
    let reuse_source = target.kind == BucketTargetKind::ExistingFill
        && source.fill.is_some()
        && remaining_surface.unsigned_area() <= 0.05
        && open_paths.is_empty();
    if reuse_source {
        if let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == asset_id)
        {
            vector.paths = selected_fill_paths;
            vector.fill = Some(color);
            vector.stroke = source.stroke;
        }
    } else {
        if target.kind == BucketTargetKind::ExistingFill && source.fill.is_some() {
            if let Some(Asset::Vector(vector)) = app
                .state
                .project
                .assets
                .iter_mut()
                .find(|asset| asset.id() == asset_id)
            {
                let mut paths = open_paths;
                paths.extend(remaining_fill_paths);
                vector.paths = paths;
                vector.fill = source.fill;
                vector.stroke = source.stroke;
            }
        }

        let new_asset_id = next_asset_id(&app.state.project);
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: new_asset_id,
            paths: selected_fill_paths,
            fill: Some(color),
            // Bucket fill never steals or duplicates the enclosing outline.
            stroke: None,
        }));
        let insertion = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.layer_id == target.layer_id)
            })
            .map(|layer| match target.kind {
                BucketTargetKind::EmptyBoundary => {
                    active_raw_placement_indices(&app.state.project, layer, layer_frame)
                        .into_iter()
                        .min()
                        .unwrap_or(placement_idx)
                }
                BucketTargetKind::ExistingFill if source.fill.is_none() => placement_idx,
                BucketTargetKind::ExistingFill => placement_idx + 1,
            })
            .unwrap_or(placement_idx);
        if let Some(layer) = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == target.layer_id)
            })
        {
            // Empty-region fills belong underneath all current raw graphics so
            // enclosing contours and any artwork already inside remain visible.
            // Recolouring an existing component can stay next to its remainder.
            layer.placements.insert(
                insertion.min(layer.placements.len()),
                Placement {
                    frame: layer_frame,
                    target: Target::Asset(new_asset_id),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                },
            );
        }
    }

    app.state.dirty = true;
    app.session.selection = Selection::None;
    app.session.status = "Region filled".to_string();
    app.textures.invalidate();
    true
}

/// Resolve the frontmost enclosed raw region. Existing filled geometry uses the
/// same connected-component semantics as Select, including hole contours. A
/// closed but currently unfilled path is also a valid bucket boundary.
fn find_bucket_target(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    cursor: Vec2,
) -> Option<BucketTarget> {
    if let Some(RawSelectionHit::Fill(refs)) =
        hit_test_raw_selection(project, q0rg_id, frame, cursor)
    {
        let first = *refs.first()?;
        let placement = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == first.q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == first.layer_id)?
            .placements
            .get(first.placement_idx)?;
        let Target::Asset(asset_id) = placement.target else {
            return None;
        };
        return Some(BucketTarget {
            layer_id: first.layer_id,
            placement_idx: first.placement_idx,
            asset_id,
            path_indices: refs
                .into_iter()
                .map(|reference| reference.path_idx)
                .collect(),
            kind: BucketTargetKind::ExistingFill,
        });
    }

    let q0rg = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
    for layer in q0rg.layers.iter().rev() {
        for placement_idx in active_raw_placement_indices(project, layer, frame)
            .into_iter()
            .rev()
        {
            let placement = &layer.placements[placement_idx];
            let Target::Asset(asset_id) = placement.target else {
                continue;
            };
            let Some(Asset::Vector(vector)) =
                project.assets.iter().find(|asset| asset.id() == asset_id)
            else {
                continue;
            };
            let best = vector
                .paths
                .iter()
                .enumerate()
                .filter(|(_, path)| path.closed)
                .filter_map(|(path_idx, path)| {
                    let points = flatten_path(path);
                    (points.len() >= 3
                        && (point_in_polygon(&points, cursor)
                            || nearest_segment_distance(&points, cursor) <= 2.0))
                        .then_some((path_idx, signed_path_area(path).abs()))
                })
                .min_by(|left, right| left.1.total_cmp(&right.1));
            if let Some((path_idx, _)) = best {
                return Some(BucketTarget {
                    layer_id: layer.layer_id,
                    placement_idx,
                    asset_id,
                    path_indices: vec![path_idx],
                    kind: BucketTargetKind::EmptyBoundary,
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{Layer, ProjectMeta, Q0rg};

    #[test]
    fn rectangle_geometry_is_independent_of_drag_direction() {
        let drags = [
            (Vec2::new(10.0, 20.0), Vec2::new(50.0, 60.0)),
            (Vec2::new(50.0, 60.0), Vec2::new(10.0, 20.0)),
            (Vec2::new(50.0, 20.0), Vec2::new(10.0, 60.0)),
            (Vec2::new(10.0, 60.0), Vec2::new(50.0, 20.0)),
        ];
        for (start, end) in drags {
            let path = primitive_path(PrimitiveKind::Rect, start, end).expect("rectangle");
            assert!(signed_path_area(&path) > 0.0, "drag {start:?} -> {end:?}");
            assert_eq!(path_bbox(&path), Some((10.0, 20.0, 50.0, 60.0)));
        }
    }

    #[test]
    fn oval_geometry_has_exact_bounds_and_cubic_handles() {
        let path = primitive_path(
            PrimitiveKind::Oval,
            Vec2::new(80.0, 70.0),
            Vec2::new(20.0, 10.0),
        )
        .expect("oval");
        assert_eq!(path.anchors.len(), 4);
        assert!(path
            .anchors
            .iter()
            .all(|anchor| anchor.in_handle.is_some() && anchor.out_handle.is_some()));
        let bounds = path_bbox(&path).expect("bounds");
        assert!((bounds.0 - 20.0).abs() < 1.0e-3);
        assert!((bounds.1 - 10.0).abs() < 1.0e-3);
        assert!((bounds.2 - 80.0).abs() < 1.0e-3);
        assert!((bounds.3 - 70.0).abs() < 1.0e-3);
        assert!(signed_path_area(&path) > 0.0);
    }

    #[test]
    fn shift_constrains_rectangle_and_oval_to_equal_sides() {
        for kind in [PrimitiveKind::Rect, PrimitiveKind::Oval] {
            let start = Vec2::new(10.0, 10.0);
            let end = constrain_primitive_end(kind, start, Vec2::new(30.0, 55.0), true);
            assert_eq!(end, Vec2::new(55.0, 55.0));
            let bounds = path_bbox(&primitive_path(kind, start, end).expect("shape")).unwrap();
            assert!(((bounds.2 - bounds.0) - (bounds.3 - bounds.1)).abs() < 1.0e-3);
        }
    }

    #[test]
    fn primitive_start_must_be_inside_the_stage() {
        let app = EditorApp::default();
        assert!(primitive_point_is_on_stage(&app, Vec2::new(0.0, 0.0)));
        assert!(primitive_point_is_on_stage(
            &app,
            Vec2::new(
                app.state.project.meta.stage_width as f32,
                app.state.project.meta.stage_height as f32,
            )
        ));
        assert!(!primitive_point_is_on_stage(&app, Vec2::new(-0.1, 10.0)));
        assert!(!primitive_point_is_on_stage(
            &app,
            Vec2::new(app.state.project.meta.stage_width as f32 + 0.1, 10.0)
        ));
    }

    #[test]
    fn zero_size_primitives_are_not_committed() {
        let point = Vec2::new(12.0, 34.0);
        assert!(primitive_path(PrimitiveKind::Rect, point, point).is_none());
        assert!(primitive_path(PrimitiveKind::Oval, point, point).is_none());
        assert!(primitive_path(PrimitiveKind::Line, point, point).is_none());
    }

    fn red_stroke() -> VStroke {
        VStroke {
            color: Rgba {
                r: 220,
                g: 30,
                b: 30,
                a: 255,
            },
            width: 2.0,
            cap: q0s_format::geom::CapShape::Round,
        }
    }
    fn blue_stroke() -> VStroke {
        VStroke {
            color: Rgba {
                r: 30,
                g: 30,
                b: 220,
                a: 255,
            },
            width: 2.0,
            cap: q0s_format::geom::CapShape::Round,
        }
    }

    fn project_with_one_drawing(stroke: VStroke, frame: u16, transform: Transform2D) -> ProjectV2 {
        let asset_id = 1;
        let asset = Asset::Vector(VectorAsset {
            asset_id,
            paths: vec![VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(0.0, 0.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(10.0, 10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: false,
            }],
            fill: None,
            stroke: Some(stroke),
        });
        ProjectV2 {
            meta: ProjectMeta {
                name: "t".into(),
                fps: 24,
                stage_width: 100,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![asset],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Scene".into(),
                frame_count: 10,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "L1".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        frame,
                        target: Target::Asset(asset_id),
                        transform,
                        tween: Tween::None,
                    }],
                }],
            }],
        }
    }

    fn project_with_held_display_object() -> ProjectV2 {
        let square = VPath {
            anchors: vec![
                anchor(Vec2::new(0.0, 0.0)),
                anchor(Vec2::new(20.0, 0.0)),
                anchor(Vec2::new(20.0, 20.0)),
                anchor(Vec2::new(0.0, 20.0)),
            ],
            closed: true,
        };
        ProjectV2 {
            meta: ProjectMeta {
                name: "held-object".into(),
                fps: 24,
                stage_width: 100,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![square],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![
                Q0rg {
                    q0rg_id: 1,
                    name: "Scene".into(),
                    frame_count: 12,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        name: "Layer".into(),
                        explicit_keyframes: Vec::new(),
                        placements: vec![Placement {
                            frame: 0,
                            target: Target::Q0rg(2),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                        }],
                    }],
                },
                Q0rg {
                    q0rg_id: 2,
                    name: "Symbol".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        name: "Artwork".into(),
                        explicit_keyframes: Vec::new(),
                        placements: vec![Placement {
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                        }],
                    }],
                },
            ],
        }
    }

    /// The existing red stroke in `project_with_one_drawing` runs from
    /// (0,0) to (10,10). A new stroke whose bbox sits next to it within
    /// MERGE_TOUCH_TOLERANCE counts as touching.
    fn touching_bbox() -> (f32, f32, f32, f32) {
        // bbox right next to the existing 0..10 segment, well within tolerance
        (8.0, 8.0, 20.0, 20.0)
    }
    fn far_bbox() -> (f32, f32, f32, f32) {
        // bbox way out of tolerance distance
        (500.0, 500.0, 520.0, 520.0)
    }

    #[test]
    fn held_raw_graphics_are_selectable_and_editing_materializes_the_full_layer_keyframe() {
        let square = |x: f32| VPath {
            anchors: vec![
                anchor(Vec2::new(x, 0.0)),
                anchor(Vec2::new(x + 20.0, 0.0)),
                anchor(Vec2::new(x + 20.0, 20.0)),
                anchor(Vec2::new(x, 20.0)),
            ],
            closed: true,
        };
        let fill = Some(Rgba {
            r: 20,
            g: 40,
            b: 80,
            a: 128,
        });
        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "held-raw".into(),
                fps: 24,
                stage_width: 200,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![
                Asset::Vector(VectorAsset {
                    asset_id: 1,
                    paths: vec![square(0.0)],
                    fill,
                    stroke: None,
                }),
                Asset::Vector(VectorAsset {
                    asset_id: 2,
                    paths: vec![square(100.0)],
                    fill,
                    stroke: None,
                }),
            ],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Scene".into(),
                frame_count: 12,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![
                        Placement {
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                        },
                        Placement {
                            frame: 0,
                            target: Target::Asset(2),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                        },
                    ],
                }],
            }],
        };

        let selection = selection_at_point_pub(&project, 1, 5, Vec2::new(10.0, 10.0))
            .expect("held raw fill must be selectable");
        let Selection::Paths(refs) = selection else {
            panic!("held raw fill must select paths, got {selection:?}");
        };
        assert_eq!(refs.len(), 1);
        let mapped = prepare_raw_path_refs_for_edit(&mut project, &refs, 5)
            .expect("held raw edit must materialize");
        assert_eq!(mapped.len(), 1);

        let layer = &project.q0rgs[0].layers[0];
        assert_eq!(layer.placements.iter().filter(|p| p.frame == 5).count(), 2);
        assert_eq!(crate::render::active_placements_at(layer, 4).len(), 2);
        assert_eq!(crate::render::active_placements_at(layer, 5).len(), 2);
        assert_ne!(mapped[0].placement_idx, refs[0].placement_idx);

        let original_asset = match layer.placements[refs[0].placement_idx].target {
            Target::Asset(id) => id,
            Target::Q0rg(_) => panic!("raw asset"),
        };
        let current_asset = match layer.placements[mapped[0].placement_idx].target {
            Target::Asset(id) => id,
            Target::Q0rg(_) => panic!("raw asset"),
        };
        assert_ne!(
            original_asset, current_asset,
            "held edit must copy on write"
        );

        let source = raw_path_clone(
            &project,
            mapped[0].q0rg_id,
            mapped[0].layer_id,
            mapped[0].placement_idx,
            mapped[0].path_idx,
        )
        .expect("current raw path");
        assert!(replace_raw_path_translated(
            &mut project,
            mapped[0].q0rg_id,
            mapped[0].layer_id,
            mapped[0].placement_idx,
            mapped[0].path_idx,
            &source,
            Vec2::new(30.0, 0.0),
        ));
        let original = project
            .assets
            .iter()
            .find(|asset| asset.id() == original_asset)
            .and_then(|asset| match asset {
                Asset::Vector(vector) => vector.paths.first(),
                Asset::Bitmap(_) | Asset::Q0v(_) => None,
            })
            .expect("original path");
        let current = project
            .assets
            .iter()
            .find(|asset| asset.id() == current_asset)
            .and_then(|asset| match asset {
                Asset::Vector(vector) => vector.paths.first(),
                Asset::Bitmap(_) | Asset::Q0v(_) => None,
            })
            .expect("current path");
        assert_eq!(original.anchors[0].point.x, 0.0);
        assert_eq!(current.anchors[0].point.x, 30.0);
    }

    #[test]
    fn merge_finds_touching_same_style_drawing() {
        let project = project_with_one_drawing(red_stroke(), 0, Transform2D::IDENTITY);
        let found =
            find_touching_drawing(&project, 1, 1, 0, None, Some(red_stroke()), touching_bbox());
        assert_eq!(found, Some(1), "touching same-style strokes must merge");
    }

    #[test]
    fn merge_skips_far_same_style_drawing() {
        // Disconnected strokes stay separate even when their styles match.
        let project = project_with_one_drawing(red_stroke(), 0, Transform2D::IDENTITY);
        let found = find_touching_drawing(&project, 1, 1, 0, None, Some(red_stroke()), far_bbox());
        assert_eq!(found, None, "far-apart same-style strokes must not merge");
    }

    #[test]
    fn merge_skips_different_style_even_when_touching() {
        let project = project_with_one_drawing(red_stroke(), 0, Transform2D::IDENTITY);
        let found = find_touching_drawing(
            &project,
            1,
            1,
            0,
            None,
            Some(blue_stroke()),
            touching_bbox(),
        );
        assert_eq!(found, None, "different colours must not merge");
    }

    #[test]
    fn merge_skips_different_frame() {
        let project = project_with_one_drawing(red_stroke(), 0, Transform2D::IDENTITY);
        let found =
            find_touching_drawing(&project, 1, 1, 5, None, Some(red_stroke()), touching_bbox());
        assert_eq!(found, None, "different frames must not merge");
    }

    #[test]
    fn merge_skips_transformed_placement() {
        // A non-identity transform marks an instance or moved object. It is
        // explicitly excluded from the merge surface so dragging an existing
        // shape doesn't accidentally make it absorb the next stroke.
        let mut transformed = Transform2D::IDENTITY;
        transformed.tx = 50.0;
        let project = project_with_one_drawing(red_stroke(), 0, transformed);
        let found =
            find_touching_drawing(&project, 1, 1, 0, None, Some(red_stroke()), touching_bbox());
        assert_eq!(
            found, None,
            "transformed placements are not part of the merge surface"
        );
    }

    #[test]
    fn brush_cursor_matches_nib_size_at_any_zoom_mode() {
        let mut settings = crate::brush::BrushSettings {
            size: 20.0,
            ..Default::default()
        };
        settings.scale_with_stage = true;
        assert!((brush_cursor_radius_px(settings, 2.0) - 20.0).abs() < 1.0e-6);
        settings.scale_with_stage = false;
        assert!((brush_cursor_radius_px(settings, 2.0) - 10.0).abs() < 1.0e-6);
        assert!((brush_cursor_radius_px(settings, 0.25) - 10.0).abs() < 1.0e-6);
    }

    #[test]
    fn raw_area_bounds_hug_selected_geometry_instead_of_marquee() {
        let mut project = EditorApp::default().state.project;
        project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![VPath {
                anchors: vec![
                    anchor(Vec2::new(40.0, 30.0)),
                    anchor(Vec2::new(60.0, 30.0)),
                    anchor(Vec2::new(60.0, 50.0)),
                    anchor(Vec2::new(40.0, 50.0)),
                ],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        })];
        project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];
        let selection = select_raw_area_by_rect(&project, 1, 0, (-100.0, -100.0, 200.0, 200.0))
            .expect("raw area");
        let Selection::RawArea {
            bounds_min,
            bounds_max,
            ..
        } = selection
        else {
            panic!("raw area selection");
        };
        assert_eq!(bounds_min, Vec2::new(40.0, 30.0));
        assert_eq!(bounds_max, Vec2::new(60.0, 50.0));
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn dragging_partial_glowing_raw_area_carries_post_material_fragment_without_new_glow() {
        let mut app = EditorApp::default();
        let original_path = VPath {
            anchors: vec![
                anchor(Vec2::new(0.0, 0.0)),
                anchor(Vec2::new(100.0, 0.0)),
                anchor(Vec2::new(100.0, 100.0)),
                anchor(Vec2::new(0.0, 100.0)),
            ],
            closed: true,
        };
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![original_path.clone()],
            fill: Some(Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        })];
        app.state.project.asset_appearances.insert(
            1,
            q0s_format::v2::VectorAppearance {
                material: q0s_format::v2::VectorMaterial::SoftHalo {
                    radius: 10.0,
                    opacity: 0.5,
                },
                erase_mask: Vec::new(),
                material_source: Vec::new(),
                clip_mask: Vec::new(),
                field_transform: q0s_format::transform::Affine::IDENTITY,
            },
        );
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];
        let source_ref = PlacementRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        let refs = cut_raw_areas_for_drag(&mut app, &[source_ref], (50.0, -10.0, 110.0, 110.0));
        assert!(!refs.is_empty());
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 2);
        assert_eq!(app.state.project.asset_appearances.len(), 2);
        assert!(
            !crate::brush::merge_touching_raw_fills_after_edit(&mut app.state.project, 1, 1, 0,),
            "post-material fragments must not be geometry-merged back into a newly evaluated glow"
        );
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 2);

        let selected_placement = refs[0].placement_idx;
        let selected_asset_id =
            match app.state.project.q0rgs[0].layers[0].placements[selected_placement].target {
                Target::Asset(id) => id,
                Target::Q0rg(_) => unreachable!(),
            };
        assert_ne!(selected_asset_id, 1);
        assert_eq!(
            app.state.project.asset_appearances[&1].material_source,
            vec![original_path.clone()]
        );
        assert_eq!(
            app.state.project.asset_appearances[&selected_asset_id].material_source,
            vec![original_path]
        );
        let selected_vector = match app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == selected_asset_id)
            .unwrap()
        {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        let before_visible = crate::appearance::visible_material_surface_for_vector(
            selected_vector,
            app.state.project.asset_appearances.get(&selected_asset_id),
        );
        let before_bounds = before_visible.bounding_rect().unwrap();
        assert!(before_bounds.min().x >= 49.9);

        let start_paths: Vec<VPath> = refs
            .iter()
            .map(|reference| {
                raw_path_clone(
                    &app.state.project,
                    reference.q0rg_id,
                    reference.layer_id,
                    reference.placement_idx,
                    reference.path_idx,
                )
                .unwrap()
            })
            .collect();
        let start_appearances =
            capture_whole_asset_appearances_for_raw_refs(&app.state.project, &refs);
        assert_eq!(start_appearances.len(), 1);
        let delta = Vec2::new(30.0, 15.0);
        for (reference, source) in refs.iter().zip(&start_paths) {
            assert!(replace_raw_path_translated(
                &mut app.state.project,
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
                reference.path_idx,
                source,
                delta,
            ));
        }
        assert!(translate_captured_appearances(
            &mut app.state.project,
            &start_appearances,
            delta,
        ));

        let selected_vector = match app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == selected_asset_id)
            .unwrap()
        {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        let after_visible = crate::appearance::visible_material_surface_for_vector(
            selected_vector,
            app.state.project.asset_appearances.get(&selected_asset_id),
        );
        let after_bounds = after_visible.bounding_rect().unwrap();
        assert!((after_bounds.min().x - (before_bounds.min().x + f64::from(delta.x))).abs() < 0.05);
        assert!((after_bounds.min().y - (before_bounds.min().y + f64::from(delta.y))).abs() < 0.05);
        assert!(
            after_bounds.min().x >= 79.9,
            "moving the fragment must not grow a new halo leftward across its cut edge"
        );
        let remainder_vector = match app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == 1)
            .unwrap()
        {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        let remainder_visible = crate::appearance::visible_material_surface_for_vector(
            remainder_vector,
            app.state.project.asset_appearances.get(&1),
        );
        assert!(remainder_visible.bounding_rect().unwrap().max().x <= 50.1);
    }

    #[test]
    fn dotted_raw_area_can_start_scaling_without_prior_move() {
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![VPath {
                anchors: vec![
                    anchor(Vec2::new(0.0, 0.0)),
                    anchor(Vec2::new(40.0, 0.0)),
                    anchor(Vec2::new(40.0, 40.0)),
                    anchor(Vec2::new(0.0, 40.0)),
                ],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];
        let placement = PlacementRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };
        assert!(begin_transforming_raw_area(
            &mut app,
            &[placement],
            (10.0, 10.0, 30.0, 30.0),
            Vec2::new(20.0, 20.0),
            TransformHit::Scale(Handle::BottomRight),
            Vec2::new(30.0, 30.0),
        ));
        assert!(matches!(
            app.session.tool_state,
            ToolState::DraggingRawHandle { .. }
        ));
        assert!(matches!(
            app.session.selection,
            Selection::Path { .. } | Selection::Paths(_)
        ));
    }

    #[test]
    fn raw_graphics_hit_test_and_move_target_only_one_contour() {
        let square = |x: f32| VPath {
            anchors: vec![
                anchor(Vec2::new(x, 0.0)),
                anchor(Vec2::new(x + 20.0, 0.0)),
                anchor(Vec2::new(x + 20.0, 20.0)),
                anchor(Vec2::new(x, 20.0)),
            ],
            closed: true,
        };
        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "raw".into(),
                fps: 24,
                stage_width: 200,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![square(0.0), square(100.0)],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Scene".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                    }],
                }],
            }],
        };

        assert_eq!(
            hit_test_raw_path(&project, 1, 0, Vec2::new(10.0, 10.0)),
            Some(RawPathHit {
                layer_id: 1,
                placement_idx: 0,
                path_idx: 0,
            })
        );
        assert_eq!(
            hit_test_raw_path(&project, 1, 0, Vec2::new(110.0, 10.0)),
            Some(RawPathHit {
                layer_id: 1,
                placement_idx: 0,
                path_idx: 1,
            })
        );
        let mut cut_app = EditorApp::default();
        cut_app.state.project = project.clone();
        let before_selection = cut_app.state.project.clone();
        let selection =
            select_raw_area_by_rect(&cut_app.state.project, 1, 0, (-2.0, -2.0, 8.0, 8.0))
                .expect("non-destructive V selection");
        assert_eq!(
            cut_app.state.project, before_selection,
            "merely selecting must not alter any contour"
        );
        let placements = match selection {
            Selection::RawArea { placements, .. } => placements,
            _ => panic!("V must keep a non-destructive area mask"),
        };
        let selected_refs =
            cut_raw_areas_for_drag(&mut cut_app, &placements, (-2.0, -2.0, 8.0, 8.0));
        assert!(!selected_refs.is_empty());
        let first_selected_idx = selected_refs.iter().map(|r| r.path_idx).min().unwrap();
        let Asset::Vector(vector) = &cut_app.state.project.assets[0] else {
            unreachable!()
        };
        let remainder_before = vector.paths[..first_selected_idx].to_vec();
        for selected_ref in selected_refs {
            let selected_before = raw_path_clone(
                &cut_app.state.project,
                selected_ref.q0rg_id,
                selected_ref.layer_id,
                selected_ref.placement_idx,
                selected_ref.path_idx,
            )
            .expect("selected fragment");
            assert!(replace_raw_path_translated(
                &mut cut_app.state.project,
                selected_ref.q0rg_id,
                selected_ref.layer_id,
                selected_ref.placement_idx,
                selected_ref.path_idx,
                &selected_before,
                Vec2::new(-10.0, -5.0),
            ));
        }
        let Asset::Vector(vector) = &cut_app.state.project.assets[0] else {
            unreachable!()
        };
        assert_eq!(
            &vector.paths[..first_selected_idx],
            remainder_before.as_slice()
        );

        let second = raw_path_clone(&project, 1, 1, 0, 1).expect("second contour");
        assert!(replace_raw_path_translated(
            &mut project,
            1,
            1,
            0,
            1,
            &second,
            Vec2::new(25.0, 5.0),
        ));
        let Asset::Vector(vector) = &project.assets[0] else {
            unreachable!();
        };
        assert_eq!(vector.paths[0].anchors[0].point, Vec2::new(0.0, 0.0));
        assert_eq!(vector.paths[1].anchors[0].point, Vec2::new(125.0, 5.0));
    }

    #[test]
    fn raw_area_selection_spans_separate_drawings_and_ignores_holes() {
        let ring = |min: f32, max: f32, reverse: bool| {
            let mut points = vec![
                Vec2::new(min, min),
                Vec2::new(max, min),
                Vec2::new(max, max),
                Vec2::new(min, max),
            ];
            if reverse {
                points.reverse();
            }
            VPath {
                anchors: points.into_iter().map(anchor).collect(),
                closed: true,
            }
        };
        let fill = Some(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        });
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "holes".into(),
                fps: 24,
                stage_width: 300,
                stage_height: 150,
                entry_q0rg_id: 1,
            },
            assets: vec![
                Asset::Vector(VectorAsset {
                    asset_id: 1,
                    paths: vec![ring(0.0, 100.0, false), ring(25.0, 75.0, true)],
                    fill,
                    stroke: None,
                }),
                Asset::Vector(VectorAsset {
                    asset_id: 2,
                    paths: vec![ring(110.0, 140.0, false)],
                    fill,
                    stroke: None,
                }),
            ],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Scene".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![
                        Placement {
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                        },
                        Placement {
                            frame: 0,
                            target: Target::Asset(2),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                        },
                    ],
                }],
            }],
        };

        assert!(hit_test_raw_path(&project, 1, 0, Vec2::new(50.0, 50.0)).is_none());
        assert!(hit_test_raw_path(&project, 1, 0, Vec2::new(10.0, 10.0)).is_some());
        let selection = select_raw_area_by_rect(&project, 1, 0, (5.0, 5.0, 130.0, 130.0))
            .expect("both separated drawings intersect the marquee");
        let Selection::RawArea { placements, .. } = selection else {
            unreachable!()
        };
        assert_eq!(placements.len(), 2);
    }

    #[test]
    fn island_inside_raw_hole_selects_and_moves_independently() {
        let square = |min: f32, max: f32, reverse: bool| {
            let mut points = vec![
                Vec2::new(min, min),
                Vec2::new(max, min),
                Vec2::new(max, max),
                Vec2::new(min, max),
            ];
            if reverse {
                points.reverse();
            }
            VPath {
                anchors: points.into_iter().map(anchor).collect(),
                closed: true,
            }
        };
        let mut project = EditorApp::default().state.project;
        project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![
                square(0.0, 200.0, false),
                square(20.0, 180.0, true),
                square(80.0, 120.0, false),
            ],
            fill: Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        })];
        project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];

        let vector = match &project.assets[0] {
            Asset::Vector(vector) => vector,
            Asset::Bitmap(_) | Asset::Q0v(_) => unreachable!(),
        };
        let surface = vector_fill_geometry(vector);
        assert!(surface.contains(&Point::new(100.0, 100.0)));
        assert!(!surface.contains(&Point::new(60.0, 60.0)));

        let island = selection_at_point_pub(&project, 1, 0, Vec2::new(100.0, 100.0))
            .expect("inner island selection");
        let Selection::Paths(island_refs) = island else {
            panic!("island must be raw path selection");
        };
        assert_eq!(island_refs.len(), 1);
        assert_eq!(island_refs[0].path_idx, 2);

        let ring =
            selection_at_point_pub(&project, 1, 0, Vec2::new(10.0, 10.0)).expect("ring selection");
        let Selection::Paths(ring_refs) = ring else {
            panic!("ring must be raw path selection");
        };
        assert_eq!(ring_refs.len(), 2);
        assert!(ring_refs.iter().any(|reference| reference.path_idx == 0));
        assert!(ring_refs.iter().any(|reference| reference.path_idx == 1));
        assert!(!ring_refs.iter().any(|reference| reference.path_idx == 2));

        for reference in ring_refs {
            let source = raw_path_clone(
                &project,
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
                reference.path_idx,
            )
            .expect("ring contour");
            assert!(replace_raw_path_translated(
                &mut project,
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
                reference.path_idx,
                &source,
                Vec2::new(250.0, 0.0),
            ));
        }
        let vector = match &project.assets[0] {
            Asset::Vector(vector) => vector,
            Asset::Bitmap(_) | Asset::Q0v(_) => unreachable!(),
        };
        let moved = vector_fill_geometry(vector);
        assert!(
            moved.contains(&Point::new(100.0, 100.0)),
            "moving the enclosing ring must not remove or move the inner stroke"
        );
        assert!(!moved.contains(&Point::new(10.0, 10.0)));
        assert!(moved.contains(&Point::new(260.0, 10.0)));
    }

    #[test]
    fn raw_fill_click_selects_only_the_connected_region() {
        let square = |min_x: f32, max_x: f32| VPath {
            anchors: vec![
                anchor(Vec2::new(min_x, 0.0)),
                anchor(Vec2::new(max_x, 0.0)),
                anchor(Vec2::new(max_x, 20.0)),
                anchor(Vec2::new(min_x, 20.0)),
            ],
            closed: true,
        };
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "connected-fill-selection".into(),
                fps: 24,
                stage_width: 200,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![square(0.0, 20.0), square(100.0, 120.0)],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Scene".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                    }],
                }],
            }],
        };

        assert_eq!(
            hit_test_selectable_placement(&project, 1, 0, Vec2::new(10.0, 10.0)),
            None,
            "raw graphics must never fall back to placement selection"
        );
        let Some(RawSelectionHit::Fill(left)) =
            hit_test_raw_selection(&project, 1, 0, Vec2::new(10.0, 10.0))
        else {
            panic!("left fill region must be selected");
        };
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].path_idx, 0);

        let Some(RawSelectionHit::Fill(right)) =
            hit_test_raw_selection(&project, 1, 0, Vec2::new(110.0, 10.0))
        else {
            panic!("right fill region must be selected");
        };
        assert_eq!(right.len(), 1);
        assert_eq!(right[0].path_idx, 1);

        assert!(matches!(
            selection_at_point_pub(&project, 1, 0, Vec2::new(10.0, 10.0)),
            Some(Selection::Paths(ref refs)) if refs.len() == 1 && refs[0].path_idx == 0
        ));
    }

    #[cfg(feature = "appearance-mask-eraser")]
    fn appearance_selection_project(mask: bool) -> ProjectV2 {
        let square = |min_x: f32, min_y: f32, max_x: f32, max_y: f32| VPath {
            anchors: vec![
                anchor(Vec2::new(min_x, min_y)),
                anchor(Vec2::new(max_x, min_y)),
                anchor(Vec2::new(max_x, max_y)),
                anchor(Vec2::new(min_x, max_y)),
            ],
            closed: true,
        };
        let mut appearances = std::collections::HashMap::new();
        appearances.insert(
            1,
            q0s_format::v2::VectorAppearance {
                material: q0s_format::v2::VectorMaterial::SoftHalo {
                    radius: 10.0,
                    opacity: 0.6,
                },
                erase_mask: if mask {
                    vec![square(7.0, 7.0, 13.0, 13.0)]
                } else {
                    Vec::new()
                },
                material_source: Vec::new(),
                clip_mask: Vec::new(),
                field_transform: q0s_format::transform::Affine::IDENTITY,
            },
        );
        ProjectV2 {
            meta: ProjectMeta {
                name: "appearance-selection".into(),
                fps: 24,
                stage_width: 100,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![square(0.0, 0.0, 20.0, 20.0)],
                fill: Some(Rgba {
                    r: 180,
                    g: 20,
                    b: 40,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: appearances,
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Scene".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                    }],
                }],
            }],
        }
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn normal_select_hits_halo_but_not_an_erased_mask_hole() {
        let project = appearance_selection_project(true);
        assert!(matches!(
            hit_test_raw_selection(&project, 1, 0, Vec2::new(-5.0, 10.0)),
            Some(RawSelectionHit::Fill(ref refs)) if refs.len() == 1 && refs[0].path_idx == 0
        ));
        assert_eq!(
            hit_test_raw_selection(&project, 1, 0, Vec2::new(10.0, 10.0)),
            None,
            "pixels removed by the appearance mask must not remain selectable"
        );
        let selected = vec![PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        }];
        assert!(
            !point_hits_selected_raw_paths(&project, &selected, Vec2::new(10.0, 10.0)),
            "selected-body drag hit must not resurrect an erased appearance hole"
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn selected_glow_body_hit_is_not_stolen_by_topmost_other_raw_graphics() {
        let mut project = appearance_selection_project(false);
        let top = VPath {
            anchors: vec![
                anchor(Vec2::new(-8.0, 6.0)),
                anchor(Vec2::new(-2.0, 6.0)),
                anchor(Vec2::new(-2.0, 14.0)),
                anchor(Vec2::new(-8.0, 14.0)),
            ],
            closed: true,
        };
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 2,
            paths: vec![top],
            fill: Some(Rgba {
                r: 20,
                g: 20,
                b: 20,
                a: 255,
            }),
            stroke: None,
        }));
        project.q0rgs[0].layers[0].placements.push(Placement {
            frame: 0,
            target: Target::Asset(2),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        });
        let point = Vec2::new(-5.0, 10.0);
        let Some(RawSelectionHit::Fill(global_hit)) = hit_test_raw_selection(&project, 1, 0, point)
        else {
            panic!("overlapping raw graphics must produce a global hit");
        };
        assert_eq!(
            global_hit[0].placement_idx, 1,
            "top raw fill owns global hit"
        );

        let selected = vec![PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        }];
        assert!(
            point_hits_selected_raw_paths(&project, &selected, point),
            "an already-selected glow must keep its own body hit even when another raw fill is topmost"
        );

        let mut app = EditorApp::default();
        app.state.project = project;
        app.session.current_q0rg_id = 1;
        app.session.current_frame = 0;
        app.session.selection = Selection::Paths(selected);
        assert!(
            selected_raw_body_contains_point(&app, point),
            "cursor and drag-start must share the selected-glow body hit"
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn raw_area_body_hit_uses_visible_selected_pixels_not_only_its_bbox() {
        let project = appearance_selection_project(true);
        let selection = select_raw_area_by_rect(&project, 1, 0, (-8.0, -8.0, 28.0, 28.0))
            .expect("visible glow marquee");
        let Selection::RawArea {
            placements,
            bounds_min,
            bounds_max,
            ..
        } = selection
        else {
            panic!("expected raw area");
        };
        assert!(raw_area_selection_contains_point(
            &project,
            &placements,
            bounds_min,
            bounds_max,
            Vec2::new(-5.0, 10.0),
        ));
        assert!(
            !raw_area_selection_contains_point(
                &project,
                &placements,
                bounds_min,
                bounds_max,
                Vec2::new(10.0, 10.0),
            ),
            "an erased hole inside the marquee bbox must not start a drag"
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn glow_selection_overlay_stipple_remains_visible_and_respects_erased_hole() {
        let project = appearance_selection_project(true);
        let vector = match &project.assets[0] {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        let appearance = &project.asset_appearances[&1];
        let tester =
            crate::appearance::prepare_visible_material_hit_tester(vector, Some(appearance))
                .expect("appearance hit tester");
        let view = StageView {
            origin: Pos2::new(0.0, 0.0),
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::new(-20.0, -20.0), Pos2::new(50.0, 50.0)),
        };
        let rect = egui::Rect::from_min_max(Pos2::new(-10.0, -10.0), Pos2::new(30.0, 30.0));
        let points = appearance_selection_stipple_points(&view, rect, &tester, None);
        assert!(
            !points.is_empty(),
            "selected glow must still produce visible overlay stipple"
        );
        assert!(
            points
                .iter()
                .any(|point| point.x < 0.0 && point.y >= 0.0 && point.y <= 20.0),
            "overlay must visibly reach the selectable halo outside source geometry: {points:?}"
        );
        assert!(
            points.iter().all(|point| {
                !(point.x >= 7.0 && point.x <= 13.0 && point.y >= 7.0 && point.y <= 13.0)
            }),
            "erased appearance hole must remain empty in the selection overlay: {points:?}"
        );
    }
    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn post_material_clip_is_the_real_halo_hitbox() {
        let mut project = appearance_selection_project(false);
        let original = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths.clone(),
            _ => unreachable!(),
        };
        let appearance = project.asset_appearances.get_mut(&1).unwrap();
        appearance.material_source = original;
        appearance.clip_mask = vec![VPath {
            anchors: vec![
                anchor(Vec2::new(24.0, 6.0)),
                anchor(Vec2::new(30.0, 6.0)),
                anchor(Vec2::new(30.0, 14.0)),
                anchor(Vec2::new(24.0, 14.0)),
            ],
            closed: true,
        }];

        assert_eq!(
            hit_test_raw_selection(&project, 1, 0, Vec2::new(-5.0, 10.0)),
            None,
            "fresh support around the hidden source must not be a draggable hitbox outside the post-material clip"
        );
        assert!(matches!(
            hit_test_raw_selection(&project, 1, 0, Vec2::new(26.0, 10.0)),
            Some(RawSelectionHit::Fill(ref refs)) if refs.len() == 1 && refs[0].path_idx == 0
        ));
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn halo_only_marquee_can_be_split_and_moved_without_dragging_the_whole_source() {
        let mut app = EditorApp::default();
        app.state.project = appearance_selection_project(false);
        let original_path = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.paths[0].clone(),
            _ => unreachable!(),
        };
        let selection = select_raw_area_by_rect(&app.state.project, 1, 0, (-8.0, 6.0, -2.0, 14.0))
            .expect("pure halo marquee must select visible appearance pixels");
        let Selection::RawArea {
            placements,
            bounds_min,
            bounds_max,
            ..
        } = selection
        else {
            panic!("halo marquee must remain a raw-area selection");
        };
        assert!(
            bounds_max.x < 0.0,
            "test rectangle must not touch source fill"
        );

        let refs = cut_raw_areas_for_drag(
            &mut app,
            &placements,
            (bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y),
        );
        assert!(
            !refs.is_empty(),
            "drag materialisation must create a movable post-material fragment even when the marquee contains halo only"
        );
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 2);
        let selected_placements: std::collections::BTreeSet<_> = refs
            .iter()
            .map(|reference| reference.placement_idx)
            .collect();
        assert_eq!(
            selected_placements.len(),
            1,
            "one halo-only marquee must materialize one selected carrier; the remainder must not leak into selection"
        );
        let selected_placement = refs[0].placement_idx;
        let selected_asset_id =
            match app.state.project.q0rgs[0].layers[0].placements[selected_placement].target {
                Target::Asset(id) => id,
                Target::Q0rg(_) => unreachable!(),
            };
        let selected_vector = match app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == selected_asset_id)
            .unwrap()
        {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        assert_eq!(
            selected_vector.paths,
            vec![original_path.clone()],
            "halo-only fragments may carry hidden source geometry internally, but selection/rendering must be clipped to the chosen visual slice"
        );
        let selected_visible = crate::appearance::visible_material_surface_for_vector(
            selected_vector,
            app.state.project.asset_appearances.get(&selected_asset_id),
        );
        let selected_bounds = selected_visible.bounding_rect().unwrap();
        assert!(selected_bounds.max().x < -1.9);
        assert!(selected_bounds.min().x >= -8.1);
        assert_eq!(
            hit_test_raw_selection(&app.state.project, 1, 0, Vec2::new(10.0, 10.0))
                .and_then(|hit| match hit {
                    RawSelectionHit::Fill(refs) => refs.first().copied(),
                    RawSelectionHit::Path(path) => Some(path),
                })
                .map(|reference| reference.placement_idx),
            Some(0),
            "the original body must still belong to the remainder fragment, not the halo-only fragment"
        );
        assert_eq!(
            hit_test_raw_selection(&app.state.project, 1, 0, Vec2::new(-5.0, 10.0))
                .and_then(|hit| match hit {
                    RawSelectionHit::Fill(refs) => refs.first().copied(),
                    RawSelectionHit::Path(path) => Some(path),
                })
                .map(|reference| reference.placement_idx),
            Some(selected_placement),
            "only the actually clipped halo slice may start the selected fragment drag"
        );

        let start_paths: Vec<VPath> = refs
            .iter()
            .map(|reference| {
                raw_path_clone(
                    &app.state.project,
                    reference.q0rg_id,
                    reference.layer_id,
                    reference.placement_idx,
                    reference.path_idx,
                )
                .unwrap()
            })
            .collect();
        let start_appearances =
            capture_whole_asset_appearances_for_raw_refs(&app.state.project, &refs);
        let delta = Vec2::new(40.0, 0.0);
        for (reference, source) in refs.iter().zip(&start_paths) {
            assert!(replace_raw_path_translated(
                &mut app.state.project,
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
                reference.path_idx,
                source,
                delta,
            ));
        }
        assert!(translate_captured_appearances(
            &mut app.state.project,
            &start_appearances,
            delta,
        ));
        let selected_vector = match app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == selected_asset_id)
            .unwrap()
        {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        let moved = crate::appearance::visible_material_surface_for_vector(
            selected_vector,
            app.state.project.asset_appearances.get(&selected_asset_id),
        );
        let moved_bounds = moved.bounding_rect().unwrap();
        assert!((moved_bounds.min().x - (selected_bounds.min().x + 40.0)).abs() < 0.05);
        assert!((moved_bounds.max().x - (selected_bounds.max().x + 40.0)).abs() < 0.05);
    }

    #[cfg(feature = "appearance-mask-eraser")]
    fn halo_only_fragment_for_transform() -> (EditorApp, Vec<PathRef>, (f32, f32, f32, f32)) {
        let mut app = EditorApp::default();
        app.state.project = appearance_selection_project(false);
        let selection = select_raw_area_by_rect(&app.state.project, 1, 0, (-8.0, 6.0, -2.0, 14.0))
            .expect("pure halo marquee");
        let Selection::RawArea {
            placements,
            bounds_min,
            bounds_max,
            ..
        } = selection
        else {
            panic!("halo marquee must be raw area");
        };
        let refs = cut_raw_areas_for_drag(
            &mut app,
            &placements,
            (bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y),
        );
        assert!(!refs.is_empty());
        let bounds = raw_path_refs_ui_bounds(&app.state.project, &refs).expect("fragment bounds");
        (app, refs, bounds)
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn scaling_halo_only_fragment_scales_appearance_with_hidden_carrier() {
        let (mut app, refs, bounds) = halo_only_fragment_for_transform();
        let before = raw_path_refs_ui_bounds(&app.state.project, &refs).unwrap();
        assert!(begin_scaling_raw_paths(
            &mut app,
            refs,
            Handle::MidRight,
            bounds,
        ));
        let ToolState::DraggingRawHandle {
            refs,
            start_paths,
            start_appearances,
            handle,
            start_bounds,
            ..
        } = app.session.tool_state.clone()
        else {
            panic!("scale state");
        };
        let cursor = Vec2::new(
            bounds.0 + (bounds.2 - bounds.0) * 2.0,
            (bounds.1 + bounds.3) * 0.5,
        );
        let transform = group_transform_affine(
            GroupTransformOperation::Scale {
                handle,
                start_bounds,
            },
            cursor,
        )
        .unwrap();
        assert!(apply_raw_affine_snapshot(
            &mut app.state.project,
            &refs,
            &start_paths,
            &start_appearances,
            transform,
        ));
        let after = raw_path_refs_ui_bounds(&app.state.project, &refs).unwrap();
        assert!(((after.2 - after.0) - (before.2 - before.0) * 2.0).abs() < 0.15);
        assert!(((after.3 - after.1) - (before.3 - before.1)).abs() < 0.15);
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn rotating_halo_only_fragment_rotates_appearance_with_hidden_carrier() {
        let (mut app, refs, bounds) = halo_only_fragment_for_transform();
        let center = Vec2::new((bounds.0 + bounds.2) * 0.5, (bounds.1 + bounds.3) * 0.5);
        let before = raw_path_refs_ui_bounds(&app.state.project, &refs).unwrap();
        assert!(begin_rotating_raw_paths(
            &mut app,
            refs,
            bounds,
            center,
            Vec2::new(center.x + 10.0, center.y),
        ));
        let ToolState::DraggingRawRotate {
            refs,
            start_paths,
            start_appearances,
            center,
            start_angle,
        } = app.session.tool_state.clone()
        else {
            panic!("rotation state");
        };
        let cursor = Vec2::new(center.x, center.y + 10.0);
        let transform = group_transform_affine(
            GroupTransformOperation::Rotate {
                center,
                start_angle,
            },
            cursor,
        )
        .unwrap();
        assert!(apply_raw_affine_snapshot(
            &mut app.state.project,
            &refs,
            &start_paths,
            &start_appearances,
            transform,
        ));
        let after = raw_path_refs_ui_bounds(&app.state.project, &refs).unwrap();
        let before_w = before.2 - before.0;
        let before_h = before.3 - before.1;
        let after_w = after.2 - after.0;
        let after_h = after.3 - after.1;
        assert!(
            (after_w - before_h).abs() < 0.15,
            "rotated halo width stayed stale: before={before:?} after={after:?}"
        );
        assert!(
            (after_h - before_w).abs() < 0.15,
            "rotated halo height stayed stale: before={before:?} after={after:?}"
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn skewing_halo_only_fragment_keeps_visible_bounds_local_to_fragment() {
        let (mut app, refs, bounds) = halo_only_fragment_for_transform();
        let start_cursor = Vec2::new(bounds.2, (bounds.1 + bounds.3) * 0.5);
        assert!(begin_skewing_raw_paths(
            &mut app,
            refs,
            TransformEdge::Right,
            bounds,
            start_cursor,
        ));
        let ToolState::DraggingRawSkew {
            refs,
            start_paths,
            start_appearances,
            edge,
            start_bounds,
            start_cursor,
            ..
        } = app.session.tool_state.clone()
        else {
            panic!("skew state");
        };
        let cursor = Vec2::new(start_cursor.x, start_cursor.y + 4.0);
        let transform = group_transform_affine(
            GroupTransformOperation::Skew {
                edge,
                start_bounds,
                start_cursor,
            },
            cursor,
        )
        .unwrap();
        assert!(apply_raw_affine_snapshot(
            &mut app.state.project,
            &refs,
            &start_paths,
            &start_appearances,
            transform,
        ));
        let after = raw_path_refs_ui_bounds(&app.state.project, &refs).unwrap();
        assert!(
            after.2 - after.0 < 20.0,
            "skew exploded halo bbox: {after:?}"
        );
        assert!(
            after.3 - after.1 < 20.0,
            "skew exploded halo bbox: {after:?}"
        );
        assert!(
            after.0 > -20.0 && after.2 < 20.0,
            "skewed fragment escaped its local neighbourhood: {after:?}"
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn raw_selection_bounds_include_soft_halo_support() {
        let project = appearance_selection_project(false);
        let bounds = raw_path_refs_bounds(
            &project,
            &[PathRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
                path_idx: 0,
            }],
        )
        .expect("glowing raw bounds");
        assert!(
            bounds.0 <= -9.9,
            "left halo missing from bounds: {bounds:?}"
        );
        assert!(bounds.1 <= -9.9, "top halo missing from bounds: {bounds:?}");
        assert!(
            bounds.2 >= 29.9,
            "right halo missing from bounds: {bounds:?}"
        );
        assert!(
            bounds.3 >= 29.9,
            "bottom halo missing from bounds: {bounds:?}"
        );

        let placement = &project.q0rgs[0].layers[0].placements[0];
        let placement_bounds = placement_bbox(&project, placement).expect("placement bounds");
        assert!(placement_bounds.0 <= -9.9 && placement_bounds.2 >= 29.9);
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn fully_erased_appearance_has_no_selection_bounds_or_placement_bbox() {
        let mut project = appearance_selection_project(false);
        project.asset_appearances.get_mut(&1).unwrap().erase_mask = vec![VPath {
            anchors: vec![
                anchor(Vec2::new(-20.0, -20.0)),
                anchor(Vec2::new(40.0, -20.0)),
                anchor(Vec2::new(40.0, 40.0)),
                anchor(Vec2::new(-20.0, 40.0)),
            ],
            closed: true,
        }];
        let reference = PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        };
        assert_eq!(raw_path_refs_bounds(&project, &[reference]), None);
        assert_eq!(
            placement_bbox(&project, &project.q0rgs[0].layers[0].placements[0]),
            None
        );
        assert_eq!(
            hit_test_raw_selection(&project, 1, 0, Vec2::new(10.0, 10.0)),
            None
        );
    }

    #[test]
    fn raw_fill_click_keeps_holes_with_their_region_and_ignores_hole_interior() {
        let ring = |min: f32, max: f32, reverse: bool| {
            let mut points = vec![
                Vec2::new(min, min),
                Vec2::new(max, min),
                Vec2::new(max, max),
                Vec2::new(min, max),
            ];
            if reverse {
                points.reverse();
            }
            VPath {
                anchors: points.into_iter().map(anchor).collect(),
                closed: true,
            }
        };
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "hole-selection".into(),
                fps: 24,
                stage_width: 240,
                stage_height: 140,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![
                    ring(0.0, 100.0, false),
                    ring(25.0, 75.0, true),
                    ring(120.0, 140.0, false),
                ],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Scene".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                    }],
                }],
            }],
        };

        let Some(RawSelectionHit::Fill(refs)) =
            hit_test_raw_selection(&project, 1, 0, Vec2::new(10.0, 10.0))
        else {
            panic!("ring surface must be selected");
        };
        let indices: std::collections::BTreeSet<usize> =
            refs.iter().map(|reference| reference.path_idx).collect();
        assert_eq!(indices, [0usize, 1usize].into_iter().collect());
        assert!(
            !indices.contains(&2),
            "disconnected fill must stay unselected"
        );
        assert!(
            hit_test_raw_selection(&project, 1, 0, Vec2::new(50.0, 50.0)).is_none(),
            "clicking inside a hole must not select the surrounding fill"
        );
    }

    #[test]
    fn q0rg_instances_remain_selectable_display_objects() {
        let square = VPath {
            anchors: vec![
                anchor(Vec2::new(0.0, 0.0)),
                anchor(Vec2::new(20.0, 0.0)),
                anchor(Vec2::new(20.0, 20.0)),
                anchor(Vec2::new(0.0, 20.0)),
            ],
            closed: true,
        };
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "q0rg-selection".into(),
                fps: 24,
                stage_width: 100,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![square],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![
                Q0rg {
                    q0rg_id: 1,
                    name: "Scene".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        name: "Layer".into(),
                        explicit_keyframes: Vec::new(),
                        placements: vec![Placement {
                            frame: 0,
                            target: Target::Q0rg(2),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                        }],
                    }],
                },
                Q0rg {
                    q0rg_id: 2,
                    name: "Symbol".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        name: "Artwork".into(),
                        explicit_keyframes: Vec::new(),
                        placements: vec![Placement {
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                        }],
                    }],
                },
            ],
        };

        assert_eq!(
            hit_test_selectable_placement(&project, 1, 0, Vec2::new(10.0, 10.0)),
            Some((1, 0))
        );
        assert!(matches!(
            selection_at_point_pub(&project, 1, 0, Vec2::new(10.0, 10.0)),
            Some(Selection::Placement {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
            })
        ));
    }

    #[test]
    fn held_display_object_remains_selectable_between_keyframes() {
        let project = project_with_held_display_object();
        let point = Vec2::new(10.0, 10.0);
        assert_eq!(hit_test_placement(&project, 1, 0, point), Some((1, 0)));
        assert_eq!(hit_test_placement(&project, 1, 5, point), Some((1, 0)));
        assert_eq!(
            hit_test_selectable_placement(&project, 1, 5, point),
            Some((1, 0))
        );
    }

    #[test]
    fn editing_held_object_materializes_keyframe_at_playhead() {
        let mut project = project_with_held_display_object();
        let new_index = materialize_placement_keyframe_for_edit(&mut project, 1, 1, 0, 5)
            .expect("held object must become editable");
        let layer = &project.q0rgs[0].layers[0];
        assert_eq!(new_index, 1);
        assert_eq!(layer.placements.len(), 2);
        assert_eq!(layer.placements[0].frame, 0);
        assert_eq!(layer.placements[0].transform, Transform2D::IDENTITY);
        assert_eq!(layer.placements[1].frame, 5);
        assert_eq!(layer.placements[1].target, Target::Q0rg(2));
        assert_eq!(layer.placements[1].transform, Transform2D::IDENTITY);
        assert!(matches!(layer.placements[1].tween, Tween::None));
    }

    #[test]
    fn editing_tweened_object_bakes_interpolated_pose() {
        let mut project = project_with_held_display_object();
        let layer = &mut project.q0rgs[0].layers[0];
        layer.placements[0].tween = Tween::Linear { to_frame: 10 };
        layer.placements.push(Placement {
            frame: 10,
            target: Target::Q0rg(2),
            transform: Transform2D {
                tx: 100.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
        });

        let new_index = materialize_placement_keyframe_for_edit(&mut project, 1, 1, 0, 5)
            .expect("tween pose must be materialized");
        let layer = &project.q0rgs[0].layers[0];
        assert_eq!(new_index, 2);
        assert_eq!(layer.placements[new_index].frame, 5);
        assert!((layer.placements[new_index].transform.tx - 50.0).abs() < 0.001);
        assert_eq!(layer.placements[0].transform.tx, 0.0);
        assert_eq!(layer.placements[1].transform.tx, 100.0);
    }

    #[test]
    fn thin_raw_graphics_keep_a_draggable_body_between_transform_zones() {
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(200.0, 100.0)),
        };
        let bounds = (0.0, 0.0, 100.0, 10.0);
        assert_eq!(
            hit_test_raw_transform(bounds, &view, Pos2::new(50.0, 5.0)),
            None,
            "the middle of a thin brush mark must drag the body"
        );
        assert_eq!(
            hit_test_raw_transform(bounds, &view, Pos2::new(50.0, 0.0)),
            Some(TransformHit::Skew(TransformEdge::Top)),
            "dragging the contour between handles must skew"
        );
        assert_eq!(
            hit_test_raw_transform(bounds, &view, Pos2::new(0.0, 0.0)),
            Some(TransformHit::Scale(Handle::TopLeft)),
            "corner handles remain available for scaling"
        );
    }

    #[test]
    fn edge_handles_scale_one_axis_and_clamp_before_crossing() {
        let start = Transform2D {
            tx: 10.0,
            ty: 20.0,
            sx: 2.0,
            sy: 3.0,
            rotation: 0.0,
            skew_x: 0.0,
            skew_y: 0.0,
        };
        let local_bbox = (0.0, 0.0, 100.0, 50.0);
        let world_bbox = (10.0, 20.0, 210.0, 170.0);
        let mut transform = start;

        assert!(apply_handle_drag(
            &mut transform,
            start,
            local_bbox,
            world_bbox,
            Handle::MidRight,
            Vec2::new(310.0, 95.0),
        ));
        assert!((transform.sx - 3.0).abs() < 1.0e-5);
        assert!((transform.sy - 3.0).abs() < 1.0e-5);
        assert_eq!(transform.skew_x, 0.0);
        assert_eq!(transform.skew_y, 0.0);

        assert!(apply_handle_drag(
            &mut transform,
            start,
            local_bbox,
            world_bbox,
            Handle::MidRight,
            Vec2::new(-100.0, 95.0),
        ));
        assert_eq!(transform.sx, 0.01);
        assert_eq!(transform.sy, start.sy);
    }

    #[test]
    fn free_transform_stays_available_after_rotation_and_skew() {
        assert!(supports_axis_resize(Transform2D::IDENTITY));
        assert!(supports_axis_resize(Transform2D {
            rotation: 0.25,
            ..Transform2D::IDENTITY
        }));
        assert!(supports_axis_resize(Transform2D {
            skew_x: 0.25,
            ..Transform2D::IDENTITY
        }));
        assert!(supports_axis_resize(Transform2D {
            sx: -1.0,
            ..Transform2D::IDENTITY
        }));
        assert!(!supports_axis_resize(Transform2D {
            sy: f32::NAN,
            ..Transform2D::IDENTITY
        }));
        assert!(!supports_axis_resize(Transform2D {
            sx: 0.0,
            ..Transform2D::IDENTITY
        }));
    }

    #[test]
    fn placement_rotation_keeps_the_local_center_fixed_in_world_space() {
        let start = Transform2D {
            tx: 15.0,
            ty: -7.0,
            sx: 1.5,
            sy: 0.75,
            rotation: 0.2,
            skew_x: 0.1,
            skew_y: -0.05,
        };
        let center_local = Vec2::new(40.0, 20.0);
        let center_world = Affine::from_transform(start).apply(center_local);
        let next = rotate_placement_about_local_point(start, center_local, center_world, 0.7);
        let after = Affine::from_transform(next).apply(center_local);
        assert!((after.x - center_world.x).abs() < 1.0e-4);
        assert!((after.y - center_world.y).abs() < 1.0e-4);
    }

    #[test]
    fn raw_rotation_rotates_points_without_moving_the_center() {
        let center = Vec2::new(10.0, 10.0);
        let source = VPath {
            anchors: vec![anchor(Vec2::new(20.0, 10.0)), anchor(Vec2::new(10.0, 20.0))],
            closed: false,
        };
        let mut project = project_with_one_drawing(red_stroke(), 0, Transform2D::IDENTITY);
        let reference = PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        };
        let transform = group_transform_affine(
            GroupTransformOperation::Rotate {
                center,
                start_angle: 0.0,
            },
            Vec2::new(center.x, center.y + 10.0),
        )
        .unwrap();
        assert!(apply_raw_affine_snapshot(
            &mut project,
            &[reference],
            std::slice::from_ref(&source),
            &[],
            transform,
        ));
        let rotated = raw_path_clone(&project, 1, 1, 0, 0).unwrap();
        assert!((rotated.anchors[0].point.x - 10.0).abs() < 1.0e-4);
        assert!((rotated.anchors[0].point.y - 20.0).abs() < 1.0e-4);
    }

    #[test]
    fn rotate_and_skew_transform_zones_hide_the_system_cursor_for_custom_icons() {
        assert_eq!(
            cursor_for_transform_hit(TransformHit::Rotate(Handle::TopLeft)),
            egui::CursorIcon::None
        );
        assert_eq!(
            cursor_for_transform_hit(TransformHit::Skew(TransformEdge::Top)),
            egui::CursorIcon::None
        );
        assert_eq!(
            cursor_for_transform_hit(TransformHit::Scale(Handle::TopLeft)),
            egui::CursorIcon::ResizeNwSe
        );
        assert_eq!(
            custom_transform_cursor_for_hit(TransformHit::Rotate(Handle::TopLeft)),
            Some(CustomTransformCursor::Rotate)
        );
        assert_eq!(
            custom_transform_cursor_for_hit(TransformHit::Skew(TransformEdge::Top)),
            Some(CustomTransformCursor::Skew(TransformEdge::Top))
        );
        assert_eq!(
            custom_transform_cursor_for_hit(TransformHit::Scale(Handle::TopLeft)),
            None
        );
    }

    #[test]
    fn transform_hit_zones_follow_the_oriented_placement_frame() {
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(400.0, 400.0)),
        };
        let transform = Transform2D {
            tx: 180.0,
            ty: 120.0,
            rotation: std::f32::consts::FRAC_PI_4,
            ..Transform2D::IDENTITY
        };
        let frame = placement_transform_frame((0.0, 0.0, 100.0, 60.0), transform);
        let corner = stage_to_screen(frame.corners[0], &view);
        assert_eq!(
            hit_test_placement_transform((0.0, 0.0, 100.0, 60.0), transform, &view, corner,),
            Some(TransformHit::Scale(Handle::TopLeft))
        );

        let center = stage_to_screen(frame.center, &view);
        let outward = (corner - center).normalized();
        let rotate = corner + outward * (ROTATE_HIT_RADIUS_PX * 0.7);
        assert_eq!(
            hit_test_placement_transform((0.0, 0.0, 100.0, 60.0), transform, &view, rotate,),
            Some(TransformHit::Rotate(Handle::TopLeft))
        );

        let top_start = stage_to_screen(frame.corners[0], &view);
        let top_end = stage_to_screen(frame.corners[1], &view);
        let skew = top_start + (top_end - top_start) * 0.3;
        assert_eq!(
            hit_test_placement_transform((0.0, 0.0, 100.0, 60.0), transform, &view, skew,),
            Some(TransformHit::Skew(TransformEdge::Top))
        );
    }

    #[test]
    fn selection_stipple_density_is_fixed_in_screen_space() {
        assert_eq!(SELECTION_STIPPLE_SPACING_PX, 4.0);
        assert_eq!(SELECTION_CONTOUR_SPACING_PX, 1.0);
    }

    #[test]
    fn selection_stipple_is_batched_into_one_mesh_geometry() {
        let points = [
            Pos2::new(1.0, 2.0),
            Pos2::new(4.0, 5.0),
            Pos2::new(7.0, 8.0),
        ];
        let mesh = stipple_mesh(&points, 0.4, Color32::WHITE);
        assert_eq!(mesh.vertices.len(), points.len() * 4);
        assert_eq!(mesh.indices.len(), points.len() * 6);
        assert_eq!(
            mesh.indices,
            vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7, 8, 9, 10, 8, 10, 11]
        );
    }

    #[test]
    fn dense_selection_contour_keeps_one_pixel_spacing_and_clips_offscreen_work() {
        let square = vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(100.0, 0.0),
            Pos2::new(100.0, 100.0),
            Pos2::new(0.0, 100.0),
        ];
        let clip = egui::Rect::from_min_max(Pos2::new(-1.0, -1.0), Pos2::new(101.0, 101.0));
        let points = dense_selection_contour_points(&[square], clip);
        assert!(
            points.len() >= 396,
            "400px contour should be effectively one-dot-per-pixel, got {} points",
            points.len()
        );

        let huge = vec![
            Pos2::new(-100_000.0, 50.0),
            Pos2::new(100_000.0, 50.0),
            Pos2::new(100_000.0, 100_000.0),
            Pos2::new(-100_000.0, 100_000.0),
        ];
        let viewport = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(200.0, 100.0));
        let visible_points = dense_selection_contour_points(&[huge], viewport);
        assert!(
            visible_points.len() <= 205,
            "off-screen contour length must not create work, got {} visible points",
            visible_points.len()
        );
    }
    #[test]
    fn valid_scale_clamps_crossing_and_repairs_non_finite_values() {
        assert_eq!(valid_scale(2.5), 2.5);
        assert_eq!(valid_scale(0.0), 0.01);
        assert_eq!(valid_scale(-8.0), 0.01);
        assert_eq!(valid_scale(f32::NAN), 1.0);
        assert_eq!(valid_scale(f32::INFINITY), 1.0);
    }

    #[test]
    fn eyedropper_prefers_visible_stroke_over_fill() {
        let fill = Rgba {
            r: 240,
            g: 180,
            b: 40,
            a: 255,
        };
        let stroke = blue_stroke();
        let vector = VectorAsset {
            asset_id: 1,
            paths: vec![VPath {
                anchors: vec![
                    anchor(Vec2::new(0.0, 0.0)),
                    anchor(Vec2::new(20.0, 0.0)),
                    anchor(Vec2::new(20.0, 20.0)),
                    anchor(Vec2::new(0.0, 20.0)),
                ],
                closed: true,
            }],
            fill: Some(fill),
            stroke: Some(stroke),
        };

        match sample_vector_paint(&vector, Affine::IDENTITY, Vec2::new(10.0, 10.0), 0.25) {
            Some(SampledPaint::Fill(sampled)) => assert_eq!(sampled, fill),
            _ => panic!("center must sample the fill"),
        }
        match sample_vector_paint(&vector, Affine::IDENTITY, Vec2::new(0.0, 10.0), 0.25) {
            Some(SampledPaint::Stroke(sampled)) => assert_eq!(sampled, stroke),
            _ => panic!("edge must sample the topmost stroke paint"),
        }
    }

    #[test]
    fn bucket_resolves_only_raw_enclosed_regions() {
        let square = |x: f32| VPath {
            anchors: vec![
                anchor(Vec2::new(x, 0.0)),
                anchor(Vec2::new(x + 20.0, 0.0)),
                anchor(Vec2::new(x + 20.0, 20.0)),
                anchor(Vec2::new(x, 20.0)),
            ],
            closed: true,
        };
        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "t".into(),
                fps: 24,
                stage_width: 200,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 7,
                paths: vec![square(0.0), square(100.0)],
                fill: Some(Rgba {
                    r: 50,
                    g: 50,
                    b: 50,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "S".into(),
                frame_count: 5,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "L".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        frame: 0,
                        target: Target::Asset(7),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                    }],
                }],
            }],
        };
        assert_eq!(
            find_bucket_target(&project, 1, 0, Vec2::new(10.0, 10.0)),
            Some(BucketTarget {
                layer_id: 1,
                placement_idx: 0,
                asset_id: 7,
                path_indices: vec![0],
                kind: BucketTargetKind::ExistingFill,
            })
        );
        assert_eq!(
            find_bucket_target(&project, 1, 0, Vec2::new(50.0, 50.0)),
            None
        );

        project.q0rgs[0].layers[0].placements[0].transform.tx = 40.0;
        assert_eq!(
            find_bucket_target(&project, 1, 0, Vec2::new(50.0, 10.0)),
            None,
            "transformed vector instances are objects, not raw bucket surfaces"
        );
    }

    #[test]
    fn bucket_on_held_frame_recolours_only_the_materialized_current_key() {
        let mut app = EditorApp::default();
        let old = Rgba {
            r: 20,
            g: 30,
            b: 40,
            a: 128,
        };
        let new = Rgba {
            r: 220,
            g: 40,
            b: 60,
            a: 128,
        };
        let square = VPath {
            anchors: vec![
                anchor(Vec2::new(0.0, 0.0)),
                anchor(Vec2::new(20.0, 0.0)),
                anchor(Vec2::new(20.0, 20.0)),
                anchor(Vec2::new(0.0, 20.0)),
            ],
            closed: true,
        };
        app.state.project.q0rgs[0].frame_count = 8;
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![square],
            fill: Some(old),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];
        app.session.current_frame = 4;
        app.session.fill_color = Some(new);

        assert!(bucket_fill_at(&mut app, Vec2::new(10.0, 10.0)));
        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(crate::render::active_placements_at(layer, 0).len(), 1);
        assert_eq!(crate::render::active_placements_at(layer, 4).len(), 1);
        let source_asset = match layer.placements[0].target {
            Target::Asset(id) => id,
            Target::Q0rg(_) => panic!("raw asset"),
        };
        let current_index = crate::render::active_placements_at(layer, 4)[0].0;
        let current_asset = match layer.placements[current_index].target {
            Target::Asset(id) => id,
            Target::Q0rg(_) => panic!("raw asset"),
        };
        assert_ne!(source_asset, current_asset);
        let source_fill = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == source_asset)
            .and_then(|asset| match asset {
                Asset::Vector(vector) => vector.fill,
                Asset::Bitmap(_) | Asset::Q0v(_) => None,
            });
        let current_fill = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == current_asset)
            .and_then(|asset| match asset {
                Asset::Vector(vector) => vector.fill,
                Asset::Bitmap(_) | Asset::Q0v(_) => None,
            });
        assert_eq!(source_fill, Some(old));
        assert_eq!(current_fill, Some(new));
    }

    #[test]
    fn bucket_splits_one_connected_fill_without_recolouring_its_neighbour() {
        let square = |x: f32| VPath {
            anchors: vec![
                anchor(Vec2::new(x, 0.0)),
                anchor(Vec2::new(x + 20.0, 0.0)),
                anchor(Vec2::new(x + 20.0, 20.0)),
                anchor(Vec2::new(x, 20.0)),
            ],
            closed: true,
        };
        let old = Rgba {
            r: 20,
            g: 30,
            b: 40,
            a: 255,
        };
        let new = Rgba {
            r: 220,
            g: 40,
            b: 60,
            a: 255,
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![square(0.0), square(100.0)],
            fill: Some(old),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];
        app.session.fill_color = Some(new);

        assert!(bucket_fill_at(&mut app, Vec2::new(10.0, 10.0)));
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 2);
        let fills: Vec<Rgba> = app
            .state
            .project
            .assets
            .iter()
            .filter_map(|asset| match asset {
                Asset::Vector(vector) => vector.fill,
                Asset::Bitmap(_) | Asset::Q0v(_) => None,
            })
            .collect();
        assert!(fills.contains(&old));
        assert!(fills.contains(&new));
        let old_paths = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(old) => Some(vector.paths.len()),
                _ => None,
            })
            .unwrap();
        let new_paths = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(new) => Some(vector.paths.len()),
                _ => None,
            })
            .unwrap();
        assert_eq!(old_paths, 1);
        assert_eq!(new_paths, 1);
    }

    #[test]
    fn bucket_filling_a_hole_keeps_the_old_fill_hollow() {
        let outer = VPath {
            anchors: vec![
                anchor(Vec2::new(0.0, 0.0)),
                anchor(Vec2::new(100.0, 0.0)),
                anchor(Vec2::new(100.0, 100.0)),
                anchor(Vec2::new(0.0, 100.0)),
            ],
            closed: true,
        };
        // Opposite winding: this is a real hole in the black surface.
        let hole = VPath {
            anchors: vec![
                anchor(Vec2::new(30.0, 30.0)),
                anchor(Vec2::new(30.0, 70.0)),
                anchor(Vec2::new(70.0, 70.0)),
                anchor(Vec2::new(70.0, 30.0)),
            ],
            closed: true,
        };
        let old = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        let new = Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![outer, hole],
            fill: Some(old),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];
        app.session.fill_color = Some(new);

        assert!(bucket_fill_at(&mut app, Vec2::new(50.0, 50.0)));
        let center = Point::new(50.0, 50.0);
        let old_vector = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(old) => Some(vector),
                _ => None,
            })
            .expect("old black ring");
        let new_vector = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(new) => Some(vector),
                _ => None,
            })
            .expect("new red interior");
        assert!(
            !vector_fill_geometry(old_vector).contains(&center),
            "moving the red fill must reveal a transparent hole, not black paint"
        );
        assert!(vector_fill_geometry(new_vector).contains(&center));
    }

    #[test]
    fn bucket_empty_hole_preserves_inner_raw_fill_instead_of_covering_it() {
        let square = |min: f32, max: f32, reverse: bool| {
            let mut points = vec![
                Vec2::new(min, min),
                Vec2::new(max, min),
                Vec2::new(max, max),
                Vec2::new(min, max),
            ];
            if reverse {
                points.reverse();
            }
            VPath {
                anchors: points.into_iter().map(anchor).collect(),
                closed: true,
            }
        };
        let old = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        let new = Rgba {
            r: 220,
            g: 40,
            b: 60,
            a: 255,
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![
                square(0.0, 100.0, false),
                square(20.0, 80.0, true),
                square(45.0, 55.0, false),
            ],
            fill: Some(old),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];
        app.session.fill_color = Some(new);

        assert!(bucket_fill_at(&mut app, Vec2::new(30.0, 50.0)));
        let island = Point::new(50.0, 50.0);
        let empty_hole = Point::new(30.0, 50.0);
        let old_vector = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(old) => Some(vector),
                _ => None,
            })
            .expect("old ring and inner brush mark");
        let new_vector = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(new) => Some(vector),
                _ => None,
            })
            .expect("new bucket fill");

        assert!(
            vector_fill_geometry(old_vector).contains(&island),
            "bucket must not delete the inner raw brush mark"
        );
        assert!(vector_fill_geometry(new_vector).contains(&empty_hole));
        assert!(
            !vector_fill_geometry(new_vector).contains(&island),
            "bucket fill must be cut around existing raw artwork"
        );

        let layer = &app.state.project.q0rgs[0].layers[0];
        let old_index = layer
            .placements
            .iter()
            .position(|placement| placement.target == Target::Asset(1))
            .expect("old placement");
        let new_asset_id = new_vector.asset_id;
        let new_index = layer
            .placements
            .iter()
            .position(|placement| placement.target == Target::Asset(new_asset_id))
            .expect("new fill placement");
        assert!(
            new_index < old_index,
            "bucket fill must render behind its enclosing raw artwork"
        );
    }

    #[test]
    fn raw_handle_scales_only_the_selected_connected_region() {
        let square = |x: f32| VPath {
            anchors: vec![
                anchor(Vec2::new(x, 0.0)),
                anchor(Vec2::new(x + 20.0, 0.0)),
                anchor(Vec2::new(x + 20.0, 20.0)),
                anchor(Vec2::new(x, 20.0)),
            ],
            closed: true,
        };
        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "raw-scale".into(),
                fps: 24,
                stage_width: 200,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![square(0.0), square(100.0)],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Scene".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![Placement {
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                    }],
                }],
            }],
        };
        let selected = PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        };
        let source = raw_path_clone(&project, 1, 1, 0, 0).expect("selected square");
        let bounds = raw_path_refs_bounds(&project, &[selected]).expect("selected bounds");

        let transform = group_transform_affine(
            GroupTransformOperation::Scale {
                handle: Handle::MidRight,
                start_bounds: bounds,
            },
            Vec2::new(40.0, 10.0),
        )
        .unwrap();
        assert!(apply_raw_affine_snapshot(
            &mut project,
            &[selected],
            std::slice::from_ref(&source),
            &[],
            transform,
        ));

        let Asset::Vector(vector) = &project.assets[0] else {
            panic!("vector asset");
        };
        assert_eq!(vector.paths[0].anchors[1].point, Vec2::new(40.0, 0.0));
        assert_eq!(vector.paths[0].anchors[2].point, Vec2::new(40.0, 20.0));
        assert_eq!(vector.paths[1].anchors[0].point, Vec2::new(100.0, 0.0));
        assert_eq!(vector.paths[1].anchors[1].point, Vec2::new(120.0, 0.0));
    }

    #[test]
    fn moving_raw_graphics_does_not_mutate_a_transformed_shared_instance() {
        let square = VPath {
            anchors: vec![
                anchor(Vec2::new(0.0, 0.0)),
                anchor(Vec2::new(20.0, 0.0)),
                anchor(Vec2::new(20.0, 20.0)),
                anchor(Vec2::new(0.0, 20.0)),
            ],
            closed: true,
        };
        let mut project = ProjectV2 {
            meta: ProjectMeta {
                name: "raw-copy-on-write".into(),
                fps: 24,
                stage_width: 200,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![square],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Scene".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![
                        Placement {
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                        },
                        Placement {
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D {
                                tx: 80.0,
                                ..Transform2D::IDENTITY
                            },
                            tween: Tween::None,
                        },
                    ],
                }],
            }],
        };
        let selected = PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        };

        prepare_raw_path_refs_for_edit(&mut project, &[selected], 0)
            .expect("raw path must become writable");
        let raw_asset_id = match project.q0rgs[0].layers[0].placements[0].target {
            Target::Asset(id) => id,
            Target::Q0rg(_) => panic!("raw asset"),
        };
        let instance_asset_id = match project.q0rgs[0].layers[0].placements[1].target {
            Target::Asset(id) => id,
            Target::Q0rg(_) => panic!("instance asset"),
        };
        assert_ne!(raw_asset_id, instance_asset_id);
        assert_eq!(instance_asset_id, 1);

        let source = raw_path_clone(&project, 1, 1, 0, 0).expect("writable raw path");
        assert!(replace_raw_path_translated(
            &mut project,
            1,
            1,
            0,
            0,
            &source,
            Vec2::new(30.0, 0.0),
        ));

        let Asset::Vector(original) = project
            .assets
            .iter()
            .find(|asset| asset.id() == instance_asset_id)
            .expect("original shared asset")
        else {
            panic!("vector asset");
        };
        let Asset::Vector(moved) = project
            .assets
            .iter()
            .find(|asset| asset.id() == raw_asset_id)
            .expect("copy-on-write asset")
        else {
            panic!("vector asset");
        };
        assert_eq!(original.paths[0].anchors[0].point, Vec2::new(0.0, 0.0));
        assert_eq!(moved.paths[0].anchors[0].point, Vec2::new(30.0, 0.0));
    }

    #[test]
    fn point_in_polygon_handles_concave() {
        // Simple "C" shape: a square with the right side notched.
        let poly = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 3.0),
            Vec2::new(3.0, 3.0),
            Vec2::new(3.0, 7.0),
            Vec2::new(10.0, 7.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
        ];
        assert!(
            point_in_polygon(&poly, Vec2::new(1.0, 5.0)),
            "left arm of C"
        );
        assert!(!point_in_polygon(&poly, Vec2::new(7.0, 5.0)), "notch hole");
        assert!(!point_in_polygon(&poly, Vec2::new(20.0, 5.0)), "outside");
    }

    #[test]
    fn nearest_segment_distance_basic() {
        let pts = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)];
        assert!((nearest_segment_distance(&pts, Vec2::new(5.0, 3.0)) - 3.0).abs() < 1e-3);
        assert!((nearest_segment_distance(&pts, Vec2::new(-2.0, 0.0)) - 2.0).abs() < 1e-3);
    }

    #[test]
    fn marquee_can_select_raw_fill_and_q0rg_together() {
        let square = |asset_id: u16, size: f32| {
            Asset::Vector(VectorAsset {
                asset_id,
                paths: vec![VPath {
                    anchors: vec![
                        anchor(Vec2::new(0.0, 0.0)),
                        anchor(Vec2::new(size, 0.0)),
                        anchor(Vec2::new(size, size)),
                        anchor(Vec2::new(0.0, size)),
                    ],
                    closed: true,
                }],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })
        };
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "mixed-marquee".into(),
                fps: 24,
                stage_width: 200,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![square(1, 20.0), square(2, 10.0)],
            asset_names: std::collections::HashMap::new(),
            asset_appearances: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![
                Q0rg {
                    q0rg_id: 1,
                    name: "Scene".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        name: "Layer".into(),
                        explicit_keyframes: Vec::new(),
                        placements: vec![
                            Placement {
                                frame: 0,
                                target: Target::Asset(1),
                                transform: Transform2D::IDENTITY,
                                tween: Tween::None,
                            },
                            Placement {
                                frame: 0,
                                target: Target::Q0rg(2),
                                transform: Transform2D {
                                    tx: 30.0,
                                    ty: 0.0,
                                    ..Transform2D::IDENTITY
                                },
                                tween: Tween::None,
                            },
                        ],
                    }],
                },
                Q0rg {
                    q0rg_id: 2,
                    name: "Child".into(),
                    frame_count: 1,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        name: "Layer".into(),
                        explicit_keyframes: Vec::new(),
                        placements: vec![Placement {
                            frame: 0,
                            target: Target::Asset(2),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                        }],
                    }],
                },
            ],
        };

        let selection = marquee_selection(&project, 1, 0, (-5.0, -5.0, 45.0, 25.0));
        let Selection::RawArea {
            placements,
            objects,
            ..
        } = selection
        else {
            panic!("expected mixed raw-area selection");
        };
        assert_eq!(placements.len(), 1);
        assert_eq!(objects.len(), 1);
        assert_eq!(placements[0].placement_idx, 0);
        assert_eq!(objects[0].placement_idx, 1);
    }

    #[test]
    fn rotation_zone_is_broad_outside_corner_but_handle_keeps_priority() {
        let view = StageView {
            origin: pos2(0.0, 0.0),
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(pos2(-200.0, -200.0), pos2(300.0, 300.0)),
        };
        let frame = axis_aligned_transform_frame((0.0, 0.0, 100.0, 100.0));
        assert_eq!(
            hit_test_transform_frame(
                frame,
                &view,
                pos2(0.0, 0.0),
                RAW_HANDLE_HIT_RADIUS_PX,
                false,
            ),
            Some(TransformHit::Scale(Handle::TopLeft)),
            "the resize handle must still win directly on the corner"
        );
        assert_eq!(
            hit_test_transform_frame(
                frame,
                &view,
                pos2(-24.0, -24.0),
                RAW_HANDLE_HIT_RADIUS_PX,
                false,
            ),
            Some(TransformHit::Rotate(Handle::TopLeft)),
            "rotation should be grabbable through a broad outward corner sector"
        );
    }

    #[test]
    fn custom_transform_anchor_stays_fixed_during_rotation() {
        let center = Vec2::new(180.0, 70.0);
        let operation = GroupTransformOperation::Rotate {
            center,
            start_angle: 0.0,
        };
        let affine = group_transform_affine(operation, Vec2::new(center.x, center.y + 40.0))
            .expect("rotation affine");
        let mapped = affine.apply(center);
        assert!((mapped.x - center.x).abs() < 1.0e-4);
        assert!((mapped.y - center.y).abs() < 1.0e-4);
    }

    #[test]
    fn mixed_raw_and_object_bounds_include_both_sides() {
        let square = |asset_id: u16| {
            Asset::Vector(VectorAsset {
                asset_id,
                paths: vec![VPath {
                    anchors: vec![
                        anchor(Vec2::new(0.0, 0.0)),
                        anchor(Vec2::new(20.0, 0.0)),
                        anchor(Vec2::new(20.0, 20.0)),
                        anchor(Vec2::new(0.0, 20.0)),
                    ],
                    closed: true,
                }],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![square(1), square(2)];
        let mut object_transform = Transform2D::IDENTITY;
        object_transform.tx = 100.0;
        app.state.project.q0rgs[0].layers[0].placements = vec![
            Placement {
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            },
            Placement {
                frame: 0,
                target: Target::Asset(2),
                transform: object_transform,
                tween: Tween::None,
            },
        ];
        app.session.selection = Selection::RawArea {
            placements: vec![PlacementRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
            }],
            objects: vec![PlacementRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 1,
            }],
            bounds_min: Vec2::new(0.0, 0.0),
            bounds_max: Vec2::new(20.0, 20.0),
        };
        assert_eq!(
            selection_transform_bounds(&app),
            Some((0.0, 0.0, 120.0, 20.0))
        );
    }

    #[test]
    fn mixed_group_move_transforms_raw_path_and_object_together() {
        let square = |asset_id: u16| {
            Asset::Vector(VectorAsset {
                asset_id,
                paths: vec![VPath {
                    anchors: vec![
                        anchor(Vec2::new(0.0, 0.0)),
                        anchor(Vec2::new(20.0, 0.0)),
                        anchor(Vec2::new(20.0, 20.0)),
                        anchor(Vec2::new(0.0, 20.0)),
                    ],
                    closed: true,
                }],
                fill: Some(Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }),
                stroke: None,
            })
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![square(1), square(2)];
        let mut object_transform = Transform2D::IDENTITY;
        object_transform.tx = 100.0;
        app.state.project.q0rgs[0].layers[0].placements = vec![
            Placement {
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            },
            Placement {
                frame: 0,
                target: Target::Asset(2),
                transform: object_transform,
                tween: Tween::None,
            },
        ];
        app.session.selection = Selection::RawArea {
            placements: vec![PlacementRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
            }],
            objects: vec![PlacementRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 1,
            }],
            bounds_min: Vec2::new(0.0, 0.0),
            bounds_max: Vec2::new(20.0, 20.0),
        };
        let selection = app.session.selection.clone();
        assert!(begin_group_transform(
            &mut app,
            selection,
            GroupTransformIntent::Move,
            Vec2::new(0.0, 0.0),
        ));
        let ToolState::DraggingGroup {
            refs,
            start_paths,
            start_appearances,
            objects,
            start_transforms,
            operation,
            start_pivot,
        } = app.session.tool_state.clone()
        else {
            panic!("mixed selection must enter one group transform")
        };
        assert!(matches!(app.session.selection, Selection::Mixed { .. }));
        assert_eq!(refs.len(), 1);
        assert_eq!(objects.len(), 1);
        assert!(apply_group_transform(
            &mut app,
            GroupTransformData {
                refs: &refs,
                start_paths: &start_paths,
                start_appearances: &start_appearances,
                objects: &objects,
                start_transforms: &start_transforms,
                operation,
                start_pivot,
            },
            Vec2::new(10.0, 5.0),
        ));
        let moved_pivot = selection_transform_pivot(&app).expect("moved pivot");
        assert!((moved_pivot.x - (start_pivot.x + 10.0)).abs() < 1.0e-4);
        assert!((moved_pivot.y - (start_pivot.y + 5.0)).abs() < 1.0e-4);
        let moved_path = raw_path_clone(
            &app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            refs[0].placement_idx,
            refs[0].path_idx,
        )
        .expect("moved raw path");
        assert!(
            (moved_path.anchors[0].point.x - (start_paths[0].anchors[0].point.x + 10.0)).abs()
                < 1.0e-4
        );
        assert!(
            (moved_path.anchors[0].point.y - (start_paths[0].anchors[0].point.y + 5.0)).abs()
                < 1.0e-4
        );
        let moved_object = placement_ref_transform(&app.state.project, objects[0]).expect("object");
        assert!((moved_object.tx - (start_transforms[0].tx + 10.0)).abs() < 1.0e-4);
        assert!((moved_object.ty - (start_transforms[0].ty + 5.0)).abs() < 1.0e-4);
    }

    #[test]
    fn synced_eraser_preserves_every_classic_brush_nib() {
        for nib in crate::brush::BrushNib::ALL {
            let mut app = EditorApp::default();
            app.session.brush.nib = nib;
            app.session.brush.sync_with_eraser = true;
            let settings = eraser_settings(&app, 1.0);
            assert_eq!(settings.nib, nib, "eraser lost {} nib", nib.label());
        }
    }

    #[test]
    fn independent_eraser_remains_circle_without_destroying_brush_nib() {
        let mut app = EditorApp::default();
        app.session.brush.nib = crate::brush::BrushNib::Slash;
        app.session.brush.sync_with_eraser = false;
        let settings = eraser_settings(&app, 1.0);
        assert_eq!(settings.nib, crate::brush::BrushNib::Circle);
        assert_eq!(app.session.brush.nib, crate::brush::BrushNib::Slash);
    }
}
