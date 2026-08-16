use egui::epaint::PathShape;
use egui::{
    pos2, Color32, ColorImage, Context, Key, Mesh, Painter, PointerButton, Pos2, Response, Shape,
    Stroke, TextureHandle, TextureOptions,
};
use geo::{
    Area, BooleanOps, BoundingRect, Contains, ConvexHull, Coord, LineString, MultiPoint,
    MultiPolygon, Point, Polygon,
};
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Anchor, Asset, Path as VPath, Placement, ProjectV2, Rgba, Stroke as VStroke, Target,
    Transform2D, Tween, Vec2, VectorAsset,
};

use crate::app::EditorApp;
use crate::render::{
    flatten_path, flatten_path_for_stroke, paint_complex_fill, paint_round_stroke_preview,
    placement_bbox, placement_local_bbox, StageView, TextureCache,
};
#[cfg(feature = "appearance-mask-eraser")]
use crate::render::{CachedInteractivePath, CachedSelectionPaint, SelectionPaintKey};

use crate::state::{
    AppearanceTransformSnapshot, BrushSizePreview, GroupTransformOperation, Handle, PathRef,
    PlacementRef, Selection, Tool, ToolState, TransformEdge, TransformPivot,
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
    // `render_stage` runs before tool input. A brush committed on pointer-up is
    // therefore absent from this frame's stage render, so its draft must stay
    // visible through the release frame. Once we enter the next tool frame the
    // committed vector has already had a chance to render and the handoff can
    // be retired before painting overlays.
    expire_brush_preview_handoff(app);

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
    let current_layer_is_visible = app
        .state
        .project
        .layer_is_visible(app.session.current_q0rg_id, app.session.current_layer_id);
    let current_layer_is_locked = app
        .state
        .project
        .layer_is_locked(app.session.current_q0rg_id, app.session.current_layer_id);
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
    let edits_current_layer = !matches!(
        app.session.current_tool,
        Tool::Select | Tool::Subselect | Tool::Hand | Tool::Eyedropper | Tool::Rig
    );
    if edits_current_layer && (!current_layer_is_visible || current_layer_is_locked) {
        if response.clicked() || response.drag_started() {
            app.session.status = if !current_layer_is_visible {
                "layer is hidden"
            } else {
                "layer is locked"
            }
            .to_string();
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
        Tool::Rig => crate::rigging::handle_stage(app, response, painter, view, ctx),
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
fn pick_cursor(
    app: &mut EditorApp,
    response: &Response,
    view: &StageView,
) -> Option<egui::CursorIcon> {
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
        Tool::Rig => CursorIcon::Default,
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
    app: &mut EditorApp,
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
    let selected_body_hit = selected_raw_body_contains_point(app, cursor_stage);
    let raw_hover_hit = if selected_body_hit {
        false
    } else {
        hit_test_raw_hover_cached(
            &app.state.project,
            &mut app.textures,
            app.session.current_q0rg_id,
            app.session.current_frame,
            cursor_stage,
        )
    };
    if selected_body_hit
        || raw_hover_hit
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

fn transform_axis_aligned_bounds(
    bounds: (f32, f32, f32, f32),
    transform: Affine,
) -> Option<(f32, f32, f32, f32)> {
    let (min_x, min_y, max_x, max_y) = bounds;
    let corners = [
        Vec2::new(min_x, min_y),
        Vec2::new(max_x, min_y),
        Vec2::new(max_x, max_y),
        Vec2::new(min_x, max_y),
    ];
    let mut result: Option<(f32, f32, f32, f32)> = None;
    for point in corners.into_iter().map(|point| transform.apply(point)) {
        if !point.x.is_finite() || !point.y.is_finite() {
            continue;
        }
        result = union_bounds(result, Some((point.x, point.y, point.x, point.y)));
    }
    result
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
    allow_pointer_fallback: bool,
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
    if raw.is_empty() && allow_pointer_fallback {
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
        let samples = advanced_input_samples(
            ctx,
            response,
            view,
            !response.drag_stopped_by(PointerButton::Primary),
        );
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
            let preview_handoff = stroke.clone();
            let region = crate::advanced_brush::advanced_finish(stroke);
            let classic_bridge = crate::brush::BrushSettings {
                color: settings.color,
                size: settings.size,
                smoothing: 0,
                nib: crate::brush::BrushNib::Circle,
                scale_with_stage: settings.scale_with_stage,
                sync_with_eraser: app.session.brush.sync_with_eraser,
                ..crate::brush::BrushSettings::default()
            };
            crate::brush::commit_brush_region_with_material(
                app,
                region,
                classic_bridge,
                advanced_material(settings),
            );
            app.session.advanced_brush_preview_handoff = Some(preview_handoff);
            ctx.request_repaint();
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
                ..crate::brush::BrushSettings::default()
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

fn classic_drag_frame_accepts_samples(
    drag_started: bool,
    dragged: bool,
    drag_stopped: bool,
) -> bool {
    !drag_stopped && (drag_started || dragged)
}

fn classic_input_samples(
    ctx: &Context,
    response: &Response,
    view: &StageView,
) -> Vec<crate::brush::BrushSample> {
    // Pointer-up terminates the gesture. Never interpret PointerMoved events
    // from the same egui frame as paint: Windows pen input can report a final
    // pressureless move around release, which would otherwise stamp a nominal
    // full-size nib at the end of a pressure/velocity stroke.
    if response.drag_stopped_by(PointerButton::Primary) {
        return Vec::new();
    }
    advanced_input_samples(ctx, response, view, true)
        .into_iter()
        .map(|sample| {
            crate::brush::BrushSample::pointer(
                sample.position,
                sample.pressure,
                sample.time_seconds,
            )
        })
        .collect()
}

fn pointer_pressure_at(ctx: &Context, screen: Pos2) -> Option<f32> {
    ctx.input(|input| {
        input.events.iter().rev().find_map(|event| match event {
            egui::Event::Touch { pos, force, .. } if (*pos - screen).length_sq() <= 9.0 => *force,
            _ => None,
        })
    })
}

fn classic_brush(app: &mut EditorApp, response: &Response, view: &StageView, ctx: &Context) {
    let settings = brush_settings_for_view(app, view.scale);
    let preview_color = Color32::from_rgba_unmultiplied(
        app.session.brush.color.r,
        app.session.brush.color.g,
        app.session.brush.color.b,
        app.session.brush.color.a,
    );

    if response.drag_started_by(PointerButton::Primary) {
        if let Some(screen) = response.interact_pointer_pos() {
            let sample = crate::brush::BrushSample::pointer(
                screen_to_stage(screen, view),
                pointer_pressure_at(ctx, screen),
                ctx.input(|input| input.time),
            );
            app.session.tool_state = ToolState::BrushDrawing {
                stroke: crate::brush::brush_begin(settings, sample),
            };
            app.textures.begin_classic_brush_preview(ctx, response.rect);
            if let ToolState::BrushDrawing { stroke } = &app.session.tool_state {
                if let Some((position, size)) = crate::brush::brush_prerender_dab_at(stroke, 0) {
                    let contour = classic_prerender_nib_outline(
                        settings.nib,
                        stage_to_screen(position, view),
                        size * view.scale,
                    );
                    app.textures
                        .raster_classic_brush_preview(&[contour], preview_color);
                }
            }
            app.session.status = "Brush: drawing".to_string();
        }
    }

    let drag_started = response.drag_started_by(PointerButton::Primary);
    let dragged = response.dragged_by(PointerButton::Primary);
    let drag_stopped = response.drag_stopped_by(PointerButton::Primary);
    if classic_drag_frame_accepts_samples(drag_started, dragged, drag_stopped) {
        let samples = classic_input_samples(ctx, response, view);
        let mut new_contours = Vec::new();
        if let ToolState::BrushDrawing { stroke } = &mut app.session.tool_state {
            let old_len = stroke.samples.len();
            for sample in samples {
                crate::brush::brush_add_sample(stroke, settings, sample);
            }
            let new_len = stroke.samples.len();
            if new_len > old_len {
                for index in old_len.max(1)..new_len {
                    let Some((start_position, start_size)) =
                        crate::brush::brush_prerender_dab_at(stroke, index - 1)
                    else {
                        continue;
                    };
                    let Some((end_position, end_size)) =
                        crate::brush::brush_prerender_dab_at(stroke, index)
                    else {
                        continue;
                    };
                    new_contours.push(classic_prerender_segment_contour(
                        settings.nib,
                        (
                            stage_to_screen(start_position, view),
                            start_size * view.scale,
                        ),
                        (stage_to_screen(end_position, view), end_size * view.scale),
                    ));
                }
            }
        }
        if !new_contours.is_empty() {
            app.textures
                .raster_classic_brush_preview(&new_contours, preview_color);
        }
    }

    if response.drag_stopped_by(PointerButton::Primary) {
        if let ToolState::BrushDrawing { stroke } =
            std::mem::replace(&mut app.session.tool_state, ToolState::Idle)
        {
            let region = crate::brush::brush_finish(stroke, settings);
            crate::brush::commit_brush_region(app, region, settings);
            app.session.classic_brush_preview_handoff = true;
            ctx.request_repaint();
        } else {
            app.textures.clear_classic_brush_preview();
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
                crate::brush::BrushSample::pointer(
                    screen_to_stage(screen, view),
                    pointer_pressure_at(ctx, screen),
                    ctx.input(|input| input.time),
                ),
            );
            let region = crate::brush::brush_finish(stroke, settings);
            crate::brush::commit_brush_region(app, region, settings);
        }
    }
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

/// Cheap exact-at-a-point NonZero fill test for interactive cursor/body hit-testing.
/// Unlike `vector_fill_geometry`, this never reconstructs or unions the complete
/// filled surface. That distinction matters for dense Advanced Brush contours:
/// Select asks this question every pointer frame, while full component geometry
/// is only required after the user actually clicks or starts a drag.
fn interactive_paths_contain_or_near(paths: &[VPath], point: Vec2, distance: f32) -> bool {
    let mut winding = 0_i32;
    let mut near_boundary = false;
    let distance = distance.max(0.0);
    for path in paths.iter().filter(|path| path.closed) {
        let points = flatten_path(path);
        if points.len() < 3 {
            continue;
        }
        if point_in_polygon(&points, point) {
            let area = points
                .iter()
                .zip(points.iter().cycle().skip(1))
                .take(points.len())
                .map(|(a, b)| f64::from(a.x) * f64::from(b.y) - f64::from(b.x) * f64::from(a.y))
                .sum::<f64>()
                * 0.5;
            winding += if area >= 0.0 { 1 } else { -1 };
        }
        if distance > 0.0 && nearest_segment_distance(&points, point) <= distance {
            near_boundary = true;
        }
    }
    winding != 0 || near_boundary
}

#[cfg(feature = "appearance-mask-eraser")]
fn interactive_visible_fill_hit(
    vector: &VectorAsset,
    appearance: Option<&q0s_format::v2::VectorAppearance>,
    point: Vec2,
    edge_tolerance: f32,
) -> bool {
    let Some(appearance) = appearance else {
        return interactive_paths_contain_or_near(&vector.paths, point, edge_tolerance);
    };
    let Some(inverse_field) = appearance.field_transform.inverse() else {
        return false;
    };
    let canonical = inverse_field.apply(point);
    let support_hit = if appearance.clip_mask.is_empty() {
        let source = if appearance.material_source.is_empty() {
            &vector.paths
        } else {
            &appearance.material_source
        };
        let radius = match appearance.material {
            q0s_format::v2::VectorMaterial::Solid => 0.0,
            q0s_format::v2::VectorMaterial::SoftHalo { radius, .. } => radius.max(0.0),
        };
        interactive_paths_contain_or_near(source, canonical, radius + edge_tolerance.max(0.0))
    } else {
        interactive_paths_contain_or_near(&appearance.clip_mask, canonical, edge_tolerance.max(0.0))
    };
    support_hit && !interactive_paths_contain_or_near(&appearance.erase_mask, canonical, 0.001)
}

#[cfg(not(feature = "appearance-mask-eraser"))]
fn interactive_visible_fill_hit(
    vector: &VectorAsset,
    _appearance: Option<&q0s_format::v2::VectorAppearance>,
    point: Vec2,
    edge_tolerance: f32,
) -> bool {
    interactive_paths_contain_or_near(&vector.paths, point, edge_tolerance)
}

pub(crate) fn vector_fill_geometry(vector: &VectorAsset) -> MultiPolygon<f64> {
    // Raw anchors are the canonical fill boundary. Bezier handles are only the
    // renderer's smooth approximation, so selection/cut/bucket geometry must
    // share the brush engine's topology instead of flattening every handle and
    // rebuilding the same NonZero surface a second time.
    crate::brush::vector_fill_geometry(vector)
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

#[cfg(any(test, not(feature = "appearance-mask-eraser")))]
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

#[cfg(not(feature = "appearance-mask-eraser"))]
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
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_is_locked(q0rg_id, layer.layer_id)
        {
            continue;
        }
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
        let whole_visual = remainder.unsigned_area() <= 0.05;
        jobs.push((*r, asset_id, selected, remainder, false, whole_visual));
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
        if whole_visual {
            let Some(Asset::Vector(original)) = app
                .state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == asset_id)
            else {
                continue;
            };
            // A rubber-band marquee that encloses the complete visible fill is
            // not a geometric cut. Keep the source VPaths verbatim so a normal
            // move/transform cannot throw away their cubic handles.
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
                    instance_id: 0,
                    frame: app.session.current_frame,
                    target: Target::Asset(selected_asset_id),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
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

fn concrete_selection_parts(selection: &Selection) -> Option<(Vec<PathRef>, Vec<PlacementRef>)> {
    match selection {
        Selection::Path {
            q0rg_id,
            layer_id,
            placement_idx,
            path_idx,
        } => Some((
            vec![PathRef {
                q0rg_id: *q0rg_id,
                layer_id: *layer_id,
                placement_idx: *placement_idx,
                path_idx: *path_idx,
            }],
            Vec::new(),
        )),
        Selection::Paths(paths) => Some((paths.clone(), Vec::new())),
        Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } => Some((
            Vec::new(),
            vec![PlacementRef {
                q0rg_id: *q0rg_id,
                layer_id: *layer_id,
                placement_idx: *placement_idx,
            }],
        )),
        Selection::Multi(objects) => Some((Vec::new(), objects.clone())),
        Selection::Mixed { paths, objects } => Some((paths.clone(), objects.clone())),
        _ => None,
    }
}

fn add_selection_hit(current: &Selection, clicked: Selection) -> Selection {
    let Some((mut paths, mut objects)) = concrete_selection_parts(current) else {
        return clicked;
    };
    let Some((clicked_paths, clicked_objects)) = concrete_selection_parts(&clicked) else {
        return clicked;
    };
    for path in clicked_paths {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    for object in clicked_objects {
        if !objects.contains(&object) {
            objects.push(object);
        }
    }
    selection_from_group_parts(paths, objects)
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
    if !paths.is_empty() {
        paths = isolate_partial_appearance_raw_refs(app, paths);
        let Some(isolated) = isolate_raw_refs_for_live_transform(app, paths) else {
            return false;
        };
        paths = isolated;
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
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(app, &paths);
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
        start_paths: std::sync::Arc::new(start_paths),
        start_appearances: std::sync::Arc::new(start_appearances),
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

fn constrain_move_delta(delta: Vec2, shift: bool) -> Vec2 {
    if !shift {
        return delta;
    }
    if delta.x.abs() >= delta.y.abs() {
        Vec2::new(delta.x, 0.0)
    } else {
        Vec2::new(0.0, delta.y)
    }
}

fn snap_rotation_delta(delta: f32, shift: bool) -> f32 {
    if !shift {
        return delta;
    }
    let step = std::f32::consts::FRAC_PI_4;
    (delta / step).round() * step
}

fn point_in_bounds(point: Vec2, bounds: (f32, f32, f32, f32)) -> bool {
    point.x >= bounds.0 && point.x <= bounds.2 && point.y >= bounds.1 && point.y <= bounds.3
}

fn group_transform_affine(
    operation: GroupTransformOperation,
    start_pivot: Vec2,
    cursor: Vec2,
    shift: bool,
    ctrl: bool,
) -> Option<Affine> {
    match operation {
        GroupTransformOperation::Move { start_cursor } => {
            let delta = constrain_move_delta(
                Vec2::new(cursor.x - start_cursor.x, cursor.y - start_cursor.y),
                shift,
            );
            Some(Affine {
                tx: delta.x,
                ty: delta.y,
                ..Affine::IDENTITY
            })
        }
        GroupTransformOperation::Scale {
            handle,
            start_bounds,
        } => {
            let (anchor, scale_x, scale_y) = raw_handle_scale(
                start_bounds,
                handle,
                cursor,
                start_pivot,
                ctrl,
                shift && handle.is_corner(),
            )?;
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
            let angle = snap_rotation_delta(
                (cursor.y - center.y).atan2(cursor.x - center.x) - start_angle,
                shift,
            );
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
                    affine.tx = -start_pivot.y * shear;
                }
                TransformEdge::Bottom => {
                    let shear = (delta.x / (max_y - min_y)).clamp(-8.0, 8.0);
                    affine.a12 = shear;
                    affine.tx = -start_pivot.y * shear;
                }
                TransformEdge::Left => {
                    let shear = (delta.y / (min_x - max_x)).clamp(-8.0, 8.0);
                    affine.a21 = shear;
                    affine.ty = -start_pivot.x * shear;
                }
                TransformEdge::Right => {
                    let shear = (delta.y / (max_x - min_x)).clamp(-8.0, 8.0);
                    affine.a21 = shear;
                    affine.ty = -start_pivot.x * shear;
                }
            }
            Some(affine)
        }
    }
}

fn apply_group_transform(
    app: &mut EditorApp,
    data: GroupTransformData<'_>,
    cursor: Vec2,
    shift: bool,
    ctrl: bool,
) -> bool {
    let Some(transform) =
        group_transform_affine(data.operation, data.start_pivot, cursor, shift, ctrl)
    else {
        return false;
    };
    let mut changed = set_raw_refs_live_transform(&mut app.state.project, data.refs, transform);
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
    let refs = isolate_partial_appearance_raw_refs(app, refs);
    let Some(refs) = isolate_raw_refs_for_live_transform(app, refs) else {
        return false;
    };
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
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(app, &refs);
    match hit {
        TransformHit::Scale(handle) => {
            app.session.tool_state = ToolState::DraggingRawHandle {
                refs,
                start_paths: std::sync::Arc::new(start_paths),
                start_appearances: std::sync::Arc::new(start_appearances),
                handle,
                start_bounds: bounds,
                start_pivot: pivot,
            };
            app.session.status = "Resizing selected fill area".to_string();
        }
        TransformHit::Rotate(_) => {
            app.session.tool_state = ToolState::DraggingRawRotate {
                refs,
                start_paths: std::sync::Arc::new(start_paths),
                start_appearances: std::sync::Arc::new(start_appearances),
                center: pivot,
                start_angle: (start_cursor.y - pivot.y).atan2(start_cursor.x - pivot.x),
            };
            app.session.status = "Rotating selected fill area".to_string();
        }
        TransformHit::Skew(edge) => {
            app.session.tool_state = ToolState::DraggingRawSkew {
                refs,
                start_paths: std::sync::Arc::new(start_paths),
                start_appearances: std::sync::Arc::new(start_appearances),
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

/// Remove project asset records as one invariant-preserving operation.
/// Sparse metadata is owned by the asset id, so it must never outlive the
/// corresponding asset. Callers are responsible for removing placements first.
pub(crate) fn remove_assets_and_metadata<I>(project: &mut ProjectV2, asset_ids: I)
where
    I: IntoIterator<Item = u16>,
{
    let removed: std::collections::BTreeSet<u16> = asset_ids.into_iter().collect();
    if removed.is_empty() {
        return;
    }
    project
        .assets
        .retain(|asset| !removed.contains(&asset.id()));
    project
        .audio_clips
        .retain(|clip| !removed.contains(&clip.asset_id));
    for asset_id in removed {
        project.asset_names.remove(&asset_id);
        project.asset_appearances.remove(&asset_id);
    }
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
            remove_assets_and_metadata(&mut app.state.project, [hit.asset_id]);
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
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_is_locked(q0rg_id, layer.layer_id)
        {
            continue;
        }
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

fn classic_preview_diameters_px(
    settings: crate::brush::BrushSettings,
    view_scale: f32,
    preview: BrushSizePreview,
) -> (f32, Option<f32>) {
    let outer = brush_cursor_radius_px(settings, view_scale) * 2.0;
    let inner = (preview == BrushSizePreview::MinimumSize)
        .then_some(outer * settings.dynamics_min_size.clamp(0.01, 1.0));
    (outer, inner)
}

fn advanced_preview_diameters_px(
    settings: crate::advanced_brush::AdvancedBrushSettings,
    view_scale: f32,
    preview: BrushSizePreview,
) -> (f32, Option<f32>) {
    let stage_size = if settings.scale_with_stage {
        settings.size
    } else {
        settings.size / view_scale.max(1.0e-4)
    };
    let outer = stage_size.max(0.1) * view_scale;
    let inner = (preview == BrushSizePreview::MinimumSize)
        .then_some(outer * settings.pressure_min_size.clamp(0.01, 1.0));
    (outer, inner)
}

fn advanced_nib_outline_points(
    center: Pos2,
    size_px: f32,
    roundness: f32,
    angle_degrees: f32,
) -> Vec<Pos2> {
    let major = size_px.max(0.1) * 0.5;
    let minor = major * roundness.clamp(0.05, 1.0);
    let angle = angle_degrees.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    (0..=40)
        .map(|index| {
            let phase = std::f32::consts::TAU * index as f32 / 40.0;
            let x = phase.cos() * major;
            let y = phase.sin() * minor;
            pos2(
                center.x + x * cos_a - y * sin_a,
                center.y + x * sin_a + y * cos_a,
            )
        })
        .collect()
}

fn minimum_nib_strokes() -> [Stroke; 2] {
    [
        Stroke::new(2.75_f32, Color32::from_black_alpha(180)),
        Stroke::new(1.5_f32, Color32::from_rgb(235, 48, 55)),
    ]
}

fn draw_minimum_nib_outline(painter: &Painter, points: Vec<Pos2>) {
    if points.len() < 3 {
        return;
    }
    for stroke in minimum_nib_strokes() {
        painter.add(Shape::Path(PathShape {
            points: points.clone(),
            closed: true,
            fill: Color32::TRANSPARENT,
            stroke,
        }));
    }
}

fn visible_stage_preview_rect(
    stage_rect: egui::Rect,
    viewport_rect: egui::Rect,
) -> Option<egui::Rect> {
    let min = pos2(
        stage_rect.min.x.max(viewport_rect.min.x),
        stage_rect.min.y.max(viewport_rect.min.y),
    );
    let max = pos2(
        stage_rect.max.x.min(viewport_rect.max.x),
        stage_rect.max.y.min(viewport_rect.max.y),
    );
    (max.x > min.x && max.y > min.y).then(|| egui::Rect::from_min_max(min, max))
}

/// Live footprint shown at the centre of the currently visible part of the stage.
pub(crate) fn draw_brush_size_preview(app: &mut EditorApp, painter: &Painter, view: &StageView) {
    if app.session.current_tool != Tool::Brush {
        app.session.brush_size_preview = None;
        return;
    }
    let Some(preview) = app.session.brush_size_preview else {
        return;
    };

    let viewport_rect = painter.clip_rect();
    let Some(visible_stage_rect) = visible_stage_preview_rect(view.stage_rect, viewport_rect)
    else {
        return;
    };
    let painter = painter.with_clip_rect(visible_stage_rect);
    let center = visible_stage_rect.center();
    match app.session.brush_mode {
        crate::advanced_brush::BrushMode::Classic => {
            let settings = app.session.brush;
            let (outer, inner) = classic_preview_diameters_px(settings, view.scale, preview);
            draw_nib_cursor_outline(&painter, center, settings.nib, outer);
            if let Some(inner) = inner {
                let points =
                    crate::brush::nib_outline(settings.nib, inner, Vec2::new(center.x, center.y))
                        .into_iter()
                        .map(|point| pos2(point.x, point.y))
                        .collect();
                draw_minimum_nib_outline(&painter, points);
            }
        }
        crate::advanced_brush::BrushMode::Advanced => {
            let settings = app.session.advanced_brush.sanitized();
            let (outer, inner) = advanced_preview_diameters_px(settings, view.scale, preview);
            draw_nib_cursor_points(
                &painter,
                advanced_nib_outline_points(
                    center,
                    outer,
                    settings.roundness,
                    settings.angle_degrees,
                ),
            );
            if let Some(inner) = inner {
                draw_minimum_nib_outline(
                    &painter,
                    advanced_nib_outline_points(
                        center,
                        inner,
                        settings.roundness,
                        settings.angle_degrees,
                    ),
                );
            }
        }
    }
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
            if let ToolState::BrushDrawing { stroke } = &app.session.tool_state {
                if let Some(current_size_px) = classic_dynamic_cursor_size_px(stroke, view.scale) {
                    draw_nib_cursor_accent_outline(
                        painter,
                        center,
                        app.session.brush.nib,
                        current_size_px,
                        selection_color(app),
                    );
                }
            }
        }
        crate::advanced_brush::BrushMode::Advanced => {
            let settings = advanced_brush_settings_for_view(app, view.scale);
            draw_nib_cursor_points(
                painter,
                advanced_nib_outline_points(
                    center,
                    settings.size * view.scale + 2.5,
                    settings.roundness,
                    settings.angle_degrees,
                ),
            );
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

fn classic_dynamic_cursor_size_px(
    stroke: &crate::brush::BrushStroke,
    view_scale: f32,
) -> Option<f32> {
    crate::brush::brush_size_dynamics_enabled(stroke)
        .then(|| crate::brush::brush_current_size(stroke) * view_scale + 2.5)
}

fn draw_nib_cursor_accent_outline(
    painter: &Painter,
    center: Pos2,
    nib: crate::brush::BrushNib,
    size_px: f32,
    accent: Color32,
) {
    let points: Vec<Pos2> = crate::brush::nib_outline(nib, size_px, Vec2::new(center.x, center.y))
        .into_iter()
        .map(|point| pos2(point.x, point.y))
        .collect();
    if points.len() < 3 {
        return;
    }
    painter.add(Shape::Path(PathShape {
        points,
        closed: true,
        fill: Color32::TRANSPARENT,
        stroke: Stroke::new(1.75_f32, accent),
    }));
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
    draw_nib_cursor_points(painter, points);
}

fn nib_cursor_strokes() -> [Stroke; 2] {
    [
        Stroke::new(2.5_f32, Color32::from_black_alpha(210)),
        Stroke::new(1.0_f32, Color32::WHITE),
    ]
}

fn draw_nib_cursor_points(painter: &Painter, points: Vec<Pos2>) {
    if points.len() < 3 {
        return;
    }
    for stroke in nib_cursor_strokes() {
        painter.add(Shape::Path(PathShape {
            points: points.clone(),
            closed: true,
            fill: Color32::TRANSPARENT,
            stroke,
        }));
    }
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

fn begin_dragging_placement(
    app: &mut EditorApp,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    start_cursor: Vec2,
    status: &str,
) -> bool {
    let previous_selection = app.session.selection.clone();
    let rig_bound = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
        .and_then(|layer| {
            active_visual_affine_for_placement(
                &app.state.project,
                q0rg_id,
                layer,
                placement_idx,
                app.session.current_frame,
            )
        })
        .is_some_and(|(_, bound)| bound);
    if rig_bound {
        app.session.status =
            "rig-bound object: pose/move it with the Rig tool or edit its binding".into();
        return false;
    }
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
                    && selected_index == placement_idx
            )
        })
        .map(|pivot| pivot.point);
    let Some(transform) = placement_transform(
        &app.state.project,
        q0rg_id,
        layer_id,
        placement_idx,
        app.session.current_frame,
    ) else {
        return false;
    };

    app.history.snapshot(&app.state.project);
    let Some(placement_idx) = materialize_placement_keyframe_for_edit(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        placement_idx,
        app.session.current_frame,
    ) else {
        return false;
    };
    app.session.selection = Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    };
    if let Some(pivot) = custom_pivot {
        set_selection_transform_pivot(app, pivot);
    }
    let cursor_offset = Vec2::new(transform.tx - start_cursor.x, transform.ty - start_cursor.y);
    let pivot_cursor_offset =
        custom_pivot.map(|pivot| Vec2::new(pivot.x - start_cursor.x, pivot.y - start_cursor.y));
    app.session.tool_state = ToolState::DraggingPlacement {
        q0rg_id,
        layer_id,
        placement_idx,
        start_cursor,
        cursor_offset,
        pivot_cursor_offset,
    };
    app.session.status = status.to_string();
    true
}

fn select(app: &mut EditorApp, response: &Response, cursor: Option<Vec2>, view: &StageView) {
    let (shift_down, ctrl_down, alt_down) = response.ctx.input(|input| {
        (
            input.modifiers.shift,
            input.modifiers.ctrl,
            input.modifiers.alt,
        )
    });
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
            if let Some(hit) = hit_test_raw_selection_cached(
                &app.state.project,
                &mut app.textures,
                app.session.current_q0rg_id,
                app.session.current_frame,
                p,
            ) {
                let clicked = match hit {
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
                app.session.selection = if shift_down {
                    add_selection_hit(&app.session.selection, clicked)
                } else {
                    clicked
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
                    let clicked = Selection::Placement {
                        q0rg_id: app.session.current_q0rg_id,
                        layer_id,
                        placement_idx: idx,
                    };
                    app.session.selection = if shift_down {
                        add_selection_hit(&app.session.selection, clicked)
                    } else {
                        clicked
                    };
                    app.session.status = "Object selected".to_string();
                }
                None => {
                    if !shift_down {
                        app.session.selection = Selection::None;
                    }
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
                let bounds = (bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y);
                if objects.is_empty()
                    && ((alt_down && point_in_bounds(p, bounds))
                        || raw_area_selection_contains_point(
                            &app.state.project,
                            &placements,
                            bounds_min,
                            bounds_max,
                            p,
                        ))
                {
                    let start_pivot = selection_transform_pivot(app);
                    let refs = cut_raw_areas_for_drag(
                        app,
                        &placements,
                        (bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y),
                    );
                    let refs = isolate_partial_appearance_raw_refs(app, refs);
                    let refs = isolate_raw_refs_for_live_transform(app, refs).unwrap_or_default();
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
                            capture_whole_asset_appearances_for_raw_refs(app, &refs);
                        app.session.tool_state = ToolState::DraggingPaths {
                            refs,
                            start_cursor: p,
                            start_paths: std::sync::Arc::new(start_paths),
                            start_appearances: std::sync::Arc::new(start_appearances),
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
            if alt_down {
                if let Selection::Placement {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                } = app.session.selection.clone()
                {
                    if selection_transform_bounds(app)
                        .is_some_and(|bounds| point_in_bounds(p, bounds))
                        && begin_dragging_placement(
                            app,
                            q0rg_id,
                            layer_id,
                            placement_idx,
                            p,
                            "Moving selection",
                        )
                    {
                        return;
                    }
                }
                if let Some(refs) = selection_raw_path_refs(&app.session.selection) {
                    if raw_path_refs_ui_bounds(&app.state.project, &refs)
                        .is_some_and(|bounds| point_in_bounds(p, bounds))
                        && begin_dragging_raw_paths(app, refs, p, "Moving selected raw graphics")
                    {
                        return;
                    }
                }
            }

            // 2. Raw fill dragging moves only the connected region under the
            // pointer. Open strokes still move as individual contours.
            if let Some(hit) = hit_test_raw_selection_cached(
                &app.state.project,
                &mut app.textures,
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
                        start_cursor: p,
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
                start_cursor,
                cursor_offset,
                pivot_cursor_offset,
            } => {
                if let Some(p) = cursor {
                    let delta = constrain_move_delta(
                        Vec2::new(p.x - start_cursor.x, p.y - start_cursor.y),
                        shift_down,
                    );
                    let constrained = Vec2::new(start_cursor.x + delta.x, start_cursor.y + delta.y);
                    let mut moved = false;
                    if let Some(pl) = placement_mut(app, q0rg_id, layer_id, placement_idx) {
                        pl.transform.tx = constrained.x + cursor_offset.x;
                        pl.transform.ty = constrained.y + cursor_offset.y;
                        moved = true;
                    }
                    if moved {
                        app.state.dirty = true;
                        if let Some(offset) = pivot_cursor_offset {
                            set_selection_transform_pivot(
                                app,
                                Vec2::new(constrained.x + offset.x, constrained.y + offset.y),
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
                    let delta = constrain_move_delta(
                        Vec2::new(p.x - start_cursor.x, p.y - start_cursor.y),
                        shift_down,
                    );
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
                        if let Some(asset_id) = raw_placement_asset_id(
                            &app.state.project,
                            q0rg_id,
                            layer_id,
                            placement_idx,
                        ) {
                            app.textures.invalidate_asset(asset_id);
                        }
                    }
                }
            }
            ToolState::DraggingPaths {
                refs,
                start_cursor,
                start_pivot,
                ..
            } => {
                if let Some(p) = cursor {
                    let delta = constrain_move_delta(
                        Vec2::new(p.x - start_cursor.x, p.y - start_cursor.y),
                        shift_down,
                    );
                    let transform = Affine {
                        tx: delta.x,
                        ty: delta.y,
                        ..Affine::IDENTITY
                    };
                    if set_raw_refs_live_transform(&mut app.state.project, &refs, transform) {
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
                handle,
                start_bounds,
                start_pivot,
                ..
            } => {
                if let Some(p) = cursor {
                    if let Some(transform) = group_transform_affine(
                        GroupTransformOperation::Scale {
                            handle,
                            start_bounds,
                        },
                        start_pivot,
                        p,
                        shift_down,
                        ctrl_down,
                    ) {
                        if set_raw_refs_live_transform(&mut app.state.project, &refs, transform) {
                            app.state.dirty = true;
                            set_selection_transform_pivot(app, transform.apply(start_pivot));
                        }
                    }
                }
            }
            ToolState::DraggingRawRotate {
                refs,
                center,
                start_angle,
                ..
            } => {
                if let Some(p) = cursor {
                    if let Some(transform) = group_transform_affine(
                        GroupTransformOperation::Rotate {
                            center,
                            start_angle,
                        },
                        center,
                        p,
                        shift_down,
                        false,
                    ) {
                        if set_raw_refs_live_transform(&mut app.state.project, &refs, transform) {
                            app.state.dirty = true;
                        }
                    }
                }
            }
            ToolState::DraggingRawSkew {
                refs,
                edge,
                start_bounds,
                start_cursor,
                start_pivot,
                ..
            } => {
                if let Some(p) = cursor {
                    if let Some(transform) = group_transform_affine(
                        GroupTransformOperation::Skew {
                            edge,
                            start_bounds,
                            start_cursor,
                        },
                        start_pivot,
                        p,
                        false,
                        false,
                    ) {
                        if set_raw_refs_live_transform(&mut app.state.project, &refs, transform) {
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
                    let delta = constrain_move_delta(
                        Vec2::new(p.x - start_cursor.x, p.y - start_cursor.y),
                        shift_down,
                    );
                    if replace_raw_path_points_translated(
                        &mut app.state.project,
                        path,
                        &anchor_indices,
                        &start_path,
                        delta,
                    ) {
                        app.state.dirty = true;
                        if let Some(asset_id) = raw_placement_asset_id(
                            &app.state.project,
                            path.q0rg_id,
                            path.layer_id,
                            path.placement_idx,
                        ) {
                            app.textures.invalidate_asset(asset_id);
                        }
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
                start_world_bbox: _,
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
                            handle,
                            p,
                            pivot_local,
                            ScaleDragModifiers {
                                ignore_pivot: ctrl_down,
                                lock_aspect: shift_down && handle.is_corner(),
                            },
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
                    let delta = snap_rotation_delta(
                        (p.y - center_world.y).atan2(p.x - center_world.x) - start_angle,
                        shift_down,
                    );
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
                        pivot_local,
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
                start_paths: _,
                start_appearances: _,
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
                            objects: &objects,
                            start_transforms: &start_transforms,
                            operation,
                            start_pivot,
                        },
                        p,
                        shift_down,
                        ctrl_down,
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
        let raw_assets = raw_edit_assets(&app.state.project, &finished_state);
        if let Some(refs) = live_raw_transform_refs(&finished_state) {
            if bake_raw_refs_live_transform(app, refs) {
                app.state.dirty = true;
            }
        }
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
        for ((q0rg_id, layer_id), edited_asset_ids) in raw_assets {
            merged |= crate::brush::merge_touching_raw_fills_after_edit_focused(
                &mut app.state.project,
                q0rg_id,
                layer_id,
                app.session.current_frame,
                &edited_asset_ids,
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

fn raw_edit_assets(
    project: &ProjectV2,
    state: &ToolState,
) -> std::collections::BTreeMap<(u16, u16), std::collections::BTreeSet<u16>> {
    let mut assets = std::collections::BTreeMap::new();
    let mut add = |q0rg_id: u16, layer_id: u16, placement_idx: usize| {
        if let Some(asset_id) = raw_placement_asset_id(project, q0rg_id, layer_id, placement_idx) {
            assets
                .entry((q0rg_id, layer_id))
                .or_insert_with(std::collections::BTreeSet::new)
                .insert(asset_id);
        }
    };
    match state {
        ToolState::DraggingPath {
            q0rg_id,
            layer_id,
            placement_idx,
            ..
        } => add(*q0rg_id, *layer_id, *placement_idx),
        ToolState::DraggingPaths { refs, .. }
        | ToolState::DraggingRawHandle { refs, .. }
        | ToolState::DraggingRawRotate { refs, .. }
        | ToolState::DraggingRawSkew { refs, .. }
        | ToolState::DraggingGroup { refs, .. } => {
            for reference in refs {
                add(
                    reference.q0rg_id,
                    reference.layer_id,
                    reference.placement_idx,
                );
            }
        }
        ToolState::DraggingPathPoints { path, .. } => {
            add(path.q0rg_id, path.layer_id, path.placement_idx);
        }
        _ => {}
    }
    assets
}

#[cfg(feature = "appearance-mask-eraser")]
fn cached_paths_contain_or_near(
    paths: &[CachedInteractivePath],
    point: Vec2,
    distance: f32,
) -> bool {
    let distance = distance.max(0.0);
    let mut winding = 0_i32;
    let mut near_boundary = false;
    for path in paths {
        let Some((min_x, min_y, max_x, max_y)) = path.bounds else {
            continue;
        };
        if point.x < min_x - distance
            || point.x > max_x + distance
            || point.y < min_y - distance
            || point.y > max_y + distance
        {
            continue;
        }
        if point_in_polygon(&path.points, point) {
            winding += if path.signed_area >= 0.0 { 1 } else { -1 };
        }
        if distance > 0.0 && nearest_segment_distance(&path.points, point) <= distance {
            near_boundary = true;
        }
    }
    winding != 0 || near_boundary
}

#[cfg(feature = "appearance-mask-eraser")]
fn interactive_visible_fill_hit_cached(
    vector: &VectorAsset,
    appearance: Option<&q0s_format::v2::VectorAppearance>,
    point: Vec2,
    edge_tolerance: f32,
    textures: &mut TextureCache,
) -> bool {
    let geometry = textures.appearance_hit_geometry(vector, appearance);
    let Some(appearance) = appearance else {
        return cached_paths_contain_or_near(&geometry.source, point, edge_tolerance);
    };
    let Some(inverse_field) = appearance.field_transform.inverse() else {
        return false;
    };
    let canonical = inverse_field.apply(point);
    let support_hit = if geometry.clip.is_empty() {
        let radius = match appearance.material {
            q0s_format::v2::VectorMaterial::Solid => 0.0,
            q0s_format::v2::VectorMaterial::SoftHalo { radius, .. } => radius.max(0.0),
        };
        cached_paths_contain_or_near(
            &geometry.source,
            canonical,
            radius + edge_tolerance.max(0.0),
        )
    } else {
        cached_paths_contain_or_near(&geometry.clip, canonical, edge_tolerance.max(0.0))
    };
    support_hit && !cached_paths_contain_or_near(&geometry.erase, canonical, 0.001)
}

fn hit_test_raw_hover_cached(
    project: &ProjectV2,
    textures: &mut TextureCache,
    q0rg_id: u16,
    frame: u16,
    cursor: Vec2,
) -> bool {
    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        let _ = textures;
        return hit_test_raw_hover(project, q0rg_id, frame, cursor);
    }
    #[cfg(feature = "appearance-mask-eraser")]
    {
        let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
            return false;
        };
        for layer in q.layers.iter().rev() {
            if !project.layer_is_visible(q0rg_id, layer.layer_id)
                || project.layer_is_locked(q0rg_id, layer.layer_id)
            {
                continue;
            }
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
                if vector.fill.is_some()
                    && interactive_visible_fill_hit_cached(
                        vector,
                        project.asset_appearances.get(&asset_id),
                        cursor,
                        2.0,
                        textures,
                    )
                {
                    return true;
                }
                let stroke_radius = vector
                    .stroke
                    .as_ref()
                    .map(|stroke| stroke.width.max(1.0) * 0.5 + 3.0)
                    .unwrap_or(3.0);
                for path in vector.paths.iter().rev() {
                    if path.closed && vector.fill.is_some() {
                        continue;
                    }
                    let points = flatten_path(path);
                    if points.len() >= 2
                        && nearest_segment_distance(&points, cursor) <= stroke_radius
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
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
#[cfg(not(feature = "appearance-mask-eraser"))]
fn hit_test_raw_hover(project: &ProjectV2, q0rg_id: u16, frame: u16, cursor: Vec2) -> bool {
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
        return false;
    };
    for layer in q.layers.iter().rev() {
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_is_locked(q0rg_id, layer.layer_id)
        {
            continue;
        }
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
            if vector.fill.is_some()
                && interactive_visible_fill_hit(
                    vector,
                    project.asset_appearances.get(&asset_id),
                    cursor,
                    2.0,
                )
            {
                return true;
            }
            let stroke_radius = vector
                .stroke
                .as_ref()
                .map(|stroke| stroke.width.max(1.0) * 0.5 + 3.0)
                .unwrap_or(3.0);
            for path in vector.paths.iter().rev() {
                if path.closed && vector.fill.is_some() {
                    continue;
                }
                let points = flatten_path(path);
                if points.len() >= 2 && nearest_segment_distance(&points, cursor) <= stroke_radius {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(feature = "appearance-mask-eraser")]
fn hit_test_raw_selection_cached(
    project: &ProjectV2,
    textures: &mut TextureCache,
    q0rg_id: u16,
    frame: u16,
    cursor: Vec2,
) -> Option<RawSelectionHit> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    for layer in q.layers.iter().rev() {
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_is_locked(q0rg_id, layer.layer_id)
        {
            continue;
        }
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
                let appearance = project.asset_appearances.get(&asset_id);
                if interactive_visible_fill_hit_cached(vector, appearance, cursor, 2.0, textures) {
                    let field = appearance
                        .map(|appearance| appearance.field_transform)
                        .unwrap_or(Affine::IDENTITY);
                    let canonical_cursor = field.inverse()?.apply(cursor);
                    if let Some(components) = textures.raw_selection_components(vector, appearance)
                    {
                        for component in &components.components {
                            let owns_point = if let Some(appearance) = appearance {
                                crate::appearance::material_support_contains_point(
                                    &component.canonical_surface,
                                    appearance.material,
                                    canonical_cursor,
                                    2.0,
                                )
                            } else {
                                crate::appearance::surface_contains_or_near(
                                    &component.canonical_surface,
                                    canonical_cursor,
                                    2.0,
                                )
                            };
                            if !owns_point {
                                continue;
                            }
                            let refs: Vec<PathRef> = component
                                .path_indices
                                .iter()
                                .copied()
                                .map(|path_idx| PathRef {
                                    q0rg_id,
                                    layer_id: layer.layer_id,
                                    placement_idx,
                                    path_idx,
                                })
                                .collect();
                            if !refs.is_empty() {
                                return Some(RawSelectionHit::Fill(refs));
                            }
                        }
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

#[cfg(not(feature = "appearance-mask-eraser"))]
fn hit_test_raw_selection_cached(
    project: &ProjectV2,
    _textures: &mut TextureCache,
    q0rg_id: u16,
    frame: u16,
    cursor: Vec2,
) -> Option<RawSelectionHit> {
    hit_test_raw_selection(project, q0rg_id, frame, cursor)
}

fn hit_test_raw_selection(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    cursor: Vec2,
) -> Option<RawSelectionHit> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    for layer in q.layers.iter().rev() {
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_is_locked(q0rg_id, layer.layer_id)
        {
            continue;
        }
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
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_is_locked(q0rg_id, layer.layer_id)
        {
            continue;
        }
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
    let mut touched_assets = std::collections::BTreeSet::new();
    #[cfg(feature = "appearance-mask-eraser")]
    let mut render_seeds = Vec::new();
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
        touched_assets.insert(original_asset_id);

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
        #[cfg(feature = "appearance-mask-eraser")]
        let render_seed = if writable_asset_id == original_asset_id
            && !app
                .state
                .project
                .asset_appearances
                .contains_key(&writable_asset_id)
        {
            app.state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == writable_asset_id)
                .and_then(|asset| match asset {
                    Asset::Vector(vector) if vector.stroke.is_none() => app
                        .textures
                        .plain_selection_split_cache_seed(vector, &path_indices),
                    _ => None,
                })
        } else {
            None
        };
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
        touched_assets.insert(writable_asset_id);
        touched_assets.insert(selected_asset_id);
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: selected_asset_id,
            paths: selected_paths,
            fill,
            stroke,
        }));
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(seed) = render_seed {
            let path_count = app
                .state
                .project
                .assets
                .iter()
                .find_map(|asset| match asset {
                    Asset::Vector(vector) if vector.asset_id == selected_asset_id => {
                        Some(vector.paths.len())
                    }
                    _ => None,
                })
                .unwrap_or(0);
            render_seeds.push((selected_asset_id, path_count, seed));
        }
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
                    instance_id: 0,
                    frame,
                    target: Target::Asset(selected_asset_id),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
                },
            );
        }
        created_assets.push((q0rg_id, layer_id, selected_asset_id));
    }

    if !emptied_assets.is_empty() {
        remove_assets_and_metadata(&mut app.state.project, emptied_assets.iter().copied());
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
    app.textures.invalidate_assets(touched_assets);
    #[cfg(feature = "appearance-mask-eraser")]
    for (asset_id, path_count, seed) in render_seeds {
        app.textures
            .install_plain_selection_split_cache_seed(asset_id, path_count, seed);
    }
    Some(placements)
}

fn raw_refs_cover_whole_placements(project: &ProjectV2, refs: &[PathRef]) -> bool {
    let mut grouped: std::collections::BTreeMap<
        (u16, u16, usize),
        std::collections::BTreeSet<usize>,
    > = std::collections::BTreeMap::new();
    for reference in refs {
        grouped
            .entry((
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
            ))
            .or_default()
            .insert(reference.path_idx);
    }
    !grouped.is_empty()
        && grouped
            .into_iter()
            .all(|((q0rg_id, layer_id, placement_idx), selected)| {
                let Some(placement) = project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                    .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
                    .and_then(|layer| layer.placements.get(placement_idx))
                else {
                    return false;
                };
                if placement.transform != Transform2D::IDENTITY {
                    return false;
                }
                let Target::Asset(asset_id) = placement.target else {
                    return false;
                };
                let Some(Asset::Vector(vector)) =
                    project.assets.iter().find(|asset| asset.id() == asset_id)
                else {
                    return false;
                };
                selected.len() == vector.paths.len()
                    && selected.iter().copied().eq(0..vector.paths.len())
            })
}

fn full_raw_refs_for_placements(
    project: &ProjectV2,
    placements: &[PlacementRef],
) -> Option<Vec<PathRef>> {
    let mut refs = Vec::new();
    for placement_ref in placements {
        let placement = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == placement_ref.q0rg_id)?
            .layers
            .iter()
            .find(|layer| layer.layer_id == placement_ref.layer_id)?
            .placements
            .get(placement_ref.placement_idx)?;
        let Target::Asset(asset_id) = placement.target else {
            return None;
        };
        let Asset::Vector(vector) = project.assets.iter().find(|asset| asset.id() == asset_id)?
        else {
            return None;
        };
        refs.extend((0..vector.paths.len()).map(|path_idx| PathRef {
            q0rg_id: placement_ref.q0rg_id,
            layer_id: placement_ref.layer_id,
            placement_idx: placement_ref.placement_idx,
            path_idx,
        }));
    }
    (!refs.is_empty()).then_some(refs)
}

/// Live Select transforms must never rewrite/tessellate thousands of raw anchors
/// on every pointer event. Isolate a partial connected selection once, then the
/// renderer can move its immutable cached vector through the placement affine.
fn isolate_raw_refs_for_live_transform(
    app: &mut EditorApp,
    refs: Vec<PathRef>,
) -> Option<Vec<PathRef>> {
    if refs.is_empty() {
        return None;
    }
    if raw_refs_cover_whole_placements(&app.state.project, &refs) {
        return Some(refs);
    }
    let placements = materialize_raw_paths_as_placements(app, &refs)?;
    full_raw_refs_for_placements(&app.state.project, &placements)
}

fn set_raw_refs_live_transform(
    project: &mut ProjectV2,
    refs: &[PathRef],
    transform: Affine,
) -> bool {
    let Some(next) = crate::app::affine_to_transform(transform) else {
        return false;
    };
    let unique: std::collections::BTreeSet<(u16, u16, usize)> = refs
        .iter()
        .map(|reference| {
            (
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
            )
        })
        .collect();
    let mut changed = false;
    for (q0rg_id, layer_id, placement_idx) in unique {
        let Some(placement) = project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == layer_id)
            })
            .and_then(|layer| layer.placements.get_mut(placement_idx))
        else {
            continue;
        };
        if placement.transform != next {
            placement.transform = next;
            changed = true;
        }
    }
    changed
}

fn bake_raw_refs_live_transform(app: &mut EditorApp, refs: &[PathRef]) -> bool {
    let unique: std::collections::BTreeSet<(u16, u16, usize)> = refs
        .iter()
        .map(|reference| {
            (
                reference.q0rg_id,
                reference.layer_id,
                reference.placement_idx,
            )
        })
        .collect();
    let mut changed = false;
    for (q0rg_id, layer_id, placement_idx) in unique {
        let Some((asset_id, transform)) = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| layer.placements.get(placement_idx))
            .and_then(|placement| match placement.target {
                Target::Asset(asset_id) => Some((asset_id, placement.transform)),
                Target::Q0rg(_) => None,
            })
        else {
            continue;
        };
        if transform == Transform2D::IDENTITY {
            continue;
        }
        let affine = Affine::from_transform(transform);
        if let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == asset_id)
        {
            for path in &mut vector.paths {
                for anchor in &mut path.anchors {
                    anchor.point = affine.apply(anchor.point);
                    if let Some(point) = &mut anchor.in_handle {
                        *point = affine.apply(*point);
                    }
                    if let Some(point) = &mut anchor.out_handle {
                        *point = affine.apply(*point);
                    }
                }
            }
            changed = true;
        }
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = app.state.project.asset_appearances.get_mut(&asset_id) {
            appearance.field_transform = Affine::compose(affine, appearance.field_transform);
            changed = true;
        }
        if let Some(placement) = app
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
            .and_then(|layer| layer.placements.get_mut(placement_idx))
        {
            placement.transform = Transform2D::IDENTITY;
        }
        app.textures.invalidate_asset(asset_id);
    }
    changed
}

fn live_raw_transform_refs(state: &ToolState) -> Option<&[PathRef]> {
    match state {
        ToolState::DraggingPaths { refs, .. }
        | ToolState::DraggingRawHandle { refs, .. }
        | ToolState::DraggingRawRotate { refs, .. }
        | ToolState::DraggingRawSkew { refs, .. }
        | ToolState::DraggingGroup { refs, .. }
            if !refs.is_empty() =>
        {
            Some(refs)
        }
        _ => None,
    }
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

        let placement_transform = Affine::from_transform(placement.transform);
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = project.asset_appearances.get(&asset_id) {
            if let Some(bounds) = crate::appearance::fast_visible_material_bounds_for_paths(
                vector,
                Some(appearance),
                &path_indices,
            ) {
                result = union_bounds(
                    result,
                    transform_axis_aligned_bounds(bounds, placement_transform),
                );
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
            // Bezier curves are contained by their control-point hull. Using the
            // anchors + handles gives a conservative transform frame in O(anchors)
            // without re-flattening a 100k-point display contour every repaint.
            for point in path.anchors.iter().flat_map(|anchor| {
                std::iter::once(anchor.point)
                    .chain(anchor.in_handle)
                    .chain(anchor.out_handle)
            }) {
                let point = placement_transform.apply(point);
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
                if interactive_visible_fill_hit(vector, Some(appearance), point, 2.0) {
                    let all_closed_selected = vector
                        .paths
                        .iter()
                        .enumerate()
                        .filter(|(_, path)| path.closed)
                        .all(|(index, _)| closed_indices.contains(&index));
                    if all_closed_selected {
                        return true;
                    }
                    let Some(inverse_field) = appearance.field_transform.inverse() else {
                        continue;
                    };
                    let canonical_point = inverse_field.apply(point);
                    let selected_paths: Vec<VPath> = closed_indices
                        .iter()
                        .filter_map(|index| vector.paths.get(*index).cloned())
                        .collect();
                    let radius = match appearance.material {
                        q0s_format::v2::VectorMaterial::Solid => 0.0,
                        q0s_format::v2::VectorMaterial::SoftHalo { radius, .. } => radius.max(0.0),
                    };
                    if interactive_paths_contain_or_near(
                        &selected_paths,
                        canonical_point,
                        radius + 2.0,
                    ) {
                        return true;
                    }
                }
                // Appearance owns the visible body. Never fall through to the
                // hidden source vector for cursor/drag hit-testing.
                continue;
            }

            let selected_paths: Vec<VPath> = closed_indices
                .iter()
                .filter_map(|index| vector.paths.get(*index).cloned())
                .collect();
            if interactive_paths_contain_or_near(&selected_paths, point, 2.0) {
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
            return interactive_visible_fill_hit(vector, Some(appearance), point, 2.0);
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

fn isolate_partial_appearance_raw_refs(app: &mut EditorApp, refs: Vec<PathRef>) -> Vec<PathRef> {
    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        let _ = app;
        return refs;
    }
    #[cfg(feature = "appearance-mask-eraser")]
    {
        let Some(first) = refs.first().copied() else {
            return refs;
        };
        if refs.iter().any(|reference| {
            reference.q0rg_id != first.q0rg_id
                || reference.layer_id != first.layer_id
                || reference.placement_idx != first.placement_idx
        }) {
            // Mixed/marquee selections are materialized through the raw-area
            // partition path. This helper is specifically the normal Select
            // click path: one connected fill inside one raw placement.
            return refs;
        }

        let Some(source_placement) = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == first.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter()
                    .find(|layer| layer.layer_id == first.layer_id)
            })
            .and_then(|layer| layer.placements.get(first.placement_idx))
            .cloned()
        else {
            return refs;
        };
        let Target::Asset(asset_id) = source_placement.target else {
            return refs;
        };
        let selected_indices: std::collections::BTreeSet<usize> =
            refs.iter().map(|reference| reference.path_idx).collect();
        let Some(original_ref) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
            .and_then(|asset| match asset {
                Asset::Vector(vector) => Some(vector),
                Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => None,
            })
        else {
            return refs;
        };
        let all_closed: std::collections::BTreeSet<usize> = original_ref
            .paths
            .iter()
            .enumerate()
            .filter_map(|(index, path)| path.closed.then_some(index))
            .collect();
        if selected_indices.is_empty()
            || selected_indices == all_closed
            || !selected_indices.iter().all(|index| {
                original_ref
                    .paths
                    .get(*index)
                    .is_some_and(|path| path.closed)
            })
        {
            // The overwhelmingly common post-split/whole-fill drag needs no
            // appearance partition. Return before cloning dense vector/material data.
            return refs;
        }
        let original = original_ref.clone();
        let Some(mut original_appearance) =
            app.state.project.asset_appearances.get(&asset_id).cloned()
        else {
            return refs;
        };
        if original_appearance.material_source.is_empty()
            && original_appearance.field_transform != Affine::IDENTITY
            && crate::appearance::freeze_material_source_from_current_body(
                &original,
                &mut original_appearance,
            )
        {
            // Repair only the impossible state emitted by the old buggy drag path.
            // A fresh identity-field appearance stays unfrozen so the fast split can
            // keep the stationary remainder lightweight.
            app.state
                .project
                .asset_appearances
                .insert(asset_id, original_appearance.clone());
            app.textures.invalidate_asset(asset_id);
        }

        let mut selected_paths = Vec::new();
        let mut remainder_paths = Vec::new();
        let mut path_remap = std::collections::BTreeMap::new();
        for (old_index, path) in original.paths.iter().cloned().enumerate() {
            if selected_indices.contains(&old_index) {
                path_remap.insert(old_index, selected_paths.len());
                selected_paths.push(path);
            } else {
                remainder_paths.push(path);
            }
        }
        if selected_paths.is_empty() || !remainder_paths.iter().any(|path| path.closed) {
            return refs;
        }

        let partition = if original_appearance.clip_mask.is_empty() {
            crate::appearance::partition_unclipped_appearance_by_source_subset(
                &original_appearance,
                &original.paths,
                &selected_paths,
            )
        } else {
            // Already-fragmented material has an explicit finite clip. Preserve
            // the exact old path for that rarer case; the common first split above
            // avoids buffering the complete glow support.
            let selected_visible = crate::appearance::visible_material_surface_for_paths(
                &original,
                Some(&original_appearance),
                &selected_indices.iter().copied().collect::<Vec<_>>(),
            );
            if selected_visible.0.is_empty() {
                return refs;
            }
            crate::appearance::partition_appearance(
                &original_appearance,
                &original.paths,
                &selected_visible,
            )
        };
        let Some((remainder_appearance, selected_appearance)) = partition else {
            return refs;
        };

        let remainder_asset_id = next_asset_id(&app.state.project);
        let Some(Asset::Vector(selected_vector)) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == asset_id)
        else {
            return refs;
        };
        selected_vector.paths = selected_paths;
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: remainder_asset_id,
            paths: remainder_paths,
            fill: original.fill,
            stroke: original.stroke,
        }));
        app.state
            .project
            .asset_appearances
            .insert(asset_id, selected_appearance);
        app.state
            .project
            .asset_appearances
            .insert(remainder_asset_id, remainder_appearance);

        let Some(layer) = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == first.q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == first.layer_id)
            })
        else {
            return refs;
        };
        let mut remainder_placement = source_placement;
        remainder_placement.target = Target::Asset(remainder_asset_id);
        let insertion = (first.placement_idx + 1).min(layer.placements.len());
        layer.placements.insert(insertion, remainder_placement);

        let remapped = refs
            .into_iter()
            .filter_map(|mut reference| {
                reference.path_idx = *path_remap.get(&reference.path_idx)?;
                Some(reference)
            })
            .collect();
        app.textures.invalidate_asset(asset_id);
        app.state.dirty = true;
        remapped
    }
}

fn capture_whole_asset_appearances_for_raw_refs(
    app: &mut EditorApp,
    refs: &[PathRef],
) -> Vec<AppearanceTransformSnapshot> {
    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        let _ = (app, refs);
        return Vec::new();
    }
    #[cfg(feature = "appearance-mask-eraser")]
    {
        let project = &mut app.state.project;
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

        let mut captured = Vec::new();
        let mut frozen_assets = Vec::new();
        for (asset_id, selected) in grouped {
            let Some(Asset::Vector(vector)) =
                project.assets.iter().find(|asset| asset.id() == asset_id)
            else {
                continue;
            };
            let editable_matches = vector.paths.iter().filter(|path| path.closed).count()
                == selected.len()
                && selected
                    .iter()
                    .all(|index| vector.paths.get(*index).is_some_and(|path| path.closed));
            if !editable_matches {
                continue;
            }
            let Some(appearance) = project.asset_appearances.get(&asset_id) else {
                continue;
            };
            if !appearance.material_source.is_empty() {
                captured.push(AppearanceTransformSnapshot {
                    asset_id,
                    field_transform: appearance.field_transform,
                });
                continue;
            }

            // Only the first affine edit needs a dense vector clone: it becomes the
            // frozen pre-transform material source. Subsequent drag starts never copy
            // the carrier just to capture six affine floats.
            let vector = vector.clone();
            let Some(appearance) = project.asset_appearances.get_mut(&asset_id) else {
                continue;
            };
            // Persist the frozen material source in the project itself before raw
            // geometry starts moving. If an older buggy edit left a non-identity
            // field, freeze_material_source_from_current_body maps it back through
            // the inverse field first.
            if crate::appearance::freeze_material_source_from_current_body(&vector, appearance) {
                frozen_assets.push(asset_id);
            }
            captured.push(AppearanceTransformSnapshot {
                asset_id,
                field_transform: appearance.field_transform,
            });
        }
        app.textures.invalidate_assets(frozen_assets);
        captured
    }
}

#[cfg(test)]
fn transform_captured_appearances(
    project: &mut ProjectV2,
    start_appearances: &[AppearanceTransformSnapshot],
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
        for source in start_appearances {
            let next_field = Affine::compose(transform, source.field_transform);
            if let Some(current) = project.asset_appearances.get_mut(&source.asset_id) {
                if current.field_transform != next_field {
                    // Drag state needs only the original field affine. Keeping a full
                    // VectorAppearance here used to clone material_source/masks with
                    // thousands of anchors on every mouse-down.
                    current.field_transform = next_field;
                    changed = true;
                }
            }
        }
        changed
    }
}

#[cfg(test)]
fn translate_captured_appearances(
    project: &mut ProjectV2,
    start_appearances: &[AppearanceTransformSnapshot],
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

#[cfg(test)]
fn apply_raw_affine_snapshot(
    project: &mut ProjectV2,
    refs: &[PathRef],
    start_paths: &[VPath],
    start_appearances: &[AppearanceTransformSnapshot],
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
    let refs = isolate_partial_appearance_raw_refs(app, refs);
    let Some(refs) = isolate_raw_refs_for_live_transform(app, refs) else {
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
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(app, &refs);
    app.session.tool_state = ToolState::DraggingPaths {
        refs,
        start_cursor,
        start_paths: std::sync::Arc::new(start_paths),
        start_appearances: std::sync::Arc::new(start_appearances),
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
    let refs = isolate_partial_appearance_raw_refs(app, refs);
    let Some(refs) = isolate_raw_refs_for_live_transform(app, refs) else {
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
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(app, &refs);
    app.session.tool_state = ToolState::DraggingRawHandle {
        refs,
        start_paths: std::sync::Arc::new(start_paths),
        start_appearances: std::sync::Arc::new(start_appearances),
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
    let refs = isolate_partial_appearance_raw_refs(app, refs);
    let Some(refs) = isolate_raw_refs_for_live_transform(app, refs) else {
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
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(app, &refs);
    app.session.tool_state = ToolState::DraggingRawRotate {
        refs,
        start_paths: std::sync::Arc::new(start_paths),
        start_appearances: std::sync::Arc::new(start_appearances),
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
    let refs = isolate_partial_appearance_raw_refs(app, refs);
    let Some(refs) = isolate_raw_refs_for_live_transform(app, refs) else {
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
    let start_appearances = capture_whole_asset_appearances_for_raw_refs(app, &refs);
    app.session.tool_state = ToolState::DraggingRawSkew {
        refs,
        start_paths: std::sync::Arc::new(start_paths),
        start_appearances: std::sync::Arc::new(start_appearances),
        edge,
        start_bounds: bounds,
        start_cursor,
        start_pivot,
    };
    app.session.status = "Skewing raw graphics".to_string();
    true
}

fn handle_point(bounds: (f32, f32, f32, f32), handle: Handle) -> Vec2 {
    let (min_x, min_y, max_x, max_y) = bounds;
    match handle {
        Handle::TopLeft => Vec2::new(min_x, min_y),
        Handle::TopRight => Vec2::new(max_x, min_y),
        Handle::BottomRight => Vec2::new(max_x, max_y),
        Handle::BottomLeft => Vec2::new(min_x, max_y),
        Handle::MidTop => Vec2::new((min_x + max_x) * 0.5, min_y),
        Handle::MidRight => Vec2::new(max_x, (min_y + max_y) * 0.5),
        Handle::MidBottom => Vec2::new((min_x + max_x) * 0.5, max_y),
        Handle::MidLeft => Vec2::new(min_x, (min_y + max_y) * 0.5),
    }
}

fn opposite_handle_point(bounds: (f32, f32, f32, f32), handle: Handle) -> Vec2 {
    let opposite = match handle {
        Handle::TopLeft => Handle::BottomRight,
        Handle::TopRight => Handle::BottomLeft,
        Handle::BottomRight => Handle::TopLeft,
        Handle::BottomLeft => Handle::TopRight,
        Handle::MidTop => Handle::MidBottom,
        Handle::MidRight => Handle::MidLeft,
        Handle::MidBottom => Handle::MidTop,
        Handle::MidLeft => Handle::MidRight,
    };
    handle_point(bounds, opposite)
}

fn handle_axes(handle: Handle) -> (bool, bool) {
    match handle {
        Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft => {
            (true, true)
        }
        Handle::MidTop | Handle::MidBottom => (false, true),
        Handle::MidLeft | Handle::MidRight => (true, false),
    }
}

fn scale_ratios_from_drag(
    bounds: (f32, f32, f32, f32),
    handle: Handle,
    cursor: Vec2,
    pivot: Vec2,
    ignore_pivot: bool,
    lock_aspect: bool,
) -> Option<(Vec2, f32, f32)> {
    let (min_x, min_y, max_x, max_y) = bounds;
    if (max_x - min_x).abs() <= 1.0e-3 || (max_y - min_y).abs() <= 1.0e-3 {
        return None;
    }
    let dragged = handle_point(bounds, handle);
    let anchor = if ignore_pivot {
        opposite_handle_point(bounds, handle)
    } else {
        pivot
    };
    let (affect_x, affect_y) = handle_axes(handle);
    let source = Vec2::new(dragged.x - anchor.x, dragged.y - anchor.y);
    let current = Vec2::new(cursor.x - anchor.x, cursor.y - anchor.y);

    let (mut scale_x, mut scale_y) = if lock_aspect && handle.is_corner() {
        let length_sq = source.x * source.x + source.y * source.y;
        if length_sq <= 1.0e-8 {
            return None;
        }
        let ratio = (current.x * source.x + current.y * source.y) / length_sq;
        (ratio, ratio)
    } else {
        let scale_x = if affect_x {
            if source.x.abs() <= 1.0e-6 {
                return None;
            }
            current.x / source.x
        } else {
            1.0
        };
        let scale_y = if affect_y {
            if source.y.abs() <= 1.0e-6 {
                return None;
            }
            current.y / source.y
        } else {
            1.0
        };
        (scale_x, scale_y)
    };
    scale_x = valid_scale(scale_x);
    scale_y = valid_scale(scale_y);
    Some((anchor, scale_x, scale_y))
}

fn raw_handle_scale(
    bounds: (f32, f32, f32, f32),
    handle: Handle,
    cursor: Vec2,
    pivot: Vec2,
    ignore_pivot: bool,
    lock_aspect: bool,
) -> Option<(Vec2, f32, f32)> {
    scale_ratios_from_drag(bounds, handle, cursor, pivot, ignore_pivot, lock_aspect)
}

#[cfg(test)]
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

fn raw_placement_asset_id(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
) -> Option<u16> {
    project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)?
        .placements
        .get(placement_idx)
        .and_then(|placement| match placement.target {
            Target::Asset(asset_id) => Some(asset_id),
            Target::Q0rg(_) => None,
        })
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
    let (_, rig_bound) =
        active_visual_affine_for_placement(project, q0rg_id, layer, placement_idx, frame)?;
    if rig_bound {
        // A rig binding owns the displayed matrix. Ordinary free-transform would
        // edit the hidden placement key and appear to do nothing, so the Select
        // tool deliberately withholds transform handles for bound objects.
        return None;
    }
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
    if project.layer_is_folder(q0rg_id, layer_id)
        || crate::audio::audio_clip_at_frame(project, q0rg_id, layer_id, frame).is_some()
    {
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
                        instance_id: source.instance_id,
                        frame,
                        target: source.target,
                        transform,
                        tween: Tween::None,
                        fx: Default::default(),
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

fn affine_transform_frame(local_bbox: (f32, f32, f32, f32), affine: Affine) -> TransformFrame {
    let (min_x, min_y, max_x, max_y) = local_bbox;
    transform_frame_from_corners([
        affine.apply(Vec2::new(min_x, min_y)),
        affine.apply(Vec2::new(max_x, min_y)),
        affine.apply(Vec2::new(max_x, max_y)),
        affine.apply(Vec2::new(min_x, max_y)),
    ])
}

fn placement_transform_frame(
    local_bbox: (f32, f32, f32, f32),
    transform: Transform2D,
) -> TransformFrame {
    affine_transform_frame(local_bbox, Affine::from_transform(transform))
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
#[derive(Clone, Copy)]
struct ScaleDragModifiers {
    ignore_pivot: bool,
    lock_aspect: bool,
}

fn apply_handle_drag(
    t: &mut Transform2D,
    start_t: Transform2D,
    local_bbox: (f32, f32, f32, f32),
    handle: Handle,
    cursor_world: Vec2,
    pivot_local: Option<Vec2>,
    modifiers: ScaleDragModifiers,
) -> bool {
    if !supports_axis_resize(start_t) {
        *t = start_t;
        return false;
    }
    let start_affine = Affine::from_transform(start_t);
    let Some(inverse) = start_affine.inverse() else {
        return false;
    };
    let cursor_local = inverse.apply(cursor_world);
    let default_pivot = Vec2::new(
        (local_bbox.0 + local_bbox.2) * 0.5,
        (local_bbox.1 + local_bbox.3) * 0.5,
    );
    let Some((anchor_local, ratio_x, ratio_y)) = scale_ratios_from_drag(
        local_bbox,
        handle,
        cursor_local,
        pivot_local.unwrap_or(default_pivot),
        modifiers.ignore_pivot,
        modifiers.lock_aspect,
    ) else {
        return false;
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
    pivot_local: Option<Vec2>,
) -> Option<Transform2D> {
    let start_affine = Affine::from_transform(start);
    let cursor_local = start_affine.inverse()?.apply(cursor_world);
    let (min_x, min_y, max_x, max_y) = local_bbox;
    let pivot_local =
        pivot_local.unwrap_or_else(|| Vec2::new((min_x + max_x) * 0.5, (min_y + max_y) * 0.5));
    let mut next = start;

    match edge {
        TransformEdge::Top => {
            let denom = (min_y - max_y).abs().max(1.0e-4);
            let delta_tan = (cursor_local.x - start_cursor_local.x) / -denom;
            next.skew_x = (start.skew_x.tan() + delta_tan).clamp(-8.0, 8.0).atan();
        }
        TransformEdge::Bottom => {
            let denom = (max_y - min_y).abs().max(1.0e-4);
            let delta_tan = (cursor_local.x - start_cursor_local.x) / denom;
            next.skew_x = (start.skew_x.tan() + delta_tan).clamp(-8.0, 8.0).atan();
        }
        TransformEdge::Left => {
            let denom = (min_x - max_x).abs().max(1.0e-4);
            let delta_tan = (cursor_local.y - start_cursor_local.y) / -denom;
            next.skew_y = (start.skew_y.tan() + delta_tan).clamp(-8.0, 8.0).atan();
        }
        TransformEdge::Right => {
            let denom = (max_x - min_x).abs().max(1.0e-4);
            let delta_tan = (cursor_local.y - start_cursor_local.y) / denom;
            next.skew_y = (start.skew_y.tan() + delta_tan).clamp(-8.0, 8.0).atan();
        }
    }

    let pivot_world = start_affine.apply(pivot_local);
    let after = apply_no_translate(next, pivot_local);
    next.tx = pivot_world.x - after.x;
    next.ty = pivot_world.y - after.y;
    Some(next)
}

fn valid_scale(value: f32) -> f32 {
    if !value.is_finite() {
        return 1.0;
    }
    if value.abs() < 0.01 {
        if value.is_sign_negative() {
            -0.01
        } else {
            0.01
        }
    } else {
        value
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
                instance_id: 0,
                frame,
                target: Target::Asset(asset_id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
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
pub(crate) fn selection_at_point_cached(
    project: &ProjectV2,
    textures: &mut TextureCache,
    q0rg_id: u16,
    frame: u16,
    p: Vec2,
) -> Option<Selection> {
    if let Some(hit) = hit_test_raw_selection_cached(project, textures, q0rg_id, frame, p) {
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
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_is_locked(q0rg_id, layer.layer_id)
        {
            continue;
        }
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
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_is_locked(q0rg_id, layer.layer_id)
        {
            continue;
        }
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
    // Double-click/enter-symbol hit testing must agree with ordinary object
    // selection. In particular a q0rg's empty rectangular bbox is not body.
    hit_test_selectable_placement(project, q0rg_id, frame, p)
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
fn asset_visible_body_contains_point(
    project: &ProjectV2,
    asset_id: u16,
    transform: Affine,
    point: Vec2,
) -> bool {
    let Some(inverse) = transform.inverse() else {
        return false;
    };
    let local = inverse.apply(point);
    let tolerance = 2.0 / transform.uniform_scale().max(0.05);
    let Some(asset) = project.assets.iter().find(|asset| asset.id() == asset_id) else {
        return false;
    };
    match asset {
        Asset::Vector(vector) => {
            if vector.fill.is_some()
                && interactive_visible_fill_hit(
                    vector,
                    project.asset_appearances.get(&asset_id),
                    local,
                    tolerance,
                )
            {
                return true;
            }
            let Some(stroke) = vector.stroke.as_ref() else {
                return false;
            };
            let radius = stroke.width.max(0.5) * 0.5 + tolerance;
            vector.paths.iter().any(|path| {
                let points = flatten_path(path);
                points.len() >= 2 && nearest_segment_distance(&points, local) <= radius
            })
        }
        Asset::Bitmap(bitmap) => {
            if local.x < 0.0
                || local.y < 0.0
                || local.x >= f32::from(bitmap.width)
                || local.y >= f32::from(bitmap.height)
            {
                return false;
            }
            let x = local.x.floor() as usize;
            let y = local.y.floor() as usize;
            let offset = (y * usize::from(bitmap.width) + x) * 4 + 3;
            bitmap.rgba.get(offset).copied().unwrap_or(0) > 0
        }
        // q0v frames are already bitmap-like display objects and decoding a frame
        // merely for pointer hit-testing would be far more expensive than its
        // ordinary rectangular media semantics.
        Asset::Q0v(video) => q0video::q0v::Q0vFile::parse(video.bytes.clone())
            .ok()
            .is_some_and(|media| {
                media.spec.video
                    && local.x >= 0.0
                    && local.y >= 0.0
                    && local.x <= media.spec.width as f32
                    && local.y <= media.spec.height as f32
            }),
        Asset::Rig(_) => false,
    }
}

fn active_visual_affines(
    project: &ProjectV2,
    q0rg_id: u16,
    layer: &q0s_format::v2::Layer,
    frame: u16,
) -> Vec<(usize, Affine, bool)> {
    let rig_pose = q0s_format::rig::rig_for_q0rg(project, q0rg_id)
        .map(|rig| q0s_format::rig::evaluate_rig(rig, f32::from(frame), &[]));
    crate::render::active_placements_at(layer, frame)
        .into_iter()
        .filter_map(|(index, transform)| {
            let placement = layer.placements.get(index)?;
            let bound = rig_pose
                .as_ref()
                .and_then(|pose| pose.binding_transform(placement.instance_id));
            Some((
                index,
                bound.unwrap_or_else(|| Affine::from_transform(transform)),
                bound.is_some(),
            ))
        })
        .collect()
}

fn active_visual_affine_for_placement(
    project: &ProjectV2,
    q0rg_id: u16,
    layer: &q0s_format::v2::Layer,
    placement_idx: usize,
    frame: u16,
) -> Option<(Affine, bool)> {
    active_visual_affines(project, q0rg_id, layer, frame)
        .into_iter()
        .find(|(index, _, _)| *index == placement_idx)
        .map(|(_, affine, bound)| (affine, bound))
}

fn affine_local_bbox_contains(
    local_bbox: (f32, f32, f32, f32),
    affine: Affine,
    point: Vec2,
) -> bool {
    let Some(inverse) = affine.inverse() else {
        return false;
    };
    let local = inverse.apply(point);
    local.x >= local_bbox.0
        && local.x <= local_bbox.2
        && local.y >= local_bbox.1
        && local.y <= local_bbox.3
}

fn q0rg_visible_body_contains_point(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    point: Vec2,
    depth: u8,
) -> bool {
    if depth > 8 {
        return false;
    }
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return false;
    };
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    for layer in q0rg.layers.iter().rev() {
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
        for (index, visual_affine, _) in active_visual_affines(project, q0rg_id, layer, local_frame)
            .into_iter()
            .rev()
        {
            let Some(placement) = layer.placements.get(index) else {
                continue;
            };
            let composed = Affine::compose(parent, visual_affine);
            let hit = match placement.target {
                Target::Asset(asset_id) => {
                    asset_visible_body_contains_point(project, asset_id, composed, point)
                }
                Target::Q0rg(child_id) if child_id != q0rg_id => q0rg_visible_body_contains_point(
                    project,
                    child_id,
                    local_frame,
                    composed,
                    point,
                    depth + 1,
                ),
                Target::Q0rg(_) => false,
            };
            if hit {
                return true;
            }
        }
    }
    false
}

fn selectable_placement_body_contains_point(
    project: &ProjectV2,
    placement: &Placement,
    visual_affine: Affine,
    frame: u16,
    point: Vec2,
) -> bool {
    match placement.target {
        Target::Q0rg(child_id) => {
            q0rg_visible_body_contains_point(project, child_id, frame, visual_affine, point, 1)
        }
        // Display-object vectors/media keep oriented local-bounds semantics, but
        // the bounds follow the resolved rig affine rather than the stale placement key.
        Target::Asset(_) => placement_local_bbox(project, placement)
            .is_some_and(|bbox| affine_local_bbox_contains(bbox, visual_affine, point)),
    }
}
fn hit_test_selectable_placement(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    p: Vec2,
) -> Option<(u16, usize)> {
    let q = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id)?;
    for layer in q.layers.iter().rev() {
        if !project.layer_is_visible(q0rg_id, layer.layer_id)
            || project.layer_is_locked(q0rg_id, layer.layer_id)
        {
            continue;
        }
        for (idx, visual_affine, _) in active_visual_affines(project, q0rg_id, layer, frame)
            .into_iter()
            .rev()
        {
            let Some(placement) = layer.placements.get(idx) else {
                continue;
            };
            if is_raw_graphics_placement(project, placement) {
                continue;
            }
            if selectable_placement_body_contains_point(project, placement, visual_affine, frame, p)
            {
                return Some((layer.layer_id, idx));
            }
        }
    }
    None
}

fn classic_prerender_nib_outline(
    nib: crate::brush::BrushNib,
    center: Pos2,
    size_px: f32,
) -> Vec<Pos2> {
    if nib == crate::brush::BrushNib::Circle {
        const SEGMENTS: usize = 24;
        let radius = size_px.max(0.1) * 0.5;
        return (0..SEGMENTS)
            .map(|index| {
                let phase = std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
                center + egui::vec2(phase.cos() * radius, phase.sin() * radius)
            })
            .collect();
    }
    crate::brush::nib_outline(nib, size_px.max(0.1), Vec2::new(center.x, center.y))
        .into_iter()
        .map(|point| pos2(point.x, point.y))
        .collect()
}

fn classic_prerender_segment_contour(
    nib: crate::brush::BrushNib,
    start: (Pos2, f32),
    end: (Pos2, f32),
) -> Vec<Pos2> {
    let points = classic_prerender_nib_outline(nib, start.0, start.1)
        .into_iter()
        .chain(classic_prerender_nib_outline(nib, end.0, end.1))
        .map(|point| Point::new(f64::from(point.x), f64::from(point.y)))
        .collect::<Vec<_>>();
    let hull = MultiPoint::new(points).convex_hull();
    let mut contour: Vec<Pos2> = hull
        .exterior()
        .0
        .iter()
        .map(|coord| pos2(coord.x as f32, coord.y as f32))
        .collect();
    if contour.len() > 1 && contour.first() == contour.last() {
        contour.pop();
    }
    contour
}

fn classic_prerender_contours(
    dabs: &[(Vec2, f32)],
    nib: crate::brush::BrushNib,
    view: &StageView,
    expansion_px: f32,
) -> Vec<Vec<Pos2>> {
    let screen_dabs: Vec<(Pos2, f32)> = dabs
        .iter()
        .map(|(position, size)| {
            (
                stage_to_screen(*position, view),
                size.max(0.1) * view.scale + expansion_px * 2.0,
            )
        })
        .collect();
    match screen_dabs.as_slice() {
        [] => Vec::new(),
        [(center, size)] => vec![classic_prerender_nib_outline(nib, *center, *size)],
        many => many
            .windows(2)
            .map(|pair| classic_prerender_segment_contour(nib, pair[0], pair[1]))
            .filter(|contour| contour.len() >= 3)
            .collect(),
    }
}

fn paint_classic_nib_preview(
    painter: &Painter,
    stroke: &crate::brush::BrushStroke,
    view: &StageView,
    fill: Color32,
    outline: Option<Color32>,
) {
    let dabs = crate::brush::brush_prerender_dabs(stroke);
    let nib = crate::brush::brush_preview_nib(stroke);

    // Keep the proven cheap circular pre-render for the common static brush.
    // It is a transient centerline buffer, not committed raw vector geometry.
    if nib == crate::brush::BrushNib::Circle && !crate::brush::brush_size_dynamics_enabled(stroke) {
        let points: Vec<Pos2> = dabs
            .iter()
            .map(|(point, _)| stage_to_screen(*point, view))
            .collect();
        let width = crate::brush::brush_preview_size(stroke) * view.scale;
        if let Some(outline) = outline {
            paint_round_stroke_preview(painter, &points, width + 2.0, outline);
        }
        paint_round_stroke_preview(painter, &points, width, fill);
        return;
    }

    // Dynamic circles and fixed polygon nibs use segment-local temporary
    // footprints. No boolean union, contour reconstruction, or smoothing runs
    // while the pointer is down; overlapping contours are tessellated as one
    // preview surface so translucent paint is still blended only once.
    if let Some(outline) = outline {
        let contours = classic_prerender_contours(&dabs, nib, view, 1.0);
        paint_complex_fill(painter, &contours, outline);
    }
    let contours = classic_prerender_contours(&dabs, nib, view, 0.0);
    paint_complex_fill(painter, &contours, fill);
}
fn append_advanced_preview_surface(
    mesh: &mut Mesh,
    dabs: &[crate::advanced_brush::AdvancedDab],
    view: &StageView,
    color: Color32,
    expansion: f32,
) {
    const SEGMENTS: usize = 18;
    if dabs.is_empty() {
        return;
    }

    mesh.vertices
        .reserve(dabs.len().saturating_mul(SEGMENTS + 1));
    mesh.indices
        .reserve(dabs.len().saturating_mul(SEGMENTS * 6 + 6));

    let mut previous_ring: Option<[u32; SEGMENTS]> = None;
    let mut first_ring: Option<[u32; SEGMENTS]> = None;
    let mut last_ring = [0_u32; SEGMENTS];

    for dab in dabs {
        let center = stage_to_screen(dab.center, view);
        let major = (dab.major_radius + expansion).max(0.05) * view.scale;
        let minor = (dab.minor_radius + expansion).max(0.05) * view.scale;
        let cos_a = dab.angle_radians.cos();
        let sin_a = dab.angle_radians.sin();
        let ring: [u32; SEGMENTS] = std::array::from_fn(|index| {
            let phase = std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
            let x = phase.cos() * major;
            let y = phase.sin() * minor;
            let point = pos2(
                center.x + x * cos_a - y * sin_a,
                center.y + x * sin_a + y * cos_a,
            );
            let vertex = mesh.vertices.len() as u32;
            mesh.colored_vertex(point, color);
            vertex
        });

        if first_ring.is_none() {
            first_ring = Some(ring);
        }
        if let Some(previous) = previous_ring {
            for index in 0..SEGMENTS {
                let next = (index + 1) % SEGMENTS;
                mesh.add_triangle(previous[index], previous[next], ring[next]);
                mesh.add_triangle(previous[index], ring[next], ring[index]);
            }
        }
        previous_ring = Some(ring);
        last_ring = ring;
    }

    // Only the two end caps need triangle fans. The ring strips already fill
    // the sweep between intermediate nibs, so adding a centre + fan for every
    // pointer sample just bloats the live mesh without changing the silhouette.
    let mut cap = |ring: [u32; SEGMENTS], center: Pos2| {
        let center_index = mesh.vertices.len() as u32;
        mesh.colored_vertex(center, color);
        for index in 0..SEGMENTS {
            mesh.add_triangle(center_index, ring[index], ring[(index + 1) % SEGMENTS]);
        }
    };
    let first_center = stage_to_screen(dabs[0].center, view);
    cap(
        first_ring.expect("non-empty advanced preview ring"),
        first_center,
    );
    if dabs.len() > 1 {
        let last_center = stage_to_screen(dabs[dabs.len() - 1].center, view);
        cap(last_ring, last_center);
    }
}

fn advanced_preview_mesh(
    dabs: &[crate::advanced_brush::AdvancedDab],
    view: &StageView,
    settings: crate::advanced_brush::AdvancedBrushSettings,
) -> Mesh {
    let mut mesh = Mesh::default();
    if dabs.is_empty() {
        return mesh;
    }

    if settings.glow {
        // Keep the same layered halo look, but append every layer into one
        // epaint mesh. This is one shape / GPU submission instead of rebuilding
        // and submitting a separate full stroke mesh for every glow layer.
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
            append_advanced_preview_surface(&mut mesh, dabs, view, glow, settings.glow_radius * t);
        }
    }

    let body = Color32::from_rgba_unmultiplied(
        settings.color.r,
        settings.color.g,
        settings.color.b,
        settings.color.a,
    );
    append_advanced_preview_surface(&mut mesh, dabs, view, body, 0.0);
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
    painter.add(Shape::Mesh(advanced_preview_mesh(&dabs, view, settings)));
}

fn expire_brush_preview_handoff(app: &mut EditorApp) {
    if std::mem::take(&mut app.session.classic_brush_preview_handoff) {
        app.textures.clear_classic_brush_preview();
    }
    app.session.advanced_brush_preview_handoff = None;
}

fn draw_in_progress_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    let red = Color32::from_rgb(0xCC, 0x33, 0x33);
    let cursor = painter.ctx().pointer_hover_pos();

    // Release-frame handoff: the committed project geometry was created after
    // this frame's stage render, so keep drawing the exact draft until the next
    // frame swaps it for the real vector.
    if app.session.classic_brush_preview_handoff {
        app.textures.paint_classic_brush_preview(painter);
    }
    if let Some(stroke) = app.session.advanced_brush_preview_handoff.as_ref() {
        paint_advanced_gpu_preview(painter, stroke, view);
    }

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
        ToolState::BrushDrawing { .. } => {
            app.textures.paint_classic_brush_preview(painter);
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
pub fn draw_selection_overlay(app: &mut EditorApp, painter: &Painter, view: &StageView) {
    draw_selection_content_overlay(app, painter, view);
    draw_group_transform_frame(app, painter, view);
    draw_transform_pivot_overlay(app, painter, view);
}

fn draw_selection_content_overlay(app: &mut EditorApp, painter: &Painter, view: &StageView) {
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
        placements,
        objects,
        bounds_min,
        bounds_max,
    } = app.session.selection.clone()
    {
        draw_raw_area_selection(
            app,
            painter,
            view,
            &placements,
            bounds_min,
            bounds_max,
            objects.is_empty(),
        );
        for reference in &objects {
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
    if let Selection::Mixed { paths, objects } = app.session.selection.clone() {
        draw_raw_paths_overlay(app, painter, view, &paths, false);
        draw_object_reference_outlines(app, painter, view, &objects);
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
    if let Selection::Paths(refs) = app.session.selection.clone() {
        draw_raw_paths_overlay(app, painter, view, &refs, true);
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
        let (visual_affine, rig_bound) = active_visual_affine_for_placement(
            &app.state.project,
            q0rg_id,
            layer,
            placement_idx,
            app.session.current_frame,
        )
        .unwrap_or((Affine::from_transform(active_transform), false));
        let mut visual = p.clone();
        visual.transform = active_transform;
        let Some(local_bbox) = placement_local_bbox(&app.state.project, &visual) else {
            // Empty q0rg (no children with bbox): show a tiny marker at the
            // resolved placement origin so rigged objects never leave their outline behind.
            let center = stage_to_screen(Vec2::new(visual_affine.tx, visual_affine.ty), view);
            painter.circle_stroke(center, 8.0, Stroke::new(2.0_f32, Color32::BLACK));
            painter.circle_stroke(center, 8.0, Stroke::new(1.0_f32, accent));
            return;
        };
        let frame = affine_transform_frame(local_bbox, visual_affine);
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

        if !rig_bound && supports_axis_resize(active_transform) {
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
const SELECTION_DOT_TEXTURE_SIDE: usize = 16;

/// A supersampled, filtered dot tile. The old 4x4 single-texel NEAREST texture
/// was effectively a pixel grid, so fractional DPI/zoom produced moire bands.
/// This tile is still one tiny repeating GPU texture, but each dot has a soft
/// circular footprint and linear filtering.
fn selection_stipple_texture(painter: &Painter, accent: Color32) -> TextureHandle {
    let id = egui::Id::new((
        "q0editor.selection-stipple-texture.v2",
        accent.r(),
        accent.g(),
        accent.b(),
        accent.a(),
    ));
    if let Some(handle) = painter
        .ctx()
        .data(|data| data.get_temp::<TextureHandle>(id))
    {
        return handle;
    }

    let side = SELECTION_DOT_TEXTURE_SIDE;
    let center = (side as f32 - 1.0) * 0.5;
    let radius = side as f32 * 0.16;
    let feather = 1.25_f32;
    let mut rgba = vec![0_u8; side * side * 4];
    for y in 0..side {
        for x in 0..side {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = ((radius + feather - distance) / feather).clamp(0.0, 1.0);
            let base_alpha = 24.0_f32;
            let alpha =
                (base_alpha + coverage * (f32::from(accent.a()) - base_alpha)).round() as u8;
            let offset = (y * side + x) * 4;
            rgba[offset..offset + 4].copy_from_slice(&[accent.r(), accent.g(), accent.b(), alpha]);
        }
    }
    let image = ColorImage::from_rgba_unmultiplied([side, side], &rgba);
    let handle = painter.ctx().load_texture(
        "q0editor-selection-stipple-v2",
        image,
        TextureOptions::LINEAR_REPEAT,
    );
    painter
        .ctx()
        .data_mut(|data| data.insert_temp(id, handle.clone()));
    handle
}

fn paint_selection_stipple_pattern(painter: &Painter, contours: &[Vec<Pos2>], accent: Color32) {
    if contours.is_empty() {
        return;
    }
    let texture = selection_stipple_texture(painter, accent);
    crate::render::paint_complex_fill_pattern(
        painter,
        contours,
        texture.id(),
        SELECTION_STIPPLE_SPACING_PX,
    );
}

/// Selection outline must preserve the exact topology of the fill contour.
/// Display-space simplification is unsafe here: at low zoom two opposite sides
/// of a thin brush shape can become subpixel-near, and a "hairpin cleanup" may
/// then remove the turn and connect distant contour vertices with a long chord.
/// Only exact duplicates/non-finite points are removed; no geometric tolerance
/// is allowed to change adjacency.
fn selection_contour_points(contour: &[Pos2]) -> Vec<Pos2> {
    let mut points = Vec::with_capacity(contour.len());
    for point in contour
        .iter()
        .copied()
        .filter(|point| point.x.is_finite() && point.y.is_finite())
    {
        if points.last().is_some_and(|previous| *previous == point) {
            continue;
        }
        points.push(point);
    }
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points
}

/// Selection outline uses the exact same transformed contours as the fill
/// pattern. It is a normal path stroke, not one quad per screen pixel, so its
/// cost depends on vector complexity and it cannot drift independently of the
/// selected geometry.
fn draw_selection_contour(painter: &Painter, contours: &[Vec<Pos2>], accent: Color32) {
    for contour in contours {
        let outline = selection_contour_points(contour);
        if outline.len() < 3 {
            continue;
        }
        crate::render::paint_closed_bevel_stroke(
            painter,
            &outline,
            2.0,
            Color32::from_black_alpha(180),
        );
        crate::render::paint_closed_bevel_stroke(painter, &outline, 1.0, accent);
    }
}

fn paint_selection_surface(painter: &Painter, contours: &[Vec<Pos2>], accent: Color32) {
    if contours.is_empty() {
        return;
    }
    paint_selection_stipple_pattern(painter, contours, accent);
    draw_selection_contour(painter, contours, accent);
}

#[cfg(all(test, feature = "appearance-mask-eraser"))]
fn appearance_selection_body_contours(
    vector: &VectorAsset,
    appearance: &q0s_format::v2::VectorAppearance,
    subset_path_indices: Option<&[usize]>,
    view: &StageView,
    textures: &mut TextureCache,
) -> Vec<Vec<Pos2>> {
    let selected_indices: Vec<usize> = match subset_path_indices {
        Some(indices) => indices
            .iter()
            .copied()
            .filter(|index| vector.paths.get(*index).is_some_and(|path| path.closed))
            .collect(),
        None => vector
            .paths
            .iter()
            .enumerate()
            .filter_map(|(index, path)| path.closed.then_some(index))
            .collect(),
    };
    if selected_indices.is_empty() {
        return Vec::new();
    }

    // Unmasked material is already cheap and preserves the original bezier body
    // exactly. Only masked/post-material fragments need the heavier boolean body.
    if appearance.erase_mask.is_empty() && appearance.clip_mask.is_empty() {
        return selected_indices
            .iter()
            .filter_map(|index| vector.paths.get(*index))
            .map(|path| {
                flatten_path(path)
                    .into_iter()
                    .map(|point| stage_to_screen(point, view))
                    .collect::<Vec<_>>()
            })
            .filter(|contour| contour.len() >= 3)
            .collect();
    }

    // Masked raw selections used to rebuild geo intersections/differences on
    // every repaint. Cache the result in canonical appearance-field space, then
    // apply only the affine field transform + view transform each frame.
    let Some(cached) = textures.selection_geometry(vector, appearance, &selected_indices) else {
        return Vec::new();
    };
    let canonical = if cached.body.is_empty() {
        &cached.fallback_material_support
    } else {
        &cached.body
    };
    canonical
        .iter()
        .map(|contour| {
            contour
                .iter()
                .map(|point| stage_to_screen(appearance.field_transform.apply(*point), view))
                .collect()
        })
        .collect()
}

#[cfg(feature = "appearance-mask-eraser")]
struct AppearanceSelectionOverlay<'a> {
    view: &'a StageView,
    rect: egui::Rect,
    vector: &'a VectorAsset,
    appearance: &'a q0s_format::v2::VectorAppearance,
    placement_transform: Affine,
    subset_path_indices: Option<&'a [usize]>,
    accent: Color32,
}

#[cfg(feature = "appearance-mask-eraser")]
fn draw_appearance_selection_overlay(
    painter: &Painter,
    textures: &mut TextureCache,
    request: AppearanceSelectionOverlay<'_>,
) {
    let AppearanceSelectionOverlay {
        view,
        rect,
        vector,
        appearance,
        placement_transform,
        subset_path_indices,
        accent,
    } = request;
    let visible_rect = rect.intersect(painter.clip_rect());
    if !visible_rect.is_positive() {
        return;
    }
    let clipped = painter.with_clip_rect(visible_rect);
    let mut path_indices: Vec<usize> = match subset_path_indices {
        Some(indices) => indices
            .iter()
            .copied()
            .filter(|index| vector.paths.get(*index).is_some_and(|path| path.closed))
            .collect(),
        None => vector
            .paths
            .iter()
            .enumerate()
            .filter_map(|(index, path)| path.closed.then_some(index))
            .collect(),
    };
    path_indices.sort_unstable();
    path_indices.dedup();
    if path_indices.is_empty() {
        return;
    }

    let texture = selection_stipple_texture(&clipped, accent);
    let field = appearance.field_transform;
    let pure_translation = (placement_transform.a11 - 1.0).abs() <= 1.0e-6
        && placement_transform.a12.abs() <= 1.0e-6
        && placement_transform.a21.abs() <= 1.0e-6
        && (placement_transform.a22 - 1.0).abs() <= 1.0e-6;
    // Translation is by far the hottest Select path. Keep one screen-space base
    // paint for the current view and move that mesh cheaply instead of asking
    // lyon to rebuild 1px/2px bevel geometry on every pointer event.
    let cache_transform = if pure_translation {
        Affine::IDENTITY
    } else {
        placement_transform
    };
    let key = SelectionPaintKey {
        asset_id: vector.asset_id,
        path_indices: path_indices.clone(),
        field_transform_bits: [
            field.a11.to_bits(),
            field.a12.to_bits(),
            field.a21.to_bits(),
            field.a22.to_bits(),
            field.tx.to_bits(),
            field.ty.to_bits(),
        ],
        placement_transform_bits: [
            cache_transform.a11.to_bits(),
            cache_transform.a12.to_bits(),
            cache_transform.a21.to_bits(),
            cache_transform.a22.to_bits(),
            cache_transform.tx.to_bits(),
            cache_transform.ty.to_bits(),
        ],
        view_origin_bits: [view.origin.x.to_bits(), view.origin.y.to_bits()],
        view_scale_bits: view.scale.to_bits(),
        accent_rgba: [accent.r(), accent.g(), accent.b(), accent.a()],
        texture_id: texture.id(),
    };

    if textures.selection_paint(&key).is_none() {
        let meshes = {
            let Some(source) = textures.selection_stage_mesh(vector, appearance, &path_indices)
            else {
                return;
            };
            if pure_translation {
                TextureCache::selection_screen_meshes_direct(
                    source,
                    Affine::IDENTITY,
                    view,
                    texture.id(),
                    SELECTION_STIPPLE_SPACING_PX,
                    accent,
                )
            } else {
                TextureCache::selection_screen_meshes(
                    source,
                    placement_transform,
                    view,
                    texture.id(),
                    SELECTION_STIPPLE_SPACING_PX,
                    accent,
                )
            }
        };
        textures.insert_selection_paint(key.clone(), CachedSelectionPaint { meshes });
    }

    let Some(cached) = textures.selection_paint(&key) else {
        return;
    };
    if pure_translation
        && (placement_transform.tx.abs() > 1.0e-6 || placement_transform.ty.abs() > 1.0e-6)
    {
        let delta = egui::vec2(
            placement_transform.tx * view.scale,
            placement_transform.ty * view.scale,
        );
        let tile = SELECTION_STIPPLE_SPACING_PX.max(1.0);
        for source in &cached.meshes {
            let mut mesh = source.clone();
            let stipple = mesh.texture_id == texture.id();
            for vertex in &mut mesh.vertices {
                vertex.pos += delta;
                if stipple {
                    vertex.uv = pos2(vertex.pos.x / tile, vertex.pos.y / tile);
                }
            }
            clipped.add(Shape::Mesh(mesh));
        }
    } else {
        for mesh in &cached.meshes {
            clipped.add(Shape::Mesh(mesh.clone()));
        }
    }
}

fn draw_raw_area_selection(
    app: &mut EditorApp,
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
    let accent = selection_color(app);
    #[cfg(not(feature = "appearance-mask-eraser"))]
    let mut surfaces: Vec<MultiPolygon<f64>> = Vec::new();
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
        {
            if let Some(appearance) = app.state.project.asset_appearances.get(&asset_id) {
                draw_appearance_selection_overlay(
                    &clipped,
                    &mut app.textures,
                    AppearanceSelectionOverlay {
                        view,
                        rect: selection_rect,
                        vector,
                        appearance,
                        placement_transform: Affine::from_transform(placement.transform),
                        subset_path_indices: None,
                        accent,
                    },
                );
                continue;
            }
            let plain_appearance = q0s_format::v2::VectorAppearance {
                material: q0s_format::v2::VectorMaterial::Solid,
                erase_mask: Vec::new(),
                material_source: Vec::new(),
                clip_mask: Vec::new(),
                field_transform: Affine::IDENTITY,
            };
            draw_appearance_selection_overlay(
                &clipped,
                &mut app.textures,
                AppearanceSelectionOverlay {
                    view,
                    rect: selection_rect,
                    vector,
                    appearance: &plain_appearance,
                    placement_transform: Affine::from_transform(placement.transform),
                    subset_path_indices: None,
                    accent,
                },
            );
            continue;
        }
        #[cfg(not(feature = "appearance-mask-eraser"))]
        {
            let surface = raw_selectable_fill_surface(&app.state.project, asset_id, vector);
            surfaces.push(surface);
        }
    }

    // Animate-style stipple is evaluated against one unioned surface, so holes
    // remain empty. Density is fixed in screen space at every zoom; only the
    // visible viewport is sampled so off-screen geometry does not create work.
    #[cfg(not(feature = "appearance-mask-eraser"))]
    let surface = match surfaces.len() {
        0 => MultiPolygon(Vec::new()),
        1 => surfaces.pop().expect("single selected surface"),
        _ => geo::unary_union(surfaces.iter()),
    };
    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        let selection_contours = surface_to_screen_contours(&surface, view);
        paint_selection_surface(&clipped, &selection_contours, accent);
    }
    if draw_box {
        draw_flash_selection_box(painter, selection_rect, accent);
    }
}

fn draw_raw_paths_overlay(
    app: &mut EditorApp,
    painter: &Painter,
    view: &StageView,
    refs: &[PathRef],
    draw_boxes: bool,
) {
    // Path refs may span multiple internal carrier placements after appearance
    // splitting. That is an implementation detail: one logical selection gets
    // exactly one transform frame. Per-carrier boxes make a single glow look
    // like two independent selections.
    let accent = selection_color(app);
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
            let placement_transform = Affine::from_transform(placement.transform);
            let raw_points: Vec<Pos2> = flatten_path(path)
                .into_iter()
                .map(|point| stage_to_screen(placement_transform.apply(point), view))
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
            if let Some(bounds) = fast_bounds.or_else(fallback_bounds) {
                let placement_transform = Affine::from_transform(placement.transform);
                let Some((min_x, min_y, max_x, max_y)) =
                    transform_axis_aligned_bounds(bounds, placement_transform)
                else {
                    continue;
                };
                let rect = egui::Rect::from_min_max(
                    stage_to_screen(Vec2::new(min_x, min_y), view),
                    stage_to_screen(Vec2::new(max_x, max_y), view),
                );
                draw_appearance_selection_overlay(
                    painter,
                    &mut app.textures,
                    AppearanceSelectionOverlay {
                        view,
                        rect,
                        vector,
                        appearance,
                        placement_transform: Affine::from_transform(placement.transform),
                        subset_path_indices: Some(&closed_indices),
                        accent,
                    },
                );
            }
            continue;
        }

        #[cfg(feature = "appearance-mask-eraser")]
        {
            // Plain raw fills use exactly the same cached stage mesh as Advanced
            // selection. The synthetic solid appearance is render-only metadata:
            // it keeps the path topology untouched while avoiding a fresh
            // MultiPolygon -> VPath -> flatten -> tessellate cycle every repaint.
            let plain_appearance = q0s_format::v2::VectorAppearance {
                material: q0s_format::v2::VectorMaterial::Solid,
                erase_mask: Vec::new(),
                material_source: Vec::new(),
                clip_mask: Vec::new(),
                field_transform: Affine::IDENTITY,
            };
            draw_appearance_selection_overlay(
                painter,
                &mut app.textures,
                AppearanceSelectionOverlay {
                    view,
                    rect: painter.clip_rect(),
                    vector,
                    appearance: &plain_appearance,
                    placement_transform: Affine::from_transform(placement.transform),
                    subset_path_indices: Some(&closed_indices),
                    accent,
                },
            );
            continue;
        }
        #[cfg(not(feature = "appearance-mask-eraser"))]
        {
            let surface =
                raw_selectable_paths_surface(&app.state.project, asset_id, vector, &closed_indices);
            let contours = surface_to_screen_contours(&surface, view);
            if contours.is_empty() {
                continue;
            }
            paint_selection_surface(painter, &contours, accent);
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

fn draw_whole_raw_fill_overlay(
    app: &mut EditorApp,
    painter: &Painter,
    view: &StageView,
    r: PathRef,
) {
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
    if path.closed && screen.len() >= 3 {
        paint_selection_surface(
            &clipped,
            std::slice::from_ref(&screen),
            selection_color(app),
        );
    } else {
        crate::render::paint_concave_fill(&clipped, &screen, selection_fill_color(app, 40));
    }
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
        if !project.layer_is_visible(q0rg_id, layer.layer_id) {
            continue;
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct BucketTarget {
    layer_id: u16,
    placement_idx: usize,
    asset_id: u16,
    path_indices: Vec<usize>,
}

#[derive(Debug, Clone, Copy)]
struct BucketBoundarySegment {
    a: Vec2,
    b: Vec2,
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

fn bucket_visible_fill_geometry(vector: &VectorAsset) -> MultiPolygon<f64> {
    if vector.fill.is_none() {
        return MultiPolygon(Vec::new());
    }

    // Empty-space bucket faces are built from `flatten_path`, which is also the
    // contour the stage renderer tessellates. Use that exact visible boundary
    // when subtracting existing paint. Mixing it with the brush engine's
    // anchor-only canonical merge geometry leaves angular bites wherever smooth
    // Bezier handles bow away from their anchor polygon.
    let paths: Vec<VPath> = vector
        .paths
        .iter()
        .filter(|path| path.closed)
        .filter_map(|path| {
            let mut points = flatten_path(path);
            if points.len() > 1
                && points
                    .first()
                    .zip(points.last())
                    .is_some_and(|(first, last)| bucket_distance_sq(*first, *last) <= 1.0e-8)
            {
                points.pop();
            }
            (points.len() >= 3).then(|| VPath {
                anchors: points.into_iter().map(anchor).collect(),
                closed: true,
            })
        })
        .collect();

    crate::brush::vector_fill_geometry(&VectorAsset {
        asset_id: vector.asset_id,
        paths,
        fill: vector.fill,
        stroke: None,
    })
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
            vector.fill.map(|_| bucket_visible_fill_geometry(vector))
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
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    let color = app.session.fill_color.unwrap_or(Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    });

    if let Some(target) = find_bucket_target(&app.state.project, q0rg_id, layer_id, frame, point) {
        return recolour_bucket_target(app, target, color);
    }

    let Some(region) =
        find_empty_bucket_region(&app.state.project, q0rg_id, layer_id, frame, point)
    else {
        return false;
    };
    fill_empty_bucket_region(app, layer_id, region, color)
}

fn recolour_bucket_target(app: &mut EditorApp, target: BucketTarget, color: Rgba) -> bool {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;

    app.history.snapshot(&app.state.project);
    let Some(mapping) = materialize_layer_keyframe_for_edit(
        &mut app.state.project,
        q0rg_id,
        target.layer_id,
        frame,
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
        frame,
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
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => None,
        })
    else {
        return false;
    };
    if source.fill.is_none() {
        return false;
    }

    let mut indices = target.path_indices;
    indices.sort_unstable();
    indices.dedup();
    if indices.is_empty() || indices.iter().any(|index| *index >= source.paths.len()) {
        return false;
    }
    let selected_indices: std::collections::BTreeSet<usize> = indices.iter().copied().collect();
    let selected_paths: Vec<VPath> = indices
        .iter()
        .map(|index| source.paths[*index].clone())
        .collect();
    let remaining_paths: Vec<VPath> = source
        .paths
        .iter()
        .enumerate()
        .filter(|(index, _)| !selected_indices.contains(index))
        .map(|(_, path)| path.clone())
        .collect();
    let selected_surface = vector_fill_geometry(&VectorAsset {
        asset_id: 0,
        paths: selected_paths.clone(),
        fill: source.fill,
        stroke: None,
    });
    if selected_surface.0.is_empty() {
        return false;
    }

    if remaining_paths.is_empty() {
        if let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == asset_id)
        {
            // Recolouring must be a style-only edit. Keep the exact source paths,
            // including every cubic handle and the original outline.
            vector.fill = Some(color);
        }
    } else {
        if let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == asset_id)
        {
            vector.paths = remaining_paths;
            vector.fill = source.fill;
            vector.stroke = source.stroke;
        }

        let new_asset_id = next_asset_id(&app.state.project);
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: new_asset_id,
            paths: selected_paths,
            fill: Some(color),
            // A split component owns its own copy of the shared outline. The
            // paths are partitioned, so this does not double-draw any stroke.
            stroke: source.stroke,
        }));
        crate::appearance::split_asset_appearance(
            &mut app.state.project,
            asset_id,
            new_asset_id,
            &source.paths,
            &selected_surface,
            false,
        );

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
            layer.placements.insert(
                (placement_idx + 1).min(layer.placements.len()),
                Placement {
                    instance_id: 0,
                    frame,
                    target: Target::Asset(new_asset_id),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
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

fn fill_empty_bucket_region(
    app: &mut EditorApp,
    layer_id: u16,
    region: MultiPolygon<f64>,
    color: Rgba,
) -> bool {
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    // Empty-space paint is inserted under the raw drawing, but it must not
    // cover existing fill islands living inside the newly enclosed face.
    let occupied = raw_fill_surface_on_layer(&app.state.project, q0rg_id, layer_id, frame);
    let bucket_surface = region.difference(&occupied);
    if bucket_surface.unsigned_area() <= 0.05 {
        return false;
    }
    let paths = geo_multi_polygon_to_linear_paths(&bucket_surface);
    if paths.is_empty() {
        return false;
    }

    app.history.snapshot(&app.state.project);
    if materialize_layer_keyframe_for_edit(&mut app.state.project, q0rg_id, layer_id, frame)
        .is_none()
    {
        return false;
    }

    let new_asset_id = next_asset_id(&app.state.project);
    app.state.project.assets.push(Asset::Vector(VectorAsset {
        asset_id: new_asset_id,
        paths,
        fill: Some(color),
        stroke: None,
    }));

    let insertion = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
        .map(|layer| {
            active_raw_placement_indices(&app.state.project, layer, frame)
                .into_iter()
                .min()
                .unwrap_or(layer.placements.len())
        })
        .unwrap_or(0);
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
        layer.placements.insert(
            insertion.min(layer.placements.len()),
            Placement {
                instance_id: 0,
                frame,
                target: Target::Asset(new_asset_id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
        );
    } else {
        return false;
    }

    app.state.dirty = true;
    app.session.selection = Selection::None;
    app.session.status = "Region filled".to_string();
    app.textures.invalidate();
    true
}

fn find_empty_bucket_region(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    cursor: Vec2,
) -> Option<MultiPolygon<f64>> {
    if !project.layer_is_visible(q0rg_id, layer_id) || project.layer_is_locked(q0rg_id, layer_id) {
        return None;
    }
    let segments = bucket_boundary_segments(project, q0rg_id, layer_id, frame);
    let faces = bucket_planar_faces(&segments);
    if faces.is_empty() {
        return None;
    }
    let point = Point::new(cursor.x as f64, cursor.y as f64);
    let selected = faces
        .iter()
        .filter(|face| face.contains(&point) || polygon_boundary_near_cursor(face, cursor, 0.5))
        .min_by(|left, right| left.unsigned_area().total_cmp(&right.unsigned_area()))?
        .clone();

    let selected_area = selected.unsigned_area();
    let mut region = MultiPolygon(vec![selected.clone()]);
    // Disconnected inner loops are separate graph components. Subtract them
    // from the chosen enclosing face so nested stroke loops produce a ring,
    // not one giant fill that steamrolls everything inside.
    for face in &faces {
        let area = face.unsigned_area();
        if area >= selected_area - 1e-6 || face.contains(&point) {
            continue;
        }
        let Some(coord) = face.exterior().0.first() else {
            continue;
        };
        if selected.contains(&Point::new(coord.x, coord.y)) {
            region = region.difference(&MultiPolygon(vec![face.clone()]));
        }
    }
    (region.unsigned_area() > 0.05).then_some(region)
}

fn bucket_boundary_segments(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Vec<BucketBoundarySegment> {
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return Vec::new();
    };
    let mut segments = Vec::new();
    for placement_idx in active_raw_placement_indices(project, layer, frame) {
        let placement = &layer.placements[placement_idx];
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(Asset::Vector(vector)) =
            project.assets.iter().find(|asset| asset.id() == asset_id)
        else {
            continue;
        };
        for path in &vector.paths {
            let visible_boundary =
                vector.stroke.is_some() || (path.closed && vector.fill.is_some());
            if !visible_boundary {
                continue;
            }
            let points = flatten_path(path);
            for pair in points.windows(2) {
                if bucket_distance_sq(pair[0], pair[1]) > 1e-8 {
                    segments.push(BucketBoundarySegment {
                        a: pair[0],
                        b: pair[1],
                    });
                }
            }
        }
    }
    segments
}

fn bucket_planar_faces(segments: &[BucketBoundarySegment]) -> Vec<Polygon<f64>> {
    let segments = bucket_split_segments_at_intersections(segments);
    if segments.len() < 3 {
        return Vec::new();
    }

    let mut nodes = Vec::<Vec2>::new();
    let mut node_map = std::collections::HashMap::<(i64, i64), usize>::new();
    let mut edges = std::collections::BTreeSet::<(usize, usize)>::new();
    for segment in segments {
        let a = bucket_graph_node(&mut nodes, &mut node_map, segment.a);
        let b = bucket_graph_node(&mut nodes, &mut node_map, segment.b);
        if a == b {
            continue;
        }
        edges.insert(if a < b { (a, b) } else { (b, a) });
    }
    if edges.len() < 3 {
        return Vec::new();
    }

    let mut adjacency = vec![Vec::<usize>::new(); nodes.len()];
    for &(a, b) in &edges {
        adjacency[a].push(b);
        adjacency[b].push(a);
    }
    for (node, neighbours) in adjacency.iter_mut().enumerate() {
        neighbours.sort_unstable();
        neighbours.dedup();
        neighbours.sort_by(|left, right| {
            let l = nodes[*left];
            let r = nodes[*right];
            let origin = nodes[node];
            let la = (l.y - origin.y).atan2(l.x - origin.x);
            let ra = (r.y - origin.y).atan2(r.x - origin.x);
            la.total_cmp(&ra)
        });
    }

    let mut visited = std::collections::BTreeSet::<(usize, usize)>::new();
    let mut faces = Vec::new();
    for &(a, b) in &edges {
        for (start_from, start_to) in [(a, b), (b, a)] {
            if visited.contains(&(start_from, start_to)) {
                continue;
            }
            let start = (start_from, start_to);
            let mut from = start_from;
            let mut to = start_to;
            let mut ring = vec![from];
            let mut closed = false;
            let max_steps = edges.len().saturating_mul(2).saturating_add(4);
            for _ in 0..max_steps {
                if !visited.insert((from, to)) {
                    break;
                }
                ring.push(to);
                let neighbours = &adjacency[to];
                let Some(reverse_index) =
                    neighbours.iter().position(|candidate| *candidate == from)
                else {
                    break;
                };
                // Follow the face on the left side of the directed edge: at the
                // next vertex take the immediately clockwise edge from reverse.
                let next_index = if reverse_index == 0 {
                    neighbours.len() - 1
                } else {
                    reverse_index - 1
                };
                let next = neighbours[next_index];
                from = to;
                to = next;
                if (from, to) == start {
                    closed = true;
                    break;
                }
            }
            if !closed || ring.len() < 4 {
                continue;
            }
            let area = bucket_signed_ring_area(&ring, &nodes);
            if area <= 0.05 {
                continue;
            }
            let coords: Vec<Coord<f64>> = ring
                .iter()
                .map(|index| Coord {
                    x: nodes[*index].x as f64,
                    y: nodes[*index].y as f64,
                })
                .collect();
            faces.push(Polygon::new(LineString(coords), Vec::new()));
        }
    }
    faces
}

fn bucket_split_segments_at_intersections(
    segments: &[BucketBoundarySegment],
) -> Vec<BucketBoundarySegment> {
    if segments.is_empty() {
        return Vec::new();
    }
    let mut splits = vec![vec![0.0_f64, 1.0_f64]; segments.len()];
    let mut cells = std::collections::HashMap::<(i32, i32), Vec<usize>>::new();
    const CELL: f32 = 64.0;
    for (index, segment) in segments.iter().enumerate() {
        let min_x = segment.a.x.min(segment.b.x);
        let max_x = segment.a.x.max(segment.b.x);
        let min_y = segment.a.y.min(segment.b.y);
        let max_y = segment.a.y.max(segment.b.y);
        let x0 = (min_x / CELL).floor() as i32;
        let x1 = (max_x / CELL).floor() as i32;
        let y0 = (min_y / CELL).floor() as i32;
        let y1 = (max_y / CELL).floor() as i32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                cells.entry((x, y)).or_default().push(index);
            }
        }
    }
    let mut pairs = std::collections::BTreeSet::<(usize, usize)>::new();
    for bucket in cells.values() {
        for left in 0..bucket.len() {
            for right in (left + 1)..bucket.len() {
                let a = bucket[left];
                let b = bucket[right];
                if a != b {
                    pairs.insert(if a < b { (a, b) } else { (b, a) });
                }
            }
        }
    }
    for (left, right) in pairs {
        if !bucket_segment_bounds_overlap(segments[left], segments[right]) {
            continue;
        }
        let (head, tail) = splits.split_at_mut(right);
        bucket_add_segment_intersections(
            segments[left],
            segments[right],
            &mut head[left],
            &mut tail[0],
        );
    }

    let mut out = Vec::new();
    for (segment, mut params) in segments.iter().copied().zip(splits) {
        params.sort_by(|a, b| a.total_cmp(b));
        params.dedup_by(|a, b| (*a - *b).abs() <= 1e-7);
        for pair in params.windows(2) {
            if pair[1] - pair[0] <= 1e-7 {
                continue;
            }
            let a = bucket_lerp(segment, pair[0]);
            let b = bucket_lerp(segment, pair[1]);
            if bucket_distance_sq(a, b) > 1e-8 {
                out.push(BucketBoundarySegment { a, b });
            }
        }
    }
    out
}

fn bucket_add_segment_intersections(
    left: BucketBoundarySegment,
    right: BucketBoundarySegment,
    left_splits: &mut Vec<f64>,
    right_splits: &mut Vec<f64>,
) {
    let px = left.a.x as f64;
    let py = left.a.y as f64;
    let rx = (left.b.x - left.a.x) as f64;
    let ry = (left.b.y - left.a.y) as f64;
    let qx = right.a.x as f64;
    let qy = right.a.y as f64;
    let sx = (right.b.x - right.a.x) as f64;
    let sy = (right.b.y - right.a.y) as f64;
    let cross = |ax: f64, ay: f64, bx: f64, by: f64| ax * by - ay * bx;
    let dot = |ax: f64, ay: f64, bx: f64, by: f64| ax * bx + ay * by;
    let rxs = cross(rx, ry, sx, sy);
    let qpx = qx - px;
    let qpy = qy - py;
    let qpxr = cross(qpx, qpy, rx, ry);
    const EPS: f64 = 1e-9;

    if rxs.abs() > EPS {
        let t = cross(qpx, qpy, sx, sy) / rxs;
        let u = cross(qpx, qpy, rx, ry) / rxs;
        if (-1e-7..=1.0 + 1e-7).contains(&t) && (-1e-7..=1.0 + 1e-7).contains(&u) {
            left_splits.push(t.clamp(0.0, 1.0));
            right_splits.push(u.clamp(0.0, 1.0));
        }
        return;
    }
    if qpxr.abs() > EPS {
        return;
    }

    let rr = dot(rx, ry, rx, ry);
    let ss = dot(sx, sy, sx, sy);
    if rr <= EPS || ss <= EPS {
        return;
    }
    for (x, y) in [(qx, qy), (qx + sx, qy + sy)] {
        let t = dot(x - px, y - py, rx, ry) / rr;
        if (-1e-7..=1.0 + 1e-7).contains(&t) {
            left_splits.push(t.clamp(0.0, 1.0));
        }
    }
    for (x, y) in [(px, py), (px + rx, py + ry)] {
        let u = dot(x - qx, y - qy, sx, sy) / ss;
        if (-1e-7..=1.0 + 1e-7).contains(&u) {
            right_splits.push(u.clamp(0.0, 1.0));
        }
    }
}

fn bucket_segment_bounds_overlap(
    left: BucketBoundarySegment,
    right: BucketBoundarySegment,
) -> bool {
    const EPS: f32 = 1e-4;
    left.a.x.min(left.b.x) <= right.a.x.max(right.b.x) + EPS
        && left.a.x.max(left.b.x) + EPS >= right.a.x.min(right.b.x)
        && left.a.y.min(left.b.y) <= right.a.y.max(right.b.y) + EPS
        && left.a.y.max(left.b.y) + EPS >= right.a.y.min(right.b.y)
}

fn bucket_lerp(segment: BucketBoundarySegment, t: f64) -> Vec2 {
    let t = t as f32;
    Vec2::new(
        segment.a.x + (segment.b.x - segment.a.x) * t,
        segment.a.y + (segment.b.y - segment.a.y) * t,
    )
}

fn bucket_distance_sq(a: Vec2, b: Vec2) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    dx * dx + dy * dy
}

fn bucket_graph_node(
    nodes: &mut Vec<Vec2>,
    node_map: &mut std::collections::HashMap<(i64, i64), usize>,
    point: Vec2,
) -> usize {
    const QUANTIZE: f64 = 10_000.0;
    let key = (
        (point.x as f64 * QUANTIZE).round() as i64,
        (point.y as f64 * QUANTIZE).round() as i64,
    );
    if let Some(index) = node_map.get(&key) {
        return *index;
    }
    let index = nodes.len();
    nodes.push(point);
    node_map.insert(key, index);
    index
}

fn bucket_signed_ring_area(ring: &[usize], nodes: &[Vec2]) -> f64 {
    if ring.len() < 4 {
        return 0.0;
    }
    let mut twice_area = 0.0_f64;
    for pair in ring.windows(2) {
        let a = nodes[pair[0]];
        let b = nodes[pair[1]];
        twice_area += a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64;
    }
    twice_area * 0.5
}
/// Resolve the connected filled raw region on the active editable layer.
/// Empty-space regions are handled separately by `find_empty_bucket_region`,
/// which polygonizes the planar boundary graph rather than guessing one path.
fn find_bucket_target(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    cursor: Vec2,
) -> Option<BucketTarget> {
    if !project.layer_is_visible(q0rg_id, layer_id) || project.layer_is_locked(q0rg_id, layer_id) {
        return None;
    }
    let layer = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)?;

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
        if vector.fill.is_none() {
            continue;
        }
        let Some(component) = selectable_component_at_cursor(project, asset_id, vector, cursor)
        else {
            continue;
        };
        let path_indices: Vec<usize> = vector
            .paths
            .iter()
            .enumerate()
            .filter(|(_, path)| path.closed)
            .filter_map(|(path_idx, path)| {
                path.anchors
                    .iter()
                    .any(|anchor| polygon_boundary_near_cursor(&component, anchor.point, 0.5))
                    .then_some(path_idx)
            })
            .collect();
        if !path_indices.is_empty() {
            return Some(BucketTarget {
                layer_id,
                placement_idx,
                asset_id,
                path_indices,
            });
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    use q0s_format::v2::{Layer, ProjectMeta, Q0rg};

    #[test]
    fn full_marquee_drag_preserves_real_curve_fixture() {
        let mut project =
            q0s_format::v2::parse(include_bytes!("../testdata/marquee_curve_regression.q1s"))
                .expect("parse user curve regression fixture");
        project.assets.retain(|asset| asset.id() == 1);
        project.q0rgs[0].layers[0]
            .placements
            .retain(|placement| placement.target == Target::Asset(1));
        let original = match &project.assets[0] {
            Asset::Vector(vector) => vector.paths[0].clone(),
            _ => panic!("fixture asset 1 must be vector"),
        };
        assert!(
            original
                .anchors
                .iter()
                .any(|anchor| anchor.in_handle.is_some() || anchor.out_handle.is_some()),
            "fixture must contain the smooth curve that reproduced the bug",
        );

        let mut app = EditorApp::default();
        app.state.project = project;
        app.session.current_q0rg_id = 1;
        app.session.current_frame = 0;
        let selection = marquee_selection(&app.state.project, 1, 0, (180.0, 30.0, 470.0, 250.0));
        let Selection::RawArea {
            placements,
            bounds_min,
            bounds_max,
            ..
        } = selection
        else {
            panic!("full rubber-band marquee must create RawArea selection");
        };
        let refs = cut_raw_areas_for_drag(
            &mut app,
            &placements,
            (bounds_min.x, bounds_min.y, bounds_max.x, bounds_max.y),
        );
        assert!(!refs.is_empty());
        let selected = raw_path_clone(
            &app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            refs[0].placement_idx,
            refs[0].path_idx,
        )
        .expect("marquee-selected curve after drag materialization");
        assert_eq!(
            selected, original,
            "fully enclosed marquee selection must not rebuild or polygonize the curve",
        );
        // Continue through the same live-transform + bake + release work that a
        // real RawArea drag performs after materialisation.
        let refs = isolate_partial_appearance_raw_refs(&mut app, refs);
        let refs = isolate_raw_refs_for_live_transform(&mut app, refs)
            .expect("full marquee selection must remain transformable");
        let delta = Vec2::new(37.0, 19.0);
        assert!(set_raw_refs_live_transform(
            &mut app.state.project,
            &refs,
            Affine {
                tx: delta.x,
                ty: delta.y,
                ..Affine::IDENTITY
            },
        ));
        assert!(bake_raw_refs_live_transform(&mut app, &refs));

        let mut expected = original.clone();
        for anchor in &mut expected.anchors {
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
        let moved = raw_path_clone(
            &app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            refs[0].placement_idx,
            refs[0].path_idx,
        )
        .expect("curve after marquee drag bake");
        assert_eq!(moved, expected, "marquee drag bake changed the curve");

        let asset_id = raw_placement_asset_id(
            &app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            refs[0].placement_idx,
        )
        .expect("moved raw asset");
        let edited = std::collections::BTreeSet::from([asset_id]);
        assert!(!crate::brush::merge_touching_raw_fills_after_edit_focused(
            &mut app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            0,
            &edited,
        ));
        let released = raw_path_clone(
            &app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            refs[0].placement_idx,
            refs[0].path_idx,
        )
        .expect("curve after marquee release");
        assert_eq!(
            released, expected,
            "release polygonized the marquee-moved curve"
        );
    }

    #[test]
    fn brush_preview_handoff_lives_for_release_frame_then_expires() {
        let mut app = EditorApp::default();
        app.session.classic_brush_preview_handoff = true;
        app.session.advanced_brush_preview_handoff = Some(crate::advanced_brush::advanced_begin(
            crate::advanced_brush::AdvancedBrushSettings::default(),
            crate::advanced_brush::AdvancedBrushSample::mouse(Vec2::new(10.0, 12.0), 0.0),
        ));

        // These flags are what `draw_in_progress_overlay` sees during the
        // pointer-up frame, after the committed geometry missed `render_stage`.
        assert!(app.session.classic_brush_preview_handoff);
        assert!(app.session.advanced_brush_preview_handoff.is_some());

        // The next tool frame starts only after `render_stage` has seen the
        // committed project, so the draft handoff must disappear here.
        expire_brush_preview_handoff(&mut app);
        assert!(!app.session.classic_brush_preview_handoff);
        assert!(app.session.advanced_brush_preview_handoff.is_none());
    }

    #[cfg(feature = "appearance-mask-eraser")]
    fn surface_mismatch_area(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> f64 {
        left.difference(right)
            .union(&right.difference(left))
            .unsigned_area()
    }

    #[test]
    fn classic_release_frame_never_accepts_paint_samples() {
        assert!(classic_drag_frame_accepts_samples(true, false, false));
        assert!(classic_drag_frame_accepts_samples(false, true, false));
        assert!(!classic_drag_frame_accepts_samples(false, false, true));
        assert!(
            !classic_drag_frame_accepts_samples(false, true, true),
            "a release frame must not paint even if egui also reports it as dragged"
        );
        assert!(
            !classic_drag_frame_accepts_samples(true, true, true),
            "pointer-up is a terminator, never another dab"
        );
    }

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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                        instance_id: 0,
                        frame,
                        target: Target::Asset(asset_id),
                        transform,
                        tween: Tween::None,
                        fx: Default::default(),
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                            instance_id: 42,
                            frame: 0,
                            target: Target::Q0rg(2),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
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
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
                        },
                        Placement {
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(2),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
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
                Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => None,
            })
            .expect("original path");
        let current = project
            .assets
            .iter()
            .find(|asset| asset.id() == current_asset)
            .and_then(|asset| match asset {
                Asset::Vector(vector) => vector.paths.first(),
                Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => None,
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
    fn classic_prerender_stays_segment_local_for_a_long_polygon_stroke() {
        let dabs: Vec<(Vec2, f32)> = (0..5_000)
            .map(|index| {
                (
                    Vec2::new(index as f32 * 0.5, (index as f32 * 0.03).sin() * 30.0),
                    4.0 + (index as f32 * 0.07).sin().abs() * 18.0,
                )
            })
            .collect();
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 600.0)),
        };
        let contours =
            classic_prerender_contours(&dabs, crate::brush::BrushNib::Horizontal, &view, 0.0);

        assert_eq!(contours.len(), dabs.len() - 1);
        let points: usize = contours.iter().map(Vec::len).sum();
        assert!(
            points <= dabs.len() * 8,
            "pre-render rebuilt a monolithic contour: {points} points for {} dabs",
            dabs.len()
        );
    }

    #[test]
    fn classic_polygon_prerender_never_draws_outside_zero_smoothing_commit() {
        use geo::Intersects;

        let settings = crate::brush::BrushSettings {
            size: 42.0,
            smoothing: 0,
            nib: crate::brush::BrushNib::Horizontal,
            pressure_size: true,
            dynamics_min_size: 0.2,
            ..Default::default()
        };
        let samples = [
            (Vec2::new(40.0, 60.0), 0.25, 0.0),
            (Vec2::new(110.0, 60.0), 1.0, 0.05),
            (Vec2::new(135.0, 115.0), 0.35, 0.10),
            (Vec2::new(205.0, 115.0), 0.8, 0.15),
            (Vec2::new(230.0, 55.0), 0.3, 0.20),
        ];
        let mut stroke = crate::brush::brush_begin(
            settings,
            crate::brush::BrushSample::pointer(samples[0].0, Some(samples[0].1), samples[0].2),
        );
        for &(position, pressure, time) in &samples[1..] {
            crate::brush::brush_add_sample(
                &mut stroke,
                settings,
                crate::brush::BrushSample::pointer(position, Some(pressure), time),
            );
        }
        let dabs = crate::brush::brush_prerender_dabs(&stroke);
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(300.0, 180.0)),
        };
        let contours = classic_prerender_contours(&dabs, settings.nib, &view, 0.0);
        let committed = crate::brush::brush_finish(stroke, settings);

        for point in contours.iter().flatten() {
            assert!(
                committed.intersects(&Point::new(f64::from(point.x), f64::from(point.y))),
                "pre-render spike escaped committed sweep at {point:?}"
            );
        }
    }
    #[test]
    fn advanced_preview_reuses_ring_vertices_instead_of_duplicating_every_bridge() {
        let settings = crate::advanced_brush::AdvancedBrushSettings {
            glow: false,
            stabilizer: 0,
            smoothing: 0,
            pressure_size: false,
            ..Default::default()
        };
        let mut stroke = crate::advanced_brush::advanced_begin(
            settings,
            crate::advanced_brush::AdvancedBrushSample::mouse(Vec2::new(0.0, 0.0), 0.0),
        );
        for index in 1..200 {
            crate::advanced_brush::advanced_add_sample(
                &mut stroke,
                settings,
                crate::advanced_brush::AdvancedBrushSample::mouse(
                    Vec2::new(index as f32 * 2.0, (index as f32 * 0.1).sin() * 12.0),
                    index as f64 / 120.0,
                ),
            );
        }
        let dabs = crate::advanced_brush::advanced_dabs(&stroke);
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 600.0)),
        };
        let mesh = advanced_preview_mesh(&dabs, &view, settings);
        assert!(
            mesh.vertices.len() <= dabs.len() * 19 + 2,
            "preview duplicated bridge vertices: {} vertices for {} dabs",
            mesh.vertices.len(),
            dabs.len()
        );

        let glow_settings = crate::advanced_brush::AdvancedBrushSettings {
            glow: true,
            glow_radius: 18.0,
            glow_opacity: 0.7,
            ..settings
        };
        let glow_mesh = advanced_preview_mesh(&dabs, &view, glow_settings);
        assert!(
            glow_mesh.vertices.len() <= dabs.len() * 91 + 10,
            "glow preview escaped its linear mesh budget: {} vertices for {} dabs",
            glow_mesh.vertices.len(),
            dabs.len()
        );
    }

    #[test]
    fn advanced_preview_strip_keeps_intermediate_dab_centers_filled() {
        fn triangle_contains(point: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
            let cross =
                |u: Pos2, v: Pos2, w: Pos2| (v.x - u.x) * (w.y - u.y) - (v.y - u.y) * (w.x - u.x);
            let ab = cross(a, b, point);
            let bc = cross(b, c, point);
            let ca = cross(c, a, point);
            (ab >= -1.0e-4 && bc >= -1.0e-4 && ca >= -1.0e-4)
                || (ab <= 1.0e-4 && bc <= 1.0e-4 && ca <= 1.0e-4)
        }

        let settings = crate::advanced_brush::AdvancedBrushSettings {
            size: 30.0,
            roundness: 0.35,
            angle_degrees: 35.0,
            stabilizer: 0,
            smoothing: 0,
            pressure_size: false,
            ..Default::default()
        };
        let mut stroke = crate::advanced_brush::advanced_begin(
            settings,
            crate::advanced_brush::AdvancedBrushSample::mouse(Vec2::new(40.0, 40.0), 0.0),
        );
        for (index, position) in [Vec2::new(100.0, 60.0), Vec2::new(125.0, 125.0)]
            .into_iter()
            .enumerate()
        {
            crate::advanced_brush::advanced_add_sample(
                &mut stroke,
                settings,
                crate::advanced_brush::AdvancedBrushSample::mouse(
                    position,
                    (index + 1) as f64 * 0.1,
                ),
            );
        }
        let dabs = crate::advanced_brush::advanced_dabs(&stroke);
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(300.0, 300.0)),
        };
        let mesh = advanced_preview_mesh(&dabs, &view, settings);
        for dab in &dabs {
            let point = stage_to_screen(dab.center, &view);
            let covered = mesh.indices.chunks_exact(3).any(|triangle| {
                triangle_contains(
                    point,
                    mesh.vertices[triangle[0] as usize].pos,
                    mesh.vertices[triangle[1] as usize].pos,
                    mesh.vertices[triangle[2] as usize].pos,
                )
            });
            assert!(
                covered,
                "preview strip left a hole at dab center {:?}",
                dab.center
            );
        }
    }

    #[test]
    fn brush_nib_cursor_keeps_dark_outer_outline_for_white_stage_visibility() {
        let [outer, inner] = nib_cursor_strokes();
        assert_eq!(outer.width, 2.5);
        assert_eq!(outer.color, Color32::from_black_alpha(210));
        assert_eq!(inner.width, 1.0);
        assert_eq!(inner.color, Color32::WHITE);
    }

    #[test]
    fn dynamic_brush_cursor_reports_current_size_inside_full_nib() {
        let mut settings = crate::brush::BrushSettings {
            size: 20.0,
            pressure_size: true,
            dynamics_min_size: 0.2,
            dynamics_sensitivity: 50,
            ..Default::default()
        };
        let mut stroke = crate::brush::brush_begin(
            settings,
            crate::brush::BrushSample::pointer(Vec2::new(0.0, 0.0), Some(0.25), 0.0),
        );
        crate::brush::brush_add_sample(
            &mut stroke,
            settings,
            crate::brush::BrushSample::pointer(Vec2::new(5.0, 0.0), Some(0.25), 0.1),
        );
        let current = classic_dynamic_cursor_size_px(&stroke, 2.0).unwrap();
        let expected = crate::brush::brush_preview_dabs(&stroke).last().unwrap().1 * 2.0 + 2.5;
        let full = brush_cursor_radius_px(settings, 2.0) * 2.0 + 2.5;
        assert!((current - expected).abs() < 1.0e-5);
        assert!(current < full, "current={current}, full={full}");

        settings.pressure_size = false;
        let static_stroke = crate::brush::brush_begin(
            settings,
            crate::brush::BrushSample::mouse(Vec2::new(0.0, 0.0)),
        );
        assert!(classic_dynamic_cursor_size_px(&static_stroke, 2.0).is_none());
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
    fn brush_size_preview_centres_on_visible_stage_intersection() {
        let stage = egui::Rect::from_min_max(pos2(-200.0, -100.0), pos2(800.0, 700.0));
        let viewport = egui::Rect::from_min_max(pos2(100.0, 50.0), pos2(500.0, 350.0));
        let visible = visible_stage_preview_rect(stage, viewport).unwrap();
        assert_eq!(visible.min, viewport.min);
        assert_eq!(visible.max, viewport.max);
        assert_eq!(visible.center(), pos2(300.0, 200.0));

        let partly_visible_stage = egui::Rect::from_min_max(pos2(350.0, 200.0), pos2(900.0, 800.0));
        let visible = visible_stage_preview_rect(partly_visible_stage, viewport).unwrap();
        assert_eq!(visible.min, pos2(350.0, 200.0));
        assert_eq!(visible.max, pos2(500.0, 350.0));
        assert_eq!(visible.center(), pos2(425.0, 275.0));

        let offscreen_stage = egui::Rect::from_min_max(pos2(600.0, 500.0), pos2(900.0, 800.0));
        assert!(visible_stage_preview_rect(offscreen_stage, viewport).is_none());
    }

    #[test]
    fn brush_size_preview_tracks_zoom_and_minimum_ratio() {
        let mut settings = crate::brush::BrushSettings {
            size: 20.0,
            dynamics_min_size: 0.25,
            scale_with_stage: true,
            ..Default::default()
        };

        let (outer, inner) =
            classic_preview_diameters_px(settings, 2.0, BrushSizePreview::MinimumSize);
        assert!((outer - 40.0).abs() < 1.0e-6);
        assert!((inner.unwrap() - 10.0).abs() < 1.0e-6);
        let (_, inner) = classic_preview_diameters_px(settings, 2.0, BrushSizePreview::Size);
        assert!(inner.is_none());

        settings.scale_with_stage = false;
        let (outer_zoomed, inner_zoomed) =
            classic_preview_diameters_px(settings, 2.0, BrushSizePreview::MinimumSize);
        let (outer_zoomed_out, _) =
            classic_preview_diameters_px(settings, 0.25, BrushSizePreview::MinimumSize);
        assert!((outer_zoomed - 20.0).abs() < 1.0e-6);
        assert!((outer_zoomed_out - 20.0).abs() < 1.0e-6);
        assert!((inner_zoomed.unwrap() - 5.0).abs() < 1.0e-6);
    }

    #[test]
    fn advanced_size_preview_preserves_tip_shape_and_pressure_minimum() {
        let settings = crate::advanced_brush::AdvancedBrushSettings {
            size: 20.0,
            roundness: 0.25,
            angle_degrees: 90.0,
            pressure_min_size: 0.3,
            scale_with_stage: true,
            ..Default::default()
        };
        let (outer, inner) =
            advanced_preview_diameters_px(settings, 3.0, BrushSizePreview::MinimumSize);
        assert!((outer - 60.0).abs() < 1.0e-6);
        assert!((inner.unwrap() - 18.0).abs() < 1.0e-6);

        let points = advanced_nib_outline_points(Pos2::ZERO, outer, 0.25, 90.0);
        let min_x = points.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let max_x = points.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
        let min_y = points.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let max_y = points.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        assert!((max_x - min_x - 15.0).abs() < 0.05);
        assert!((max_y - min_y - 60.0).abs() < 0.05);
    }

    #[test]
    fn minimum_size_preview_uses_a_red_inner_nib() {
        let [underlay, red] = minimum_nib_strokes();
        assert_eq!(underlay.color, Color32::from_black_alpha(180));
        assert_eq!(red.color, Color32::from_rgb(235, 48, 55));
        assert!(red.width < underlay.width);
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
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
        let start_appearances = capture_whole_asset_appearances_for_raw_refs(&mut app, &refs);
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
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
                        },
                        Placement {
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(2),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
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
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];

        let vector = match &project.assets[0] {
            Asset::Vector(vector) => vector,
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
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
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
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
        let mut project = ProjectV2 {
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
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

        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 1),
            q0s_format::v2::LayerMetadata {
                hidden: true,
                ..Default::default()
            },
        );
        assert!(
            selection_at_point_pub(&project, 1, 0, Vec2::new(10.0, 10.0)).is_none(),
            "hidden raw artwork must not remain selectable"
        );

        project.layer_metadata.insert(
            q0s_format::v2::LayerKey::new(1, 1),
            q0s_format::v2::LayerMetadata {
                locked: true,
                ..Default::default()
            },
        );
        assert!(
            selection_at_point_pub(&project, 1, 0, Vec2::new(10.0, 10.0)).is_none(),
            "locked raw artwork must remain visible but not selectable"
        );
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
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
    fn cached_connected_selection_matches_exact_halo_hole_semantics_and_reuses_components() {
        let project = appearance_selection_project(true);
        let mut cache = TextureCache::default();
        for point in [
            Vec2::new(-5.0, 10.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(25.0, 10.0),
            Vec2::new(40.0, 40.0),
        ] {
            assert_eq!(
                selection_at_point_cached(&project, &mut cache, 1, 0, point),
                selection_at_point_pub(&project, 1, 0, point),
                "cached connected selection changed exact raw semantics at {point:?}",
            );
        }
        assert_eq!(
            cache.raw_selection_components_build_count(),
            1,
            "one immutable raw asset must resolve its connected source components once",
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn interactive_glow_hover_matches_exact_material_hit_without_component_rebuild() {
        let project = appearance_selection_project(true);
        let vector = match &project.assets[0] {
            Asset::Vector(vector) => vector,
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
        };
        let appearance = project.asset_appearances.get(&1).expect("appearance");
        let mut cache = TextureCache::default();
        for point in [
            Vec2::new(-5.0, 10.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(25.0, 10.0),
            Vec2::new(40.0, 40.0),
        ] {
            let exact = interactive_visible_fill_hit(vector, Some(appearance), point, 2.0);
            assert_eq!(
                interactive_visible_fill_hit_cached(
                    vector,
                    Some(appearance),
                    point,
                    2.0,
                    &mut cache,
                ),
                exact,
                "cached interactive hover changed raw hit semantics at {point:?}",
            );
            assert_eq!(
                exact,
                crate::appearance::visible_material_contains_point(
                    vector,
                    Some(appearance),
                    point,
                    2.0,
                ),
                "interactive hover diverged from exact visible material at {point:?}",
            );
        }
        assert_eq!(
            cache.appearance_hit_build_count(),
            1,
            "one immutable raw asset must flatten source/clip/erase paths once",
        );
        assert!(hit_test_raw_hover_cached(
            &project,
            &mut cache,
            1,
            0,
            Vec2::new(-5.0, 10.0),
        ));
        assert!(!hit_test_raw_hover_cached(
            &project,
            &mut cache,
            1,
            0,
            Vec2::new(10.0, 10.0),
        ));
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn dense_advanced_glow_hover_does_not_reenter_full_component_geometry_per_frame() {
        let mut app = EditorApp::default();
        let settings = crate::advanced_brush::AdvancedBrushSettings {
            size: 18.0,
            smoothing: 35,
            stabilizer: 0,
            pressure_size: false,
            glow: true,
            glow_radius: 14.0,
            glow_opacity: 0.6,
            ..Default::default()
        };
        let samples = (0..240)
            .map(|index| crate::advanced_brush::AdvancedBrushSample {
                position: Vec2::new(
                    40.0 + index as f32 * 1.8,
                    180.0 + (index as f32 * 0.11).sin() * 45.0,
                ),
                pressure: None,
                time_seconds: index as f64 / 120.0,
            })
            .collect();
        let region =
            crate::advanced_brush::advanced_finish(crate::advanced_brush::AdvancedBrushStroke {
                samples,
                settings,
            });
        let bridge = crate::brush::BrushSettings {
            color: settings.color,
            size: settings.size,
            smoothing: 0,
            nib: crate::brush::BrushNib::Circle,
            scale_with_stage: true,
            sync_with_eraser: true,
            ..crate::brush::BrushSettings::default()
        };
        crate::brush::commit_brush_region_with_material(
            &mut app,
            region,
            bridge,
            settings.material(),
        );

        let mut hover_cache = TextureCache::default();
        let start = std::time::Instant::now();
        for index in 0..64 {
            let point = if index % 2 == 0 {
                Vec2::new(120.0 + index as f32, 180.0)
            } else {
                Vec2::new(600.0, 440.0)
            };
            let _ = hit_test_raw_hover_cached(&app.state.project, &mut hover_cache, 1, 0, point);
        }
        assert_eq!(
            hover_cache.appearance_hit_build_count(),
            1,
            "dense Advanced Glow hover must reuse one flattened hit geometry cache",
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "dense Advanced Glow hover fell back to frame-by-frame component reconstruction: {:?}",
            start.elapsed(),
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn first_glow_drag_freezes_material_source_in_project_before_moving_body() {
        let mut app = EditorApp::default();
        app.state.project = appearance_selection_project(false);
        let before_vector = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector.clone(),
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
        };
        let before_appearance = app.state.project.asset_appearances[&1].clone();
        let before_visible = crate::appearance::visible_material_surface_for_vector(
            &before_vector,
            Some(&before_appearance),
        );
        assert!(before_appearance.material_source.is_empty());
        let reference = PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        };
        let before_bounds = raw_path_refs_ui_bounds(&app.state.project, &[reference])
            .expect("initial glow selection bounds");

        assert!(begin_dragging_raw_paths(
            &mut app,
            vec![reference],
            Vec2::new(10.0, 10.0),
            "test drag",
        ));
        let frozen = app
            .state
            .project
            .asset_appearances
            .get(&1)
            .expect("frozen appearance");
        assert_eq!(
            frozen.material_source, before_vector.paths,
            "the renderer must not keep falling back to a carrier that is about to move",
        );

        let ToolState::DraggingPaths {
            refs,
            start_paths,
            start_appearances,
            ..
        } = app.session.tool_state.clone()
        else {
            unreachable!();
        };
        let delta = Vec2::new(31.0, 17.0);
        let transform = Affine {
            tx: delta.x,
            ty: delta.y,
            ..Affine::IDENTITY
        };
        assert!(apply_raw_affine_snapshot(
            &mut app.state.project,
            &refs,
            &start_paths,
            &start_appearances,
            transform,
        ));
        let moved_vector = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector,
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
        };
        let moved_appearance = app
            .state
            .project
            .asset_appearances
            .get(&1)
            .expect("moved appearance");
        let moved_visible = crate::appearance::visible_material_surface_for_vector(
            moved_vector,
            Some(moved_appearance),
        );
        let expected_visible = crate::appearance::transform_surface(&before_visible, transform);
        assert!(
            surface_mismatch_area(&moved_visible, &expected_visible) < 0.05,
            "resolved glow did not follow the carrier by exactly one drag affine",
        );
        assert_eq!(moved_vector.paths[0].anchors[0].point, delta);
        assert_eq!(moved_appearance.field_transform.tx, delta.x);
        assert_eq!(moved_appearance.field_transform.ty, delta.y);
        let moved_bounds = raw_path_refs_ui_bounds(&app.state.project, &[refs[0]])
            .expect("moved glow selection bounds");
        for (actual, expected) in [
            (moved_bounds.0, before_bounds.0 + delta.x),
            (moved_bounds.1, before_bounds.1 + delta.y),
            (moved_bounds.2, before_bounds.2 + delta.x),
            (moved_bounds.3, before_bounds.3 + delta.y),
        ] {
            assert!(
                (actual - expected).abs() < 0.01,
                "selection frame did not follow moved body/glow: actual={actual}, expected={expected}"
            );
        }
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn old_empty_source_with_moved_field_is_repaired_to_canonical_space() {
        let mut app = EditorApp::default();
        app.state.project = appearance_selection_project(false);
        let old_delta = Vec2::new(40.0, -13.0);
        let old_field = Affine {
            tx: old_delta.x,
            ty: old_delta.y,
            ..Affine::IDENTITY
        };
        let Asset::Vector(vector) = &mut app.state.project.assets[0] else {
            unreachable!();
        };
        for path in &mut vector.paths {
            for anchor in &mut path.anchors {
                anchor.point = old_field.apply(anchor.point);
                if let Some(point) = &mut anchor.in_handle {
                    *point = old_field.apply(*point);
                }
                if let Some(point) = &mut anchor.out_handle {
                    *point = old_field.apply(*point);
                }
            }
        }
        app.state
            .project
            .asset_appearances
            .get_mut(&1)
            .unwrap()
            .field_transform = old_field;
        assert!(app.state.project.asset_appearances[&1]
            .material_source
            .is_empty());

        assert!(begin_dragging_raw_paths(
            &mut app,
            vec![PathRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
                path_idx: 0,
            }],
            Vec2::new(old_delta.x + 10.0, old_delta.y + 10.0),
            "repair drag",
        ));
        let repaired = app
            .state
            .project
            .asset_appearances
            .get(&1)
            .expect("repaired appearance");
        assert_eq!(
            repaired.material_source[0].anchors[0].point,
            Vec2::new(0.0, 0.0)
        );
        assert_eq!(repaired.field_transform, old_field);
        let current_vector = match &app.state.project.assets[0] {
            Asset::Vector(vector) => vector,
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
        };
        let expected = crate::appearance::material_support(
            &vector_fill_geometry(current_vector),
            repaired.material,
        );
        let visible =
            crate::appearance::visible_material_surface_for_vector(current_vector, Some(repaired));
        assert!(surface_mismatch_area(&visible, &expected) < 0.05);
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn moving_one_glowing_region_splits_its_material_and_keeps_neighbor_stationary() {
        let mut app = EditorApp::default();
        app.state.project = appearance_selection_project(false);
        let square = |min_x: f32, max_x: f32| VPath {
            anchors: vec![
                anchor(Vec2::new(min_x, 0.0)),
                anchor(Vec2::new(max_x, 0.0)),
                anchor(Vec2::new(max_x, 20.0)),
                anchor(Vec2::new(min_x, 20.0)),
            ],
            closed: true,
        };
        let Asset::Vector(vector) = &mut app.state.project.assets[0] else {
            unreachable!();
        };
        vector.paths.push(square(60.0, 80.0));

        let Some(RawSelectionHit::Fill(left_refs)) =
            hit_test_raw_selection(&app.state.project, 1, 0, Vec2::new(10.0, 10.0))
        else {
            panic!("left glowing region must be independently selectable");
        };
        assert_eq!(left_refs.len(), 1);
        assert_eq!(left_refs[0].path_idx, 0);
        assert!(begin_dragging_raw_paths(
            &mut app,
            left_refs,
            Vec2::new(10.0, 10.0),
            "move left glow",
        ));
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 2);
        assert_eq!(app.state.project.asset_appearances.len(), 2);
        assert!(matches!(
            app.session.selection,
            Selection::Path { .. } | Selection::Paths(_)
        ));

        let ToolState::DraggingPaths {
            refs,
            start_paths,
            start_appearances,
            ..
        } = app.session.tool_state.clone()
        else {
            unreachable!();
        };
        let selected_asset_id =
            match app.state.project.q0rgs[0].layers[0].placements[refs[0].placement_idx].target {
                Target::Asset(asset_id) => asset_id,
                Target::Q0rg(_) => unreachable!(),
            };
        let neighbor_placement = app.state.project.q0rgs[0].layers[0]
            .placements
            .iter()
            .find(|placement| placement.target != Target::Asset(selected_asset_id))
            .expect("remainder placement");
        let Target::Asset(neighbor_asset_id) = neighbor_placement.target else {
            unreachable!();
        };
        let selected_before = match app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == selected_asset_id)
            .unwrap()
        {
            Asset::Vector(vector) => crate::appearance::visible_material_surface_for_vector(
                vector,
                app.state.project.asset_appearances.get(&selected_asset_id),
            ),
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
        };
        let neighbor_before = match app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == neighbor_asset_id)
            .unwrap()
        {
            Asset::Vector(vector) => crate::appearance::visible_material_surface_for_vector(
                vector,
                app.state.project.asset_appearances.get(&neighbor_asset_id),
            ),
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
        };
        let neighbor_vector_before = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == neighbor_asset_id)
            .cloned()
            .unwrap();

        let delta = Vec2::new(25.0, 30.0);
        let transform = Affine {
            tx: delta.x,
            ty: delta.y,
            ..Affine::IDENTITY
        };
        assert!(apply_raw_affine_snapshot(
            &mut app.state.project,
            &refs,
            &start_paths,
            &start_appearances,
            transform,
        ));
        let selected_after = match app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == selected_asset_id)
            .unwrap()
        {
            Asset::Vector(vector) => crate::appearance::visible_material_surface_for_vector(
                vector,
                app.state.project.asset_appearances.get(&selected_asset_id),
            ),
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
        };
        let neighbor_after = match app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == neighbor_asset_id)
            .unwrap()
        {
            Asset::Vector(vector) => crate::appearance::visible_material_surface_for_vector(
                vector,
                app.state.project.asset_appearances.get(&neighbor_asset_id),
            ),
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
        };
        let expected_selected = crate::appearance::transform_surface(&selected_before, transform);
        assert!(surface_mismatch_area(&selected_after, &expected_selected) < 0.05);
        assert!(surface_mismatch_area(&neighbor_after, &neighbor_before) < 0.05);
        assert_eq!(
            app.state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == neighbor_asset_id)
                .unwrap(),
            &neighbor_vector_before,
            "moving one connected glowing region mutated its disconnected neighbor",
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn cloning_glow_drag_state_reuses_shared_snapshots() {
        let project = appearance_selection_project(false);
        let vector = match &project.assets[0] {
            Asset::Vector(vector) => vector,
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => unreachable!(),
        };
        let paths = std::sync::Arc::new(vector.paths.clone());
        let appearance = project.asset_appearances.get(&1).expect("appearance");
        let appearances = std::sync::Arc::new(vec![AppearanceTransformSnapshot {
            asset_id: 1,
            field_transform: appearance.field_transform,
        }]);
        assert!(
            std::mem::size_of::<AppearanceTransformSnapshot>() <= 32,
            "drag transform snapshot must stay independent of material-source size",
        );
        let state = ToolState::DraggingPaths {
            refs: vec![PathRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
                path_idx: 0,
            }],
            start_cursor: Vec2::new(0.0, 0.0),
            start_paths: paths.clone(),
            start_appearances: appearances.clone(),
            start_pivot: None,
        };
        let cloned = state.clone();
        let ToolState::DraggingPaths {
            start_paths: cloned_paths,
            start_appearances: cloned_appearances,
            ..
        } = cloned
        else {
            unreachable!();
        };
        assert!(std::sync::Arc::ptr_eq(&paths, &cloned_paths));
        assert!(std::sync::Arc::ptr_eq(&appearances, &cloned_appearances));
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn glow_drag_updates_only_field_transform_without_cloning_material_source() {
        let mut project = appearance_selection_project(false);
        let dense_path = VPath {
            anchors: (0..2048)
                .map(|index| anchor(Vec2::new(index as f32 * 0.25, (index % 17) as f32)))
                .collect(),
            closed: true,
        };
        {
            let appearance = project.asset_appearances.get_mut(&1).expect("appearance");
            appearance.material_source = vec![dense_path];
        }
        let start = vec![AppearanceTransformSnapshot {
            asset_id: 1,
            field_transform: project
                .asset_appearances
                .get(&1)
                .expect("appearance")
                .field_transform,
        }];
        let before_paths_ptr = project.asset_appearances[&1].material_source.as_ptr();
        let before_anchors_ptr = project.asset_appearances[&1].material_source[0]
            .anchors
            .as_ptr();

        assert!(transform_captured_appearances(
            &mut project,
            &start,
            Affine {
                tx: 37.0,
                ty: -12.0,
                ..Affine::IDENTITY
            },
        ));
        let moved = project.asset_appearances.get(&1).expect("moved appearance");
        assert_eq!(moved.material_source.as_ptr(), before_paths_ptr);
        assert_eq!(
            moved.material_source[0].anchors.as_ptr(),
            before_anchors_ptr
        );
        assert_eq!(moved.field_transform.tx, 37.0);
        assert_eq!(moved.field_transform.ty, -12.0);
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
            instance_id: 0,
            frame: 0,
            target: Target::Asset(2),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
    fn glow_selection_visual_follows_body_not_soft_halo_sampling_grid() {
        let project = appearance_selection_project(true);
        let vector = match &project.assets[0] {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        let appearance = &project.asset_appearances[&1];
        let view = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::new(-20.0, -20.0), Pos2::new(50.0, 50.0)),
        };
        let mut cache = TextureCache::default();
        let contours =
            appearance_selection_body_contours(vector, appearance, None, &view, &mut cache);
        assert!(
            !contours.is_empty(),
            "selected glow body needs a visible cue"
        );
        assert!(
            contours
                .iter()
                .flatten()
                .all(|point| point.x >= -0.01 && point.x <= 20.01),
            "selection visuals must not expand into the soft halo: {contours:?}",
        );
        let body = crate::appearance::visible_source_surface_for_vector(vector, Some(appearance));
        assert!(
            !body.contains(&Point::new(10.0, 10.0)),
            "erased body hole must remain absent from the geometric selection cue",
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
        let start_appearances = capture_whole_asset_appearances_for_raw_refs(&mut app, &refs);
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
            Vec2::new((bounds.0 + bounds.2) * 0.5, (bounds.1 + bounds.3) * 0.5),
            cursor,
            false,
            true,
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
            center,
            cursor,
            false,
            false,
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
            Vec2::new((bounds.0 + bounds.2) * 0.5, (bounds.1 + bounds.3) * 0.5),
            cursor,
            false,
            false,
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
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
        let l_shape = VPath {
            anchors: vec![
                anchor(Vec2::new(0.0, 0.0)),
                anchor(Vec2::new(6.0, 0.0)),
                anchor(Vec2::new(6.0, 14.0)),
                anchor(Vec2::new(20.0, 14.0)),
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
                paths: vec![l_shape],
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                            instance_id: 0,
                            frame: 0,
                            target: Target::Q0rg(2),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
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
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
                        }],
                    }],
                },
            ],
        };

        assert_eq!(
            hit_test_selectable_placement(&project, 1, 0, Vec2::new(3.0, 10.0)),
            Some((1, 0))
        );
        assert!(matches!(
            selection_at_point_pub(&project, 1, 0, Vec2::new(3.0, 10.0)),
            Some(Selection::Placement {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
            })
        ));
        let empty_inside_bbox = Vec2::new(12.0, 5.0);
        let placement = &project.q0rgs[0].layers[0].placements[0];
        assert!(
            placement_bbox(&project, placement)
                .is_some_and(|bounds| point_in_bounds(empty_inside_bbox, bounds)),
            "regression probe must really be inside the q0rg transform bbox"
        );
        assert_eq!(
            hit_test_selectable_placement(&project, 1, 0, empty_inside_bbox),
            None,
            "empty space inside the q0rg bbox must not select the instance"
        );
        assert_eq!(
            hit_test_placement(&project, 1, 0, empty_inside_bbox),
            None,
            "double-click hit testing must not resurrect the rectangular q0rg body"
        );
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
        assert_eq!(layer.placements[0].instance_id, 42);
        assert_eq!(layer.placements[1].instance_id, 42);
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
            instance_id: 0,
            frame: 10,
            target: Target::Q0rg(2),
            transform: Transform2D {
                tx: 100.0,
                ..Transform2D::IDENTITY
            },
            tween: Tween::None,
            fx: Default::default(),
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
    fn edge_handles_scale_one_axis_and_can_cross_for_mirroring() {
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
        let mut transform = start;

        assert!(apply_handle_drag(
            &mut transform,
            start,
            local_bbox,
            Handle::MidRight,
            Vec2::new(310.0, 95.0),
            Some(Vec2::new(50.0, 25.0)),
            ScaleDragModifiers {
                ignore_pivot: true,
                lock_aspect: false,
            },
        ));
        assert!((transform.sx - 3.0).abs() < 1.0e-5);
        assert!((transform.sy - 3.0).abs() < 1.0e-5);
        assert_eq!(transform.skew_x, 0.0);
        assert_eq!(transform.skew_y, 0.0);

        assert!(apply_handle_drag(
            &mut transform,
            start,
            local_bbox,
            Handle::MidRight,
            Vec2::new(-100.0, 95.0),
            Some(Vec2::new(50.0, 25.0)),
            ScaleDragModifiers {
                ignore_pivot: true,
                lock_aspect: false,
            },
        ));
        assert!((transform.sx - -1.1).abs() < 1.0e-5);
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
    fn placement_scale_and_skew_keep_the_custom_anchor_fixed() {
        let start = Transform2D {
            tx: 14.0,
            ty: -6.0,
            sx: 1.2,
            sy: 0.8,
            rotation: 0.15,
            skew_x: 0.05,
            skew_y: 0.0,
        };
        let bounds = (0.0, 0.0, 100.0, 50.0);
        let pivot_local = Vec2::new(31.0, 17.0);
        let pivot_world = Affine::from_transform(start).apply(pivot_local);

        let mut scaled = start;
        assert!(apply_handle_drag(
            &mut scaled,
            start,
            bounds,
            Handle::BottomRight,
            Affine::from_transform(start).apply(Vec2::new(135.0, 82.0)),
            Some(pivot_local),
            ScaleDragModifiers {
                ignore_pivot: false,
                lock_aspect: false,
            },
        ));
        let after_scale = Affine::from_transform(scaled).apply(pivot_local);
        assert!((after_scale.x - pivot_world.x).abs() < 1.0e-4);
        assert!((after_scale.y - pivot_world.y).abs() < 1.0e-4);

        let start_cursor_local = Vec2::new(50.0, 0.0);
        let skewed = skew_placement_from_cursor(
            start,
            bounds,
            TransformEdge::Top,
            start_cursor_local,
            Affine::from_transform(start).apply(Vec2::new(75.0, 0.0)),
            Some(pivot_local),
        )
        .expect("placement skew");
        let after_skew = Affine::from_transform(skewed).apply(pivot_local);
        assert!((after_skew.x - pivot_world.x).abs() < 1.0e-4);
        assert!((after_skew.y - pivot_world.y).abs() < 1.0e-4);
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
            center,
            Vec2::new(center.x, center.y + 10.0),
            false,
            false,
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
    }

    #[test]
    fn selection_dot_tile_is_filtered_circle_not_single_texel_grid() {
        let side = SELECTION_DOT_TEXTURE_SIDE;
        let center = (side as f32 - 1.0) * 0.5;
        let radius = side as f32 * 0.16;
        let feather = 1.25_f32;
        let coverage = |x: usize, y: usize| {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            ((radius + feather - distance) / feather).clamp(0.0, 1.0)
        };
        let lit = (0..side)
            .flat_map(|y| (0..side).map(move |x| (x, y)))
            .filter(|&(x, y)| coverage(x, y) > 0.0)
            .count();
        let partial = (0..side)
            .flat_map(|y| (0..side).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let value = coverage(x, y);
                value > 0.0 && value < 1.0
            })
            .count();
        assert!(lit > 4, "dot tile regressed to a single-pixel grid");
        assert!(partial > 0, "dot edge needs filtered alpha coverage");
        assert!(coverage(side / 2, side / 2) > 0.99);
        assert_eq!(coverage(0, 0), 0.0);
    }

    #[test]
    fn geometric_selection_contour_cost_does_not_scale_with_screen_length() {
        let small = vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(100.0, 0.0),
            Pos2::new(100.0, 100.0),
            Pos2::new(0.0, 100.0),
        ];
        let huge = vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(100_000.0, 0.0),
            Pos2::new(100_000.0, 100_000.0),
            Pos2::new(0.0, 100_000.0),
        ];
        let small_outline = selection_contour_points(&small);
        let huge_outline = selection_contour_points(&huge);
        assert_eq!(small_outline.len(), huge_outline.len());
        assert_eq!(huge_outline.len(), 4);
    }

    #[test]
    fn selection_contour_zoom_out_preserves_thin_hairpin_topology_without_chords() {
        // A thin ribbon with a deep return bend. At 0.5% zoom the two sides are
        // only 0.02 screen pixels apart. The old display-space sanitizer treated
        // them as a hairpin and progressively removed turns, producing the long
        // diagonal chords seen in the selection overlay.
        let stage = [
            Pos2::new(0.0, 0.0),
            Pos2::new(100.0, 0.0),
            Pos2::new(100.0, 4.0),
            Pos2::new(70.0, 4.0),
            Pos2::new(70.0, 80.0),
            Pos2::new(30.0, 80.0),
            Pos2::new(30.0, 4.0),
            Pos2::new(0.0, 4.0),
        ];
        let screen: Vec<Pos2> = stage
            .iter()
            .map(|point| Pos2::new(point.x * 0.005, point.y * 0.005))
            .chain(std::iter::once(Pos2::new(0.0, 0.0)))
            .collect();
        let outline = selection_contour_points(&screen);

        assert_eq!(outline.len(), stage.len());
        for (actual, expected) in outline.iter().zip(stage.iter()) {
            assert!((actual.x - expected.x * 0.005).abs() <= f32::EPSILON);
            assert!((actual.y - expected.y * 0.005).abs() <= f32::EPSILON);
        }
        assert!(
            outline[1].distance(outline[2]) < 0.03,
            "subpixel-separated opposite sides must stay distinct instead of collapsing the turn",
        );

        #[cfg(feature = "appearance-mask-eraser")]
        {
            let path = VPath {
                anchors: stage
                    .iter()
                    .map(|point| anchor(Vec2::new(point.x, point.y)))
                    .collect(),
                closed: true,
            };
            let view = StageView {
                origin: Pos2::ZERO,
                scale: 0.005,
                stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0)),
            };
            let adaptive = crate::render::adaptive_selection_bezier_screen_path_for_test(
                &path,
                Affine::IDENTITY,
                &view,
            );
            assert_eq!(adaptive.len(), stage.len());
            for (actual, expected) in adaptive.iter().zip(stage.iter()) {
                assert!((actual.x - expected.x * 0.005).abs() <= f32::EPSILON);
                assert!((actual.y - expected.y * 0.005).abs() <= f32::EPSILON);
            }
            assert!(adaptive[1].distance(adaptive[2]) < 0.03);
        }
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn masked_raw_selection_reuses_exact_tessellated_mesh_until_view_changes() {
        let mut app = EditorApp::default();
        app.state.project = appearance_selection_project(true);
        app.session.current_q0rg_id = 1;
        app.session.current_frame = 0;
        app.session.selection = Selection::Paths(vec![PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        }]);
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(200.0, 200.0));
        let view = StageView {
            origin: Pos2::new(20.0, 20.0),
            scale: 2.0,
            stage_rect: rect,
        };
        for _ in 0..2 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new("selection-mesh-cache"),
                    ));
                    draw_selection_overlay(&mut app, &painter, &view);
                },
            );
        }
        assert_eq!(
            app.textures.selection_paint_build_count(),
            1,
            "stable masked raw selection rebuilt the same final screen mesh",
        );
        assert_eq!(
            app.textures.selection_stage_mesh_build_count(),
            1,
            "stable selection must build stage-space fill/stroke topology once",
        );

        let zoomed = StageView {
            origin: view.origin,
            scale: 3.0,
            stage_rect: rect,
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("selection-mesh-cache-zoom"),
                ));
                draw_selection_overlay(&mut app, &painter, &zoomed);
            },
        );
        assert_eq!(
            app.textures.selection_paint_build_count(),
            2,
            "screen-space selection mesh must rebuild when zoom changes",
        );
        assert_eq!(
            app.textures.selection_stage_mesh_build_count(),
            1,
            "zoom must transform cached stage topology instead of retessellating it",
        );

        app.textures.invalidate_asset(1);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("selection-mesh-cache-invalidated"),
                ));
                draw_selection_overlay(&mut app, &painter, &zoomed);
            },
        );
        assert_eq!(
            app.textures.selection_paint_build_count(),
            3,
            "asset mutation must evict cached raw selection meshes",
        );
        assert_eq!(
            app.textures.selection_stage_mesh_build_count(),
            2,
            "asset mutation must evict cached stage-space selection topology",
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn glow_selection_visual_point_count_is_independent_of_viewport_area() {
        let project = appearance_selection_project(false);
        let vector = match &project.assets[0] {
            Asset::Vector(vector) => vector,
            _ => unreachable!(),
        };
        let appearance = &project.asset_appearances[&1];
        let view_a = StageView {
            origin: Pos2::ZERO,
            scale: 1.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0)),
        };
        let view_b = StageView {
            origin: Pos2::new(1000.0, 500.0),
            scale: 12.0,
            stage_rect: egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(8000.0, 6000.0)),
        };
        let mut cache = TextureCache::default();
        let a = appearance_selection_body_contours(vector, appearance, None, &view_a, &mut cache);
        let b = appearance_selection_body_contours(vector, appearance, None, &view_b, &mut cache);
        assert_eq!(
            a.iter().map(Vec::len).collect::<Vec<_>>(),
            b.iter().map(Vec::len).collect::<Vec<_>>(),
            "selection visuals must transform geometry, not resample a screen-space pixel grid",
        );
    }

    #[test]
    fn valid_scale_preserves_reflection_and_repairs_degenerate_values() {
        assert_eq!(valid_scale(2.5), 2.5);
        assert_eq!(valid_scale(0.0), 0.01);
        assert_eq!(valid_scale(-0.001), -0.01);
        assert_eq!(valid_scale(-8.0), -8.0);
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(7),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        };
        assert_eq!(
            find_bucket_target(&project, 1, 1, 0, Vec2::new(10.0, 10.0)),
            Some(BucketTarget {
                layer_id: 1,
                placement_idx: 0,
                asset_id: 7,
                path_indices: vec![0],
            })
        );
        assert_eq!(
            find_bucket_target(&project, 1, 1, 0, Vec2::new(50.0, 50.0)),
            None
        );

        project.q0rgs[0].layers[0].placements[0].transform.tx = 40.0;
        assert_eq!(
            find_bucket_target(&project, 1, 1, 0, Vec2::new(50.0, 10.0)),
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
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
                Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => None,
            });
        let current_fill = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == current_asset)
            .and_then(|asset| match asset {
                Asset::Vector(vector) => vector.fill,
                Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => None,
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
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
                Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => None,
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
    fn bucket_curved_hole_has_no_angular_bites_against_visible_boundary() {
        let outer = VPath {
            anchors: vec![
                anchor(Vec2::new(0.0, 0.0)),
                anchor(Vec2::new(100.0, 0.0)),
                anchor(Vec2::new(100.0, 100.0)),
                anchor(Vec2::new(0.0, 100.0)),
            ],
            closed: true,
        };

        // The canonical raw anchors form a diamond, while the render handles
        // make the visible hole circular. Bucket geometry must use one visual
        // boundary consistently; mixing the two carves four angular bites out
        // of the newly painted region next to the contour.
        let k = 16.568_542_f32;
        let mut top = anchor(Vec2::new(50.0, 20.0));
        top.in_handle = Some(Vec2::new(50.0 + k, 20.0));
        top.out_handle = Some(Vec2::new(50.0 - k, 20.0));
        let mut left = anchor(Vec2::new(20.0, 50.0));
        left.in_handle = Some(Vec2::new(20.0, 50.0 - k));
        left.out_handle = Some(Vec2::new(20.0, 50.0 + k));
        let mut bottom = anchor(Vec2::new(50.0, 80.0));
        bottom.in_handle = Some(Vec2::new(50.0 - k, 80.0));
        bottom.out_handle = Some(Vec2::new(50.0 + k, 80.0));
        let mut right = anchor(Vec2::new(80.0, 50.0));
        right.in_handle = Some(Vec2::new(80.0, 50.0 + k));
        right.out_handle = Some(Vec2::new(80.0, 50.0 - k));
        let curved_hole = VPath {
            anchors: vec![top, left, bottom, right],
            closed: true,
        };

        let old = Rgba {
            r: 10,
            g: 10,
            b: 10,
            a: 255,
        };
        let new = Rgba {
            r: 230,
            g: 45,
            b: 70,
            a: 255,
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![outer, curved_hole],
            fill: Some(old),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];
        app.session.fill_color = Some(new);

        assert!(bucket_fill_at(&mut app, Vec2::new(50.0, 50.0)));
        let filled = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(new) => Some(vector),
                _ => None,
            })
            .expect("bucket-created fill");
        let surface = vector_fill_geometry(filled);
        for probe in [
            Point::new(65.0, 30.0),
            Point::new(70.0, 65.0),
            Point::new(35.0, 70.0),
            Point::new(30.0, 35.0),
        ] {
            assert!(
                surface.contains(&probe),
                "curved contour left an angular bucket bite at {probe:?}"
            );
        }
    }

    #[test]
    fn bucket_fills_region_formed_by_multiple_open_strokes() {
        let line = |a: Vec2, b: Vec2| VPath {
            anchors: vec![anchor(a), anchor(b)],
            closed: false,
        };
        let new = Rgba {
            r: 230,
            g: 35,
            b: 55,
            a: 255,
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![
                line(Vec2::new(20.0, 20.0), Vec2::new(80.0, 20.0)),
                line(Vec2::new(80.0, 20.0), Vec2::new(80.0, 80.0)),
                line(Vec2::new(80.0, 80.0), Vec2::new(20.0, 80.0)),
                line(Vec2::new(20.0, 80.0), Vec2::new(20.0, 20.0)),
            ],
            fill: None,
            stroke: Some(blue_stroke()),
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];
        app.session.fill_color = Some(new);

        assert!(
            bucket_fill_at(&mut app, Vec2::new(50.0, 50.0)),
            "four ordinary open stroke paths must form a fillable planar region"
        );
        let filled = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(new) => Some(vector),
                _ => None,
            })
            .expect("bucket-created fill");
        let surface = vector_fill_geometry(filled);
        assert!(surface.contains(&Point::new(50.0, 50.0)));
        assert!(!surface.contains(&Point::new(10.0, 10.0)));
    }

    #[test]
    fn bucket_fills_face_created_by_crossing_open_strokes() {
        let line = |a: Vec2, b: Vec2| VPath {
            anchors: vec![anchor(a), anchor(b)],
            closed: false,
        };
        let new = Rgba {
            r: 210,
            g: 45,
            b: 70,
            a: 255,
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![
                line(Vec2::new(0.0, 20.0), Vec2::new(100.0, 20.0)),
                line(Vec2::new(0.0, 80.0), Vec2::new(100.0, 80.0)),
                line(Vec2::new(20.0, 0.0), Vec2::new(20.0, 100.0)),
                line(Vec2::new(80.0, 0.0), Vec2::new(80.0, 100.0)),
            ],
            fill: None,
            stroke: Some(blue_stroke()),
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];
        app.session.fill_color = Some(new);

        assert!(bucket_fill_at(&mut app, Vec2::new(50.0, 50.0)));
        let filled = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(new) => Some(vector),
                _ => None,
            })
            .expect("central planar face");
        let surface = vector_fill_geometry(filled);
        assert!(surface.contains(&Point::new(50.0, 50.0)));
        assert!(
            !surface.contains(&Point::new(10.0, 50.0)),
            "bucket must stop at the intersection-created left boundary"
        );
    }

    #[test]
    fn bucket_only_edits_the_active_layer() {
        let square = |asset_id: u16, color: Rgba| {
            Asset::Vector(VectorAsset {
                asset_id,
                paths: vec![VPath {
                    anchors: vec![
                        anchor(Vec2::new(0.0, 0.0)),
                        anchor(Vec2::new(40.0, 0.0)),
                        anchor(Vec2::new(40.0, 40.0)),
                        anchor(Vec2::new(0.0, 40.0)),
                    ],
                    closed: true,
                }],
                fill: Some(color),
                stroke: None,
            })
        };
        let bottom = Rgba {
            r: 20,
            g: 30,
            b: 40,
            a: 255,
        };
        let top = Rgba {
            r: 70,
            g: 80,
            b: 90,
            a: 255,
        };
        let new = Rgba {
            r: 220,
            g: 50,
            b: 70,
            a: 255,
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![square(1, bottom), square(2, top)];
        app.state.project.q0rgs[0].layers = vec![
            Layer {
                layer_id: 1,
                name: "active".into(),
                explicit_keyframes: Vec::new(),
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(1),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
                }],
            },
            Layer {
                layer_id: 2,
                name: "other".into(),
                explicit_keyframes: Vec::new(),
                placements: vec![Placement {
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(2),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
                }],
            },
        ];
        app.session.current_layer_id = 1;
        app.session.fill_color = Some(new);

        assert!(bucket_fill_at(&mut app, Vec2::new(20.0, 20.0)));
        let fill_for = |asset_id| {
            app.state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == asset_id)
                .and_then(|asset| match asset {
                    Asset::Vector(vector) => vector.fill,
                    _ => None,
                })
        };
        assert_eq!(fill_for(1), Some(new));
        assert_eq!(
            fill_for(2),
            Some(top),
            "a visually higher raw layer must not steal a paint-bucket click from the active layer"
        );
    }
    #[test]
    fn bucket_recolour_preserves_bezier_geometry_exactly() {
        let old = Rgba {
            r: 25,
            g: 30,
            b: 35,
            a: 255,
        };
        let new = Rgba {
            r: 225,
            g: 45,
            b: 65,
            a: 255,
        };
        let mut a0 = anchor(Vec2::new(10.0, 20.0));
        a0.out_handle = Some(Vec2::new(35.0, -5.0));
        let mut a1 = anchor(Vec2::new(90.0, 20.0));
        a1.in_handle = Some(Vec2::new(65.0, -5.0));
        let curved = VPath {
            anchors: vec![
                a0,
                a1,
                anchor(Vec2::new(90.0, 90.0)),
                anchor(Vec2::new(10.0, 90.0)),
            ],
            closed: true,
        };
        let original = curved.clone();
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![curved],
            fill: Some(old),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];
        app.session.fill_color = Some(new);

        assert!(bucket_fill_at(&mut app, Vec2::new(50.0, 50.0)));
        let recoloured = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(new) => Some(vector),
                _ => None,
            })
            .expect("recoloured vector");
        assert_eq!(
            recoloured.paths,
            vec![original],
            "paint bucket must never polygonize an existing bezier fill just to change its colour"
        );
    }

    #[test]
    fn bucket_split_recolour_keeps_selected_outline() {
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
            r: 30,
            g: 40,
            b: 50,
            a: 255,
        };
        let new = Rgba {
            r: 220,
            g: 60,
            b: 80,
            a: 255,
        };
        let outline = blue_stroke();
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![square(0.0), square(100.0)],
            fill: Some(old),
            stroke: Some(outline),
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];
        app.session.fill_color = Some(new);

        assert!(bucket_fill_at(&mut app, Vec2::new(10.0, 10.0)));
        let recoloured = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(new) => Some(vector),
                _ => None,
            })
            .expect("split recoloured component");
        assert_eq!(
            recoloured.stroke.as_ref(),
            Some(&outline),
            "recolouring one component must not delete its original outline"
        );
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
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
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
            Vec2::new(10.0, 10.0),
            Vec2::new(40.0, 10.0),
            false,
            true,
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
                        },
                        Placement {
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(1),
                            transform: Transform2D {
                                tx: 80.0,
                                ..Transform2D::IDENTITY
                            },
                            tween: Tween::None,
                            fx: Default::default(),
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
            audio_clips: Vec::new(),
            runtime: Default::default(),
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
                                instance_id: 0,
                                frame: 0,
                                target: Target::Asset(1),
                                transform: Transform2D::IDENTITY,
                                tween: Tween::None,
                                fx: Default::default(),
                            },
                            Placement {
                                instance_id: 0,
                                frame: 0,
                                target: Target::Q0rg(2),
                                transform: Transform2D {
                                    tx: 30.0,
                                    ty: 0.0,
                                    ..Transform2D::IDENTITY
                                },
                                tween: Tween::None,
                                fx: Default::default(),
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
                            instance_id: 0,
                            frame: 0,
                            target: Target::Asset(2),
                            transform: Transform2D::IDENTITY,
                            tween: Tween::None,
                            fx: Default::default(),
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
        let affine = group_transform_affine(
            operation,
            center,
            Vec2::new(center.x, center.y + 40.0),
            false,
            false,
        )
        .expect("rotation affine");
        let mapped = affine.apply(center);
        assert!((mapped.x - center.x).abs() < 1.0e-4);
        assert!((mapped.y - center.y).abs() < 1.0e-4);
    }

    #[test]
    fn custom_transform_anchor_stays_fixed_during_scale_and_skew() {
        let bounds = (0.0, 0.0, 100.0, 50.0);
        let pivot = Vec2::new(27.0, 19.0);
        let scale = group_transform_affine(
            GroupTransformOperation::Scale {
                handle: Handle::BottomRight,
                start_bounds: bounds,
            },
            pivot,
            Vec2::new(130.0, 80.0),
            false,
            false,
        )
        .expect("scale affine");
        let scaled_pivot = scale.apply(pivot);
        assert!((scaled_pivot.x - pivot.x).abs() < 1.0e-4);
        assert!((scaled_pivot.y - pivot.y).abs() < 1.0e-4);

        let skew = group_transform_affine(
            GroupTransformOperation::Skew {
                edge: TransformEdge::Top,
                start_bounds: bounds,
                start_cursor: Vec2::new(50.0, 0.0),
            },
            pivot,
            Vec2::new(75.0, 0.0),
            false,
            false,
        )
        .expect("skew affine");
        let skewed_pivot = skew.apply(pivot);
        assert!((skewed_pivot.x - pivot.x).abs() < 1.0e-4);
        assert!((skewed_pivot.y - pivot.y).abs() < 1.0e-4);
    }

    #[test]
    fn ctrl_scale_ignores_custom_anchor_and_uses_opposite_corner() {
        let bounds = (0.0, 0.0, 100.0, 50.0);
        let pivot = Vec2::new(27.0, 19.0);
        let affine = group_transform_affine(
            GroupTransformOperation::Scale {
                handle: Handle::BottomRight,
                start_bounds: bounds,
            },
            pivot,
            Vec2::new(150.0, 90.0),
            false,
            true,
        )
        .expect("ctrl scale affine");
        let opposite = Vec2::new(0.0, 0.0);
        let fixed = affine.apply(opposite);
        assert!((fixed.x - opposite.x).abs() < 1.0e-4);
        assert!((fixed.y - opposite.y).abs() < 1.0e-4);
        let moved_pivot = affine.apply(pivot);
        assert!((moved_pivot.x - pivot.x).abs() > 0.1 || (moved_pivot.y - pivot.y).abs() > 0.1);
    }

    #[test]
    fn shift_corner_scale_is_uniform_and_crossing_anchor_reflects() {
        let bounds = (0.0, 0.0, 100.0, 50.0);
        let pivot = Vec2::new(50.0, 25.0);
        let uniform = group_transform_affine(
            GroupTransformOperation::Scale {
                handle: Handle::BottomRight,
                start_bounds: bounds,
            },
            pivot,
            Vec2::new(150.0, 45.0),
            true,
            false,
        )
        .expect("uniform scale");
        assert!((uniform.a11 - uniform.a22).abs() < 1.0e-6);

        let reflected = group_transform_affine(
            GroupTransformOperation::Scale {
                handle: Handle::MidRight,
                start_bounds: bounds,
            },
            pivot,
            Vec2::new(25.0, 25.0),
            false,
            false,
        )
        .expect("reflected scale");
        assert!(reflected.a11 < 0.0);
        let fixed = reflected.apply(pivot);
        assert!((fixed.x - pivot.x).abs() < 1.0e-4);
        assert!((fixed.y - pivot.y).abs() < 1.0e-4);
    }

    #[test]
    fn shift_move_and_rotation_snap_to_clean_axes_and_angles() {
        assert_eq!(
            constrain_move_delta(Vec2::new(40.0, 9.0), true),
            Vec2::new(40.0, 0.0)
        );
        assert_eq!(
            constrain_move_delta(Vec2::new(7.0, -30.0), true),
            Vec2::new(0.0, -30.0)
        );
        let snapped = snap_rotation_delta(0.61, true);
        assert!((snapped - std::f32::consts::FRAC_PI_4).abs() < 1.0e-6);
    }

    #[test]
    fn shift_click_combines_raw_and_object_selections_without_duplicates() {
        let object = Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 3,
        };
        let path = Selection::Path {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 2,
        };
        let mixed = add_selection_hit(&object, path.clone());
        let Selection::Mixed { paths, objects } = mixed else {
            panic!("shift-click must create a mixed selection")
        };
        assert_eq!(paths.len(), 1);
        assert_eq!(objects.len(), 1);

        let mixed = add_selection_hit(&Selection::Mixed { paths, objects }, path);
        let Selection::Mixed { paths, objects } = mixed else {
            panic!("selection stays mixed")
        };
        assert_eq!(paths.len(), 1);
        assert_eq!(objects.len(), 1);
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
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(2),
                transform: object_transform,
                tween: Tween::None,
                fx: Default::default(),
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

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn first_partial_raw_drag_reuses_warm_selected_mesh_after_split() {
        let square = |x: f32| VPath {
            anchors: vec![
                anchor(Vec2::new(x, 0.0)),
                anchor(Vec2::new(x + 20.0, 0.0)),
                anchor(Vec2::new(x + 20.0, 20.0)),
                anchor(Vec2::new(x, 20.0)),
            ],
            closed: true,
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![square(0.0), square(100.0)],
            fill: Some(Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];
        let source_ref = PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        };
        app.session.selection = Selection::Path {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        };
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(300.0, 200.0));
        let view = StageView {
            origin: Pos2::new(20.0, 20.0),
            scale: 1.0,
            stage_rect: rect,
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("warm-before-partial-drag"),
                ));
                crate::render::render_stage(
                    &painter,
                    &app.state.project,
                    1,
                    0,
                    &view,
                    &mut app.textures,
                    ctx,
                );
                draw_selection_overlay(&mut app, &painter, &view);
            },
        );
        let render_builds = app.textures.vector_render_geometry_build_count();
        let selection_builds = app.textures.selection_stage_mesh_build_count();
        assert_eq!(render_builds, 1);
        assert_eq!(selection_builds, 1);

        assert!(begin_dragging_raw_paths(
            &mut app,
            vec![source_ref],
            Vec2::new(10.0, 10.0),
            "test drag",
        ));
        let ToolState::DraggingPaths { refs, .. } = app.session.tool_state.clone() else {
            panic!("partial raw fill must enter live drag")
        };
        assert!(set_raw_refs_live_transform(
            &mut app.state.project,
            &refs,
            Affine {
                tx: 5.0,
                ty: 3.0,
                ..Affine::IDENTITY
            },
        ));
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("first-frame-after-partial-drag"),
                ));
                crate::render::render_stage(
                    &painter,
                    &app.state.project,
                    1,
                    0,
                    &view,
                    &mut app.textures,
                    ctx,
                );
                draw_selection_overlay(&mut app, &painter, &view);
            },
        );
        assert_eq!(
            app.textures.vector_render_geometry_build_count(),
            render_builds + 1,
            "only the stationary remainder may need a render rebuild; the moved split must inherit its warm mesh",
        );
        assert_eq!(
            app.textures.selection_stage_mesh_build_count(),
            selection_builds,
            "the moved split rebuilt a selection stage mesh that was already warm before mouse-down",
        );
    }

    #[test]
    fn raw_drag_moves_isolated_placement_then_bakes_once_without_moving_neighbor() {
        let square = |x: f32| VPath {
            anchors: vec![
                anchor(Vec2::new(x, 0.0)),
                anchor(Vec2::new(x + 20.0, 0.0)),
                anchor(Vec2::new(x + 20.0, 20.0)),
                anchor(Vec2::new(x, 20.0)),
            ],
            closed: true,
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![square(0.0), square(100.0)],
            fill: Some(Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];

        let source_ref = PathRef {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        };
        assert!(begin_dragging_raw_paths(
            &mut app,
            vec![source_ref],
            Vec2::new(10.0, 10.0),
            "test drag",
        ));
        let ToolState::DraggingPaths { refs, .. } = app.session.tool_state.clone() else {
            panic!("raw fill must enter path drag")
        };
        assert_eq!(refs.len(), 1);
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 2);

        let selected_before = raw_path_clone(
            &app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            refs[0].placement_idx,
            refs[0].path_idx,
        )
        .expect("isolated selected path");
        let neighbor_before = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.asset_id == 1 => Some(vector.clone()),
                _ => None,
            })
            .expect("stationary neighbor asset");
        let before_bounds = raw_path_refs_ui_bounds(&app.state.project, &refs).expect("bounds");

        let delta = Vec2::new(25.0, 7.0);
        assert!(set_raw_refs_live_transform(
            &mut app.state.project,
            &refs,
            Affine {
                tx: delta.x,
                ty: delta.y,
                ..Affine::IDENTITY
            },
        ));
        let selected_during = raw_path_clone(
            &app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            refs[0].placement_idx,
            refs[0].path_idx,
        )
        .expect("selected path during drag");
        assert_eq!(selected_during, selected_before);
        let during_bounds =
            raw_path_refs_ui_bounds(&app.state.project, &refs).expect("moved bounds");
        assert!((during_bounds.0 - (before_bounds.0 + delta.x)).abs() < 1.0e-4);
        assert!((during_bounds.1 - (before_bounds.1 + delta.y)).abs() < 1.0e-4);
        let neighbor_during = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.asset_id == 1 => Some(vector.clone()),
                _ => None,
            })
            .expect("neighbor during drag");
        assert_eq!(neighbor_during, neighbor_before);

        assert!(bake_raw_refs_live_transform(&mut app, &refs));
        let selected_after = raw_path_clone(
            &app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            refs[0].placement_idx,
            refs[0].path_idx,
        )
        .expect("selected path after release");
        assert!(
            (selected_after.anchors[0].point.x - (selected_before.anchors[0].point.x + delta.x))
                .abs()
                < 1.0e-4
        );
        assert!(
            (selected_after.anchors[0].point.y - (selected_before.anchors[0].point.y + delta.y))
                .abs()
                < 1.0e-4
        );
        let placement = PlacementRef {
            q0rg_id: refs[0].q0rg_id,
            layer_id: refs[0].layer_id,
            placement_idx: refs[0].placement_idx,
        };
        assert_eq!(
            placement_ref_transform(&app.state.project, placement),
            Some(Transform2D::IDENTITY)
        );
        let neighbor_after = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.asset_id == 1 => Some(vector.clone()),
                _ => None,
            })
            .expect("neighbor after drag");
        assert_eq!(neighbor_after, neighbor_before);
    }

    #[test]
    fn raw_drag_release_merge_keeps_cubic_boundary_handles() {
        let color = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        let circle = |center_x: f32| {
            let radius = 10.0;
            let k = radius * 0.552_284_8;
            VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(center_x, -radius),
                        in_handle: Some(Vec2::new(center_x - k, -radius)),
                        out_handle: Some(Vec2::new(center_x + k, -radius)),
                    },
                    Anchor {
                        point: Vec2::new(center_x + radius, 0.0),
                        in_handle: Some(Vec2::new(center_x + radius, -k)),
                        out_handle: Some(Vec2::new(center_x + radius, k)),
                    },
                    Anchor {
                        point: Vec2::new(center_x, radius),
                        in_handle: Some(Vec2::new(center_x + k, radius)),
                        out_handle: Some(Vec2::new(center_x - k, radius)),
                    },
                    Anchor {
                        point: Vec2::new(center_x - radius, 0.0),
                        in_handle: Some(Vec2::new(center_x - radius, k)),
                        out_handle: Some(Vec2::new(center_x - radius, -k)),
                    },
                ],
                closed: true,
            }
        };
        let original_left = circle(0.0);
        let right = circle(12.0);
        let mut app = EditorApp::default();
        app.state.project.assets = vec![
            Asset::Vector(VectorAsset {
                asset_id: 1,
                paths: vec![original_left.clone()],
                fill: Some(color),
                stroke: None,
            }),
            Asset::Vector(VectorAsset {
                asset_id: 2,
                paths: vec![right.clone()],
                fill: Some(color),
                stroke: None,
            }),
        ];
        app.state.project.q0rgs[0].layers[0].placements = vec![
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
        ];

        assert!(begin_dragging_raw_paths(
            &mut app,
            vec![PathRef {
                q0rg_id: 1,
                layer_id: 1,
                placement_idx: 0,
                path_idx: 0,
            }],
            Vec2::new(0.0, 0.0),
            "test curved drag",
        ));
        let finished_state = app.session.tool_state.clone();
        let edited_assets = raw_edit_assets(&app.state.project, &finished_state);
        let ToolState::DraggingPaths { refs, .. } = finished_state else {
            panic!("raw circle must enter path drag");
        };
        let delta = Vec2::new(2.0, 3.0);
        assert!(set_raw_refs_live_transform(
            &mut app.state.project,
            &refs,
            Affine {
                tx: delta.x,
                ty: delta.y,
                ..Affine::IDENTITY
            },
        ));
        assert!(bake_raw_refs_live_transform(&mut app, &refs));

        let mut merged = false;
        for ((q0rg_id, layer_id), asset_ids) in edited_assets {
            merged |= crate::brush::merge_touching_raw_fills_after_edit_focused(
                &mut app.state.project,
                q0rg_id,
                layer_id,
                0,
                &asset_ids,
            );
        }
        assert!(
            merged,
            "the moved circle still overlaps its same-style neighbour"
        );
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 1);
        assert_eq!(app.state.project.assets.len(), 1);
        let Asset::Vector(vector) = &app.state.project.assets[0] else {
            panic!("merged raw drag must remain vector");
        };
        let mut expected_left = original_left;
        for anchor in &mut expected_left.anchors {
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
        assert_eq!(vector.paths.len(), 2);
        assert!(vector.paths.contains(&expected_left));
        assert!(vector.paths.contains(&right));
        assert_eq!(
            vector
                .paths
                .iter()
                .flat_map(|path| &path.anchors)
                .filter(|anchor| anchor.in_handle.is_some() || anchor.out_handle.is_some())
                .count(),
            8,
            "raw drag release polygonized cubic boundaries",
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
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(2),
                transform: object_transform,
                tween: Tween::None,
                fx: Default::default(),
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
            start_appearances: _,
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
                objects: &objects,
                start_transforms: &start_transforms,
                operation,
                start_pivot,
            },
            Vec2::new(10.0, 5.0),
            false,
            false,
        ));
        let moved_pivot = selection_transform_pivot(&app).expect("moved pivot");
        assert!((moved_pivot.x - (start_pivot.x + 10.0)).abs() < 1.0e-4);
        assert!((moved_pivot.y - (start_pivot.y + 5.0)).abs() < 1.0e-4);
        let live_raw_transform = placement_ref_transform(
            &app.state.project,
            PlacementRef {
                q0rg_id: refs[0].q0rg_id,
                layer_id: refs[0].layer_id,
                placement_idx: refs[0].placement_idx,
            },
        )
        .expect("live raw placement");
        assert!((live_raw_transform.tx - 10.0).abs() < 1.0e-4);
        assert!((live_raw_transform.ty - 5.0).abs() < 1.0e-4);
        let live_path = raw_path_clone(
            &app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            refs[0].placement_idx,
            refs[0].path_idx,
        )
        .expect("live raw path");
        assert_eq!(
            live_path, start_paths[0],
            "live drag must transform the cached placement instead of rewriting raw anchors"
        );

        assert!(bake_raw_refs_live_transform(&mut app, &refs));
        let baked_transform = placement_ref_transform(
            &app.state.project,
            PlacementRef {
                q0rg_id: refs[0].q0rg_id,
                layer_id: refs[0].layer_id,
                placement_idx: refs[0].placement_idx,
            },
        )
        .expect("baked raw placement");
        assert_eq!(baked_transform, Transform2D::IDENTITY);
        let moved_path = raw_path_clone(
            &app.state.project,
            refs[0].q0rg_id,
            refs[0].layer_id,
            refs[0].placement_idx,
            refs[0].path_idx,
        )
        .expect("baked raw path");
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

    #[test]
    fn normal_select_hit_test_follows_rigged_display_object_body() {
        let square = Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![VPath {
                closed: true,
                anchors: vec![
                    Anchor {
                        point: Vec2::new(0.0, 0.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(10.0, 0.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(10.0, 10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(0.0, 10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
            }],
            fill: Some(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
            stroke: None,
        });
        let rig = Asset::Rig(q0s_format::v2::RigAsset {
            asset_id: 2,
            owner_q0rg_id: 1,
            nodes: vec![q0s_format::v2::RigNode {
                node_id: 1,
                name: "root".into(),
                parent: None,
                rest: Transform2D::IDENTITY,
                length: 10.0,
                binding: Some(q0s_format::v2::RigBinding {
                    instance_id: 1,
                    bind_offset: Affine::IDENTITY,
                }),
            }],
            controls: vec![q0s_format::v2::RigControl {
                control_id: 1,
                name: "move".into(),
                kind: q0s_format::v2::RigControlKind::Position2D,
                target_node: Some(1),
                rest_x: 40.0,
                rest_y: 10.0,
                rest_value: 0.0,
                min_value: -100.0,
                max_value: 100.0,
                public_in_simple: true,
            }],
            constraints: Vec::new(),
            channels: Vec::new(),
            drivers: Vec::new(),
            poses: Vec::new(),
            deformers: Vec::new(),
            mirror_pairs: Vec::new(),
            pose_drivers: Vec::new(),
            variants: Vec::new(),
        });
        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "rig select".into(),
                fps: 24,
                stage_width: 100,
                stage_height: 100,
                entry_q0rg_id: 1,
            },
            assets: vec![square, rig],
            asset_names: Default::default(),
            asset_appearances: Default::default(),
            layer_metadata: Default::default(),
            audio_clips: Vec::new(),
            runtime: Default::default(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "Stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "objects".into(),
                    explicit_keyframes: vec![0],
                    placements: vec![Placement {
                        instance_id: 1,
                        frame: 0,
                        target: Target::Asset(1),
                        transform: Transform2D {
                            tx: 1.0,
                            ..Transform2D::IDENTITY
                        },
                        tween: Tween::None,
                        fx: Default::default(),
                    }],
                }],
            }],
        };
        q0s_format::v2::validate(&project).unwrap();

        assert_eq!(
            hit_test_selectable_placement(&project, 1, 0, Vec2::new(45.0, 15.0)),
            Some((1, 0))
        );
        assert_eq!(
            hit_test_selectable_placement(&project, 1, 0, Vec2::new(5.0, 5.0)),
            None,
            "the old placement key must not leave a ghost hitbox behind"
        );
        assert!(
            drag_start_data(&project, 1, 1, 0, 0).is_none(),
            "ordinary free-transform is intentionally withheld while a rig owns the displayed matrix"
        );
    }
}
