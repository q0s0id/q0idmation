use egui::epaint::PathShape;
use egui::{
    pos2, vec2, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, WidgetInfo,
    WidgetType,
};

use crate::app::{Action, EditorApp};
use crate::state::Tool;

const TOOL_BUTTON_SIZE: f32 = 30.0;
const TOOL_ICON_SIZE: f32 = 19.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ToolIcon {
    SelectPointer,
    Hand,
    SubselectPointer,
    Pen,
    Pencil,
    Brush,
    Eraser,
    Line,
    Rectangle,
    Oval,
    Bucket,
    Eyedropper,
}

impl ToolIcon {
    const fn for_tool(tool: Tool) -> Self {
        match tool {
            Tool::Select => Self::SelectPointer,
            Tool::Hand => Self::Hand,
            Tool::Subselect => Self::SubselectPointer,
            Tool::Pen => Self::Pen,
            Tool::Pencil => Self::Pencil,
            Tool::Brush => Self::Brush,
            Tool::Eraser => Self::Eraser,
            Tool::Line => Self::Line,
            Tool::Rectangle => Self::Rectangle,
            Tool::Oval => Self::Oval,
            Tool::Bucket => Self::Bucket,
            Tool::Eyedropper => Self::Eyedropper,
        }
    }
}

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(4.0);
        for tool in Tool::ALL {
            let selected = app.session.current_tool == tool;
            let response = tool_button(
                ui,
                tool,
                selected,
                app.settings.theme.text.to_color32(),
                app.settings.theme.panel.to_color32(),
                app.settings.theme.accent.to_color32(),
            )
            .on_hover_text(format!("{} ({})", tool.label(), tool.glyph()));
            if response.clicked() {
                app.queue(Action::SelectTool(tool));
            }
        }
        ui.add_space(8.0);
        ui.separator();
    });
}

fn tool_button(
    ui: &mut Ui,
    tool: Tool,
    selected: bool,
    text: Color32,
    panel: Color32,
    accent: Color32,
) -> Response {
    let desired_size = vec2(TOOL_BUTTON_SIZE, TOOL_BUTTON_SIZE);
    let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            selected,
            tool.label().to_string(),
        )
    });

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.highlighted() || response.has_focus();
        if selected {
            let background = blend_rgb(panel, accent, 0.30);
            ui.painter().rect(
                rect.shrink(1.0),
                3.0,
                background,
                Stroke::new(1.5_f32, accent),
            );
            ui.painter().rect_filled(
                Rect::from_min_max(
                    pos2(rect.left() + 1.0, rect.top() + 4.0),
                    pos2(rect.left() + 3.5, rect.bottom() - 4.0),
                ),
                1.25,
                accent,
            );
        } else if hovered {
            let visuals = ui.style().interact(&response);
            ui.painter().rect(
                rect.shrink(1.0),
                visuals.rounding,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
            );
        }

        let icon_rect = Rect::from_center_size(rect.center(), vec2(TOOL_ICON_SIZE, TOOL_ICON_SIZE));
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

fn blend_rgb(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

fn tool_icon_colors(text: Color32, accent: Color32, _selected: bool) -> (Color32, Color32) {
    // Selection belongs to the button chrome, never to the icon silhouette.
    // Keeping these stable prevents the icon from visually changing shape
    // or losing contrast when the user selects a tool.
    (text, accent)
}

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

    fn polyline(self, painter: &Painter, points: &[(f32, f32)], stroke: Stroke) {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| self.point(x, y)).collect(),
            stroke,
        ));
    }

    fn polygon(self, painter: &Painter, points: &[(f32, f32)], fill: Color32, stroke: Stroke) {
        painter.add(Shape::Path(PathShape {
            points: points.iter().map(|&(x, y)| self.point(x, y)).collect(),
            closed: true,
            fill,
            stroke,
        }));
    }
}

fn draw_tool_icon(
    painter: &Painter,
    rect: Rect,
    icon: ToolIcon,
    primary: Color32,
    secondary: Color32,
) {
    let c = IconCanvas { rect };
    let line = Stroke::new(1.45_f32, primary);
    let fine = Stroke::new(1.05_f32, secondary);

    match icon {
        ToolIcon::SelectPointer => draw_select_pointer(painter, c, primary, secondary),
        ToolIcon::Hand => draw_hand(painter, c, primary, secondary),
        ToolIcon::SubselectPointer => draw_subselect_pointer(painter, c, primary, secondary),
        ToolIcon::Pen => draw_pen(painter, c, primary, secondary),
        ToolIcon::Pencil => draw_pencil(painter, c, primary, secondary),
        ToolIcon::Brush => draw_brush(painter, c, primary, secondary),
        ToolIcon::Eraser => draw_eraser(painter, c, primary, secondary),
        ToolIcon::Line => {
            painter.line_segment([c.point(0.15, 0.82), c.point(0.85, 0.18)], line);
            painter.circle_filled(c.point(0.15, 0.82), 1.7, secondary);
            painter.circle_filled(c.point(0.85, 0.18), 1.7, secondary);
        }
        ToolIcon::Rectangle => {
            painter.rect_stroke(c.rect(0.14, 0.22, 0.86, 0.78), 1.5, line);
            painter.line_segment([c.point(0.14, 0.22), c.point(0.30, 0.22)], fine);
        }
        ToolIcon::Oval => {
            let points: Vec<Pos2> = (0..=28)
                .map(|index| {
                    let angle = std::f32::consts::TAU * index as f32 / 28.0;
                    c.point(0.5 + angle.cos() * 0.37, 0.5 + angle.sin() * 0.29)
                })
                .collect();
            painter.add(Shape::line(points, line));
            painter.circle_filled(c.point(0.50, 0.21), 1.4, secondary);
        }
        ToolIcon::Bucket => draw_bucket(painter, c, primary, secondary),
        ToolIcon::Eyedropper => draw_eyedropper(painter, c, primary, secondary),
    }
}

fn draw_select_pointer(painter: &Painter, c: IconCanvas, primary: Color32, accent: Color32) {
    c.polygon(
        painter,
        &[
            (0.17, 0.10),
            (0.74, 0.58),
            (0.51, 0.61),
            (0.68, 0.88),
            (0.55, 0.96),
            (0.39, 0.67),
            (0.23, 0.84),
        ],
        primary,
        Stroke::new(0.9_f32, accent),
    );
}

fn draw_subselect_pointer(painter: &Painter, c: IconCanvas, primary: Color32, accent: Color32) {
    c.polygon(
        painter,
        &[
            (0.16, 0.10),
            (0.68, 0.54),
            (0.48, 0.58),
            (0.62, 0.82),
            (0.51, 0.89),
            (0.36, 0.64),
            (0.22, 0.78),
        ],
        Color32::TRANSPARENT,
        Stroke::new(1.45_f32, primary),
    );
    painter.rect_filled(c.rect(0.67, 0.13, 0.88, 0.34), 1.0, accent);
    painter.rect_stroke(
        c.rect(0.67, 0.13, 0.88, 0.34),
        1.0,
        Stroke::new(0.9_f32, primary),
    );
}

fn draw_hand(painter: &Painter, c: IconCanvas, primary: Color32, accent: Color32) {
    // A real open palm: four separate fingers, a side thumb, palm and wrist.
    // This deliberately does not reuse the four-way move-arrow silhouette.
    let finger_width = c.rect.width() * 0.15;
    let finger_radius = finger_width * 0.5;
    for (x, top, bottom) in [
        (0.35, 0.19, 0.55),
        (0.47, 0.08, 0.55),
        (0.59, 0.13, 0.55),
        (0.70, 0.23, 0.58),
    ] {
        painter.line_segment(
            [c.point(x, top + 0.04), c.point(x, bottom)],
            Stroke::new(finger_width, primary),
        );
        painter.circle_filled(c.point(x, top + 0.04), finger_radius, primary);
    }
    painter.rect_filled(c.rect(0.28, 0.43, 0.76, 0.82), finger_radius, primary);
    painter.line_segment(
        [c.point(0.32, 0.62), c.point(0.13, 0.48)],
        Stroke::new(finger_width * 1.05, primary),
    );
    painter.circle_filled(c.point(0.13, 0.48), finger_radius * 1.05, primary);
    painter.rect_filled(c.rect(0.38, 0.76, 0.66, 0.95), 1.5, primary);
    painter.line_segment(
        [c.point(0.34, 0.78), c.point(0.70, 0.78)],
        Stroke::new(1.0_f32, accent),
    );
}

fn draw_pen(painter: &Painter, c: IconCanvas, primary: Color32, accent: Color32) {
    c.polygon(
        painter,
        &[(0.50, 0.08), (0.78, 0.42), (0.50, 0.90), (0.22, 0.42)],
        Color32::TRANSPARENT,
        Stroke::new(1.45_f32, primary),
    );
    painter.line_segment(
        [c.point(0.50, 0.14), c.point(0.50, 0.59)],
        Stroke::new(1.2_f32, accent),
    );
    painter.circle_stroke(c.point(0.50, 0.55), 2.0, Stroke::new(1.0_f32, primary));
    c.polyline(
        painter,
        &[(0.22, 0.42), (0.50, 0.55), (0.78, 0.42)],
        Stroke::new(1.0_f32, accent),
    );
}

fn draw_pencil(painter: &Painter, c: IconCanvas, primary: Color32, accent: Color32) {
    c.polygon(
        painter,
        &[(0.16, 0.75), (0.62, 0.19), (0.81, 0.35), (0.34, 0.91)],
        Color32::TRANSPARENT,
        Stroke::new(1.45_f32, primary),
    );
    c.polygon(
        painter,
        &[(0.16, 0.75), (0.34, 0.91), (0.10, 0.96)],
        accent,
        Stroke::new(0.8_f32, primary),
    );
    painter.line_segment(
        [c.point(0.57, 0.25), c.point(0.76, 0.41)],
        Stroke::new(1.1_f32, accent),
    );
}

fn draw_brush(painter: &Painter, c: IconCanvas, primary: Color32, accent: Color32) {
    painter.line_segment(
        [c.point(0.66, 0.16), c.point(0.42, 0.58)],
        Stroke::new(4.2_f32, primary),
    );
    painter.line_segment(
        [c.point(0.68, 0.13), c.point(0.43, 0.55)],
        Stroke::new(1.1_f32, accent),
    );
    c.polygon(
        painter,
        &[
            (0.35, 0.48),
            (0.55, 0.64),
            (0.38, 0.90),
            (0.12, 0.92),
            (0.25, 0.75),
        ],
        primary,
        Stroke::new(0.9_f32, accent),
    );
}

fn draw_eraser(painter: &Painter, c: IconCanvas, primary: Color32, accent: Color32) {
    c.polygon(
        painter,
        &[(0.18, 0.66), (0.52, 0.20), (0.84, 0.45), (0.50, 0.88)],
        Color32::TRANSPARENT,
        Stroke::new(1.5_f32, primary),
    );
    painter.line_segment(
        [c.point(0.34, 0.45), c.point(0.67, 0.70)],
        Stroke::new(1.15_f32, accent),
    );
    painter.line_segment(
        [c.point(0.13, 0.91), c.point(0.57, 0.91)],
        Stroke::new(1.15_f32, accent),
    );
}

fn draw_bucket(painter: &Painter, c: IconCanvas, primary: Color32, accent: Color32) {
    // Classic tilted paint bucket: oval rim, tapered body, arched handle,
    // visible paint at the lip and one falling drop.
    let rim: Vec<Pos2> = (0..=20)
        .map(|index| {
            let angle = std::f32::consts::TAU * index as f32 / 20.0;
            c.point(
                0.43 + angle.cos() * 0.25,
                0.34 + angle.sin() * 0.105 + angle.cos() * -0.055,
            )
        })
        .collect();
    painter.add(Shape::line(rim, Stroke::new(1.45_f32, primary)));

    c.polygon(
        painter,
        &[(0.19, 0.34), (0.67, 0.23), (0.76, 0.65), (0.34, 0.78)],
        Color32::TRANSPARENT,
        Stroke::new(1.45_f32, primary),
    );
    painter.line_segment(
        [c.point(0.31, 0.69), c.point(0.74, 0.57)],
        Stroke::new(1.0_f32, primary),
    );

    let handle: Vec<Pos2> = (0..=18)
        .map(|index| {
            let t = index as f32 / 18.0;
            let angle = std::f32::consts::PI * (1.08 + 0.84 * t);
            c.point(0.44 + angle.cos() * 0.31, 0.34 + angle.sin() * 0.34)
        })
        .collect();
    painter.add(Shape::line(handle, Stroke::new(1.15_f32, primary)));
    painter.circle_filled(c.point(0.14, 0.39), 1.5, primary);
    painter.circle_filled(c.point(0.70, 0.26), 1.5, primary);

    c.polyline(
        painter,
        &[(0.23, 0.35), (0.36, 0.37), (0.48, 0.32), (0.62, 0.31)],
        Stroke::new(2.0_f32, accent),
    );
    painter.line_segment(
        [c.point(0.69, 0.35), c.point(0.80, 0.58)],
        Stroke::new(2.0_f32, accent),
    );
    painter.circle_filled(c.point(0.84, 0.72), 2.2, accent);
}

fn draw_eyedropper(painter: &Painter, c: IconCanvas, primary: Color32, accent: Color32) {
    painter.line_segment(
        [c.point(0.68, 0.21), c.point(0.29, 0.67)],
        Stroke::new(4.0_f32, primary),
    );
    painter.line_segment(
        [c.point(0.68, 0.21), c.point(0.29, 0.67)],
        Stroke::new(1.1_f32, accent),
    );
    painter.circle_stroke(c.point(0.73, 0.17), 4.0, Stroke::new(1.4_f32, primary));
    c.polygon(
        painter,
        &[(0.24, 0.62), (0.34, 0.71), (0.14, 0.91)],
        accent,
        Stroke::new(0.8_f32, primary),
    );
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
