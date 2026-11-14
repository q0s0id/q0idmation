use egui::epaint::{CubicBezierShape, PathShape};
use egui::{
    pos2, vec2, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, WidgetInfo,
    WidgetType,
};

use crate::app::{Action, EditorApp};
use crate::state::Tool;

const TOOL_BUTTON_SIZE: f32 = 32.0;
const TOOL_ICON_SIZE: f32 = 19.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ToolIcon {
    SelectPointer,
    Lasso,
    Hand,
    SubselectPointer,
    Pen,
    Pencil,
    Brush,
    Eraser,
    Knife,
    Line,
    Rectangle,
    Oval,
    Bucket,
    Eyedropper,
    Text,
    Rig,
}

impl ToolIcon {
    const fn for_tool(tool: Tool) -> Self {
        match tool {
            Tool::Select => Self::SelectPointer,
            Tool::Lasso => Self::Lasso,
            Tool::Hand => Self::Hand,
            Tool::Subselect => Self::SubselectPointer,
            Tool::Pen => Self::Pen,
            Tool::Pencil => Self::Pencil,
            Tool::Brush => Self::Brush,
            Tool::Eraser => Self::Eraser,
            Tool::Knife => Self::Knife,
            Tool::Line => Self::Line,
            Tool::Rectangle => Self::Rectangle,
            Tool::Oval => Self::Oval,
            Tool::Bucket => Self::Bucket,
            Tool::Eyedropper => Self::Eyedropper,
            Tool::Text => Self::Text,
            Tool::Rig => Self::Rig,
        }
    }
}

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    let three = crate::tools3d::offered(&app.state.project);
    if ui.available_width() >= 90.0 {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for tool in Tool::ALL {
                render_tool_button(app, ui, tool);
            }
        });
        if three {
            ui.add_space(4.0);
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for tool in crate::tools3d::Tool3d::ALL {
                    render_tool3d_button(app, ui, tool);
                }
            });
        }
    } else {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(4.0);
            for tool in Tool::ALL {
                if matches!(tool, Tool::Pen | Tool::Line | Tool::Bucket | Tool::Text) {
                    ui.add_space(3.0);
                    ui.separator();
                }
                render_tool_button(app, ui, tool);
            }
            if three {
                ui.add_space(3.0);
                ui.separator();
                ui.label(
                    egui::RichText::new("3D")
                        .small()
                        .color(app.settings.theme.text_dim.to_color32()),
                );
                for tool in crate::tools3d::Tool3d::ALL {
                    render_tool3d_button(app, ui, tool);
                }
            }
            ui.add_space(8.0);
        });
    }
}

/// The magnet in the stage's header: snapping on or off.
pub(crate) fn render_snap_button(app: &mut EditorApp, ui: &mut Ui) {
    let size = if crate::touch::ENABLED { 30.0 } else { 22.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let on = app.settings.aids.snap;
    let text = app.settings.theme.text.to_color32();
    let accent = app.settings.theme.accent.to_color32();
    if ui.is_rect_visible(rect) {
        crate::ui::paint_control(ui, &response, rect.shrink(1.0), 5.0, on);
        let painter = ui.painter_at(rect);
        let _ = accent;
        paint_magnet(&painter, rect.center(), rect.width() * 0.3, text);
    }
    let hint = app.settings.hotkeys.hint(
        crate::hotkeys::ShortcutCommand::ToggleSnap,
        if on { "Snapping: on" } else { "Snapping: off" },
    );
    let response = response.on_hover_text(format!("{hint}\nwithin 10 screen pixels: anchors, line ends, outlines, object corners and centers. a ring marks a point snap; a square marks an outline snap."));
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, on, "Snapping"));
    if response.clicked() {
        app.queue(Action::ToggleSnap);
    }
}

/// A horseshoe magnet opening downwards.
fn paint_magnet(painter: &Painter, center: Pos2, radius: f32, color: Color32) {
    let width = radius * 0.55;
    let arc: Vec<Pos2> = (0..=16)
        .map(|step| {
            let angle = std::f32::consts::PI * (1.0 + step as f32 / 16.0);
            center + vec2(angle.cos(), angle.sin()) * radius * 0.75
        })
        .collect();
    let mut points = vec![pos2(center.x - radius * 0.75, center.y + radius * 0.9)];
    points.extend(arc);
    points.push(pos2(center.x + radius * 0.75, center.y + radius * 0.9));
    painter.add(Shape::line(points, Stroke::new(width, color)));
    for side in [-1.0, 1.0] {
        let tip = Rect::from_center_size(
            pos2(center.x + side * radius * 0.75, center.y + radius * 0.95),
            vec2(width, width * 0.7),
        );
        painter.rect_filled(tip, 0.0, color.gamma_multiply(0.55));
    }
}

/// A 3D tool's button, styled like the 2D ones.
fn render_tool3d_button(app: &mut EditorApp, ui: &mut Ui, tool: crate::tools3d::Tool3d) {
    let selected = app.session.tools3d.tool == Some(tool);
    let size = tool_button_size();
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, selected, tool.label()));
    let text = app.settings.theme.text.to_color32();
    let accent = app.settings.theme.accent.to_color32();
    if ui.is_rect_visible(rect) {
        crate::ui::paint_control(
            ui,
            &response,
            rect.shrink(1.0),
            (size * 0.25).round(),
            selected,
        );
        let icon = Rect::from_center_size(
            rect.center(),
            vec2(TOOL_ICON_SIZE, TOOL_ICON_SIZE) * (size / TOOL_BUTTON_SIZE),
        );
        crate::tools3d::paint_icon(ui.painter(), icon, tool, text, accent);
    }
    if response.on_hover_text(tool.hint()).clicked() && crate::text_tool::finish_edit(app) {
        app.plugins.active_tool = None;
        app.session.tools3d.tool = Some(tool);
        app.session.tool_state = crate::state::ToolState::Idle;
    }
}

fn render_tool_button(app: &mut EditorApp, ui: &mut Ui, tool: Tool) {
    let response = tool_button(
        ui,
        tool,
        app.session.current_tool == tool && !app.session.tools3d.active(),
        app.settings.theme.text.to_color32(),
        app.settings.theme.panel.to_color32(),
        app.settings.theme.accent.to_color32(),
    )
    .on_hover_text(
        tool_shortcut(tool)
            .map(|command| app.settings.hotkeys.hint(command, tool.label()))
            .unwrap_or_else(|| tool.label().to_string()),
    );
    if response.clicked() {
        app.queue(Action::SelectTool(tool));
    }
}

fn tool_shortcut(tool: Tool) -> Option<crate::hotkeys::ShortcutCommand> {
    use crate::hotkeys::ShortcutCommand as C;
    Some(match tool {
        Tool::Select => C::ToolSelect,
        Tool::Lasso => C::ToolLasso,
        Tool::Subselect => C::ToolSubselect,
        Tool::Hand => C::ToolHand,
        Tool::Pen => C::ToolPen,
        Tool::Pencil => C::ToolPencil,
        Tool::Brush => C::ToolBrush,
        Tool::Eraser => C::ToolEraser,
        Tool::Knife => C::ToolKnife,
        Tool::Line => C::ToolLine,
        Tool::Rectangle => C::ToolRectangle,
        Tool::Oval => C::ToolOval,
        Tool::Bucket => C::ToolBucket,
        Tool::Eyedropper => C::ToolEyedropper,
        Tool::Text => C::ToolText,
        Tool::Rig => C::ToolRig,
    })
}

/// Tool buttons grow to fingertip size in the touch layout.
fn tool_button_size() -> f32 {
    if crate::touch::ENABLED {
        crate::ui::TOUCH_CONTROL_SIZE
    } else {
        TOOL_BUTTON_SIZE
    }
}

fn tool_button(
    ui: &mut Ui,
    tool: Tool,
    selected: bool,
    text: Color32,
    _panel: Color32,
    accent: Color32,
) -> Response {
    let size = tool_button_size();
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            selected,
            tool.label().to_string(),
        )
    });
    if ui.is_rect_visible(rect) {
        crate::ui::paint_control(
            ui,
            &response,
            rect.shrink(1.0),
            (size * 0.25).round(),
            selected,
        );
        let sink = if response.is_pointer_button_down_on() {
            vec2(0.0, 0.5)
        } else {
            egui::Vec2::ZERO
        };
        let icon_rect = Rect::from_center_size(
            rect.center() + sink,
            vec2(TOOL_ICON_SIZE, TOOL_ICON_SIZE) * (size / TOOL_BUTTON_SIZE),
        );
        let (primary, secondary) = tool_icon_colors(text, accent, selected);
        draw_tool_icon(
            ui.painter(),
            icon_rect,
            ToolIcon::for_tool(tool),
            primary,
            secondary,
        );
    }
    response
}

fn tool_icon_colors(text: Color32, accent: Color32, _selected: bool) -> (Color32, Color32) {
    // Selection belongs to the button chrome, never to the icon silhouette.
    // Keeping these stable prevents the icon from visually changing shape
    // or losing contrast when the user selects a tool.
    (text, accent)
}

/// Unit-square drawing helpers for the tool icons.
#[derive(Clone, Copy)]
struct IconCanvas {
    rect: Rect,
}

impl IconCanvas {
    fn point(self, x: f32, y: f32) -> Pos2 {
        pos2(
            self.rect.left() + self.rect.width() * x,
            self.rect.top() + self.rect.height() * y,
        )
    }

    fn rect(self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(self.point(x0, y0), self.point(x1, y1))
    }

    fn unit(self) -> f32 {
        self.rect.width().min(self.rect.height())
    }

    /// `t` of the way from `tip` to `end`, pushed `side` across the line.
    fn along(self, tip: (f32, f32), end: (f32, f32), t: f32, side: f32) -> Pos2 {
        let (dx, dy) = (end.0 - tip.0, end.1 - tip.1);
        let length = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
        let (nx, ny) = (-dy / length, dx / length);
        self.point(tip.0 + dx * t + nx * side, tip.1 + dy * t + ny * side)
    }

    /// The slice of a stick from `t0` to `t1`, `half` thick on each side.
    fn band(self, tip: (f32, f32), end: (f32, f32), t0: f32, t1: f32, half: f32) -> Vec<Pos2> {
        vec![
            self.along(tip, end, t0, -half),
            self.along(tip, end, t1, -half),
            self.along(tip, end, t1, half),
            self.along(tip, end, t0, half),
        ]
    }

    /// A pill from `a` to `b`, `radius` round at both ends.
    fn capsule(self, a: (f32, f32), b: (f32, f32), radius: f32) -> Vec<Pos2> {
        use std::f32::consts::{FRAC_PI_2, PI};
        let angle = (b.1 - a.1).atan2(b.0 - a.0);
        let arc = |center: (f32, f32), from: f32| {
            (0..=8).map(move |step| {
                let turn = from + PI * step as f32 / 8.0;
                (
                    center.0 + turn.cos() * radius,
                    center.1 + turn.sin() * radius,
                )
            })
        };
        arc(b, angle - FRAC_PI_2)
            .chain(arc(a, angle + FRAC_PI_2))
            .map(|(x, y)| self.point(x, y))
            .collect()
    }

    fn ellipse(self, cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<Pos2> {
        (0..32)
            .map(|step| {
                let angle = std::f32::consts::TAU * step as f32 / 32.0;
                self.point(cx + angle.cos() * rx, cy + angle.sin() * ry)
            })
            .collect()
    }

    fn pts(self, points: &[(f32, f32)]) -> Vec<Pos2> {
        points.iter().map(|&(x, y)| self.point(x, y)).collect()
    }
}

fn fill(painter: &Painter, points: Vec<Pos2>, color: Color32, stroke: Stroke) {
    painter.add(Shape::Path(PathShape {
        points,
        closed: true,
        fill: color,
        stroke,
    }));
}

/// The tool icons: the tool's own shape outlined in the text colour, with
/// the part that does the work (a nib, bristles, paint) in the accent.
fn draw_tool_icon(
    painter: &Painter,
    rect: Rect,
    icon: ToolIcon,
    primary: Color32,
    accent: Color32,
) {
    let c = IconCanvas { rect };
    let unit = c.unit();
    let width = (unit * 0.085).clamp(1.3, 2.6);
    let line = Stroke::new(width, primary);
    let fine = Stroke::new(width * 0.7, primary);
    let soft = primary.gamma_multiply(0.22);
    let glow = accent.gamma_multiply(0.26);
    let none = Stroke::NONE;
    // Diagonal tools lie from the lower left (their working end) up.
    let tip = (0.13, 0.87);
    let end = (0.87, 0.13);

    match icon {
        ToolIcon::SelectPointer => {
            fill(
                painter,
                c.pts(&[(0.24, 0.08), (0.24, 0.78), (0.74, 0.58)]),
                primary,
                none,
            );
            painter.line_segment(
                [c.point(0.44, 0.62), c.point(0.57, 0.88)],
                Stroke::new(unit * 0.13, primary),
            );
            painter.circle_filled(c.point(0.57, 0.88), unit * 0.065, primary);
        }
        ToolIcon::SubselectPointer => {
            fill(
                painter,
                c.pts(&[(0.20, 0.10), (0.20, 0.76), (0.66, 0.57)]),
                soft,
                line,
            );
            fill(
                painter,
                c.pts(&[(0.37, 0.62), (0.47, 0.58), (0.59, 0.84), (0.49, 0.88)]),
                soft,
                line,
            );
            painter.rect(c.rect(0.70, 0.12, 0.90, 0.32), unit * 0.03, accent, fine);
        }
        ToolIcon::Lasso => {
            fill(painter, c.ellipse(0.52, 0.40, 0.35, 0.26), glow, line);
            painter.add(CubicBezierShape::from_points_stroke(
                [
                    c.point(0.30, 0.60),
                    c.point(0.20, 0.72),
                    c.point(0.40, 0.80),
                    c.point(0.30, 0.92),
                ],
                false,
                Color32::TRANSPARENT,
                Stroke::new(width, accent),
            ));
            painter.circle_filled(c.point(0.30, 0.60), unit * 0.06, accent);
        }
        ToolIcon::Hand => {
            // Fingers, palm and thumb as one solid shape.
            let finger = |x: f32, top: f32, radius: f32| {
                fill(
                    painter,
                    c.capsule((x, 0.56), (x, top), radius),
                    primary,
                    none,
                );
            };
            finger(0.36, 0.20, 0.058);
            finger(0.50, 0.12, 0.058);
            finger(0.64, 0.17, 0.058);
            finger(0.77, 0.32, 0.052);
            painter.rect_filled(c.rect(0.30, 0.46, 0.83, 0.88), unit * 0.17, primary);
            fill(
                painter,
                c.capsule((0.36, 0.72), (0.15, 0.50), 0.06),
                primary,
                none,
            );
            painter.line_segment(
                [c.point(0.40, 0.80), c.point(0.72, 0.80)],
                Stroke::new(width * 0.8, accent),
            );
        }
        ToolIcon::Pen => {
            fill(
                painter,
                c.pts(&[
                    (0.50, 0.90),
                    (0.26, 0.54),
                    (0.35, 0.32),
                    (0.65, 0.32),
                    (0.74, 0.54),
                ]),
                glow,
                line,
            );
            painter.line_segment([c.point(0.50, 0.60), c.point(0.50, 0.88)], fine);
            painter.circle_filled(c.point(0.50, 0.57), unit * 0.055, primary);
            painter.rect_filled(c.rect(0.35, 0.10, 0.65, 0.26), unit * 0.04, primary);
        }
        ToolIcon::Pencil => {
            let half = 0.11;
            fill(painter, c.band(tip, end, 0.24, 0.80, half), accent, line);
            fill(
                painter,
                vec![
                    c.along(tip, end, 0.0, 0.0),
                    c.along(tip, end, 0.24, -half),
                    c.along(tip, end, 0.24, half),
                ],
                soft,
                line,
            );
            fill(
                painter,
                vec![
                    c.along(tip, end, 0.0, 0.0),
                    c.along(tip, end, 0.09, -half * 0.38),
                    c.along(tip, end, 0.09, half * 0.38),
                ],
                primary,
                none,
            );
            fill(painter, c.band(tip, end, 0.80, 0.86, half), primary, none);
            fill(painter, c.band(tip, end, 0.86, 0.99, half), soft, line);
        }
        ToolIcon::Brush => {
            fill(painter, c.band(tip, end, 0.52, 1.0, 0.05), primary, none);
            fill(painter, c.band(tip, end, 0.38, 0.52, 0.085), soft, line);
            fill(
                painter,
                vec![
                    c.along(tip, end, 0.0, 0.0),
                    c.along(tip, end, 0.12, -0.075),
                    c.along(tip, end, 0.38, -0.088),
                    c.along(tip, end, 0.38, 0.088),
                    c.along(tip, end, 0.12, 0.075),
                ],
                accent,
                fine,
            );
        }
        ToolIcon::Eraser => {
            let (a, b) = ((0.14, 0.66), (0.86, 0.34));
            fill(painter, c.band(a, b, 0.0, 0.42, 0.17), accent, none);
            fill(painter, c.band(a, b, 0.42, 1.0, 0.17), soft, none);
            fill(
                painter,
                c.band(a, b, 0.0, 1.0, 0.17),
                Color32::TRANSPARENT,
                line,
            );
            painter.line_segment(
                [c.along(a, b, 0.42, -0.17), c.along(a, b, 0.42, 0.17)],
                fine,
            );
            painter.line_segment(
                [c.point(0.10, 0.92), c.point(0.60, 0.92)],
                Stroke::new(width * 0.8, primary.gamma_multiply(0.55)),
            );
        }
        ToolIcon::Knife => {
            fill(
                painter,
                vec![
                    c.along(tip, end, 0.0, 0.0),
                    c.along(tip, end, 0.6, -0.10),
                    c.along(tip, end, 0.6, 0.03),
                    c.along(tip, end, 0.3, 0.055),
                ],
                soft,
                line,
            );
            painter.line_segment(
                [
                    c.along(tip, end, 0.10, 0.03),
                    c.along(tip, end, 0.52, 0.045),
                ],
                Stroke::new(width * 0.9, accent),
            );
            fill(painter, c.band(tip, end, 0.63, 0.98, 0.075), primary, none);
        }
        ToolIcon::Line => {
            painter.line_segment([c.point(0.17, 0.83), c.point(0.83, 0.17)], line);
            for (x, y) in [(0.17, 0.83), (0.83, 0.17)] {
                painter.circle(c.point(x, y), unit * 0.085, accent, fine);
            }
        }
        ToolIcon::Rectangle => {
            painter.rect(c.rect(0.12, 0.22, 0.88, 0.78), unit * 0.07, glow, line);
        }
        ToolIcon::Oval => {
            fill(painter, c.ellipse(0.5, 0.5, 0.38, 0.29), glow, line);
        }
        ToolIcon::Bucket => {
            // A pail tipped to the right, paint running from its lip.
            let (cx, cy, turn) = (0.42, 0.50, 0.55_f32);
            let rotate = |x: f32, y: f32| {
                let (sin, cos) = turn.sin_cos();
                (cx + x * cos - y * sin, cy + x * sin + y * cos)
            };
            let corners = [
                rotate(-0.25, -0.20),
                rotate(0.25, -0.20),
                rotate(0.19, 0.22),
                rotate(-0.19, 0.22),
            ];
            fill(painter, c.pts(&corners), soft, line);
            let (left, right) = (corners[0], corners[1]);
            painter.add(CubicBezierShape::from_points_stroke(
                [
                    c.point(left.0, left.1),
                    c.point(left.0 - 0.14, left.1 - 0.30),
                    c.point(right.0 - 0.30, right.1 - 0.26),
                    c.point(right.0, right.1),
                ],
                false,
                Color32::TRANSPARENT,
                fine,
            ));
            fill(
                painter,
                c.pts(&[(0.80, 0.50), (0.88, 0.70), (0.72, 0.70)]),
                accent,
                none,
            );
            painter.circle_filled(c.point(0.80, 0.73), unit * 0.085, accent);
            painter.line_segment(
                [c.point(right.0, right.1), c.point(0.80, 0.52)],
                Stroke::new(width, accent),
            );
        }
        ToolIcon::Eyedropper => {
            fill(
                painter,
                c.capsule(
                    (0.13 + 0.74 * 0.80, 0.87 - 0.74 * 0.80),
                    (0.13 + 0.74 * 0.96, 0.87 - 0.74 * 0.96),
                    0.10,
                ),
                primary,
                none,
            );
            fill(painter, c.band(tip, end, 0.70, 0.78, 0.12), primary, none);
            fill(painter, c.band(tip, end, 0.12, 0.70, 0.065), soft, line);
            fill(painter, c.band(tip, end, 0.15, 0.42, 0.040), accent, none);
            painter.line_segment(
                [c.along(tip, end, 0.0, 0.0), c.along(tip, end, 0.12, 0.0)],
                line,
            );
        }
        ToolIcon::Text => {
            painter.rect_filled(c.rect(0.10, 0.14, 0.72, 0.26), unit * 0.02, primary);
            painter.rect_filled(c.rect(0.10, 0.14, 0.17, 0.36), unit * 0.02, primary);
            painter.rect_filled(c.rect(0.65, 0.14, 0.72, 0.36), unit * 0.02, primary);
            painter.rect_filled(c.rect(0.34, 0.14, 0.48, 0.86), 0.0, primary);
            painter.rect_filled(c.rect(0.25, 0.80, 0.57, 0.88), unit * 0.02, primary);
            let caret = Stroke::new(width, accent);
            painter.line_segment([c.point(0.86, 0.40), c.point(0.86, 0.90)], caret);
            painter.line_segment([c.point(0.80, 0.40), c.point(0.92, 0.40)], caret);
            painter.line_segment([c.point(0.80, 0.90), c.point(0.92, 0.90)], caret);
        }
        ToolIcon::Rig => {
            let joints = [(0.20, 0.78), (0.50, 0.47), (0.82, 0.20)];
            for pair in joints.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                fill(
                    painter,
                    vec![
                        c.point(a.0, a.1),
                        c.along(a, b, 0.25, 0.08),
                        c.point(b.0, b.1),
                        c.along(a, b, 0.25, -0.08),
                    ],
                    soft,
                    line,
                );
            }
            for (x, y) in joints {
                painter.circle(c.point(x, y), unit * 0.09, accent, fine);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_tool_has_its_own_painter_icon() {
        let icons: HashSet<_> = Tool::ALL.into_iter().map(ToolIcon::for_tool).collect();
        assert_eq!(icons.len(), Tool::ALL.len());
    }

    #[test]
    fn selected_tool_keeps_the_same_icon_colours() {
        let text = Color32::from_rgb(230, 230, 230);
        let accent = Color32::from_rgb(200, 16, 46);
        assert_eq!(
            tool_icon_colors(text, accent, false),
            tool_icon_colors(text, accent, true)
        );
    }

    #[test]
    fn hand_tool_uses_a_hand_not_the_selection_pointer() {
        assert_eq!(ToolIcon::for_tool(Tool::Hand), ToolIcon::Hand);
        assert_ne!(
            ToolIcon::for_tool(Tool::Hand),
            ToolIcon::for_tool(Tool::Select)
        );
        assert_ne!(
            ToolIcon::for_tool(Tool::Hand),
            ToolIcon::for_tool(Tool::Subselect)
        );
    }
}
