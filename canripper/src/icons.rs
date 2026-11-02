//! Small colour icons in the manner of 2000s desktop software: a yellow
//! folder, a picture frame, a floppy disk, a speaker. They are painted from
//! shapes on a 16-unit grid, so they stay sharp at any size and need no
//! image files.

use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke};

use crate::document::{AssetVisualKind, Document, DocumentData, EditRef, PreviewRef, TreeNode};
use crate::swf_assets::Category;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiIcon {
    // Toolbar.
    Open,
    Save,
    SaveAs,
    Export,
    ToQ1s,
    ReplaceImage,
    Extract,
    Theme,
    About,
    // Transport and view.
    Play,
    Pause,
    First,
    Previous,
    Next,
    Last,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    ZoomActual,
    Search,
    Clear,
    // Tree.
    Folder,
    FolderOpen,
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
    Shape,
    MorphShape,
    Sprite,
    Button,
    Text,
    Font,
    Frame,
    Movie,
    Other,
}

const FOLDER: Color32 = Color32::from_rgb(0xF2, 0xC9, 0x4C);
const FOLDER_DARK: Color32 = Color32::from_rgb(0xA8, 0x80, 0x1E);
const PAPER: Color32 = Color32::from_rgb(0xFC, 0xFC, 0xF8);
const INK: Color32 = Color32::from_rgb(0x2B, 0x2B, 0x2B);
const BLUE: Color32 = Color32::from_rgb(0x2F, 0x6F, 0xD1);
const SKY: Color32 = Color32::from_rgb(0x8C, 0xC4, 0xF2);
const GREEN: Color32 = Color32::from_rgb(0x3B, 0xA5, 0x3A);
const RED: Color32 = Color32::from_rgb(0xD1, 0x3B, 0x33);
const ORANGE: Color32 = Color32::from_rgb(0xF0, 0x8A, 0x24);
const PURPLE: Color32 = Color32::from_rgb(0x7D, 0x55, 0xC7);
const GREY: Color32 = Color32::from_rgb(0x8A, 0x8F, 0x96);

/// Which icon a tree node shows. SWF tags show what kind of tag they are.
pub fn icon_for_tree_node(node: &TreeNode, document: Option<&Document>, open: bool) -> UiIcon {
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
        Some(PreviewRef::SwfTimeline) => UiIcon::Movie,
        Some(PreviewRef::SwfFrame(_)) => UiIcon::Frame,
        Some(PreviewRef::SwfTag(index)) => {
            let code = document.and_then(|document| match &document.data {
                DocumentData::Swf(movie) | DocumentData::Projector { movie, .. } => {
                    movie.tags.get(*index).map(|tag| tag.code)
                }
                _ => None,
            });
            match code.map(Category::of) {
                Some(Category::Shape) => UiIcon::Shape,
                Some(Category::MorphShape) => UiIcon::MorphShape,
                Some(Category::Sprite) => UiIcon::Sprite,
                Some(Category::Button) => UiIcon::Button,
                Some(Category::Image) => UiIcon::Bitmap,
                Some(Category::Text) => UiIcon::Text,
                Some(Category::Font) => UiIcon::Font,
                Some(Category::Sound) => UiIcon::Audio,
                Some(Category::Video) => UiIcon::Video,
                Some(Category::BinaryData) => UiIcon::Binary,
                Some(Category::Script) => UiIcon::Code,
                Some(Category::Control) if code == Some(1) => UiIcon::Frame,
                _ => UiIcon::Other,
            }
        }
        None if !node.children.is_empty() && open => UiIcon::FolderOpen,
        None if !node.children.is_empty() => UiIcon::Folder,
        None => UiIcon::Document,
    }
}

/// Points on the icon's 16-unit grid.
#[derive(Clone, Copy)]
struct Grid {
    rect: Rect,
}

impl Grid {
    fn p(self, x: f32, y: f32) -> Pos2 {
        pos2(
            self.rect.left() + self.rect.width() * x / 16.0,
            self.rect.top() + self.rect.height() * y / 16.0,
        )
    }

    fn r(self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(self.p(x0, y0), self.p(x1, y1))
    }

    fn unit(self) -> f32 {
        self.rect.width() / 16.0
    }

    fn poly(self, painter: &Painter, points: &[(f32, f32)], fill: Color32, outline: Color32) {
        painter.add(Shape::convex_polygon(
            points.iter().map(|&(x, y)| self.p(x, y)).collect(),
            fill,
            Stroke::new(self.unit().max(0.8), outline),
        ));
    }

    fn line(self, painter: &Painter, points: &[(f32, f32)], width: f32, color: Color32) {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| self.p(x, y)).collect(),
            Stroke::new(width * self.unit(), color),
        ));
    }

    fn boxed(self, painter: &Painter, rect: Rect, fill: Color32, outline: Color32, round: f32) {
        painter.rect(
            rect,
            round * self.unit(),
            fill,
            Stroke::new(self.unit().max(0.8), outline),
        );
    }

    fn text(self, painter: &Painter, x: f32, y: f32, text: &str, size: f32, color: Color32) {
        painter.text(
            self.p(x, y),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(size * self.unit()),
            color,
        );
    }
}

fn darker(color: Color32) -> Color32 {
    Color32::from_rgb(
        (u16::from(color.r()) * 3 / 5) as u8,
        (u16::from(color.g()) * 3 / 5) as u8,
        (u16::from(color.b()) * 3 / 5) as u8,
    )
}

fn page(painter: &Painter, g: Grid, fill: Color32) {
    g.poly(
        painter,
        &[
            (3.0, 1.0),
            (10.0, 1.0),
            (13.5, 4.5),
            (13.5, 15.0),
            (3.0, 15.0),
        ],
        fill,
        GREY,
    );
    g.poly(
        painter,
        &[(10.0, 1.0), (13.5, 4.5), (10.0, 4.5)],
        Color32::from_gray(220),
        GREY,
    );
}

fn picture(painter: &Painter, g: Grid) {
    g.boxed(
        painter,
        g.r(1.0, 2.5, 15.0, 13.5),
        SKY,
        Color32::from_gray(90),
        0.5,
    );
    g.poly(
        painter,
        &[(2.0, 12.5), (6.5, 7.0), (10.0, 12.5)],
        GREEN,
        darker(GREEN),
    );
    g.poly(
        painter,
        &[(7.0, 12.5), (10.5, 8.5), (14.0, 12.5)],
        darker(GREEN),
        darker(GREEN),
    );
    painter.circle_filled(
        g.p(11.5, 5.5),
        1.3 * g.unit(),
        Color32::from_rgb(0xFF, 0xD3, 0x3A),
    );
}

fn floppy(painter: &Painter, g: Grid) {
    g.poly(
        painter,
        &[
            (1.5, 1.5),
            (13.0, 1.5),
            (14.5, 3.0),
            (14.5, 14.5),
            (1.5, 14.5),
        ],
        BLUE,
        darker(BLUE),
    );
    g.boxed(
        painter,
        g.r(4.0, 1.5, 11.5, 6.0),
        Color32::from_gray(225),
        Color32::from_gray(120),
        0.0,
    );
    painter.rect_filled(g.r(8.8, 2.3, 10.4, 5.2), 0.0, BLUE);
    g.boxed(
        painter,
        g.r(3.5, 8.5, 12.5, 14.5),
        PAPER,
        Color32::from_gray(140),
        0.0,
    );
}

fn arrow(painter: &Painter, g: Grid, from: (f32, f32), to: (f32, f32), color: Color32) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / length, dy / length);
    let back = (to.0 - ux * 3.5, to.1 - uy * 3.5);
    g.line(painter, &[from, back], 2.0, color);
    g.poly(
        painter,
        &[
            to,
            (back.0 - uy * 2.6, back.1 + ux * 2.6),
            (back.0 + uy * 2.6, back.1 - ux * 2.6),
        ],
        color,
        darker(color),
    );
}

fn magnifier(painter: &Painter, g: Grid) {
    g.line(
        painter,
        &[(9.5, 9.5), (14.5, 14.5)],
        2.6,
        Color32::from_rgb(0x6B, 0x4B, 0x2A),
    );
    painter.circle(
        g.p(6.5, 6.5),
        5.0 * g.unit(),
        Color32::from_rgba_unmultiplied(0xCF, 0xE8, 0xFF, 200),
        Stroke::new(1.4 * g.unit(), Color32::from_gray(80)),
    );
}

/// Paints `icon` in `rect`. Disabled icons are washed out with the panel
/// colour, as greyed toolbar icons were.
pub fn draw_icon(painter: &Painter, rect: Rect, icon: UiIcon, enabled: bool) {
    paint_icon(painter, rect, icon);
    if !enabled {
        let face = painter.ctx().style().visuals.panel_fill;
        painter.rect_filled(rect.expand(1.0), 0.0, face.gamma_multiply(0.62));
    }
}

fn paint_icon(painter: &Painter, rect: Rect, icon: UiIcon) {
    let g = Grid { rect };
    let u = g.unit();
    match icon {
        UiIcon::Folder | UiIcon::Open => {
            g.poly(
                painter,
                &[
                    (1.0, 3.0),
                    (6.0, 3.0),
                    (7.5, 4.5),
                    (15.0, 4.5),
                    (15.0, 13.5),
                    (1.0, 13.5),
                ],
                FOLDER,
                FOLDER_DARK,
            );
            painter.line_segment(
                [g.p(1.5, 6.0), g.p(14.5, 6.0)],
                Stroke::new(u, Color32::from_rgb(0xFF, 0xE6, 0x96)),
            );
            if icon == UiIcon::Open {
                arrow(painter, g, (8.0, 12.0), (8.0, 1.0), GREEN);
            }
        }
        UiIcon::FolderOpen => {
            g.poly(
                painter,
                &[
                    (1.0, 3.0),
                    (6.0, 3.0),
                    (7.5, 4.5),
                    (13.0, 4.5),
                    (13.0, 13.5),
                    (1.0, 13.5),
                ],
                FOLDER_DARK,
                darker(FOLDER_DARK),
            );
            g.poly(
                painter,
                &[(3.5, 7.0), (15.5, 7.0), (13.0, 13.5), (1.0, 13.5)],
                FOLDER,
                FOLDER_DARK,
            );
        }
        UiIcon::Save => floppy(painter, g),
        UiIcon::SaveAs => {
            floppy(painter, g);
            g.line(painter, &[(7.5, 15.0), (15.0, 7.5)], 2.6, ORANGE);
            g.line(
                painter,
                &[(14.0, 8.5), (15.5, 7.0)],
                2.6,
                Color32::from_rgb(0xF6, 0xA0, 0xA0),
            );
        }
        UiIcon::Export => {
            page(painter, g, PAPER);
            arrow(painter, g, (5.0, 10.5), (15.5, 10.5), GREEN);
        }
        UiIcon::ToQ1s => {
            page(painter, g, PAPER);
            g.text(painter, 8.0, 8.5, "q1", 6.0, RED);
        }
        UiIcon::Extract => {
            g.poly(
                painter,
                &[(1.5, 8.0), (14.5, 8.0), (14.5, 15.0), (1.5, 15.0)],
                Color32::from_rgb(0xC8, 0x9B, 0x5E),
                Color32::from_rgb(0x7A, 0x55, 0x26),
            );
            painter.line_segment(
                [g.p(1.5, 10.5), g.p(14.5, 10.5)],
                Stroke::new(u, Color32::from_rgb(0x7A, 0x55, 0x26)),
            );
            arrow(painter, g, (8.0, 0.5), (8.0, 9.5), GREEN);
        }
        UiIcon::ReplaceImage => {
            picture(painter, g);
            arrow(painter, g, (4.0, 15.0), (12.0, 15.0), ORANGE);
        }
        UiIcon::Theme => {
            painter.circle(
                g.p(8.0, 8.0),
                7.0 * u,
                Color32::from_rgb(0xE8, 0xD2, 0xA8),
                Stroke::new(u, Color32::from_rgb(0x8A, 0x6A, 0x3A)),
            );
            for (x, y, color) in [
                (5.0, 5.0, RED),
                (9.0, 4.0, BLUE),
                (12.0, 7.0, GREEN),
                (4.5, 9.5, ORANGE),
            ] {
                painter.circle_filled(g.p(x, y), 1.4 * u, color);
            }
            painter.circle_filled(
                g.p(9.5, 11.0),
                1.8 * u,
                painter.ctx().style().visuals.panel_fill,
            );
        }
        UiIcon::About => {
            painter.circle(g.p(8.0, 8.0), 7.0 * u, BLUE, Stroke::new(u, darker(BLUE)));
            g.text(painter, 8.0, 8.3, "i", 11.0, Color32::WHITE);
        }
        UiIcon::Play => g.poly(
            painter,
            &[(4.0, 2.0), (14.0, 8.0), (4.0, 14.0)],
            GREEN,
            darker(GREEN),
        ),
        UiIcon::Pause => {
            g.boxed(painter, g.r(3.5, 2.5, 7.0, 13.5), BLUE, darker(BLUE), 0.5);
            g.boxed(painter, g.r(9.0, 2.5, 12.5, 13.5), BLUE, darker(BLUE), 0.5);
        }
        UiIcon::First | UiIcon::Last | UiIcon::Previous | UiIcon::Next => {
            let left = matches!(icon, UiIcon::First | UiIcon::Previous);
            let tri: [(f32, f32); 3] = if left {
                [(13.0, 3.0), (13.0, 13.0), (5.0, 8.0)]
            } else {
                [(3.0, 3.0), (3.0, 13.0), (11.0, 8.0)]
            };
            g.poly(painter, &tri, INK, INK);
            if matches!(icon, UiIcon::First | UiIcon::Last) {
                let x = if left { 2.5 } else { 11.5 };
                painter.rect_filled(g.r(x, 3.0, x + 2.0, 13.0), 0.0, INK);
            }
        }
        UiIcon::ZoomIn | UiIcon::ZoomOut => {
            magnifier(painter, g);
            g.line(painter, &[(4.0, 6.5), (9.0, 6.5)], 1.6, INK);
            if icon == UiIcon::ZoomIn {
                g.line(painter, &[(6.5, 4.0), (6.5, 9.0)], 1.6, INK);
            }
        }
        UiIcon::Search => magnifier(painter, g),
        UiIcon::ZoomFit => {
            for (cx, cy, sx, sy) in [
                (1.5, 1.5, 1.0, 1.0),
                (14.5, 1.5, -1.0, 1.0),
                (1.5, 14.5, 1.0, -1.0),
                (14.5, 14.5, -1.0, -1.0),
            ] {
                g.line(
                    painter,
                    &[(cx + sx * 4.5, cy), (cx, cy), (cx, cy + sy * 4.5)],
                    1.8,
                    INK,
                );
            }
            g.boxed(painter, g.r(5.0, 5.0, 11.0, 11.0), SKY, BLUE, 0.5);
        }
        UiIcon::ZoomActual => {
            g.boxed(painter, g.r(1.0, 2.0, 15.0, 14.0), PAPER, GREY, 1.0);
            g.text(painter, 8.0, 8.2, "1:1", 7.0, INK);
        }
        UiIcon::Clear => {
            painter.circle_filled(g.p(8.0, 8.0), 6.5 * u, Color32::from_gray(150));
            g.line(painter, &[(5.0, 5.0), (11.0, 11.0)], 1.8, Color32::WHITE);
            g.line(painter, &[(11.0, 5.0), (5.0, 11.0)], 1.8, Color32::WHITE);
        }
        UiIcon::Vector => {
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [
                    g.p(2.0, 13.0),
                    g.p(3.0, 2.0),
                    g.p(13.0, 14.0),
                    g.p(14.0, 3.0),
                ],
                false,
                Color32::TRANSPARENT,
                Stroke::new(1.6 * u, PURPLE),
            ));
            for (x, y) in [(2.0, 13.0), (14.0, 3.0)] {
                g.boxed(
                    painter,
                    Rect::from_center_size(g.p(x, y), vec2(3.0 * u, 3.0 * u)),
                    PAPER,
                    INK,
                    0.0,
                );
            }
        }
        UiIcon::Shape => {
            painter.circle(
                g.p(6.0, 7.0),
                5.0 * u,
                Color32::from_rgb(0x55, 0x9C, 0xE8),
                Stroke::new(u, darker(BLUE)),
            );
            g.poly(
                painter,
                &[(8.0, 6.0), (15.0, 14.5), (5.0, 14.5)],
                PURPLE,
                darker(PURPLE),
            );
        }
        UiIcon::MorphShape => {
            painter.circle(
                g.p(5.0, 6.0),
                4.0 * u,
                Color32::from_rgb(0x55, 0x9C, 0xE8),
                Stroke::new(u, darker(BLUE)),
            );
            g.boxed(
                painter,
                g.r(9.0, 8.5, 15.0, 14.5),
                PURPLE,
                darker(PURPLE),
                0.0,
            );
            g.line(painter, &[(7.0, 10.0), (9.0, 12.0)], 1.2, INK);
        }
        UiIcon::Sprite | UiIcon::Symbol => {
            g.boxed(painter, g.r(1.5, 1.5, 14.5, 14.5), BLUE, darker(BLUE), 2.0);
            g.boxed(
                painter,
                g.r(4.0, 4.0, 12.0, 12.0),
                Color32::from_rgb(0x7F, 0xB2, 0xF0),
                Color32::WHITE,
                1.0,
            );
            if icon == UiIcon::Sprite {
                g.poly(
                    painter,
                    &[(6.5, 5.5), (10.5, 8.0), (6.5, 10.5)],
                    Color32::WHITE,
                    Color32::WHITE,
                );
            }
        }
        UiIcon::Button => {
            g.boxed(
                painter,
                g.r(1.0, 4.0, 15.0, 12.0),
                Color32::from_rgb(0xD8, 0xE6, 0xF5),
                Color32::from_gray(90),
                3.0,
            );
            painter.line_segment(
                [g.p(3.0, 5.5), g.p(13.0, 5.5)],
                Stroke::new(u, Color32::WHITE),
            );
            g.text(painter, 8.0, 8.2, "OK", 6.0, INK);
        }
        UiIcon::Bitmap => picture(painter, g),
        UiIcon::Text => g.text(
            painter,
            8.0,
            8.5,
            "A",
            15.0,
            Color32::from_rgb(0x1E, 0x3F, 0x8C),
        ),
        UiIcon::Font => {
            g.text(painter, 6.0, 8.5, "A", 13.0, RED);
            g.text(painter, 12.0, 10.5, "a", 9.0, darker(RED));
        }
        UiIcon::Audio => {
            g.poly(
                painter,
                &[
                    (1.5, 6.0),
                    (4.5, 6.0),
                    (8.5, 2.5),
                    (8.5, 13.5),
                    (4.5, 10.0),
                    (1.5, 10.0),
                ],
                Color32::from_gray(110),
                INK,
            );
            for (r, w) in [(3.0, 1.3), (5.5, 1.3)] {
                let points: Vec<Pos2> = (-4..=4)
                    .map(|i| {
                        let a = i as f32 * 0.17;
                        pos2(
                            g.p(8.5, 8.0).x + a.cos() * r * u,
                            g.p(8.5, 8.0).y + a.sin() * r * u,
                        )
                    })
                    .collect();
                painter.add(Shape::line(points, Stroke::new(w * u, ORANGE)));
            }
        }
        UiIcon::Video | UiIcon::Media | UiIcon::Movie => {
            g.boxed(
                painter,
                g.r(1.0, 3.0, 15.0, 13.0),
                Color32::from_gray(50),
                INK,
                0.5,
            );
            for x in [2.0, 5.0, 8.0, 11.0, 13.5] {
                painter.rect_filled(g.r(x, 3.8, x + 1.4, 5.0), 0.0, PAPER);
                painter.rect_filled(g.r(x, 11.0, x + 1.4, 12.2), 0.0, PAPER);
            }
            if icon == UiIcon::Movie {
                g.poly(
                    painter,
                    &[(6.0, 6.0), (11.0, 8.0), (6.0, 10.0)],
                    ORANGE,
                    ORANGE,
                );
            } else {
                painter.rect_filled(g.r(3.0, 6.0, 13.0, 10.0), 0.0, SKY);
            }
        }
        UiIcon::Frame => {
            g.boxed(painter, g.r(2.0, 2.0, 14.0, 14.0), PAPER, GREY, 0.0);
            painter.circle_filled(g.p(8.0, 8.0), 2.2 * u, INK);
        }
        UiIcon::Binary => {
            page(painter, g, Color32::from_rgb(0xE6, 0xF2, 0xE6));
            g.text(painter, 8.0, 7.0, "01", 5.0, darker(GREEN));
            g.text(painter, 8.0, 11.5, "10", 5.0, darker(GREEN));
        }
        UiIcon::Code => {
            page(painter, g, PAPER);
            g.line(painter, &[(7.0, 6.0), (5.0, 9.0), (7.0, 12.0)], 1.3, BLUE);
            g.line(painter, &[(9.5, 6.0), (11.5, 9.0), (9.5, 12.0)], 1.3, BLUE);
        }
        UiIcon::Rig => {
            g.line(
                painter,
                &[(3.0, 13.0), (8.0, 8.0), (13.0, 3.0)],
                2.2,
                ORANGE,
            );
            for (x, y) in [(3.0, 13.0), (8.0, 8.0), (13.0, 3.0)] {
                painter.circle(g.p(x, y), 1.8 * u, PAPER, Stroke::new(u, darker(ORANGE)));
            }
        }
        UiIcon::Document => {
            page(painter, g, PAPER);
            for y in [7.0, 9.5, 12.0] {
                g.line(painter, &[(5.0, y), (11.5, y)], 1.0, GREY);
            }
        }
        UiIcon::Other => {
            painter.circle(
                g.p(8.0, 8.0),
                5.5 * u,
                Color32::from_gray(170),
                Stroke::new(u, Color32::from_gray(90)),
            );
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let c = g.p(8.0, 8.0);
                painter.circle_filled(
                    pos2(c.x + a.cos() * 6.2 * u, c.y + a.sin() * 6.2 * u),
                    1.3 * u,
                    Color32::from_gray(140),
                );
            }
            painter.circle_filled(g.p(8.0, 8.0), 2.0 * u, Color32::from_gray(235));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_kinds_map_to_their_icons() {
        let make = |preview| TreeNode {
            id: 0,
            label: String::new(),
            detail: String::new(),
            preview: Some(preview),
            edit: None,
            children: Vec::new(),
        };
        let vector = make(PreviewRef::QProjectAsset {
            path: Vec::new(),
            asset_id: 1,
            kind: AssetVisualKind::Vector,
        });
        assert_eq!(icon_for_tree_node(&vector, None, false), UiIcon::Vector);
        assert_eq!(
            icon_for_tree_node(&make(PreviewRef::Q0vStandaloneAudio), None, false),
            UiIcon::Audio
        );
        assert_eq!(
            icon_for_tree_node(&make(PreviewRef::SwfFrame(3)), None, false),
            UiIcon::Frame
        );
        let folder = TreeNode {
            preview: None,
            children: vec![vector.clone()],
            ..vector
        };
        assert_eq!(icon_for_tree_node(&folder, None, true), UiIcon::FolderOpen);
    }
}
