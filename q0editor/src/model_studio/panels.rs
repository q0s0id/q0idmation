//! The studio's screen: the top bar, the tool settings bar, tools and
//! actions on the left, the viewport, the outliner, transform, materials
//! and model settings on the right, and the animation bar along the bottom.

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::{
    Align2, Color32, Context, FontId, Painter, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2,
};
use q0model::ops::{Axis, Projection};
use q0model::prim::Primitive;
use q0s_format::three::model::{mesh_matrix, Clip, Outline, Shading};
use q0s_format::three::{Quat, V3};
use q0s_format::v2::Rgba;

use super::commands::{self, label_of, Command};
use super::gizmo;
use super::view::{self, Palette};
use super::{Element, Mode, ModelStudio, Orientation, Tool};
use crate::app::EditorApp;

fn color32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

fn edit_color(ui: &mut Ui, value: &mut Rgba) -> bool {
    let mut color = color32(*value);
    let changed = egui::color_picker::color_edit_button_srgba(
        ui,
        &mut color,
        egui::color_picker::Alpha::OnlyBlend,
    )
    .changed();
    if changed {
        let [r, g, b, a] = color.to_srgba_unmultiplied();
        *value = Rgba { r, g, b, a };
    }
    changed
}

/// Little pictures for tools, modes and kinds of element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Icon {
    Tool(Tool),
    Mode(Mode),
    Element(Element),
}

fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, colour: Color32) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.42;
    let s = Stroke::new(1.6_f32, colour);
    let p = |x: f32, y: f32| c + Vec2::new(x * r, y * r);
    let cube = |painter: &Painter, fill: bool| {
        let (top, left, right, mid) = (p(0.0, -0.95), p(-0.85, -0.45), p(0.85, -0.45), p(0.0, 0.0));
        let (low_l, low_r, bottom) = (p(-0.85, 0.5), p(0.85, 0.5), p(0.0, 0.98));
        if fill {
            painter.add(Shape::convex_polygon(
                vec![top, right, mid, left],
                colour.gamma_multiply(0.45),
                Stroke::NONE,
            ));
        }
        for (a, b) in [
            (top, right),
            (right, low_r),
            (low_r, bottom),
            (bottom, low_l),
            (low_l, left),
            (left, top),
            (left, mid),
            (mid, right),
            (mid, bottom),
        ] {
            painter.line_segment([a, b], s);
        }
    };
    match icon {
        Icon::Tool(Tool::Select) => {
            painter.add(Shape::convex_polygon(
                vec![p(-0.6, -0.95), p(0.55, 0.15), p(0.0, 0.2), p(-0.6, 0.9)],
                colour,
                Stroke::NONE,
            ));
            painter.line_segment([p(0.0, 0.2), p(0.45, 0.95)], Stroke::new(2.2_f32, colour));
        }
        Icon::Tool(Tool::Move) => {
            painter.line_segment([p(-0.95, 0.0), p(0.95, 0.0)], s);
            painter.line_segment([p(0.0, -0.95), p(0.0, 0.95)], s);
            for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let tip = p(dx, dy);
                let side = Vec2::new(-dy, dx) * r * 0.28;
                let back = Vec2::new(-dx, -dy) * r * 0.32;
                painter.add(Shape::convex_polygon(
                    vec![tip, tip + back + side, tip + back - side],
                    colour,
                    Stroke::NONE,
                ));
            }
        }
        Icon::Tool(Tool::Rotate) => {
            let points: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let a = std::f32::consts::PI * (0.15 + 1.6 * i as f32 / 24.0);
                    p(a.cos() * 0.85, a.sin() * 0.85)
                })
                .collect();
            painter.add(Shape::line(points.clone(), s));
            let end = *points.last().unwrap();
            painter.add(Shape::convex_polygon(
                vec![
                    end + Vec2::new(r * 0.35, -r * 0.05),
                    end + Vec2::new(-r * 0.1, -r * 0.4),
                    end + Vec2::new(-r * 0.15, r * 0.25),
                ],
                colour,
                Stroke::NONE,
            ));
        }
        Icon::Tool(Tool::Scale) => {
            painter.rect_stroke(Rect::from_min_max(p(-0.9, -0.2), p(0.2, 0.9)), 1.0, s);
            painter.line_segment([p(-0.1, 0.1), p(0.85, -0.85)], s);
            painter.add(Shape::convex_polygon(
                vec![p(0.95, -0.95), p(0.35, -0.85), p(0.85, -0.35)],
                colour,
                Stroke::NONE,
            ));
        }
        Icon::Tool(Tool::AddShape) => {
            cube(painter, false);
            painter.circle_filled(p(0.65, 0.65), r * 0.42, colour);
            painter.text(
                p(0.65, 0.63),
                Align2::CENTER_CENTER,
                "+",
                FontId::proportional(r * 0.9),
                Color32::BLACK,
            );
        }
        Icon::Tool(Tool::Extrude) => {
            painter.add(Shape::convex_polygon(
                vec![p(-0.9, 0.4), p(0.1, 0.9), p(0.9, 0.4), p(-0.1, -0.1)],
                colour.gamma_multiply(0.5),
                s,
            ));
            painter.line_segment([p(0.0, 0.4), p(0.0, -0.75)], Stroke::new(2.0_f32, colour));
            painter.add(Shape::convex_polygon(
                vec![p(0.0, -1.0), p(-0.3, -0.6), p(0.3, -0.6)],
                colour,
                Stroke::NONE,
            ));
        }
        Icon::Tool(Tool::Inset) => {
            painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(r * 1.8)), 1.0, s);
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(r * 0.9)), 1.0, colour);
        }
        Icon::Tool(Tool::LoopCut) => {
            painter.rect_stroke(
                Rect::from_center_size(c, Vec2::new(r * 1.9, r * 1.4)),
                1.0,
                s,
            );
            painter.add(Shape::dashed_line(
                &[p(0.0, -1.0), p(0.0, 1.0)],
                Stroke::new(2.0_f32, colour),
                3.0,
                2.0,
            ));
        }
        Icon::Tool(Tool::Paint) => {
            painter.add(Shape::convex_polygon(
                vec![p(-0.1, -0.9), p(0.6, -0.2), p(0.0, 0.4), p(-0.7, -0.3)],
                colour.gamma_multiply(0.5),
                s,
            ));
            painter.circle_filled(p(0.55, 0.65), r * 0.3, colour);
        }
        Icon::Tool(Tool::DrawBone) | Icon::Mode(Mode::Pose) => {
            let (a, b) = (p(-0.75, 0.75), p(0.75, -0.75));
            let d = b - a;
            let side = Vec2::new(-d.y, d.x).normalized() * r * 0.32;
            let waist = a + d * 0.22;
            painter.add(Shape::convex_polygon(
                vec![a, waist + side, b, waist - side],
                colour.gamma_multiply(0.6),
                s,
            ));
            painter.circle_filled(a, r * 0.18, colour);
        }
        Icon::Mode(Mode::Object) => cube(painter, true),
        Icon::Mode(Mode::Edit) | Icon::Element(Element::Vertex) => {
            let pts = [p(-0.8, 0.7), p(0.0, -0.8), p(0.8, 0.7)];
            painter.add(Shape::closed_line(
                pts.to_vec(),
                Stroke::new(1.2_f32, colour.gamma_multiply(0.6)),
            ));
            for q in pts {
                painter.circle_filled(q, r * 0.22, colour);
            }
        }
        Icon::Element(Element::Edge) => {
            let pts = [p(-0.8, 0.7), p(0.0, -0.8), p(0.8, 0.7)];
            painter.add(Shape::closed_line(
                pts.to_vec(),
                Stroke::new(1.0_f32, colour.gamma_multiply(0.5)),
            ));
            painter.line_segment([pts[0], pts[1]], Stroke::new(2.6_f32, colour));
        }
        Icon::Element(Element::Face) => {
            let pts = vec![p(-0.8, 0.7), p(0.0, -0.8), p(0.8, 0.7)];
            painter.add(Shape::convex_polygon(
                pts,
                colour.gamma_multiply(0.7),
                Stroke::new(1.2_f32, colour),
            ));
        }
    }
}

/// The editor's accent, as the studio paints with it.
fn accent(ui: &Ui) -> Color32 {
    ui.ctx()
        .data(|d| d.get_temp::<Color32>(egui::Id::new("studio_accent")))
        .unwrap_or(ui.visuals().selection.bg_fill)
}

/// A key's name in a small rounded box, like a keycap.
fn key_chip(painter: &Painter, right_center: Pos2, key: &str, colour: Color32) -> Rect {
    let galley = painter.layout_no_wrap(key.to_owned(), FontId::monospace(10.5), colour);
    let rect = Rect::from_min_size(
        right_center - Vec2::new(galley.size().x + 10.0, galley.size().y * 0.5 + 2.0),
        galley.size() + Vec2::new(10.0, 4.0),
    );
    painter.rect(
        rect,
        4.0,
        Color32::from_white_alpha(10),
        Stroke::new(1.0_f32, colour.gamma_multiply(0.35)),
    );
    painter.galley(rect.min + Vec2::new(5.0, 2.0), galley, colour);
    rect
}

/// A full-width row with a picture, a name and its key.
fn tool_button(ui: &mut Ui, icon: Icon, label: &str, key: &str, selected: bool) -> egui::Response {
    let height = 32.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let accent = accent(ui);
    let visuals = ui.visuals();
    let text = if selected {
        Color32::WHITE
    } else {
        visuals.text_color()
    };
    let painter = ui.painter_at(rect.expand(1.0));
    if selected {
        painter.rect_filled(rect, 7.0, accent.gamma_multiply(0.55));
        painter.rect_filled(
            Rect::from_min_size(
                rect.min + Vec2::new(0.0, 7.0),
                Vec2::new(3.0, height - 14.0),
            ),
            1.5,
            accent,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, 7.0, Color32::from_white_alpha(12));
    }
    paint_icon(
        &painter,
        Rect::from_min_size(rect.min + Vec2::new(9.0, 6.0), Vec2::splat(20.0)),
        icon,
        text,
    );
    painter.text(
        rect.min + Vec2::new(38.0, height * 0.5),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        text,
    );
    if !key.is_empty() {
        key_chip(
            &painter,
            rect.right_center() - Vec2::new(8.0, 0.0),
            key,
            text.gamma_multiply(0.7),
        );
    }
    response
}

/// A tile with a picture over its name, one of a row that picks one.
fn tile(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    width: f32,
    hint: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 50.0), Sense::click());
    let accent = accent(ui);
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect.expand(1.0));
    let (fill, stroke) = if selected {
        (accent.gamma_multiply(0.5), Stroke::new(1.0_f32, accent))
    } else if response.hovered() {
        (
            Color32::from_white_alpha(14),
            Stroke::new(1.0_f32, Color32::from_white_alpha(20)),
        )
    } else {
        (
            Color32::from_white_alpha(6),
            Stroke::new(1.0_f32, Color32::from_white_alpha(12)),
        )
    };
    painter.rect(rect, 8.0, fill, stroke);
    let colour = if selected {
        Color32::WHITE
    } else {
        visuals.text_color()
    };
    paint_icon(
        &painter,
        Rect::from_center_size(rect.center() - Vec2::new(0.0, 7.0), Vec2::splat(22.0)),
        icon,
        colour,
    );
    painter.text(
        rect.center_bottom() - Vec2::new(0.0, 6.0),
        Align2::CENTER_BOTTOM,
        label,
        FontId::proportional(11.0),
        colour.gamma_multiply(if selected { 1.0 } else { 0.8 }),
    );
    response.on_hover_text(hint)
}

/// A small square button with only a picture.
fn icon_button(
    ui: &mut Ui,
    enabled: bool,
    hint: &str,
    paint: impl Fn(&Painter, Rect, Color32),
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::splat(28.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let colour = if enabled {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color().gamma_multiply(0.6)
    };
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, 7.0, Color32::from_white_alpha(16));
    }
    paint(ui.painter(), rect.shrink(7.0), colour);
    response.on_hover_text(hint)
}

/// A curved arrow: its head on the left for undo, on the right (`forward`)
/// for redo.
fn paint_turn_arrow(painter: &Painter, rect: Rect, colour: Color32, forward: bool) {
    let c = rect.center() + Vec2::new(0.0, rect.height() * 0.12);
    let r = rect.width() * 0.42;
    let sign = if forward { -1.0 } else { 1.0 };
    let points: Vec<Pos2> = (0..=16)
        .map(|i| {
            let a = std::f32::consts::PI * (1.05 + 0.95 * i as f32 / 16.0);
            c + Vec2::new(a.cos() * r * sign, a.sin() * r)
        })
        .collect();
    painter.add(Shape::line(points.clone(), Stroke::new(1.8_f32, colour)));
    let tip = points[0];
    painter.add(Shape::convex_polygon(
        vec![
            tip + Vec2::new(-4.0 * sign, -1.0),
            tip + Vec2::new(3.0 * sign, -4.5),
            tip + Vec2::new(2.5 * sign, 3.5),
        ],
        colour,
        Stroke::NONE,
    ));
}

/// A rounded pill that is on or off; neighbours make a segmented row.
fn pill(ui: &mut Ui, on: bool, text: &str) -> egui::Response {
    let accent = accent(ui);
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), FontId::proportional(13.0), Color32::WHITE);
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + Vec2::new(18.0, 10.0), Sense::click());
    let visuals = ui.visuals();
    let fill = if on {
        accent.gamma_multiply(0.6)
    } else if response.hovered() {
        Color32::from_white_alpha(16)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 7.0, fill);
    let colour = if on {
        Color32::WHITE
    } else {
        visuals.text_color()
    };
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, colour);
    response
}

/// Pills grouped on one rounded track.
fn segmented<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(Color32::from_black_alpha(60))
        .stroke(Stroke::new(1.0_f32, Color32::from_white_alpha(14)))
        .rounding(9.0)
        .inner_margin(egui::Margin::same(2.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(add).inner
        })
        .inner
}

/// A filled button in the accent: the thing to press.
fn primary_button(ui: &mut Ui, text: &str) -> egui::Response {
    let accent = accent(ui);
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), FontId::proportional(13.5), Color32::WHITE);
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + Vec2::new(22.0, 12.0), Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        accent.gamma_multiply(0.8)
    } else if response.hovered() {
        lighten(accent, 0.12)
    } else {
        accent
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, Color32::WHITE);
    response
}

/// An outlined button for the things beside the main one.
fn ghost_button(ui: &mut Ui, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        FontId::proportional(13.5),
        ui.visuals().text_color(),
    );
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + Vec2::new(22.0, 12.0), Sense::click());
    let fill = if response.hovered() {
        Color32::from_white_alpha(14)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect(
        rect,
        8.0,
        fill,
        Stroke::new(1.0_f32, Color32::from_white_alpha(40)),
    );
    ui.painter().galley(
        rect.center() - galley.size() * 0.5,
        galley,
        ui.visuals().text_color(),
    );
    response
}

fn lighten(c: Color32, k: f32) -> Color32 {
    let mix = |v: u8| (f32::from(v) + (255.0 - f32::from(v)) * k).round() as u8;
    Color32::from_rgb(mix(c.r()), mix(c.g()), mix(c.b()))
}

/// An action in the side column: even width, its name in the middle.
fn action_button(ui: &mut Ui, label: &str, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        Color32::from_white_alpha(26)
    } else if response.hovered() {
        Color32::from_white_alpha(18)
    } else {
        Color32::from_white_alpha(8)
    };
    ui.painter().rect(
        rect,
        6.0,
        fill,
        Stroke::new(1.0_f32, Color32::from_white_alpha(12)),
    );
    let galley = ui.painter().layout(
        label.to_owned(),
        FontId::proportional(12.5),
        ui.visuals().text_color(),
        width - 8.0,
    );
    ui.painter().galley(
        rect.center() - galley.size() * 0.5,
        galley,
        ui.visuals().text_color(),
    );
    response
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(
        RichText::new(text)
            .size(10.5)
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(3.0);
}

/// A section of the side column on a card of its own, open or folded.
fn card(ui: &mut Ui, title: &str, open_by_default: bool, body: impl FnOnce(&mut Ui)) {
    let id = ui.make_persistent_id(("studio_card", title));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        open_by_default,
    );
    egui::Frame::none()
        .fill(Color32::from_white_alpha(6))
        .stroke(Stroke::new(1.0_f32, Color32::from_white_alpha(12)))
        .rounding(10.0)
        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::click());
            let open = state.is_open();
            let colour = ui.visuals().strong_text_color();
            ui.painter().text(
                rect.left_center(),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional(14.0),
                colour,
            );
            let c = rect.right_center() - Vec2::new(6.0, 0.0);
            let chevron = if open {
                vec![
                    c + Vec2::new(-4.0, -2.0),
                    c + Vec2::new(4.0, -2.0),
                    c + Vec2::new(0.0, 3.0),
                ]
            } else {
                vec![
                    c + Vec2::new(-2.0, -4.0),
                    c + Vec2::new(-2.0, 4.0),
                    c + Vec2::new(3.0, 0.0),
                ]
            };
            ui.painter().add(Shape::convex_polygon(
                chevron,
                colour.gamma_multiply(0.6),
                Stroke::NONE,
            ));
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                state.toggle(ui);
            }
            if state.is_open() {
                ui.add_space(6.0);
                body(ui);
            }
        });
    state.store(ui.ctx());
    ui.add_space(8.0);
}

/// An open or shut eye.
fn paint_eye(painter: &Painter, rect: Rect, colour: Color32, open: bool) {
    let c = rect.center();
    let w = rect.width() * 0.5;
    let lid: Vec<Pos2> = (0..=12)
        .map(|i| {
            let t = i as f32 / 12.0 * std::f32::consts::PI;
            c + Vec2::new(-t.cos() * w, -t.sin() * w * 0.55)
        })
        .collect();
    painter.add(Shape::line(lid, Stroke::new(1.4_f32, colour)));
    if open {
        let low: Vec<Pos2> = (0..=12)
            .map(|i| {
                let t = i as f32 / 12.0 * std::f32::consts::PI;
                c + Vec2::new(-t.cos() * w, t.sin() * w * 0.55)
            })
            .collect();
        painter.add(Shape::line(low, Stroke::new(1.4_f32, colour)));
        painter.circle_filled(c, w * 0.32, colour);
    } else {
        for k in [-0.6, 0.0, 0.6] {
            let at = c + Vec2::new(k * w, w * 0.15);
            painter.line_segment(
                [at, at + Vec2::new(k * 2.0, 3.0)],
                Stroke::new(1.2_f32, colour),
            );
        }
    }
}

/// The whole studio screen.
pub fn render(app: &mut EditorApp, ctx: &Context) {
    let Some(mut studio) = app.model_studio.take() else {
        return;
    };
    let mut close = false;
    let mut save = false;
    let fps = app.state.project.meta.fps.max(1);

    ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::new("studio_accent"),
            app.settings.theme.accent.to_color32(),
        )
    });
    egui::TopBottomPanel::top("model_studio_top").show(ctx, |ui| {
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // The model's badge and name.
            let (badge, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
            ui.painter()
                .rect_filled(badge, 8.0, accent(ui).gamma_multiply(0.45));
            paint_icon(
                ui.painter(),
                badge.shrink(6.0),
                Icon::Mode(Mode::Object),
                Color32::WHITE,
            );
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(
                    RichText::new("MODEL")
                        .size(9.5)
                        .strong()
                        .color(ui.visuals().weak_text_color()),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut studio.name)
                        .desired_width(170.0)
                        .frame(false)
                        .font(FontId::proportional(15.0))
                        .hint_text("name"),
                );
            });
            if studio.changed {
                ui.label(RichText::new("● unsaved").small().color(accent(ui)));
            }
            ui.add_space(8.0);
            if icon_button(ui, studio.can_undo(), "Undo (Ctrl Z)", |p, r, c| {
                paint_turn_arrow(p, r, c, false)
            })
            .clicked()
            {
                studio.undo();
            }
            if icon_button(ui, studio.can_redo(), "Redo (Ctrl Y)", |p, r, c| {
                paint_turn_arrow(p, r, c, true)
            })
            .clicked()
            {
                studio.redo();
            }
            ui.add_space(8.0);
            segmented(ui, |ui| {
                if pill(ui, studio.wire, "Wireframe")
                    .on_hover_text("Lines over the surfaces")
                    .clicked()
                {
                    studio.wire = !studio.wire;
                }
                if pill(ui, studio.xray, "See through")
                    .on_hover_text("Show and pick things hidden behind others")
                    .clicked()
                {
                    studio.xray = !studio.xray;
                }
                if pill(ui, studio.show_bones, "Bones").clicked() {
                    studio.show_bones = !studio.show_bones;
                }
                if pill(ui, studio.show_grid, "Floor").clicked() {
                    studio.show_grid = !studio.show_grid;
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ghost_button(ui, "Close").clicked() {
                    if studio.changed {
                        studio.show_help = false;
                        studio.status = String::new();
                        studio.confirm_close = true;
                    } else {
                        close = true;
                    }
                }
                if primary_button(ui, "Save to library")
                    .on_hover_text("Every placement of this model updates")
                    .clicked()
                {
                    save = true;
                }
                let help = icon_button(ui, true, "Mouse, keys and tools", |p, r, c| {
                    p.circle_stroke(r.center(), r.width() * 0.6, Stroke::new(1.4_f32, c));
                    p.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        "?",
                        FontId::proportional(12.0),
                        c,
                    );
                });
                if help.clicked() {
                    studio.show_help = !studio.show_help;
                }
            });
        });
        ui.add_space(5.0);
    });

    egui::TopBottomPanel::top("model_studio_settings")
        .frame(
            egui::Frame::side_top_panel(&ctx.style())
                .fill(ctx.style().visuals.panel_fill.gamma_multiply(0.92)),
        )
        .show(ctx, |ui| {
            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| tool_settings(&mut studio, ui));
            ui.add_space(3.0);
        });

    egui::SidePanel::left("model_studio_tools")
        .resizable(false)
        .exact_width(196.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| toolbar(&mut studio, ui));
        });
    egui::SidePanel::right("model_studio_side")
        .resizable(true)
        .default_width(300.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| side(&mut studio, ui));
        });
    egui::TopBottomPanel::bottom("model_studio_anim")
        .resizable(false)
        .show(ctx, |ui| {
            animation_bar(&mut studio, ui, fps);
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show(ctx, |ui| viewport(&mut studio, ui, fps));

    if studio.show_help {
        help(&mut studio, ctx);
    }
    if studio.confirm_close {
        egui::Window::new("Save the model?")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("This model has changes that are not in the library yet.");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if primary_button(ui, "Save and close").clicked() {
                        save = true;
                        close = true;
                    }
                    if ghost_button(ui, "Close without saving").clicked() {
                        close = true;
                    }
                    if ghost_button(ui, "Keep editing").clicked() {
                        studio.confirm_close = false;
                    }
                });
            });
    }

    let wants_texture = std::mem::take(&mut studio.wants_texture);
    app.model_studio = Some(studio);
    if wants_texture {
        app.queue(crate::app::Action::StudioPickTexture);
    }
    if save {
        super::apply(app);
    }
    if close {
        app.model_studio = None;
        app.textures.invalidate();
    }
}

fn tool_settings(studio: &mut ModelStudio, ui: &mut Ui) {
    // The tool's name on a chip, its settings after it.
    let accent = accent(ui);
    egui::Frame::none()
        .fill(accent.gamma_multiply(0.35))
        .rounding(8.0)
        .inner_margin(egui::Margin::symmetric(8.0, 3.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                paint_icon(ui.painter(), rect, Icon::Tool(studio.tool), Color32::WHITE);
                ui.label(
                    RichText::new(studio.tool.label())
                        .strong()
                        .color(Color32::WHITE),
                );
            });
        });
    ui.add_space(6.0);
    match studio.tool {
        Tool::Move | Tool::Rotate | Tool::Scale | Tool::Extrude => {
            ui.label(RichText::new("Axes").weak());
            segmented(ui, |ui| {
                if pill(
                    ui,
                    studio.settings.orientation == Orientation::Global,
                    "Model",
                )
                .clicked()
                {
                    studio.settings.orientation = Orientation::Global;
                }
                if pill(ui, studio.settings.orientation == Orientation::Local, "Own")
                    .on_hover_text("The picked part's or bone's own axes")
                    .clicked()
                {
                    studio.settings.orientation = Orientation::Local;
                }
            });
            ui.add_space(6.0);
            ui.checkbox(&mut studio.settings.snap, "Snap")
                .on_hover_text("Hold Ctrl to switch snapping for one drag");
            if studio.tool == Tool::Rotate {
                egui::ComboBox::from_id_source("studio_angle_step")
                    .width(60.0)
                    .selected_text(format!("{}°", studio.settings.angle_step))
                    .show_ui(ui, |ui| {
                        for step in [1.0, 5.0, 10.0, 15.0, 30.0, 45.0, 90.0] {
                            ui.selectable_value(
                                &mut studio.settings.angle_step,
                                step,
                                format!("{step}°"),
                            );
                        }
                    });
            } else {
                ui.label(RichText::new(format!("step {}", trim(studio.grid_step()))).weak());
            }
        }
        Tool::AddShape => {
            segmented(ui, |ui| {
                for primitive in Primitive::ALL {
                    if pill(ui, studio.settings.shape == primitive, primitive.label()).clicked() {
                        studio.settings.shape = primitive;
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Size").weak());
            let mut size = studio.shape_size();
            let speed = f64::from(size) * 0.02;
            if ui
                .add(
                    egui::DragValue::new(&mut size)
                        .speed(speed)
                        .clamp_range(1.0e-4..=1.0e6)
                        .max_decimals(3),
                )
                .changed()
            {
                studio.settings.shape_size = Some(size);
            }
            let mut fit = studio.settings.shape_size.is_none();
            if ui
                .checkbox(&mut fit, "Fit to view")
                .on_hover_text("Size new shapes to what the camera shows")
                .changed()
            {
                studio.settings.shape_size = (!fit).then_some(size);
            }
            if !matches!(studio.settings.shape, Primitive::Cube | Primitive::Plane) {
                ui.label(RichText::new("Detail").weak());
                ui.add(egui::Slider::new(&mut studio.settings.detail, 3..=48));
            }
        }
        Tool::Inset | Tool::LoopCut => {
            ui.checkbox(&mut studio.settings.snap, "Snap");
        }
        Tool::Paint => {
            ui.label("Colour with");
            material_swatches(studio, ui, 22.0);
        }
        Tool::DrawBone => {
            ui.label(RichText::new("Bones grow from the picked bone's tip. Click a bone to grow from it, or empty space to start a new chain.").weak());
        }
        Tool::Select => {
            ui.label(
                RichText::new(
                    "Shift adds to the pick. Drag a box. See through picks hidden things too.",
                )
                .weak(),
            );
        }
    }
    if studio.mode == Mode::Pose {
        ui.separator();
        ui.checkbox(&mut studio.rest_edit, "Edit skeleton")
            .on_hover_text(
            "On: move bones in the rest pose. Off: turning bones keys the clip at the playhead.",
        );
    }
}

fn trim(value: f32) -> String {
    let text = format!("{value:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn toolbar(studio: &mut ModelStudio, ui: &mut Ui) {
    heading(ui, "WORK ON");
    let width = (ui.available_width() - 8.0) / 3.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (mode, hint) in [
            (Mode::Object, "Parts: whole meshes (Tab)"),
            (Mode::Edit, "Shape: points, edges and faces (Tab)"),
            (Mode::Pose, "Bones: skeleton and animation (Ctrl Tab)"),
        ] {
            if tile(
                ui,
                Icon::Mode(mode),
                mode.label(),
                studio.mode == mode,
                width,
                hint,
            )
            .clicked()
            {
                studio.set_mode(mode);
            }
        }
    });
    if studio.mode == Mode::Edit {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (element, label, hint) in [
                (Element::Vertex, "Points", "Points (1)"),
                (Element::Edge, "Edges", "Edges (2)"),
                (Element::Face, "Faces", "Faces (3)"),
            ] {
                if tile(
                    ui,
                    Icon::Element(element),
                    label,
                    studio.element == element,
                    width,
                    hint,
                )
                .clicked()
                {
                    studio.element = element;
                }
            }
        });
        if let Some(mesh) = studio.model.meshes.get(studio.mesh) {
            ui.label(
                RichText::new(format!("shaping {}", mesh.name))
                    .small()
                    .weak(),
            );
        }
    }
    heading(ui, "TOOLS");
    ui.spacing_mut().item_spacing.y = 2.0;
    for tool in Tool::of(studio.mode) {
        if tool_button(
            ui,
            Icon::Tool(*tool),
            tool.label(),
            tool.key(),
            studio.tool == *tool,
        )
        .on_hover_text(tool.hint())
        .clicked()
        {
            studio.tool = *tool;
        }
    }
    heading(ui, "ACTIONS");
    let groups: Vec<(&str, Vec<Command>)> = match studio.mode {
        Mode::Object => vec![
            (
                "",
                vec![Command::Duplicate, Command::DeleteObjects, Command::Join],
            ),
            ("Shading", vec![Command::ShadeSmooth, Command::ShadeFlat]),
        ],
        Mode::Edit => vec![
            (
                "",
                vec![
                    Command::Fill,
                    Command::Merge,
                    Command::Delete,
                    Command::Duplicate,
                    Command::Weld,
                ],
            ),
            (
                "Detail",
                vec![Command::Subdivide, Command::Smooth, Command::Triangulate],
            ),
            (
                "Mirror",
                vec![
                    Command::Mirror(Axis::X),
                    Command::Mirror(Axis::Y),
                    Command::Mirror(Axis::Z),
                ],
            ),
            (
                "Faces",
                vec![
                    Command::ShadeSmooth,
                    Command::ShadeFlat,
                    Command::Flip,
                    Command::FixNormals,
                ],
            ),
            (
                "Texture positions",
                vec![
                    Command::Uvs(Projection::Box),
                    Command::Uvs(Projection::Front),
                    Command::Uvs(Projection::Cylinder),
                ],
            ),
            (
                "Pick",
                vec![Command::SelectAll, Command::SelectNone, Command::Linked],
            ),
        ],
        Mode::Pose => vec![
            (
                "Skeleton",
                vec![
                    Command::AddBone,
                    Command::DeleteBone,
                    Command::Bind,
                    Command::Unbind,
                ],
            ),
            (
                "Keys",
                vec![Command::KeyBone, Command::KeyAll, Command::DeleteKey],
            ),
        ],
    };
    let half = (ui.available_width() - 4.0) / 2.0;
    for (title, commands) in groups {
        if !title.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(title).small().weak());
        }
        for pair in commands.chunks(2) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for command in pair {
                    let width = if pair.len() == 1 {
                        half * 2.0 + 4.0
                    } else {
                        half
                    };
                    let label = label_of(*command);
                    let (name, key) = match label.split_once(" (") {
                        Some((name, key)) => {
                            (name.to_string(), key.trim_end_matches(')').to_string())
                        }
                        None => (label.to_string(), String::new()),
                    };
                    let response = action_button(ui, &name, width);
                    let response = if key.is_empty() {
                        response
                    } else {
                        response.on_hover_text(key)
                    };
                    if response.clicked() {
                        commands::run(studio, *command);
                    }
                }
            });
            ui.add_space(2.0);
        }
    }
    if studio.mode == Mode::Pose {
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Bind gives every part weights from the nearest bones, so they bend with them.",
            )
            .small()
            .weak(),
        );
    }
}

fn material_swatches(studio: &mut ModelStudio, ui: &mut Ui, size: f32) {
    let mut picked = None;
    for (index, material) in studio.model.materials.iter().enumerate() {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
        let selected = studio.material == index;
        let r = size * 0.5 - 2.0;
        if selected {
            ui.painter().circle_stroke(
                rect.center(),
                r + 1.5,
                Stroke::new(2.0_f32, Color32::WHITE),
            );
        } else if response.hovered() {
            ui.painter().circle_stroke(
                rect.center(),
                r + 1.5,
                Stroke::new(1.5_f32, Color32::from_white_alpha(90)),
            );
        }
        let mut colour = color32(material.color);
        if material.shading == Shading::Glass {
            // Glass shows as a glint over its tint.
            colour = Color32::from_rgba_unmultiplied(
                material.color.r,
                material.color.g,
                material.color.b,
                160,
            );
        }
        ui.painter().circle(
            rect.center(),
            r - 1.0,
            colour,
            Stroke::new(1.0_f32, Color32::from_black_alpha(150)),
        );
        if material.shading == Shading::Glass {
            ui.painter().line_segment(
                [
                    rect.center() + Vec2::new(-r * 0.45, -r * 0.1),
                    rect.center() + Vec2::new(-r * 0.1, -r * 0.45),
                ],
                Stroke::new(1.5_f32, Color32::WHITE),
            );
        }
        if material.texture.is_some() {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "▦",
                FontId::proportional(size * 0.55),
                Color32::from_black_alpha(160),
            );
        }
        if response.on_hover_text(&material.name).clicked() {
            picked = Some(index);
        }
    }
    if let Some(index) = picked {
        studio.material = index;
    }
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let colour = if response.hovered() {
        Color32::WHITE
    } else {
        ui.visuals().text_color().gamma_multiply(0.7)
    };
    ui.painter().circle_stroke(
        rect.center(),
        size * 0.5 - 3.0,
        Stroke::new(1.2_f32, colour),
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "+",
        FontId::proportional(size * 0.6),
        colour,
    );
    if response.on_hover_text("New material").clicked() {
        new_material(studio);
    }
}

fn new_material(studio: &mut ModelStudio) {
    studio.snapshot();
    let mut copy = studio
        .model
        .materials
        .get(studio.material)
        .cloned()
        .unwrap_or_default();
    copy.name = format!("Material {}", studio.model.materials.len() + 1);
    // A fresh colour, so the new material is easy to tell apart.
    let hue = studio.model.materials.len() as f32 * 0.61803;
    let [r, g, b] = hsv(hue.fract(), 0.55, 0.9);
    copy.color = Rgba { r, g, b, a: 255 };
    copy.texture = None;
    studio.model.materials.push(copy);
    studio.material = studio.model.materials.len() - 1;
    studio.touch();
}

fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

/// A three-number row; returns whether it changed and whether an edit began.
fn v3_row(ui: &mut Ui, label: &str, value: &mut V3, speed: f64, suffix: &str) -> (bool, bool) {
    let mut changed = false;
    let mut began = false;
    ui.label(label);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for ((v, colour), axis) in [
            (&mut value.x, gizmo::AXIS_COLOURS[0]),
            (&mut value.y, gizmo::AXIS_COLOURS[1]),
            (&mut value.z, gizmo::AXIS_COLOURS[2]),
        ]
        .into_iter()
        .zip(["X", "Y", "Z"])
        {
            ui.label(RichText::new(axis).size(11.0).strong().color(colour));
            let response = ui.add_sized(
                [54.0, 20.0],
                egui::DragValue::new(v)
                    .speed(speed)
                    .max_decimals(2)
                    .suffix(suffix),
            );
            began |= response.drag_started() || response.gained_focus();
            changed |= response.changed();
        }
    });
    ui.end_row();
    (changed, began)
}

fn side(studio: &mut ModelStudio, ui: &mut Ui) {
    ui.add_space(8.0);
    card(ui, "Outliner", true, |ui| outliner(studio, ui));
    card(ui, "Transform", true, |ui| transform(studio, ui));
    card(ui, "Material", true, |ui| material(studio, ui));
    card(ui, "On the stage", false, |ui| stage_settings(studio, ui));
}

/// One row of the outliner: an eye, a picture, a name; picked rows lit.
fn outliner_row(
    ui: &mut Ui,
    name: &str,
    picked: bool,
    hidden: Option<bool>,
    bone: bool,
    depth: usize,
) -> (egui::Response, bool) {
    let accent = accent(ui);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
    let painter = ui.painter_at(rect.expand(1.0));
    if picked {
        painter.rect_filled(rect, 6.0, accent.gamma_multiply(0.5));
    } else if response.hovered() {
        painter.rect_filled(rect, 6.0, Color32::from_white_alpha(12));
    }
    let text = if picked {
        Color32::WHITE
    } else {
        ui.visuals().text_color()
    };
    let mut left = rect.left() + 6.0;
    let mut toggled = false;
    if let Some(hidden) = hidden {
        let eye = Rect::from_min_size(Pos2::new(left, rect.center().y - 8.0), Vec2::splat(16.0));
        let eye_response = ui.interact(
            eye.expand(2.0),
            ui.id().with(("studio_eye", name, rect.top() as i32)),
            Sense::click(),
        );
        paint_eye(
            &painter,
            eye,
            text.gamma_multiply(if hidden { 0.4 } else { 0.85 }),
            !hidden,
        );
        toggled = eye_response.on_hover_text("Show or hide").clicked();
        left += 22.0;
    }
    left += (depth as f32 * 12.0).min(120.0);
    let icon = Rect::from_min_size(Pos2::new(left, rect.center().y - 8.0), Vec2::splat(16.0));
    paint_icon(
        &painter,
        icon,
        if bone {
            Icon::Mode(Mode::Pose)
        } else {
            Icon::Mode(Mode::Object)
        },
        text.gamma_multiply(0.85),
    );
    painter.text(
        Pos2::new(left + 22.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(13.5),
        text,
    );
    (response, toggled)
}

fn outliner(studio: &mut ModelStudio, ui: &mut Ui) {
    let mut pick_part = None;
    let mut hide = None;
    ui.spacing_mut().item_spacing.y = 1.0;
    for (index, mesh) in studio.model.meshes.iter().enumerate() {
        let picked = (studio.mode == Mode::Object && studio.objects.contains(&index))
            || (studio.mode == Mode::Edit && studio.mesh == index);
        let name = if mesh.skin.is_some() {
            format!("{}  · bound", mesh.name)
        } else {
            mesh.name.clone()
        };
        let (row, toggled) = outliner_row(ui, &name, picked, Some(mesh.hidden), false, 0);
        if toggled {
            hide = Some((index, !mesh.hidden));
        } else if row.clicked() {
            pick_part = Some(index);
        }
    }
    if let Some((index, hidden)) = hide {
        studio.model.meshes[index].hidden = hidden;
        studio.touch();
    }
    if let Some(index) = pick_part {
        studio.objects = BTreeSet::from([index]);
        studio.mesh = index;
        studio.clear_elements();
        if studio.mode == Mode::Pose {
            studio.set_mode(Mode::Object);
        }
    }
    if studio.model.meshes.is_empty() {
        ui.label(
            RichText::new("No parts yet: pick Add shape and click on the floor.")
                .small()
                .weak(),
        );
    }
    let mut depth = vec![0usize; studio.model.nodes.len()];
    for (i, node) in studio.model.nodes.iter().enumerate() {
        depth[i] = node
            .parent
            .map_or(0, |p| depth.get(usize::from(p)).copied().unwrap_or(0) + 1);
    }
    let bones: Vec<(u16, usize, String)> = studio
        .model
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.bone)
        .map(|(i, n)| (i as u16, depth[i], n.name.clone()))
        .collect();
    if !bones.is_empty() {
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_source("studio_bones")
            .max_height(200.0)
            .show(ui, |ui| {
                for (index, depth, name) in &bones {
                    let (row, _) =
                        outliner_row(ui, name, studio.bone == Some(*index), None, true, *depth);
                    if row.clicked() {
                        studio.bone = Some(*index);
                        if studio.mode != Mode::Pose {
                            studio.set_mode(Mode::Pose);
                        }
                    }
                }
            });
    }
    ui.add_space(6.0);
    // Renaming what is picked.
    match studio.mode {
        Mode::Pose => {
            if let Some(bone) = studio.bone {
                let node = &mut studio.model.nodes[usize::from(bone)];
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut node.name);
                });
                ui.horizontal(|ui| {
                    ui.label("Length");
                    if ui
                        .add(
                            egui::DragValue::new(&mut node.length)
                                .speed(0.01)
                                .clamp_range(0.0..=1.0e6),
                        )
                        .changed()
                    {
                        studio.revision += 1;
                    }
                });
            }
        }
        _ => {
            if let Some(mesh) = studio.model.meshes.get_mut(studio.mesh) {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut mesh.name);
                });
            }
        }
    }
}

fn transform(studio: &mut ModelStudio, ui: &mut Ui) {
    match studio.mode {
        Mode::Object => {
            let Some(&index) = studio.objects.iter().next() else {
                ui.label(
                    RichText::new("Pick a part to see where it is.")
                        .small()
                        .weak(),
                );
                return;
            };
            let Some(node) = studio.model.meshes.get(index).and_then(|m| m.node) else {
                ui.label(
                    RichText::new("This part sits at the model's origin.")
                        .small()
                        .weak(),
                );
                if ui.small_button("Give it its own place").clicked() {
                    studio.snapshot();
                    studio.ensure_node(index);
                    studio.touch();
                }
                return;
            };
            let n = &studio.model.nodes[usize::from(node)];
            let (mut t, mut r, mut s) = (n.translation, n.rotation.to_euler_deg(), n.scale);
            let mut changed = false;
            let mut began = false;
            egui::Grid::new("studio_part_transform")
                .num_columns(2)
                .spacing([6.0, 4.0])
                .show(ui, |ui| {
                    for (label, value, speed, suffix) in [
                        ("Position", &mut t, 0.01, ""),
                        ("Rotation", &mut r, 0.5, "°"),
                        ("Scale", &mut s, 0.01, ""),
                    ] {
                        let (c, b) = v3_row(ui, label, value, speed, suffix);
                        changed |= c;
                        began |= b;
                    }
                });
            if began {
                studio.snapshot();
            }
            if changed {
                let n = &mut studio.model.nodes[usize::from(node)];
                n.translation = t;
                n.rotation = Quat::from_euler_deg(r.x, r.y, r.z);
                n.scale = s;
                studio.touch();
            }
            if studio.objects.len() > 1 {
                ui.label(
                    RichText::new(format!(
                        "{} parts picked; this shows the first.",
                        studio.objects.len()
                    ))
                    .small()
                    .weak(),
                );
            }
        }
        Mode::Edit => {
            let Some(centre) = studio.selection_centre() else {
                ui.label(RichText::new("Pick points, edges or faces.").small().weak());
                return;
            };
            let mut moved = centre;
            let mut result = (false, false);
            egui::Grid::new("studio_shape_transform")
                .num_columns(2)
                .spacing([6.0, 4.0])
                .show(ui, |ui| {
                    result = v3_row(ui, "Middle", &mut moved, 0.01, "");
                });
            let (changed, began) = result;
            if began {
                studio.snapshot();
            }
            if changed {
                let delta = moved - centre;
                let world = studio.model.rest_world();
                let verts = studio.moving_verts();
                if let Some(mesh) = studio.model.meshes.get_mut(studio.mesh) {
                    let inverse = mesh_matrix(mesh, &world).inverse().unwrap_or_default();
                    let local = inverse.transform_vector(delta);
                    for v in verts {
                        if let Some(p) = mesh.positions.get_mut(v as usize) {
                            *p += local;
                        }
                    }
                    mesh.corner_normals.clear();
                }
                studio.touch();
            }
            let count = studio.moving_verts().len();
            ui.label(RichText::new(format!("{count} point(s)")).small().weak());
        }
        Mode::Pose => {
            let Some(bone) = studio.bone else {
                ui.label(RichText::new("Pick a bone.").small().weak());
                return;
            };
            let local = studio.locals()[usize::from(bone)];
            let (mut t, mut r) = (local.translation, local.rotation.to_euler_deg());
            let mut changed = false;
            let mut began = false;
            egui::Grid::new("studio_bone_transform")
                .num_columns(2)
                .spacing([6.0, 4.0])
                .show(ui, |ui| {
                    for (label, value, speed, suffix) in [
                        ("Position", &mut t, 0.01, ""),
                        ("Rotation", &mut r, 0.5, "°"),
                    ] {
                        let (c, b) = v3_row(ui, label, value, speed, suffix);
                        changed |= c;
                        began |= b;
                    }
                });
            if began {
                studio.snapshot();
            }
            if changed {
                let new = q0s_format::three::Trs {
                    translation: t,
                    rotation: Quat::from_euler_deg(r.x, r.y, r.z),
                    scale: local.scale,
                };
                if studio.rest_edit {
                    let n = &mut studio.model.nodes[usize::from(bone)];
                    n.translation = new.translation;
                    n.rotation = new.rotation;
                } else {
                    studio.key_bone(bone, new);
                }
                studio.touch();
            }
            ui.label(
                RichText::new(if studio.rest_edit {
                    "Changes the skeleton itself."
                } else {
                    "Changes key the clip at the playhead."
                })
                .small()
                .weak(),
            );
        }
    }
}

fn material(studio: &mut ModelStudio, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| material_swatches(studio, ui, 26.0));
    let count = studio.model.materials.len();
    studio.material = studio.material.min(count.saturating_sub(1));
    let mut changed = false;
    let mut load = false;
    if let Some(material) = studio.model.materials.get_mut(studio.material) {
        egui::Grid::new("studio_material")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Name");
                changed |= ui.text_edit_singleline(&mut material.name).changed();
                ui.end_row();
                ui.label("Colour");
                changed |= edit_color(ui, &mut material.color);
                ui.end_row();
                ui.label("Look");
                segmented(ui, |ui| {
                    for (shading, label) in [
                        (Shading::Lit, "Smooth"),
                        (Shading::Toon, "Cartoon"),
                        (Shading::Unlit, "Flat"),
                        (Shading::Glass, "Glass"),
                    ] {
                        if pill(ui, material.shading == shading, label).clicked()
                            && material.shading != shading
                        {
                            material.shading = shading;
                            changed = true;
                            // Glass made of an opaque colour would hide what is behind it.
                            if shading == Shading::Glass && material.color.a == 255 {
                                material.color.a = 70;
                            }
                        }
                    }
                });
                ui.end_row();
                if material.shading == Shading::Glass {
                    ui.label("Clear");
                    let mut clear = 1.0 - f32::from(material.color.a) / 255.0;
                    if ui
                        .add(egui::Slider::new(&mut clear, 0.0..=1.0))
                        .on_hover_text("How much shows through: 1 is clear glass, 0 is milky")
                        .changed()
                    {
                        material.color.a = ((1.0 - clear) * 255.0).round() as u8;
                        changed = true;
                    }
                    ui.end_row();
                }
                if material.shading == Shading::Toon {
                    ui.label("Bands");
                    changed |= ui
                        .add(egui::Slider::new(&mut material.toon_bands, 2..=6))
                        .changed();
                    ui.end_row();
                }
                ui.label("Rough");
                changed |= ui
                    .add(egui::Slider::new(&mut material.roughness, 0.0..=1.0))
                    .changed();
                ui.end_row();
                ui.label("Metal");
                changed |= ui
                    .add(egui::Slider::new(&mut material.metallic, 0.0..=1.0))
                    .changed();
                ui.end_row();
                ui.label("Glow");
                changed |= edit_color(ui, &mut material.emissive);
                ui.end_row();
                ui.label("Both sides");
                changed |= ui.checkbox(&mut material.double_sided, "").changed();
                ui.end_row();
                ui.label("Picture");
                ui.horizontal(|ui| {
                    if ui
                        .small_button(if material.texture.is_some() {
                            "Replace…"
                        } else {
                            "Load…"
                        })
                        .clicked()
                    {
                        load = true;
                    }
                    if material.texture.is_some() && ui.small_button("Remove").clicked() {
                        material.texture = None;
                        changed = true;
                    }
                });
                ui.end_row();
            });
    }
    if load {
        studio.wants_texture = true;
    }
    if changed {
        studio.touch();
    }
    ui.horizontal(|ui| {
        if ui
            .button("Use on picked")
            .on_hover_text("Give the picked faces or parts this material (or use the Paint tool)")
            .clicked()
        {
            studio.snapshot();
            let material = studio.material as u16;
            match studio.mode {
                Mode::Edit => {
                    let faces = studio.working_faces();
                    if let Some(mesh) = studio.model.meshes.get_mut(studio.mesh) {
                        q0model::ops::set_material(mesh, &faces, material);
                    }
                }
                _ => {
                    for m in studio.objects.clone() {
                        if let Some(mesh) = studio.model.meshes.get_mut(m) {
                            let all = (0..mesh.faces.len()).collect();
                            q0model::ops::set_material(mesh, &all, material);
                        }
                    }
                }
            }
            studio.touch();
        }
        if ui.button("Paint tool").clicked() {
            studio.tool = Tool::Paint;
            if studio.mode == Mode::Pose {
                studio.set_mode(Mode::Object);
            }
        }
    });
}

fn stage_settings(studio: &mut ModelStudio, ui: &mut Ui) {
    let mut size = studio.model.scale;
    ui.horizontal(|ui| {
        ui.label("Size");
        if ui
            .add(
                egui::DragValue::new(&mut size)
                    .speed(0.5)
                    .clamp_range(0.001..=100_000.0)
                    .suffix(" px per unit"),
            )
            .changed()
        {
            studio.model.scale = size;
            studio.touch();
        }
    });
    let across = studio.size() * studio.model.scale;
    ui.label(
        RichText::new(format!("about {across:.0} px across on the stage"))
            .small()
            .weak(),
    );
    let mut outline = studio.model.outline.is_some();
    if ui.checkbox(&mut outline, "Cartoon outline").changed() {
        studio.model.outline = outline.then_some(Outline {
            width: 2.5,
            color: Rgba {
                r: 15,
                g: 15,
                b: 20,
                a: 255,
            },
        });
        studio.touch();
    }
    if let Some(outline) = &mut studio.model.outline {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Width");
            changed |= ui
                .add(egui::Slider::new(&mut outline.width, 0.5..=12.0).suffix(" px"))
                .changed();
            changed |= edit_color(ui, &mut outline.color);
        });
        if changed {
            studio.touch();
        }
    }
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!(
            "{} parts · {} points · {} triangles · {} bones · {} clips",
            studio.model.meshes.len(),
            studio.model.vertex_count(),
            studio.model.triangle_count(),
            studio.model.nodes.iter().filter(|n| n.bone).count(),
            studio.model.clips.len()
        ))
        .small()
        .weak(),
    );
}

fn animation_bar(studio: &mut ModelStudio, ui: &mut Ui, fps: u16) {
    let fps_f = f32::from(fps);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("ANIMATION")
                .size(10.5)
                .strong()
                .color(ui.visuals().weak_text_color()),
        );
        let name = studio
            .clip
            .and_then(|c| studio.model.clips.get(c))
            .map_or("rest pose".to_string(), |c| c.name.clone());
        let mut picked = None;
        egui::ComboBox::from_id_source("studio_clip")
            .selected_text(name)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(studio.clip.is_none(), "rest pose")
                    .clicked()
                {
                    picked = Some(None);
                }
                for (i, clip) in studio.model.clips.iter().enumerate() {
                    if ui
                        .selectable_label(studio.clip == Some(i), &clip.name)
                        .clicked()
                    {
                        picked = Some(Some(i));
                    }
                }
            });
        if let Some(clip) = picked {
            studio.clip = clip;
            studio.time = 0.0;
            studio.touch();
        }
        if ghost_button(ui, "+ New clip").clicked() {
            studio.snapshot();
            studio.model.clips.push(Clip {
                name: format!("Clip {}", studio.model.clips.len() + 1),
                duration: 1.0,
                channels: Vec::new(),
            });
            studio.clip = Some(studio.model.clips.len() - 1);
            studio.time = 0.0;
            if studio.mode != Mode::Pose {
                studio.set_mode(Mode::Pose);
            }
            studio.rest_edit = false;
            studio.tool = Tool::Rotate;
            studio.touch();
        }
        let mut remove = false;
        if let Some(clip) = studio.clip.and_then(|c| studio.model.clips.get_mut(c)) {
            ui.add(egui::TextEdit::singleline(&mut clip.name).desired_width(110.0));
            ui.label("Length");
            if ui
                .add(
                    egui::DragValue::new(&mut clip.duration)
                        .speed(0.01)
                        .clamp_range(0.05..=600.0)
                        .suffix(" s"),
                )
                .changed()
            {
                studio.revision += 1;
            }
            remove = ui.small_button("Delete clip").clicked();
        }
        if remove {
            let index = studio.clip.unwrap_or(0);
            studio.snapshot();
            studio.model.clips.remove(index);
            studio.clip = None;
            studio.touch();
        }
    });
    let Some(clip_index) = studio.clip.filter(|c| *c < studio.model.clips.len()) else {
        ui.label(
            RichText::new(
                "Make a clip, then turn bones in Bones mode: every change keys the playhead.",
            )
            .small()
            .weak(),
        );
        ui.add_space(4.0);
        return;
    };
    let duration = studio.model.clips[clip_index].duration.max(0.01);
    let keys = studio.model.clips[clip_index].key_times();
    let bone_keys: Vec<f32> = studio
        .bone
        .map(|b| {
            let mut t: Vec<f32> = studio.model.clips[clip_index]
                .channels
                .iter()
                .filter(|c| c.node == b)
                .flat_map(|c| c.times.iter().copied())
                .collect();
            t.sort_by(f32::total_cmp);
            t.dedup_by(|a, b| (*a - *b).abs() < 1.0e-4);
            t
        })
        .unwrap_or_default();
    ui.horizontal(|ui| {
        let step = 1.0 / fps_f;
        let mut seek = None;
        if ui.small_button("⏮").on_hover_text("First frame").clicked() {
            seek = Some(0.0);
        }
        if ui
            .small_button("◀")
            .on_hover_text("Previous frame")
            .clicked()
        {
            seek = Some((studio.time - step).max(0.0));
        }
        if ui
            .button(if studio.playing { "⏸" } else { "▶" })
            .on_hover_text("Play (Space)")
            .clicked()
        {
            studio.playing = !studio.playing;
            studio.touch();
        }
        if ui.small_button("▶|").on_hover_text("Next frame").clicked() {
            seek = Some((studio.time + step).min(duration));
        }
        if ui.small_button("⏭").on_hover_text("Last frame").clicked() {
            seek = Some(duration);
        }
        ui.toggle_value(&mut studio.looping, "⟲")
            .on_hover_text("Loop");
        if let Some(t) = seek {
            studio.time = t;
            studio.playing = false;
            studio.touch();
        }
        let frame = (studio.time * fps_f).round() as i64;
        ui.label(RichText::new(format!("{:>5.2}s  frame {}", studio.time, frame + 1)).monospace());
        if studio.mode == Mode::Pose {
            for command in [Command::KeyBone, Command::KeyAll, Command::DeleteKey] {
                if ui.small_button(label_of(command)).clicked() {
                    commands::run(studio, command);
                }
            }
        }
    });
    let width = ui.available_width() - 4.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width.max(80.0), 30.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        8.0,
        Color32::from_black_alpha(70),
        Stroke::new(1.0_f32, Color32::from_white_alpha(14)),
    );
    let x_of = |t: f32| rect.left() + 8.0 + (t / duration).clamp(0.0, 1.0) * (rect.width() - 16.0);
    let frames = (duration * fps_f).ceil() as i32;
    let every = ((frames as f32 / (rect.width() / 12.0)).ceil() as i32).max(1);
    for f in (0..=frames).step_by(every as usize) {
        let x = x_of(f as f32 / fps_f);
        let tall = f % (every * 5) == 0;
        painter.line_segment(
            [
                Pos2::new(x, rect.bottom() - if tall { 10.0 } else { 5.0 }),
                Pos2::new(x, rect.bottom()),
            ],
            Stroke::new(1.0_f32, Color32::from_gray(if tall { 120 } else { 80 })),
        );
        if tall {
            painter.text(
                Pos2::new(x + 2.0, rect.top() + 2.0),
                Align2::LEFT_TOP,
                format!("{}", f + 1),
                FontId::monospace(9.0),
                Color32::from_gray(120),
            );
        }
    }
    for t in &keys {
        painter.circle_filled(
            Pos2::new(x_of(*t), rect.center().y + 2.0),
            3.0,
            Color32::from_gray(160),
        );
    }
    for t in &bone_keys {
        let c = Pos2::new(x_of(*t), rect.center().y + 2.0);
        painter.add(Shape::convex_polygon(
            vec![
                c + Vec2::new(0.0, -6.0),
                c + Vec2::new(6.0, 0.0),
                c + Vec2::new(0.0, 6.0),
                c + Vec2::new(-6.0, 0.0),
            ],
            Color32::from_rgb(255, 190, 70),
            Stroke::new(1.0_f32, Color32::from_black_alpha(140)),
        ));
    }
    let x = x_of(studio.time);
    let head = accent(ui);
    painter.line_segment(
        [
            Pos2::new(x, rect.top() + 2.0),
            Pos2::new(x, rect.bottom() - 2.0),
        ],
        Stroke::new(2.0_f32, head),
    );
    painter.add(Shape::convex_polygon(
        vec![
            Pos2::new(x - 5.0, rect.top() + 1.0),
            Pos2::new(x + 5.0, rect.top() + 1.0),
            Pos2::new(x, rect.top() + 7.0),
        ],
        head,
        Stroke::NONE,
    ));
    if let Some(pos) = response.interact_pointer_pos() {
        if response.dragged() || response.clicked() {
            let t =
                ((pos.x - rect.left() - 8.0) / (rect.width() - 16.0)).clamp(0.0, 1.0) * duration;
            studio.time = (t * fps_f).round() / fps_f;
            studio.playing = false;
            if studio.mode != Mode::Pose {
                studio.set_mode(Mode::Pose);
            }
            studio.touch();
        }
    }
    ui.add_space(4.0);
}

/// Where the navigation ball and its buttons sit.
fn nav_area(rect: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.right() - 110.0, rect.top() + 30.0),
        Pos2::new(rect.right(), rect.top() + 320.0),
    )
}

fn viewport(studio: &mut ModelStudio, ui: &mut Ui, fps: u16) {
    let rect = ui.available_rect_before_wrap();
    studio.viewport = rect;
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let view = studio.view(rect);
    let ctx = ui.ctx().clone();
    let over_nav = ctx
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| nav_area(rect).contains(p))
        && !response.dragged();
    super::input::viewport(studio, ui, &response, &view, over_nav);

    if studio.playing {
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let duration = studio
            .clip
            .and_then(|c| studio.model.clips.get(c))
            .map_or(0.0, |c| c.duration);
        if duration > 0.0 {
            studio.time += dt;
            if studio.time > duration {
                if studio.looping {
                    studio.time %= duration;
                } else {
                    studio.time = duration;
                    studio.playing = false;
                }
            }
        }
        ctx.request_repaint();
    }
    let _ = fps;

    let palette = Palette::dark();
    let frame = studio.frame(rect, &palette);
    let painter = ui.painter_at(rect);
    // A soft studio backdrop: lighter at the top, like a sky over the floor.
    let mut backdrop = egui::Mesh::default();
    let (top, bottom) = (Color32::from_rgb(62, 65, 74), Color32::from_rgb(36, 37, 42));
    for (pos, colour) in [
        (rect.left_top(), top),
        (rect.right_top(), top),
        (rect.right_bottom(), bottom),
        (rect.left_bottom(), bottom),
    ] {
        backdrop.colored_vertex(pos, colour);
    }
    backdrop.add_triangle(0, 1, 2);
    backdrop.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(backdrop));
    q0glblend::paint_frame3d(&painter, rect, Arc::new(frame));

    // Over the picture: bones, the gizmo, previews, the box.
    let world = studio.world();
    gizmo::paint_bones(&painter, studio, &view, &world);
    if studio.op.is_none() {
        if let Some(g) = gizmo::layout(studio, &view) {
            gizmo::paint(&painter, &g, studio.handle_hover, studio.handle_drag);
        }
    } else if let Some(g) = gizmo::layout(studio, &view) {
        gizmo::paint(&painter, &g, None, studio.handle_drag);
    }
    if let (Tool::LoopCut, Some((edge, t)), None) = (studio.tool, studio.loop_preview, &studio.op) {
        for [a, b] in view::loop_preview(studio, &view, edge, t) {
            painter.line_segment(
                [a, b],
                Stroke::new(2.5_f32, Color32::from_rgb(255, 230, 90)),
            );
        }
    }
    if let (Some((head, _)), Some(mouse)) =
        (studio.bone_start, ctx.input(|i| i.pointer.hover_pos()))
    {
        if let Some(a) = studio.screen(&view, head) {
            painter.line_segment(
                [a, mouse],
                Stroke::new(3.0_f32, Color32::from_rgb(255, 200, 90)),
            );
            painter.circle_filled(a, 4.0, Color32::from_rgb(255, 200, 90));
        }
    }
    if let Some((a, b)) = studio.box_select {
        gizmo::paint_box(&painter, Rect::from_two_pos(a, b));
    }
    if studio.tool == Tool::AddShape {
        if let Some(mouse) = ctx
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p) && !nav_area(rect).contains(*p))
        {
            let point = view::place_point(studio, &view, mouse);
            if let Some(at) = studio.screen(&view, point) {
                painter.circle_stroke(
                    at,
                    9.0,
                    Stroke::new(2.0_f32, Color32::from_rgb(255, 220, 90)),
                );
                painter.text(
                    at + Vec2::new(12.0, -12.0),
                    Align2::LEFT_BOTTOM,
                    studio.settings.shape.label(),
                    FontId::proportional(12.0),
                    Color32::from_rgb(255, 220, 90),
                );
            }
        }
    }

    // Words: the value of a running change by the pointer, a hint below,
    // the status at the top.
    if let (Some(op), Some(mouse)) = (&studio.op, ctx.input(|i| i.pointer.hover_pos())) {
        if !op.value.is_empty() {
            let text = if op.label.is_empty() {
                op.value.clone()
            } else {
                format!("{}  {}", op.label, op.value)
            };
            let pos = mouse + Vec2::new(18.0, 14.0);
            let galley = painter.layout_no_wrap(text, FontId::monospace(13.0), Color32::WHITE);
            let bg = Rect::from_min_size(pos, galley.size()).expand(4.0);
            painter.rect_filled(bg, 4.0, Color32::from_black_alpha(170));
            painter.galley(pos, galley, Color32::WHITE);
        }
    }
    // The hint, in a pill along the bottom.
    let hint = gizmo::hint(studio);
    let galley = painter.layout(
        hint,
        FontId::proportional(12.5),
        Color32::from_gray(230),
        (rect.width() - 200.0).max(160.0),
    );
    let pill_rect = Rect::from_center_size(
        Pos2::new(
            rect.center().x,
            rect.bottom() - 18.0 - galley.size().y * 0.5,
        ),
        galley.size() + Vec2::new(24.0, 12.0),
    );
    painter.rect(
        pill_rect,
        12.0,
        Color32::from_black_alpha(140),
        Stroke::new(1.0_f32, Color32::from_white_alpha(18)),
    );
    painter.galley(
        pill_rect.min + Vec2::new(12.0, 6.0),
        galley,
        Color32::from_gray(230),
    );

    // Where we are: the mode and tool on a chip in the corner.
    let chip_text = format!("{}  ·  {}", studio.mode.label(), studio.tool.label());
    let chip_galley = painter.layout_no_wrap(chip_text, FontId::proportional(12.5), Color32::WHITE);
    let chip = Rect::from_min_size(
        rect.left_top() + Vec2::new(12.0, 12.0),
        chip_galley.size() + Vec2::new(40.0, 12.0),
    );
    painter.rect(
        chip,
        9.0,
        Color32::from_black_alpha(130),
        Stroke::new(1.0_f32, Color32::from_white_alpha(18)),
    );
    paint_icon(
        &painter,
        Rect::from_min_size(
            chip.min + Vec2::new(8.0, chip.height() * 0.5 - 8.0),
            Vec2::splat(16.0),
        ),
        Icon::Tool(studio.tool),
        Color32::WHITE,
    );
    painter.galley(chip.min + Vec2::new(30.0, 6.0), chip_galley, Color32::WHITE);

    // What just happened, as a toast that fades.
    let now = ctx.input(|i| i.time);
    let toast_id = egui::Id::new("studio_toast");
    let (shown, since) = ctx
        .data(|d| d.get_temp::<(String, f64)>(toast_id))
        .unwrap_or_default();
    let since = if shown != studio.status { now } else { since };
    ctx.data_mut(|d| d.insert_temp(toast_id, (studio.status.clone(), since)));
    let age = (now - since) as f32;
    if !studio.status.is_empty() && age < 3.0 {
        let fade = (1.0 - (age - 2.2).max(0.0) / 0.8).clamp(0.0, 1.0);
        let galley = painter.layout_no_wrap(
            studio.status.clone(),
            FontId::proportional(13.0),
            Color32::WHITE.gamma_multiply(fade),
        );
        let toast = Rect::from_center_size(
            Pos2::new(
                rect.center().x,
                rect.top() + 26.0 - (1.0 - (age / 0.15).min(1.0)) * 8.0,
            ),
            galley.size() + Vec2::new(28.0, 14.0),
        );
        painter.rect(
            toast,
            10.0,
            accent(ui).gamma_multiply(0.75 * fade),
            Stroke::NONE,
        );
        painter.galley(toast.min + Vec2::new(14.0, 7.0), galley, Color32::WHITE);
        ctx.request_repaint();
    }

    gizmo::navigator(ui, studio, &view, rect);

    // Right-click without dragging: the actions at hand.
    response.context_menu(|ui| context_menu(studio, ui));
}

fn context_menu(studio: &mut ModelStudio, ui: &mut Ui) {
    let commands: Vec<Command> = match studio.mode {
        Mode::Object => vec![
            Command::Duplicate,
            Command::DeleteObjects,
            Command::Join,
            Command::ShadeSmooth,
            Command::ShadeFlat,
            Command::SelectAll,
            Command::SelectNone,
        ],
        Mode::Edit => vec![
            Command::Fill,
            Command::Merge,
            Command::Delete,
            Command::Duplicate,
            Command::Subdivide,
            Command::Smooth,
            Command::Flip,
            Command::ShadeSmooth,
            Command::ShadeFlat,
            Command::SelectAll,
            Command::SelectNone,
            Command::Linked,
        ],
        Mode::Pose => vec![
            Command::KeyBone,
            Command::KeyAll,
            Command::DeleteKey,
            Command::AddBone,
            Command::DeleteBone,
            Command::Bind,
        ],
    };
    for command in commands {
        if ui.button(label_of(command)).clicked() {
            commands::run(studio, command);
            ui.close_menu();
        }
    }
    ui.separator();
    if ui.button("Frame picked (F)").clicked() {
        view::frame_on(studio, !studio.has_selection());
        ui.close_menu();
    }
    if ui.button("Frame everything (Home)").clicked() {
        view::frame_on(studio, true);
        ui.close_menu();
    }
}

fn help(studio: &mut ModelStudio, ctx: &Context) {
    let mut open = true;
    egui::Window::new("Model studio")
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            let rows: &[(&str, &str)] = &[
                ("Look around", "drag with the right or middle button, or Alt + left; or drag the ball in the corner"),
                ("Slide the view", "Shift + right/middle drag, or the ✥ button"),
                ("Zoom", "wheel, pinch, or the 🔍 button"),
                ("Look along an axis", "click X, Y or Z on the ball"),
                ("Frame", "F for the picked things, Home for everything"),
                ("Parts / Shape / Bones", "Tab switches Parts and Shape; Ctrl Tab goes to Bones"),
                ("Points / edges / faces", "1, 2, 3 in Shape"),
                ("Tools", "Q select · W move · E rotate · R scale · Shift A add · T extrude · I inset · C loop cut · P paint · B bones"),
                ("Gizmo", "arrows move along an axis, squares in a plane, the dot freely; rings turn; boxes stretch"),
                ("Quick move", "with Move, drag anything straight away; G moves the pick with the mouse (X Y Z to keep to an axis)"),
                ("Snapping", "Ctrl while dragging, or Snap in the tool bar"),
                ("Pick", "click, Shift adds, drag a box; A all, Alt A none, L linked"),
                ("Delete / duplicate", "X or Delete; Shift D"),
                ("Undo", "Ctrl Z, Ctrl Y"),
                ("Animate", "make a clip, pick a bone, turn it: it keys the playhead. K keys the bone, Space plays"),
            ];
            ui.set_width(600.0);
            for (index, (what, how)) in rows.iter().enumerate() {
                let fill = if index % 2 == 0 { ui.visuals().faint_bg_color } else { Color32::TRANSPARENT };
                egui::Frame::none().fill(fill).inner_margin(egui::Margin::symmetric(6.0, 4.0)).show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(150.0, 16.0), Sense::hover());
                        let colour = ui.visuals().strong_text_color();
                        ui.painter().text(rect.left_top(), Align2::LEFT_TOP, *what, FontId::proportional(14.0), colour);
                        ui.add(egui::Label::new(*how).wrap(true));
                    });
                });
            }
        });
    if !open {
        studio.show_help = false;
    }
}
