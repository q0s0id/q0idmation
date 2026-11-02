//! On-screen handles: the move, rotate, scale and extrude gizmos, bones
//! drawn as shapes, and the navigation ball in the viewport's corner.

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use q0s_format::three::draw::View3d;
use q0s_format::three::model::mesh_matrix;
use q0s_format::three::{Mat4, V3};

use super::{flip, Element, Hit, Mode, ModelStudio, Tool};

/// A part of a gizmo the mouse can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// Move in the view's plane.
    Free,
    Axis(usize),
    /// Slide in the plane at right angles to this axis.
    Plane(usize),
    Ring(usize),
    /// Turn about the view direction.
    ViewRing,
    ScaleAxis(usize),
    ScaleAll,
    Extrude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Move,
    Rotate,
    Scale,
    Extrude,
}

pub const AXIS_COLOURS: [Color32; 3] = [
    Color32::from_rgb(232, 72, 72),
    Color32::from_rgb(112, 204, 72),
    Color32::from_rgb(70, 134, 245),
];
const HOT: Color32 = Color32::from_rgb(255, 222, 80);
const VIEW_RING: f32 = 108.0;

pub struct Gizmo {
    pub kind: Kind,
    pub centre: Pos2,
    pub tips: [Option<Pos2>; 3],
    /// Ring points and whether each faces the camera.
    pub rings: [Vec<(Pos2, bool)>; 3],
    pub normal_tip: Option<Pos2>,
}

/// Where the current tool's gizmo stands, if it has one and something is
/// picked.
pub fn layout(studio: &ModelStudio, view: &View3d) -> Option<Gizmo> {
    let kind = match studio.tool {
        Tool::Move => Kind::Move,
        Tool::Rotate => Kind::Rotate,
        Tool::Scale if studio.mode != Mode::Pose => Kind::Scale,
        Tool::Extrude if studio.mode == Mode::Edit => Kind::Extrude,
        _ => return None,
    };
    if !studio.has_selection() {
        return None;
    }
    let pivot = studio.selection_centre()?;
    let centre = studio.screen(view, pivot)?;
    let depth = view.view.transform_point(flip(pivot)).z;
    let per_unit = view.stage_scale_at(depth).max(1.0e-6);
    let length = 92.0 / per_unit;
    let axes = studio.axes();
    let tips = axes.map(|a| studio.screen(view, pivot + a * length));
    let toward = -studio.orbit.forward();
    let rings = std::array::from_fn(|i| {
        let (u, v) = (axes[(i + 1) % 3], axes[(i + 2) % 3]);
        (0..=64)
            .filter_map(|k| {
                let a = k as f32 / 64.0 * std::f32::consts::TAU;
                let offset = (u * a.cos() + v * a.sin()) * (length * 0.82);
                let p = studio.screen(view, pivot + offset)?;
                Some((p, offset.dot(toward) >= -length * 0.05))
            })
            .collect()
    });
    let normal = if kind == Kind::Extrude {
        let mesh = studio.model.meshes.get(studio.mesh)?;
        let m = mesh_matrix(mesh, &studio.world());
        let faces = studio.working_faces();
        if faces.is_empty() {
            return None;
        }
        faces
            .iter()
            .filter_map(|f| mesh.faces.get(*f))
            .fold(V3::ZERO, |s, f| s + m.transform_vector(mesh.face_normal(f)))
            .normalized()
    } else {
        V3::Y
    };
    let normal_tip = studio.screen(view, pivot + normal * length * 1.1);
    Some(Gizmo {
        kind,
        centre,
        tips,
        rings,
        normal_tip,
    })
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    super::view::segment_distance(p, a, b)
}

/// The handle under `pos`.
pub fn hit(gizmo: &Gizmo, pos: Pos2) -> Option<Handle> {
    let c = gizmo.centre;
    match gizmo.kind {
        Kind::Move => {
            if pos.distance(c) < 11.0 {
                return Some(Handle::Free);
            }
            for k in 0..3 {
                let (a, b) = ((k + 1) % 3, (k + 2) % 3);
                if let (Some(ta), Some(tb)) = (gizmo.tips[a], gizmo.tips[b]) {
                    let square = c + (ta - c) * 0.32 + (tb - c) * 0.32;
                    if pos.distance(square) < 10.0 {
                        return Some(Handle::Plane(k));
                    }
                }
            }
            nearest_arrow(gizmo, pos).map(Handle::Axis)
        }
        Kind::Scale => {
            if pos.distance(c) < 12.0 {
                return Some(Handle::ScaleAll);
            }
            for k in 0..3 {
                let (a, b) = ((k + 1) % 3, (k + 2) % 3);
                if let (Some(ta), Some(tb)) = (gizmo.tips[a], gizmo.tips[b]) {
                    let square = c + (ta - c) * 0.32 + (tb - c) * 0.32;
                    if pos.distance(square) < 10.0 {
                        return Some(Handle::Plane(k));
                    }
                }
            }
            nearest_arrow(gizmo, pos).map(Handle::ScaleAxis)
        }
        Kind::Rotate => {
            let mut best: Option<(usize, f32)> = None;
            for (i, ring) in gizmo.rings.iter().enumerate() {
                for pair in ring.windows(2) {
                    let d = segment_distance(pos, pair[0].0, pair[1].0);
                    // Prefer the near half of each ring.
                    let d = if pair[0].1 { d } else { d + 3.0 };
                    if d < 8.0 && best.is_none_or(|(_, bd)| d < bd) {
                        best = Some((i, d));
                    }
                }
            }
            if let Some((i, _)) = best {
                return Some(Handle::Ring(i));
            }
            ((pos.distance(c) - VIEW_RING).abs() < 7.0).then_some(Handle::ViewRing)
        }
        Kind::Extrude => gizmo
            .normal_tip
            .filter(|tip| {
                segment_distance(pos, c, *tip) < 9.0
                    || pos.distance(*tip) < 14.0
                    || pos.distance(knob(c, *tip)) < 13.0
            })
            .map(|_| Handle::Extrude),
    }
}

/// The extrude gizmo's round "+" knob, a little past the arrow's tip.
fn knob(centre: Pos2, tip: Pos2) -> Pos2 {
    let dir = tip - centre;
    let dir = if dir.length() > 4.0 {
        dir.normalized()
    } else {
        Vec2::new(0.0, -1.0)
    };
    tip + dir * 18.0
}

fn nearest_arrow(gizmo: &Gizmo, pos: Pos2) -> Option<usize> {
    let c = gizmo.centre;
    let mut best: Option<(usize, f32)> = None;
    for (i, tip) in gizmo.tips.iter().enumerate() {
        let Some(tip) = tip else { continue };
        let dir = *tip - c;
        if dir.length() < 6.0 {
            continue;
        }
        let start = c + dir.normalized() * 14.0;
        let d = segment_distance(pos, start, *tip + dir.normalized() * 8.0);
        if d < 9.0 && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

fn arrow(painter: &Painter, from: Pos2, to: Pos2, colour: Color32, width: f32) {
    let dir = to - from;
    if dir.length() < 1.0 {
        return;
    }
    let n = dir.normalized();
    let side = Vec2::new(-n.y, n.x);
    painter.line_segment([from, to], Stroke::new(width, colour));
    painter.add(Shape::convex_polygon(
        vec![to + n * 13.0, to + side * 6.0, to - side * 6.0],
        colour,
        Stroke::new(1.0_f32, Color32::from_black_alpha(140)),
    ));
}

/// Draws the gizmo, the `hover`ed handle lit and the `active` one held.
pub fn paint(painter: &Painter, gizmo: &Gizmo, hover: Option<Handle>, active: Option<Handle>) {
    let c = gizmo.centre;
    let lit = |handle: Handle| hover == Some(handle) || active == Some(handle);
    let colour = |i: usize, handle: Handle| if lit(handle) { HOT } else { AXIS_COLOURS[i] };
    let shadow = Stroke::new(4.0_f32, Color32::from_black_alpha(70));
    match gizmo.kind {
        Kind::Move | Kind::Scale => {
            // Far arrows first.
            let mut order: Vec<usize> = (0..3).collect();
            order.sort_by(|a, b| {
                let da = gizmo.tips[*a].map_or(0.0, |t| (t - c).length());
                let db = gizmo.tips[*b].map_or(0.0, |t| (t - c).length());
                da.total_cmp(&db)
            });
            for (k, axis_colour) in AXIS_COLOURS.iter().enumerate() {
                let (a, b) = ((k + 1) % 3, (k + 2) % 3);
                if let (Some(ta), Some(tb)) = (gizmo.tips[a], gizmo.tips[b]) {
                    let square = c + (ta - c) * 0.32 + (tb - c) * 0.32;
                    let r = 7.0;
                    let fill = if lit(Handle::Plane(k)) {
                        HOT
                    } else {
                        axis_colour.gamma_multiply(0.7)
                    };
                    painter.rect(
                        Rect::from_center_size(square, Vec2::splat(r * 2.0)),
                        2.0,
                        fill,
                        Stroke::new(1.0_f32, Color32::from_black_alpha(160)),
                    );
                }
            }
            for i in order {
                let Some(tip) = gizmo.tips[i] else { continue };
                let dir = tip - c;
                if dir.length() < 4.0 {
                    continue;
                }
                let from = c + dir.normalized() * 12.0;
                let handle = if gizmo.kind == Kind::Move {
                    Handle::Axis(i)
                } else {
                    Handle::ScaleAxis(i)
                };
                painter.line_segment([from, tip], shadow);
                if gizmo.kind == Kind::Move {
                    arrow(painter, from, tip, colour(i, handle), 3.0);
                } else {
                    painter.line_segment([from, tip], Stroke::new(3.0_f32, colour(i, handle)));
                    painter.rect(
                        Rect::from_center_size(tip, Vec2::splat(13.0)),
                        2.0,
                        colour(i, handle),
                        Stroke::new(1.0_f32, Color32::from_black_alpha(160)),
                    );
                }
            }
            let free = if gizmo.kind == Kind::Move {
                Handle::Free
            } else {
                Handle::ScaleAll
            };
            painter.circle(
                c,
                8.0,
                if lit(free) {
                    HOT
                } else {
                    Color32::from_white_alpha(200)
                },
                Stroke::new(1.5_f32, Color32::from_black_alpha(180)),
            );
        }
        Kind::Rotate => {
            painter.circle_stroke(
                c,
                VIEW_RING,
                Stroke::new(
                    2.0_f32,
                    if lit(Handle::ViewRing) {
                        HOT
                    } else {
                        Color32::from_white_alpha(150)
                    },
                ),
            );
            for (i, ring) in gizmo.rings.iter().enumerate() {
                let col = colour(i, Handle::Ring(i));
                for pair in ring.windows(2) {
                    let front = pair[0].1 && pair[1].1;
                    let stroke = if front {
                        Stroke::new(3.0_f32, col)
                    } else {
                        Stroke::new(1.5_f32, col.gamma_multiply(0.35))
                    };
                    painter.line_segment([pair[0].0, pair[1].0], stroke);
                }
            }
            painter.circle_filled(c, 3.5, Color32::WHITE);
        }
        Kind::Extrude => {
            if let Some(tip) = gizmo.normal_tip {
                let col = if lit(Handle::Extrude) {
                    HOT
                } else {
                    Color32::from_rgb(255, 160, 60)
                };
                painter.line_segment([c, tip], shadow);
                arrow(painter, c, tip, col, 3.5);
                painter.circle_filled(knob(c, tip), 9.0, col);
                painter.text(
                    knob(c, tip),
                    Align2::CENTER_CENTER,
                    "+",
                    FontId::proportional(15.0),
                    Color32::BLACK,
                );
            }
        }
    }
}

/// Bones as tapered shapes from head to tail, over everything.
pub fn paint_bones(painter: &Painter, studio: &ModelStudio, view: &View3d, world: &[Mat4]) {
    if !(studio.show_bones || studio.mode == Mode::Pose) {
        return;
    }
    let size = studio.size();
    for (index, node) in studio.model.nodes.iter().enumerate() {
        if !node.bone {
            continue;
        }
        let head = world[index].translation_part();
        let tail = world[index].transform_point(V3::new(0.0, node.length.max(size * 0.01), 0.0));
        let (Some(a), Some(b)) = (studio.screen(view, head), studio.screen(view, tail)) else {
            continue;
        };
        let picked = studio.bone == Some(index as u16);
        let hovered = studio.hover == Hit::Bone(index as u16) && studio.mode == Mode::Pose;
        let (fill, line) = if picked {
            (
                Color32::from_rgba_unmultiplied(255, 160, 50, 170),
                Color32::from_rgb(255, 210, 120),
            )
        } else if hovered {
            (
                Color32::from_rgba_unmultiplied(255, 255, 255, 120),
                Color32::WHITE,
            )
        } else if studio.mode == Mode::Pose {
            (
                Color32::from_rgba_unmultiplied(90, 170, 230, 120),
                Color32::from_rgb(170, 220, 255),
            )
        } else {
            (
                Color32::from_rgba_unmultiplied(90, 170, 230, 60),
                Color32::from_rgba_unmultiplied(170, 220, 255, 150),
            )
        };
        let d = b - a;
        let len = d.length();
        if len < 2.0 {
            painter.circle(a, 4.0, fill, Stroke::new(1.0_f32, line));
            continue;
        }
        let n = d / len;
        let side = Vec2::new(-n.y, n.x) * (len * 0.12).clamp(3.0, 11.0);
        let waist = a + d * 0.18;
        painter.add(Shape::convex_polygon(
            vec![a, waist + side, b, waist - side],
            fill,
            Stroke::new(1.3_f32, line),
        ));
        painter.circle(a, 3.5, line, Stroke::NONE);
    }
}

/// The navigation ball and buttons in the viewport's top right corner.
/// Returns true while they have the pointer.
pub fn navigator(ui: &mut Ui, studio: &mut ModelStudio, view: &View3d, rect: Rect) -> bool {
    let radius = 44.0;
    let centre = Pos2::new(rect.right() - radius - 14.0, rect.top() + radius + 40.0);
    let ball = Rect::from_center_size(centre, Vec2::splat(radius * 2.0));
    let painter = ui.painter_at(rect);
    let response = ui.interact(
        ball,
        ui.id().with("studio_nav_ball"),
        Sense::click_and_drag(),
    );
    let hot = response.hovered() || response.dragged();
    painter.circle(
        centre,
        radius,
        Color32::from_black_alpha(if hot { 90 } else { 50 }),
        Stroke::new(1.0_f32, Color32::from_white_alpha(40)),
    );
    // Where each model axis points on screen, by the camera's turn alone.
    let mut ends: Vec<(f32, Pos2, usize, bool)> = Vec::new();
    for (i, axis) in [V3::X, V3::Y, V3::Z].into_iter().enumerate() {
        for positive in [true, false] {
            let a = if positive { axis } else { -axis };
            let v = view.view.transform_vector(flip(a));
            let p = centre + Vec2::new(v.x, v.y) * (radius - 12.0);
            ends.push((v.z, p, i, positive));
        }
    }
    // Far ends first so near ones sit on top.
    ends.sort_by(|a, b| b.0.total_cmp(&a.0));
    let pointer = response.hover_pos();
    let mut clicked_axis = None;
    for (_, p, i, positive) in &ends {
        let colour = AXIS_COLOURS[*i];
        if *positive {
            painter.line_segment([centre, *p], Stroke::new(2.0_f32, colour));
        }
        let near = pointer.is_some_and(|m| m.distance(*p) < 9.0);
        painter.circle(
            *p,
            if *positive { 8.0 } else { 6.0 },
            if *positive {
                colour
            } else {
                colour.gamma_multiply(if near { 0.8 } else { 0.35 })
            },
            Stroke::new(if near { 2.0_f32 } else { 0.0 }, Color32::WHITE),
        );
        if *positive {
            painter.text(
                *p,
                Align2::CENTER_CENTER,
                ["X", "Y", "Z"][*i],
                FontId::proportional(11.0),
                Color32::BLACK,
            );
        }
        if near && response.clicked() {
            clicked_axis = Some((*i, *positive));
        }
    }
    if let Some((axis, positive)) = clicked_axis {
        let (yaw, pitch) = match (axis, positive) {
            (0, true) => (90.0, 0.0),
            (0, false) => (-90.0, 0.0),
            (1, true) => (0.0, 89.0),
            (1, false) => (0.0, -89.0),
            (_, true) => (0.0, 0.0),
            (_, false) => (180.0, 0.0),
        };
        studio.orbit.yaw = yaw;
        studio.orbit.pitch = pitch;
    }
    if response.dragged() {
        let d = response.drag_delta();
        studio.orbit.yaw -= d.x * 0.5;
        studio.orbit.pitch = (studio.orbit.pitch + d.y * 0.5).clamp(-89.0, 89.0);
    }
    let response = response.on_hover_text("Drag to look around; click an axis to look along it");
    let mut used = response.hovered() || response.dragged();

    // Buttons under the ball.
    let size = 30.0;
    let mut y = centre.y + radius + 12.0;
    let buttons: [(&str, &str); 4] = [
        ("✥", "Drag to slide the view"),
        ("🔍", "Drag up and down to zoom"),
        ("⛶", "Fit the picked things (or everything) in view"),
        (
            if studio.orbit.ortho { "▭" } else { "◬" },
            "Switch between perspective and a flat view",
        ),
    ];
    for (index, (glyph, hint)) in buttons.into_iter().enumerate() {
        let r = Rect::from_center_size(Pos2::new(centre.x, y + size * 0.5), Vec2::splat(size));
        let b = ui.interact(
            r,
            ui.id().with(("studio_nav_button", index)),
            Sense::click_and_drag(),
        );
        let hot = b.hovered() || b.dragged();
        painter.circle(
            r.center(),
            size * 0.5,
            Color32::from_black_alpha(if hot { 110 } else { 60 }),
            Stroke::new(
                1.0_f32,
                Color32::from_white_alpha(if hot { 120 } else { 40 }),
            ),
        );
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(15.0),
            Color32::from_gray(230),
        );
        match index {
            0 if b.dragged() => {
                let d = b.drag_delta();
                pan(studio, view, d);
            }
            1 if b.dragged() => {
                let d = b.drag_delta();
                studio.orbit.distance =
                    (studio.orbit.distance * (1.0 + d.y * 0.01)).clamp(1.0e-4, 1.0e7);
            }
            2 if b.clicked() => {
                let everything = !studio.has_selection();
                super::view::frame_on(studio, everything);
            }
            3 if b.clicked() => studio.orbit.ortho = !studio.orbit.ortho,
            _ => {}
        }
        let b = b.on_hover_text(hint);
        used |= b.hovered() || b.dragged();
        y += size + 6.0;
    }
    used
}

/// Slides the camera by a screen-space drag.
pub fn pan(studio: &mut ModelStudio, view: &View3d, delta: Vec2) {
    let depth = view.view.transform_point(flip(studio.orbit.target)).z;
    let scale = view.stage_scale_at(depth).max(1.0e-6);
    let inverse = view.view.inverse().unwrap_or(Mat4::IDENTITY);
    let right = flip(inverse.transform_vector(V3::X));
    let down = flip(inverse.transform_vector(V3::Y));
    studio.orbit.target -= (right * delta.x + down * delta.y) / scale;
}

/// A short line saying what the mouse will do, for the corner.
pub fn hint(studio: &ModelStudio) -> String {
    if let Some(op) = &studio.op {
        let what = match op.kind {
            super::OpKind::Grab => "Moving",
            super::OpKind::Rotate => "Turning",
            super::OpKind::Scale => "Sizing",
            super::OpKind::Inset => "Inset",
            super::OpKind::LoopCut => "Loop cut",
        };
        let along = if op.label.is_empty() {
            String::new()
        } else {
            format!(" ({})", op.label)
        };
        let release = if op.dragging {
            "let go to finish"
        } else {
            "click to finish"
        };
        return format!(
            "{what}{along}: {}   ·   Ctrl snaps · {release} · Esc cancels",
            op.value
        );
    }
    let element = match studio.element {
        Element::Vertex => "points",
        Element::Edge => "edges",
        Element::Face => "faces",
    };
    match (studio.mode, studio.tool) {
        (Mode::Edit, Tool::Select) => format!("Click {element} to pick them, Shift adds, drag a box. 1 2 3 switch points/edges/faces."),
        (_, tool) => tool.hint().to_string(),
    }
}

/// Paints the dragged-out box of a box pick.
pub fn paint_box(painter: &Painter, area: Rect) {
    painter.rect(
        area,
        0.0,
        Color32::from_rgba_unmultiplied(255, 150, 40, 22),
        Stroke::new(1.0_f32, Color32::from_rgb(255, 150, 40)),
    );
}
