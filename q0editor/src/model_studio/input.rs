//! The viewport's mouse and keys: the camera, gizmo drags, and what each
//! tool does with a click or a drag.

use egui::{Key, Modifiers, PointerButton, Pos2, Rect, Response, Ui};
use q0s_format::three::draw::View3d;

use super::commands::{self, Command};
use super::gizmo::{self, Handle};
use super::{view, Constraint, Element, Hit, Mode, ModelStudio, OpKind, Tool};

/// Starts the change a gizmo handle stands for.
fn start_handle(studio: &mut ModelStudio, handle: Handle, press: Pos2) {
    let axes = studio.axes();
    let names = ["x", "y", "z"];
    let started = match handle {
        Handle::Free => view::begin(studio, OpKind::Grab, press, Constraint::Free, "free"),
        Handle::Axis(i) => view::begin(
            studio,
            OpKind::Grab,
            press,
            Constraint::Axis(axes[i]),
            names[i],
        ),
        Handle::Plane(i) => {
            let kind = if studio.tool == Tool::Scale {
                OpKind::Scale
            } else {
                OpKind::Grab
            };
            let label = ["yz plane", "xz plane", "xy plane"][i];
            view::begin(studio, kind, press, Constraint::Plane(axes[i]), label)
        }
        Handle::Ring(i) => view::begin(
            studio,
            OpKind::Rotate,
            press,
            Constraint::Axis(axes[i]),
            names[i],
        ),
        Handle::ViewRing => view::begin(studio, OpKind::Rotate, press, Constraint::Free, "view"),
        Handle::ScaleAxis(i) => view::begin(
            studio,
            OpKind::Scale,
            press,
            Constraint::Axis(axes[i]),
            names[i],
        ),
        Handle::ScaleAll => view::begin(studio, OpKind::Scale, press, Constraint::Free, "even"),
        Handle::Extrude => commands::extrude(studio, press, true),
    };
    if started {
        if let Some(op) = &mut studio.op {
            op.dragging = true;
        }
        studio.handle_drag = Some(handle);
    }
}

/// Is `hit` already part of the selection?
fn picked(studio: &ModelStudio, hit: Hit) -> bool {
    match hit {
        Hit::None => false,
        Hit::Part(m) => studio.objects.contains(&m),
        Hit::Vertex(v) => studio.verts.contains(&v),
        Hit::Edge(e) => studio.edges.contains(&e),
        Hit::Face(f) => studio.faces.contains(&f),
        Hit::Bone(b) => studio.bone == Some(b),
    }
}

/// Everything the viewport does with the pointer and keys this frame.
pub fn viewport(
    studio: &mut ModelStudio,
    ui: &mut Ui,
    response: &Response,
    view: &View3d,
    nav_busy: bool,
) {
    let ctx = ui.ctx().clone();
    let (modifiers, pointer, scroll, zoom) = ctx.input(|i| {
        (
            i.modifiers,
            i.pointer.clone(),
            i.raw_scroll_delta,
            i.zoom_delta(),
        )
    });
    let hovered = response.hovered() && !nav_busy;
    let mouse = pointer.hover_pos();
    let snapping = studio.settings.snap ^ (modifiers.ctrl || modifiers.command);

    // The camera: wheel and pinch zoom, right or middle drag to orbit,
    // with Shift to slide, Alt and the left button to orbit too.
    if hovered && scroll.y != 0.0 {
        studio.orbit.distance =
            (studio.orbit.distance * 0.9f32.powf(scroll.y / 40.0)).clamp(1.0e-4, 1.0e7);
    }
    if hovered && (zoom - 1.0).abs() > 1.0e-4 {
        studio.orbit.distance /= zoom;
    }
    let delta = pointer.delta();
    let side_drag =
        response.dragged_by(PointerButton::Secondary) || response.dragged_by(PointerButton::Middle);
    let alt_drag = response.dragged_by(PointerButton::Primary) && modifiers.alt;
    if side_drag && modifiers.shift {
        gizmo::pan(studio, view, delta);
    } else if side_drag || alt_drag {
        studio.orbit.yaw -= delta.x * 0.4;
        studio.orbit.pitch = (studio.orbit.pitch + delta.y * 0.4).clamp(-89.0, 89.0);
    }

    // A running change follows the mouse.
    if studio.op.is_some() {
        if let (Some(op), Some(mouse)) = (&mut studio.op, mouse) {
            if op.fresh && hovered {
                op.start_mouse = mouse;
                op.fresh = false;
            }
        }
        let fresh = studio.op.as_ref().is_some_and(|op| op.fresh);
        if let (Some(mouse), false) = (mouse, fresh) {
            view::update(studio, view, mouse, snapping);
        }
        let (enter, escape) =
            ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        let dragging = studio.op.as_ref().is_some_and(|op| op.dragging);
        if escape || (!dragging && pointer.button_pressed(PointerButton::Secondary)) {
            view::cancel(studio);
            studio.handle_drag = None;
        } else if dragging {
            if !pointer.primary_down() {
                view::confirm(studio);
                studio.handle_drag = None;
            }
        } else if enter || (hovered && pointer.primary_pressed()) {
            view::confirm(studio);
        }
        // X, Y or Z keep a key-started move to an axis; Shift with one keeps
        // it to the plane across that axis.
        if let Some(op) = &mut studio.op {
            if !op.dragging && !ctx.wants_keyboard_input() {
                let axes = studio_axes(&ctx, op);
                if let Some(constraint) = axes {
                    op.constraint = constraint.0;
                    op.label = constraint.1;
                }
            }
        }
        return;
    }

    // What the mouse is over: a gizmo handle first, then the model.
    let busy = pointer.any_down() && !response.drag_started();
    let gizmo = gizmo::layout(studio, view);
    if hovered && !busy {
        let at = mouse.unwrap_or_default();
        studio.handle_hover = gizmo.as_ref().and_then(|g| gizmo::hit(g, at));
        let big = studio.model.triangle_count() > 400_000;
        studio.hover = if studio.handle_hover.is_some() || big {
            Hit::None
        } else {
            match studio.tool {
                Tool::LoopCut | Tool::AddShape | Tool::DrawBone => Hit::None,
                Tool::Paint => match view::face_under(studio, view, at) {
                    Some((part, face)) if studio.mode == Mode::Edit => {
                        let _ = part;
                        Hit::Face(face)
                    }
                    Some((part, _)) => Hit::Part(part),
                    None => Hit::None,
                },
                _ => view::pick_at(studio, view, at),
            }
        };
        studio.loop_preview = (studio.tool == Tool::LoopCut && studio.mode == Mode::Edit)
            .then(|| view::nearest_edge(studio, view, at, 24.0))
            .flatten();
        if studio.handle_hover.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        } else if studio.tool == Tool::AddShape || studio.tool == Tool::DrawBone {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    } else if !response.hovered() && !response.dragged() {
        studio.hover = Hit::None;
        studio.handle_hover = None;
        studio.loop_preview = None;
    }

    // Pressing and dragging with the left button.
    if response.drag_started_by(PointerButton::Primary) && !modifiers.alt {
        let press = pointer.press_origin().unwrap_or_default();
        let at_press_handle = gizmo.as_ref().and_then(|g| gizmo::hit(g, press));
        if let Some(handle) = at_press_handle {
            start_handle(studio, handle, press);
            if let Some(mouse) = mouse {
                view::update(studio, view, mouse, snapping);
            }
            return;
        }
        match studio.tool {
            Tool::Move if studio.mode != Mode::Pose || studio.bone.is_some() => {
                // Grab what is under the pointer and drag it along.
                let hit = view::pick_at(studio, view, press);
                if hit != Hit::None {
                    if !picked(studio, hit) {
                        view::select(studio, hit, false);
                    }
                    if view::begin(studio, OpKind::Grab, press, Constraint::Free, "free") {
                        if let Some(op) = &mut studio.op {
                            op.dragging = true;
                        }
                        return;
                    }
                } else {
                    studio.box_select = Some((press, press));
                }
            }
            Tool::Inset if studio.mode == Mode::Edit => {
                if view::begin(studio, OpKind::Inset, press, Constraint::Free, "") {
                    if let Some(op) = &mut studio.op {
                        op.dragging = true;
                    }
                }
                return;
            }
            Tool::LoopCut if studio.mode == Mode::Edit => {
                if let Some((edge, _)) = view::nearest_edge(studio, view, press, 24.0) {
                    if view::begin_loop_cut(studio, edge, press) {
                        if let Some(mouse) = mouse {
                            view::update(studio, view, mouse, snapping);
                        }
                    }
                }
                return;
            }
            Tool::Paint => {
                studio.snapshot();
                studio.painting = true;
            }
            Tool::DrawBone if studio.mode == Mode::Pose => {
                let fallback = view::on_view_plane(studio, view, press, studio.orbit.target)
                    .unwrap_or(studio.orbit.target);
                studio.bone_start = Some(commands::bone_start(studio, fallback));
            }
            _ => studio.box_select = Some((press, press)),
        }
    }

    // A paint stroke paints whatever it passes over.
    if studio.painting {
        if let Some(at) = mouse {
            if let Some((part, face)) = view::face_under(studio, view, at) {
                commands::paint(studio, part, face);
            }
        }
        if !pointer.primary_down() {
            studio.painting = false;
        }
    }

    // A bone being drawn ends where the button is let go.
    if let Some((head, parent)) = studio.bone_start {
        if !pointer.primary_down() {
            studio.bone_start = None;
            if let Some(at) = mouse {
                if let Some(tail) = view::on_view_plane(studio, view, at, head) {
                    commands::add_bone(studio, head, tail, parent);
                }
            }
        }
    }

    // Box picking.
    if let (Some((start, _)), Some(at)) = (studio.box_select, mouse) {
        studio.box_select = Some((start, at));
        if !pointer.primary_down() {
            let area = Rect::from_two_pos(start, at);
            if area.width() > 3.0 || area.height() > 3.0 {
                view::box_select(studio, view, area, modifiers.shift);
            }
            studio.box_select = None;
        }
    }

    // Clicks.
    if response.clicked_by(PointerButton::Primary) && !modifiers.alt {
        let at = response.interact_pointer_pos().unwrap_or_default();
        match studio.tool {
            Tool::AddShape => {
                let point = view::place_point(studio, view, at);
                commands::add_shape_at(studio, point);
            }
            Tool::LoopCut if studio.mode == Mode::Edit => {
                if let Some((edge, _)) = view::nearest_edge(studio, view, at, 24.0) {
                    if view::begin_loop_cut(studio, edge, at) {
                        view::update(studio, view, at, snapping);
                        view::confirm(studio);
                    }
                }
            }
            Tool::Paint => {
                if let Some((part, face)) = view::face_under(studio, view, at) {
                    studio.snapshot();
                    commands::paint(studio, part, face);
                }
            }
            Tool::DrawBone => {
                let hit = view::pick_at(studio, view, at);
                view::select(studio, hit, false);
            }
            _ => {
                let hit = view::pick_at(studio, view, at);
                view::select(studio, hit, modifiers.shift);
            }
        }
    }

    if !ctx.wants_keyboard_input() && (hovered || response.has_focus()) {
        keys(
            studio,
            &ctx,
            modifiers,
            mouse.unwrap_or(studio.viewport.center()),
        );
    }
}

/// X, Y or Z pressed during a key-started change: the constraint they ask
/// for.
fn studio_axes(ctx: &egui::Context, op: &super::Op) -> Option<(Constraint, String)> {
    let shift = ctx.input(|i| i.modifiers.shift);
    for (key, index, name) in [(Key::X, 0usize, "x"), (Key::Y, 1, "y"), (Key::Z, 2, "z")] {
        if ctx.input(|i| i.key_pressed(key)) {
            let axis = [
                q0s_format::three::V3::X,
                q0s_format::three::V3::Y,
                q0s_format::three::V3::Z,
            ][index];
            let constraint = if shift {
                Constraint::Plane(axis)
            } else {
                Constraint::Axis(axis)
            };
            if op.constraint == constraint {
                return Some((Constraint::Free, "free".into()));
            }
            let label = if shift {
                format!("not {name}")
            } else {
                name.to_string()
            };
            return Some((constraint, label));
        }
    }
    None
}

/// The keyboard over the viewport.
fn keys(studio: &mut ModelStudio, ctx: &egui::Context, modifiers: Modifiers, mouse: Pos2) {
    let pressed = |key: Key| ctx.input(|i| i.key_pressed(key));
    let command = modifiers.ctrl || modifiers.command;
    if command {
        if pressed(Key::Z) {
            if modifiers.shift {
                studio.redo();
            } else {
                studio.undo();
            }
        } else if pressed(Key::Y) {
            studio.redo();
        } else if pressed(Key::Tab) {
            studio.set_mode(Mode::Pose);
        }
        return;
    }
    if pressed(Key::Tab) {
        let next = if studio.mode == Mode::Edit {
            Mode::Object
        } else {
            Mode::Edit
        };
        studio.set_mode(next);
        return;
    }
    if studio.mode == Mode::Edit {
        for (key, element) in [
            (Key::Num1, Element::Vertex),
            (Key::Num2, Element::Edge),
            (Key::Num3, Element::Face),
        ] {
            if pressed(key) {
                studio.element = element;
            }
        }
    }
    let tools = [
        (Key::Q, Tool::Select),
        (Key::W, Tool::Move),
        (Key::E, Tool::Rotate),
        (Key::R, Tool::Scale),
        (Key::T, Tool::Extrude),
        (Key::I, Tool::Inset),
        (Key::C, Tool::LoopCut),
        (Key::P, Tool::Paint),
        (Key::B, Tool::DrawBone),
    ];
    for (key, tool) in tools {
        if pressed(key) && !modifiers.shift {
            if tool == Tool::DrawBone && studio.mode != Mode::Pose {
                studio.set_mode(Mode::Pose);
            }
            if Tool::of(studio.mode).contains(&tool) {
                studio.tool = tool;
            }
        }
    }
    if modifiers.shift && pressed(Key::A) {
        if studio.mode == Mode::Pose {
            studio.set_mode(Mode::Object);
        }
        studio.tool = Tool::AddShape;
        return;
    }
    if pressed(Key::G) && view::begin(studio, OpKind::Grab, mouse, Constraint::Free, "free") {
        if let Some(op) = &mut studio.op {
            op.fresh = true;
        }
    }
    if pressed(Key::F) {
        view::frame_on(studio, !studio.has_selection());
    }
    if pressed(Key::Home) {
        view::frame_on(studio, true);
    }
    if pressed(Key::Delete) || pressed(Key::X) {
        match studio.mode {
            Mode::Object => commands::run(studio, Command::DeleteObjects),
            Mode::Edit => commands::run(studio, Command::Delete),
            Mode::Pose => commands::run(studio, Command::DeleteBone),
        }
    }
    if modifiers.shift && pressed(Key::D) {
        commands::run(studio, Command::Duplicate);
        if view::begin(studio, OpKind::Grab, mouse, Constraint::Free, "free") {
            if let Some(op) = &mut studio.op {
                op.fresh = true;
            }
        }
    }
    if pressed(Key::A) && !modifiers.shift {
        let command = if modifiers.alt {
            Command::SelectNone
        } else {
            Command::SelectAll
        };
        commands::run(studio, command);
    }
    if studio.mode == Mode::Edit {
        if pressed(Key::L) {
            commands::run(studio, Command::Linked);
        }
        if pressed(Key::M) {
            commands::run(studio, Command::Merge);
        }
    }
    if studio.mode == Mode::Pose && pressed(Key::K) {
        commands::run(studio, Command::KeyBone);
    }
    if pressed(Key::Space) {
        studio.playing = !studio.playing;
        studio.touch();
    }
}
