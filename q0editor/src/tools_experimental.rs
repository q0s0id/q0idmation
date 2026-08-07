use egui::epaint::PathShape;
use egui::{Color32, Context, Painter, PointerButton, Pos2, Response, Shape, Stroke};
use q0s_format::v2::Vec2;

use crate::app::EditorApp;
use crate::render::StageView;
use crate::state::{Tool, ToolState};

// Keep the established tools module API intact. Glob imports are deliberately
// weak in Rust name resolution, so the local `handle` below shadows only the
// legacy entry point while every other crate-visible helper remains available
// to app/brush/selection code unchanged.
pub(crate) use crate::tools_legacy::*;
pub use crate::tools_legacy::{
    brush_outline, draw_selection_overlay, hit_test_placement_pub, next_asset_id_pub,
    selection_at_point_pub,
};

pub fn handle(
    app: &mut EditorApp,
    response: &Response,
    painter: &Painter,
    view: &StageView,
    ctx: &Context,
) {
    // Hand/pan and folder guards are subtle and already battle-tested in the
    // classic tools entry point. Never duplicate those semantics here.
    let current_layer_is_folder = app
        .state
        .project
        .layer_is_folder(app.session.current_q0rg_id, app.session.current_layer_id);
    if app.session.viewport.hand_active
        || app.session.viewport.panning
        || (current_layer_is_folder
            && !matches!(
                app.session.current_tool,
                Tool::Select | Tool::Subselect | Tool::Hand
            ))
    {
        crate::tools_legacy::handle(app, response, painter, view, ctx);
        crate::appearance::render_registered_appearances(app, painter, view);
        return;
    }

    if app.session.current_tool != Tool::Eraser {
        let tag_brush_after = app.session.current_tool == Tool::Brush
            && (response.drag_stopped_by(PointerButton::Primary)
                || response.clicked_by(PointerButton::Primary));
        crate::tools_legacy::handle(app, response, painter, view, ctx);
        if tag_brush_after {
            crate::appearance::tag_current_brush_surface(app);
        }
        crate::appearance::render_registered_appearances(app, painter, view);
        return;
    }

    experimental_eraser(app, response, view, ctx, painter);
    crate::appearance::render_registered_appearances(app, painter, view);
    draw_experimental_eraser_cursor(app, painter, view, ctx);
}

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

fn experimental_eraser(
    app: &mut EditorApp,
    response: &Response,
    view: &StageView,
    ctx: &Context,
    painter: &Painter,
) {
    let settings = eraser_settings(app, view.scale);

    if response.drag_started_by(PointerButton::Primary) {
        if let Some(screen) = response.interact_pointer_pos() {
            let sample = crate::brush::BrushSample::mouse(screen_to_stage(screen, view));
            app.session.tool_state = ToolState::EraserDrawing {
                stroke: crate::brush::brush_begin(settings, sample),
            };
            app.session.status = "Eraser: visible appearance".to_string();
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
        // Finish a clone first. If no visible raw appearance was hit, leave the
        // original ToolState intact and hand the exact gesture to the legacy
        // handler so open strokes and display objects retain their old fallback.
        let region = match &app.session.tool_state {
            ToolState::EraserDrawing { stroke } => {
                Some(crate::brush::brush_finish(stroke.clone(), settings))
            }
            _ => None,
        };
        if let Some(region) = region {
            if crate::appearance::erase_visible_region(app, region) {
                app.session.tool_state = ToolState::Idle;
                return;
            }
        }
        crate::tools_legacy::handle(app, response, painter, view, ctx);
        return;
    }

    // A click without a drag is one nib dab, just like the classic eraser.
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
            if crate::appearance::erase_visible_region(app, region) {
                return;
            }
        }
        crate::tools_legacy::handle(app, response, painter, view, ctx);
    }
}

fn screen_to_stage(pos: Pos2, view: &StageView) -> Vec2 {
    Vec2::new(
        (pos.x - view.origin.x) / view.scale,
        (pos.y - view.origin.y) / view.scale,
    )
}

fn draw_experimental_eraser_cursor(
    app: &EditorApp,
    painter: &Painter,
    view: &StageView,
    ctx: &Context,
) {
    let Some(center) = ctx.pointer_hover_pos() else {
        return;
    };
    if !painter.clip_rect().contains(center) {
        return;
    }
    ctx.set_cursor_icon(egui::CursorIcon::None);
    let settings = eraser_settings(app, view.scale);
    let points: Vec<Pos2> = crate::brush::nib_outline(
        settings.nib,
        settings.size * view.scale + 2.0,
        Vec2::new(center.x, center.y),
    )
    .into_iter()
    .map(|point| Pos2::new(point.x, point.y))
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
