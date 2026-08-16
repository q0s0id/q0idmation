use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};

use canripper::document::{AssetVisualKind, EditRef, PreviewRef, TreeNode};

const TOOL_BUTTON: f32 = 30.0;
const TOOL_ICON: f32 = 17.0;
const TREE_ICON: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiIcon {
    Open,
    Save,
    SaveAs,
    Export,
    ReplaceImage,
    Extract,
    Theme,
    Folder,
    Vector,
    Bitmap,
    Media,
    Rig,
    Symbol,
    Code,
    Video,
    Audio,
    Binary,
    Document,
}

pub fn toolbar_button(
    ui: &mut Ui,
    icon: UiIcon,
    enabled: bool,
    tooltip: &str,
    accent: Color32,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(TOOL_BUTTON, TOOL_BUTTON), sense);
    let response = response.on_hover_text(tooltip);
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() && enabled {
            ui.painter().rect(
                rect.shrink(1.0),
                4.0,
                visuals.weak_bg_fill,
                Stroke::new(1.0_f32, accent.gamma_multiply(0.75)),
            );
        }
        let foreground = if enabled {
            visuals.text_color()
        } else {
            visuals.text_color().gamma_multiply(0.35)
        };
        let secondary = if enabled {
            accent
        } else {
            accent.gamma_multiply(0.25)
        };
        draw_icon(
            ui.painter(),
            Rect::from_center_size(rect.center(), vec2(TOOL_ICON, TOOL_ICON)),
            icon,
            foreground,
            secondary,
        );
    }
    response
}

pub fn tree_row(ui: &mut Ui, node: &TreeNode, selected: bool, accent: Color32) -> Response {
    let galley = egui::WidgetText::from(node.label.clone()).into_galley(
        ui,
        None,
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let padding = ui.spacing().button_padding;
    let content_height = TREE_ICON.max(galley.size().y);
    let desired = vec2(
        ui.available_width().max(40.0),
        (content_height + padding.y * 2.0).max(ui.spacing().interact_size.y),
    );
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() || response.has_focus() {
            ui.painter().rect(
                rect,
                3.0,
                visuals.weak_bg_fill,
                if selected {
                    Stroke::new(1.0_f32, accent.gamma_multiply(0.72))
                } else {
                    Stroke::NONE
                },
            );
        }
        if selected {
            ui.painter().rect_filled(
                Rect::from_min_max(
                    pos2(rect.left(), rect.top() + 4.0),
                    pos2(rect.left() + 2.0, rect.bottom() - 4.0),
                ),
                1.0,
                accent,
            );
        }
        let content = rect.shrink2(padding);
        let icon_rect = Rect::from_center_size(
            pos2(content.left() + TREE_ICON * 0.5, content.center().y),
            vec2(TREE_ICON, TREE_ICON),
        );
        draw_icon(
            ui.painter(),
            icon_rect,
            icon_for_tree_node(node),
            visuals.text_color(),
            accent,
        );
        ui.painter().galley(
            pos2(
                icon_rect.right() + 6.0,
                content.center().y - galley.size().y * 0.5,
            ),
            galley,
            visuals.text_color(),
        );
    }
    response
}

pub fn tiny_icon(ui: &mut Ui, icon: UiIcon, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
    draw_icon(
        ui.painter(),
        Rect::from_center_size(rect.center(), vec2(14.0, 14.0)),
        icon,
        ui.visuals().text_color(),
        accent,
    );
}

pub fn icon_for_tree_node(node: &TreeNode) -> UiIcon {
    if let Some(edit) = &node.edit {
        return match edit {
            EditRef::QProjectQ0rgScript { .. } => UiIcon::Symbol,
            EditRef::QProjectFrameScript { .. } | EditRef::QProjectQ0lang { .. } => UiIcon::Code,
        };
    }
    match &node.preview {
        Some(PreviewRef::QProjectAsset { kind, .. }) => match kind {
            AssetVisualKind::Bitmap => UiIcon::Bitmap,
            AssetVisualKind::Vector => UiIcon::Vector,
            AssetVisualKind::Media => UiIcon::Media,
            AssetVisualKind::Rig => UiIcon::Rig,
        },
        Some(PreviewRef::QProjectQ0rg { .. }) => UiIcon::Symbol,
        Some(PreviewRef::QProjectEmbedded { .. }) => UiIcon::Document,
        Some(PreviewRef::Q1LegacyBitmap(_) | PreviewRef::Q0LegacyBitmap(_)) => UiIcon::Bitmap,
        Some(PreviewRef::Q0vStandaloneVideo) => UiIcon::Video,
        Some(PreviewRef::Q0vStandaloneAudio) => UiIcon::Audio,
        Some(PreviewRef::SwfTag(_)) => UiIcon::Binary,
        None if !node.children.is_empty() => UiIcon::Folder,
        None => UiIcon::Document,
    }
}

#[derive(Clone, Copy)]
struct Canvas {
    rect: Rect,
}

impl Canvas {
    fn p(self, x: f32, y: f32) -> Pos2 {
        pos2(
            self.rect.left() + self.rect.width() * x,
            self.rect.top() + self.rect.height() * y,
        )
    }

    fn r(self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(self.p(x0, y0), self.p(x1, y1))
    }

    fn line(self, painter: &Painter, points: &[(f32, f32)], stroke: Stroke) {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| self.p(x, y)).collect(),
            stroke,
        ));
    }

    fn polygon(self, painter: &Painter, points: &[(f32, f32)], fill: Color32, stroke: Stroke) {
        painter.add(Shape::convex_polygon(
            points.iter().map(|&(x, y)| self.p(x, y)).collect(),
            fill,
            stroke,
        ));
    }
}

pub fn draw_icon(
    painter: &Painter,
    rect: Rect,
    icon: UiIcon,
    foreground: Color32,
    accent: Color32,
) {
    let c = Canvas {
        rect: rect.shrink(0.75),
    };
    let fg = Stroke::new(1.35_f32, foreground);
    let ac = Stroke::new(1.45_f32, accent);
    match icon {
        UiIcon::Open | UiIcon::Folder => {
            c.line(painter, &[(0.08, 0.30), (0.08, 0.82), (0.92, 0.82)], fg);
            c.line(
                painter,
                &[
                    (0.08, 0.30),
                    (0.34, 0.30),
                    (0.43, 0.18),
                    (0.68, 0.18),
                    (0.75, 0.30),
                ],
                fg,
            );
            c.polygon(
                painter,
                &[(0.10, 0.39), (0.94, 0.39), (0.80, 0.82), (0.08, 0.82)],
                accent.gamma_multiply(0.16),
                ac,
            );
        }
        UiIcon::Save | UiIcon::SaveAs => {
            painter.rect_stroke(c.r(0.13, 0.08, 0.87, 0.92), 1.5, fg);
            painter.rect_stroke(c.r(0.29, 0.08, 0.70, 0.35), 0.75, ac);
            painter.rect_stroke(c.r(0.28, 0.60, 0.72, 0.92), 1.0, fg);
            if icon == UiIcon::SaveAs {
                c.line(
                    painter,
                    &[(0.58, 0.71), (0.91, 0.38)],
                    Stroke::new(2.0_f32, accent),
                );
                c.polygon(
                    painter,
                    &[(0.88, 0.35), (0.95, 0.42), (0.91, 0.46), (0.84, 0.39)],
                    accent,
                    Stroke::NONE,
                );
            }
        }
        UiIcon::Export => {
            painter.rect_stroke(c.r(0.10, 0.12, 0.62, 0.88), 1.0, fg);
            c.line(painter, &[(0.50, 0.50), (0.90, 0.50)], ac);
            c.line(painter, &[(0.72, 0.31), (0.91, 0.50), (0.72, 0.69)], ac);
        }
        UiIcon::ReplaceImage | UiIcon::Bitmap => {
            painter.rect_stroke(c.r(0.08, 0.14, 0.92, 0.86), 1.5, fg);
            painter.circle_filled(c.p(0.73, 0.34), rect.width() * 0.075, accent);
            c.line(
                painter,
                &[
                    (0.13, 0.76),
                    (0.36, 0.49),
                    (0.51, 0.66),
                    (0.64, 0.55),
                    (0.87, 0.77),
                ],
                ac,
            );
            if icon == UiIcon::ReplaceImage {
                c.line(painter, &[(0.54, 0.93), (0.80, 0.93), (0.91, 0.80)], fg);
                c.line(painter, &[(0.91, 0.80), (0.91, 0.94)], fg);
            }
        }
        UiIcon::Extract => {
            painter.rect_stroke(c.r(0.13, 0.50, 0.87, 0.91), 1.2, fg);
            c.line(painter, &[(0.50, 0.08), (0.50, 0.66)], ac);
            c.line(painter, &[(0.29, 0.45), (0.50, 0.67), (0.71, 0.45)], ac);
        }
        UiIcon::Theme => {
            painter.circle_stroke(c.p(0.48, 0.51), rect.width() * 0.37, ac);
            painter.circle_filled(c.p(0.35, 0.31), rect.width() * 0.055, foreground);
            painter.circle_filled(c.p(0.57, 0.27), rect.width() * 0.055, accent);
            painter.circle_filled(c.p(0.70, 0.46), rect.width() * 0.055, foreground);
            painter.circle_filled(c.p(0.28, 0.58), rect.width() * 0.055, accent);
            painter.circle_filled(
                c.p(0.56, 0.74),
                rect.width() * 0.13,
                painter.ctx().style().visuals.panel_fill,
            );
        }
        UiIcon::Vector => {
            let a = c.p(0.12, 0.76);
            let b = c.p(0.49, 0.20);
            let d = c.p(0.88, 0.70);
            let h1 = c.p(0.27, 0.33);
            let h2 = c.p(0.72, 0.31);
            painter.line_segment(
                [a, h1],
                Stroke::new(0.8_f32, foreground.gamma_multiply(0.55)),
            );
            painter.line_segment(
                [b, h2],
                Stroke::new(0.8_f32, foreground.gamma_multiply(0.55)),
            );
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [a, h1, h2, d],
                false,
                Color32::TRANSPARENT,
                ac,
            ));
            for p in [a, b, d] {
                painter.circle_filled(p, rect.width() * 0.075, foreground);
                painter.circle_stroke(p, rect.width() * 0.075, Stroke::new(0.7_f32, accent));
            }
        }
        UiIcon::Media | UiIcon::Video => {
            painter.rect_stroke(c.r(0.08, 0.16, 0.92, 0.84), 1.5, fg);
            c.polygon(
                painter,
                &[(0.42, 0.34), (0.42, 0.68), (0.70, 0.51)],
                accent,
                Stroke::NONE,
            );
            if icon == UiIcon::Media {
                for x in [0.16, 0.84] {
                    painter
                        .line_segment([c.p(x, 0.18), c.p(x, 0.82)], Stroke::new(1.0_f32, accent));
                }
            }
        }
        UiIcon::Rig => {
            let a = c.p(0.20, 0.75);
            let b = c.p(0.48, 0.50);
            let d = c.p(0.78, 0.23);
            painter.line_segment([a, b], Stroke::new(2.2_f32, foreground));
            painter.line_segment([b, d], Stroke::new(2.2_f32, accent));
            for p in [a, b, d] {
                painter.circle_filled(
                    p,
                    rect.width() * 0.09,
                    painter.ctx().style().visuals.panel_fill,
                );
                painter.circle_stroke(p, rect.width() * 0.09, ac);
            }
        }
        UiIcon::Symbol => {
            painter.rect_stroke(c.r(0.28, 0.12, 0.88, 0.72), 1.5, fg);
            painter.rect_stroke(c.r(0.10, 0.30, 0.70, 0.90), 1.5, ac);
        }
        UiIcon::Code => {
            c.line(painter, &[(0.36, 0.24), (0.13, 0.50), (0.36, 0.76)], ac);
            c.line(painter, &[(0.64, 0.24), (0.87, 0.50), (0.64, 0.76)], ac);
            painter.line_segment([c.p(0.57, 0.16), c.p(0.43, 0.84)], fg);
        }
        UiIcon::Audio => {
            let bars = [
                (0.16, 0.36),
                (0.30, 0.62),
                (0.44, 0.82),
                (0.58, 0.54),
                (0.72, 0.72),
                (0.86, 0.30),
            ];
            for (index, (x, height)) in bars.into_iter().enumerate() {
                let color = if index % 2 == 0 { fg } else { ac };
                painter.line_segment(
                    [c.p(x, 0.5 - height * 0.35), c.p(x, 0.5 + height * 0.35)],
                    color,
                );
            }
        }
        UiIcon::Binary => {
            for y in 0..3 {
                for x in 0..3 {
                    let center = c.p(0.25 + x as f32 * 0.25, 0.25 + y as f32 * 0.25);
                    let on = (x + y) % 2 == 0;
                    if on {
                        painter.rect_filled(
                            Rect::from_center_size(center, vec2(3.0, 3.0)),
                            0.5,
                            accent,
                        );
                    } else {
                        painter.circle_stroke(center, 1.6, fg);
                    }
                }
            }
        }
        UiIcon::Document => {
            c.polygon(
                painter,
                &[
                    (0.18, 0.08),
                    (0.65, 0.08),
                    (0.85, 0.28),
                    (0.85, 0.92),
                    (0.18, 0.92),
                ],
                Color32::TRANSPARENT,
                fg,
            );
            c.line(painter, &[(0.65, 0.08), (0.65, 0.29), (0.85, 0.29)], ac);
            c.line(painter, &[(0.31, 0.50), (0.72, 0.50)], fg);
            c.line(painter, &[(0.31, 0.66), (0.66, 0.66)], fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canripper::document::TreeNode;

    #[test]
    fn resource_kinds_map_to_typed_vector_icons() {
        let make = |preview| TreeNode {
            id: 0,
            label: String::new(),
            detail: String::new(),
            preview: Some(preview),
            edit: None,
            children: Vec::new(),
        };
        assert_eq!(
            icon_for_tree_node(&make(PreviewRef::QProjectAsset {
                path: Vec::new(),
                asset_id: 1,
                kind: AssetVisualKind::Vector,
            })),
            UiIcon::Vector
        );
        assert_eq!(
            icon_for_tree_node(&make(PreviewRef::QProjectAsset {
                path: Vec::new(),
                asset_id: 2,
                kind: AssetVisualKind::Bitmap,
            })),
            UiIcon::Bitmap
        );
        assert_eq!(
            icon_for_tree_node(&make(PreviewRef::Q0vStandaloneAudio)),
            UiIcon::Audio
        );
    }
}
