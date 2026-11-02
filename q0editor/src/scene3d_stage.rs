//! 3D lights on the stage: bulbs, spots and suns drawn where the camera
//! sees them while the 3D window is open, dragged across the stage at the
//! depth they already have.

use egui::{Color32, Painter, Pos2, Response, Stroke};
use q0s_format::runtime_scene::RuntimeSceneState;
use q0s_format::three::scene::Light3dKind;
use q0s_format::three::Mat4;

use crate::app::EditorApp;
use crate::render::StageView;

/// Where each light and what it aims at show on the screen.
fn light_handles(app: &EditorApp, view: &StageView) -> Vec<(u16, Light3dKind, Pos2, Option<Pos2>)> {
    let project = &app.state.project;
    let q0rg_id = app.session.current_q0rg_id;
    let Some(setup) = project.runtime.scene3d.setup(q0rg_id) else {
        return Vec::new();
    };
    let frame = app.session.current_frame;
    let scene = RuntimeSceneState::new();
    let view3d = crate::tools3d::stage_view3d(app, view);
    setup
        .lights
        .iter()
        .filter_map(|light| {
            let key = light.animated(frame);
            let anchor = match light.follow {
                Some(instance) => q0s_format::raster::runtime_instance_matrix(
                    project, q0rg_id, frame, instance, &scene,
                )?,
                None => Mat4::IDENTITY,
            };
            let at = view3d.project(anchor.transform_point(key.position))?;
            let target = (light.kind != Light3dKind::Point)
                .then(|| view3d.project(anchor.transform_point(key.target)))
                .flatten()
                .map(|[x, y, _]| Pos2::new(x, y));
            Some((light.light_id, light.kind, Pos2::new(at[0], at[1]), target))
        })
        .collect()
}

pub fn paint_stage_overlay(app: &EditorApp, painter: &Painter, view: &StageView) {
    if !app.session.scene3d.open {
        return;
    }
    let selected = app.session.scene3d.selected_light;
    for (id, kind, at, target) in light_handles(app, view) {
        let chosen = selected == Some(id);
        let colour = if chosen {
            Color32::from_rgb(255, 200, 70)
        } else {
            Color32::from_rgb(250, 240, 200)
        };
        let outline = Stroke::new(
            if chosen { 2.0_f32 } else { 1.2_f32 },
            Color32::from_black_alpha(200),
        );
        if let Some(target) = target {
            painter.line_segment(
                [at, target],
                Stroke::new(1.0_f32, colour.gamma_multiply(0.8)),
            );
            painter.circle(
                target,
                4.0,
                Color32::TRANSPARENT,
                Stroke::new(1.5_f32, colour),
            );
        }
        match kind {
            Light3dKind::Sun => {
                for i in 0..8 {
                    let a = i as f32 * std::f32::consts::FRAC_PI_4;
                    let d = egui::vec2(a.cos(), a.sin());
                    painter
                        .line_segment([at + d * 9.0, at + d * 14.0], Stroke::new(1.5_f32, colour));
                }
                painter.circle(at, 7.0, colour, outline);
            }
            Light3dKind::Spot => {
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        at + egui::vec2(-7.0, -6.0),
                        at + egui::vec2(7.0, -6.0),
                        at + egui::vec2(0.0, 8.0),
                    ],
                    colour,
                    outline,
                ));
            }
            Light3dKind::Point => {
                painter.circle(at, 8.0, colour, outline);
                painter.circle_filled(at, 3.0, Color32::from_rgb(255, 160, 40));
            }
        }
    }
}

/// Drags a light, or its aim, keeping its depth. True when it took the
/// pointer.
pub fn handle_stage(app: &mut EditorApp, response: &Response, view: &StageView) -> bool {
    if !app.session.scene3d.open {
        app.session.scene3d.drag = None;
        return false;
    }
    let handles = light_handles(app, view);
    let near = |pos: Pos2| {
        handles
            .iter()
            .flat_map(|(id, _, at, target)| {
                std::iter::once((*id, false, at.distance(pos)))
                    .chain(target.map(|t| (*id, true, t.distance(pos))))
            })
            .filter(|(_, _, d)| *d <= 12.0)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(id, aim, _)| (id, aim))
    };
    if response.drag_started_by(egui::PointerButton::Primary) {
        let press = response
            .ctx
            .input(|i| i.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos());
        if let Some((id, aim)) = press.and_then(near) {
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
            app.session.scene3d.selected_light = Some(id);
            app.session.scene3d.drag = Some((id, aim));
            return true;
        }
        return false;
    }
    if response.clicked() {
        if let Some((id, _)) = response.interact_pointer_pos().and_then(near) {
            app.session.scene3d.selected_light = Some(id);
            return true;
        }
    }
    let Some((id, aim)) = app.session.scene3d.drag else {
        if response.hover_pos().and_then(near).is_some() {
            response.ctx.set_cursor_icon(egui::CursorIcon::Move);
        }
        return false;
    };
    if response.drag_stopped() || !response.dragged() {
        app.session.scene3d.drag = None;
        return true;
    }
    let Some(pointer) = response.interact_pointer_pos() else {
        return true;
    };
    let project = &app.state.project;
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let view3d = crate::tools3d::stage_view3d(app, view);
    let Some((origin, direction)) = view3d.ray(pointer.x, pointer.y) else {
        return true;
    };
    let (w, h) = (project.meta.stage_width, project.meta.stage_height);
    let Some(light) = project
        .runtime
        .scene3d
        .setup(q0rg_id)
        .and_then(|s| s.light(id))
        .cloned()
    else {
        return true;
    };
    let scene = RuntimeSceneState::new();
    let anchor = match light.follow {
        Some(instance) => {
            q0s_format::raster::runtime_instance_matrix(project, q0rg_id, frame, instance, &scene)
        }
        None => Some(Mat4::IDENTITY),
    };
    let Some(anchor) = anchor else {
        return true;
    };
    let key = light.animated(frame);
    let world = anchor.transform_point(if aim { key.target } else { key.position });
    if direction.z.abs() < 1.0e-6 {
        return true;
    }
    let t = (world.z - origin.z) / direction.z;
    let moved = origin + direction * t;
    let local = anchor
        .inverse()
        .unwrap_or(Mat4::IDENTITY)
        .transform_point(moved);
    let setup = app.state.project.runtime.scene3d.setup_mut(q0rg_id, w, h);
    if let Some(stored) = setup.light_mut(id) {
        stored.position = key.position;
        stored.target = key.target;
        stored.color = key.color;
        stored.intensity = key.intensity;
        stored.range = key.range;
        if aim {
            stored.target = local;
        } else {
            stored.position = local;
        }
        if !stored.keys.is_empty() {
            stored.set_key(frame);
        }
    }
    app.state.mark_dirty();
    app.textures.invalidate();
    true
}
