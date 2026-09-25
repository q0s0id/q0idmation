use std::time::Duration;

use egui::{pos2, vec2, Align2, Color32, FontId, Id, Rect, Rounding, Sense, Stroke, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Color32,
    pub panel: Color32,
    pub window: Color32,
    pub deep_bg: Color32,
    pub text: Color32,
    pub text_dim: Color32,
}

pub const GLORIOUS_MESSAGE: &str = "thanks to our glorious contributers! you guys keep Q0E alive.";
pub const FIRST_PATRON: &str = "karma";
pub const CONTRIBUTORS: [&str; 6] = [
    "karma",
    "Loonmoon",
    "Poisonous943",
    "Lapki",
    "francrafteador",
    "5feetman",
];

const MARGIN: f32 = 28.0;
const GRID_TOP: f32 = 190.0;
const CARD_HEIGHT: f32 = 104.0;
const GAP: f32 = 13.0;

pub fn show(ui: &mut Ui, palette: &Palette, elapsed_seconds: f32) {
    let reveal = ease_out_cubic((elapsed_seconds / 0.65).clamp(0.0, 1.0));
    let width = ui.available_width().max(360.0);
    let columns = if width >= 600.0 { 2 } else { 1 };
    let rows = CONTRIBUTORS.len().div_ceil(columns);
    let grid_height = rows as f32 * CARD_HEIGHT + (rows - 1) as f32 * GAP;
    let height = GRID_TOP + grid_height + 64.0;
    let (rect, _) = ui.allocate_exact_size(
        vec2(width, height.max(ui.available_height())),
        Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    let inner = rect.shrink2(vec2(MARGIN, 0.0));

    paint_background(&painter, rect, palette, reveal);
    paint_header(&painter, inner, palette, reveal);

    let card_width = (inner.width() - (columns - 1) as f32 * GAP) / columns as f32;
    for (index, name) in CONTRIBUTORS.iter().enumerate() {
        let column = index % columns;
        let row = index / columns;
        let progress = staggered_progress(reveal, index, CONTRIBUTORS.len());
        let card = Rect::from_min_size(
            pos2(
                inner.left() + column as f32 * (card_width + GAP),
                rect.top() + GRID_TOP + row as f32 * (CARD_HEIGHT + GAP) + (1.0 - progress) * 14.0,
            ),
            vec2(card_width, CARD_HEIGHT),
        );
        let hovered = ui
            .interact(card, Id::new(("q0credits_name", *name)), Sense::hover())
            .hovered();
        paint_card(ui, &painter, card, palette, name, index, progress, hovered);
    }

    let footer_y = rect.top() + GRID_TOP + grid_height + 20.0;
    painter.line_segment(
        [pos2(inner.left(), footer_y), pos2(inner.right(), footer_y)],
        Stroke::new(
            1.0_f32,
            alpha(
                mix(palette.text_dim, palette.window, 0.55),
                (130.0 * reveal) as u8,
            ),
        ),
    );
    painter.text(
        pos2(inner.left(), footer_y + 15.0),
        Align2::LEFT_TOP,
        "Q0E  /  THANK YOU",
        FontId::proportional(11.0),
        alpha(palette.text_dim, (220.0 * reveal) as u8),
    );
    painter.text(
        pos2(inner.right(), footer_y + 15.0),
        Align2::RIGHT_TOP,
        format!("{:02} PEOPLE", CONTRIBUTORS.len()),
        FontId::proportional(11.0),
        alpha(palette.text_dim, (220.0 * reveal) as u8),
    );

    if reveal < 1.0 {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}

fn paint_background(painter: &egui::Painter, rect: Rect, palette: &Palette, reveal: f32) {
    painter.rect_filled(
        rect,
        Rounding::same(10.0),
        mix(palette.deep_bg, palette.window, 0.50),
    );
    let center = pos2(rect.right() - 28.0, rect.top() + 20.0);
    for radius in [78.0, 130.0, 187.0] {
        painter.circle_stroke(
            center,
            radius,
            Stroke::new(1.0_f32, alpha(palette.accent, (17.0 * reveal) as u8)),
        );
    }
    painter.rect_filled(
        Rect::from_min_size(rect.min, vec2(4.0, rect.height())),
        0.0,
        alpha(palette.accent, (165.0 * reveal) as u8),
    );
}

fn paint_header(painter: &egui::Painter, inner: Rect, palette: &Palette, reveal: f32) {
    let top = inner.top();
    let mark = Rect::from_min_size(pos2(inner.left(), top + 22.0), vec2(35.0, 35.0));
    painter.rect_filled(
        mark,
        Rounding::same(9.0),
        alpha(palette.accent, (220.0 * reveal) as u8),
    );
    painter.text(
        mark.center(),
        Align2::CENTER_CENTER,
        "q0",
        FontId::proportional(17.0),
        alpha(on_accent(palette.accent), (255.0 * reveal) as u8),
    );
    painter.text(
        pos2(mark.right() + 13.0, mark.top() + 3.0),
        Align2::LEFT_TOP,
        "Q0E  /  CREDITS",
        FontId::proportional(12.0),
        alpha(palette.text_dim, (230.0 * reveal) as u8),
    );
    painter.text(
        pos2(mark.right() + 13.0, mark.top() + 19.0),
        Align2::LEFT_TOP,
        "The people behind the project",
        FontId::proportional(12.0),
        alpha(palette.text, (235.0 * reveal) as u8),
    );

    let title_size = if inner.width() < 500.0 { 26.0 } else { 33.0 };
    painter.text(
        pos2(inner.left(), top + 75.0),
        Align2::LEFT_TOP,
        "Made possible by you.",
        FontId::proportional(title_size),
        alpha(palette.text, (255.0 * reveal) as u8),
    );
    let message_color = alpha(palette.text_dim, (235.0 * reveal) as u8);
    let message = painter.layout(
        GLORIOUS_MESSAGE.to_owned(),
        FontId::proportional(15.0),
        message_color,
        inner.width() - 8.0,
    );
    painter.galley(pos2(inner.left(), top + 119.0), message, message_color);

    let divider_y = top + 168.0;
    painter.line_segment(
        [
            pos2(inner.left(), divider_y),
            pos2(inner.right(), divider_y),
        ],
        Stroke::new(
            1.0_f32,
            alpha(
                mix(palette.text_dim, palette.window, 0.52),
                (175.0 * reveal) as u8,
            ),
        ),
    );
    painter.line_segment(
        [
            pos2(inner.left(), divider_y),
            pos2(inner.left() + 92.0, divider_y),
        ],
        Stroke::new(2.0_f32, alpha(palette.accent, (230.0 * reveal) as u8)),
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_card(
    ui: &Ui,
    painter: &egui::Painter,
    rect: Rect,
    palette: &Palette,
    name: &str,
    index: usize,
    progress: f32,
    hovered: bool,
) {
    let hover = ui
        .ctx()
        .animate_bool_with_time(Id::new(("q0credits_hover", name)), hovered, 0.18);
    let featured = name == FIRST_PATRON;
    let fill = if featured {
        mix(palette.panel, palette.accent, 0.15 + hover * 0.04)
    } else {
        mix(palette.panel, palette.window, 0.50 + hover * 0.12)
    };
    let border = if featured {
        mix(palette.text_dim, palette.accent, 0.58 + hover * 0.20)
    } else {
        mix(palette.text_dim, palette.panel, 0.63 - hover * 0.17)
    };
    painter.rect_filled(
        rect,
        Rounding::same(10.0),
        alpha(fill, (255.0 * progress) as u8),
    );
    painter.rect_stroke(
        rect,
        Rounding::same(10.0),
        Stroke::new(1.0_f32, alpha(border, (160.0 * progress) as u8)),
    );
    painter.rect_filled(
        Rect::from_min_size(
            pos2(rect.left(), rect.top() + 15.0),
            vec2(3.0, rect.height() - 30.0),
        ),
        1.0,
        alpha(
            if featured {
                palette.accent
            } else {
                palette.text_dim
            },
            ((180.0 + hover * 40.0) * progress) as u8,
        ),
    );

    let left = rect.left() + 20.0;
    painter.text(
        pos2(left, rect.top() + 14.0),
        Align2::LEFT_TOP,
        format!("{:02} / {:02}", index + 1, CONTRIBUTORS.len()),
        FontId::monospace(11.0),
        alpha(palette.text_dim, (205.0 * progress) as u8),
    );
    let name_size = fitted_name_font_size(painter, name, rect.width() - 75.0);
    painter.text(
        pos2(left, rect.top() + 35.0 - hover * 1.5),
        Align2::LEFT_TOP,
        name,
        FontId::proportional(name_size),
        alpha(
            mix(
                palette.text,
                palette.accent,
                if featured { 0.24 } else { hover * 0.11 },
            ),
            (255.0 * progress) as u8,
        ),
    );
    painter.text(
        pos2(left, rect.bottom() - 23.0),
        Align2::LEFT_TOP,
        if featured {
            "FIRST PATRON"
        } else {
            "CONTRIBUTOR"
        },
        FontId::proportional(10.0),
        alpha(
            if featured {
                palette.accent
            } else {
                palette.text_dim
            },
            (215.0 * progress) as u8,
        ),
    );

    let jewel = pos2(rect.right() - 27.0, rect.center().y);
    painter.circle_stroke(
        jewel,
        13.0,
        Stroke::new(
            1.0_f32,
            alpha(
                if featured {
                    palette.accent
                } else {
                    palette.text_dim
                },
                ((112.0 + hover * 65.0) * progress) as u8,
            ),
        ),
    );
    let diamond = [
        pos2(jewel.x, jewel.y - 5.0),
        pos2(jewel.x + 5.0, jewel.y),
        pos2(jewel.x, jewel.y + 5.0),
        pos2(jewel.x - 5.0, jewel.y),
    ];
    for i in 0..diamond.len() {
        painter.line_segment(
            [diamond[i], diamond[(i + 1) % diamond.len()]],
            Stroke::new(1.2_f32, alpha(palette.accent, (205.0 * progress) as u8)),
        );
    }
    if hovered {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}

fn fitted_name_font_size(painter: &egui::Painter, name: &str, width: f32) -> f32 {
    let initial =
        painter.layout_no_wrap(name.to_owned(), FontId::proportional(26.0), Color32::WHITE);
    if initial.size().x > width {
        (26.0 * width / initial.size().x).max(18.0)
    } else {
        26.0
    }
}

fn staggered_progress(reveal: f32, index: usize, count: usize) -> f32 {
    let delay = if count <= 1 {
        0.0
    } else {
        index as f32 / (count - 1) as f32 * 0.24
    };
    ease_out_cubic(((reveal - delay) / (1.0 - delay)).clamp(0.0, 1.0))
}

fn ease_out_cubic(value: f32) -> f32 {
    1.0 - (1.0 - value.clamp(0.0, 1.0)).powi(3)
}

fn on_accent(color: Color32) -> Color32 {
    let luminance = (0.2126 * f32::from(color.r())
        + 0.7152 * f32::from(color.g())
        + 0.0722 * f32::from(color.b()))
        / 255.0;
    if luminance > 0.52 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

fn alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn contributors_match_the_public_credits() {
        assert_eq!(
            CONTRIBUTORS,
            [
                "karma",
                "Loonmoon",
                "Poisonous943",
                "Lapki",
                "francrafteador",
                "5feetman"
            ]
        );
        assert_eq!(
            CONTRIBUTORS.iter().copied().collect::<HashSet<_>>().len(),
            CONTRIBUTORS.len()
        );
        assert_eq!(FIRST_PATRON, "karma");
        assert_eq!(
            CONTRIBUTORS
                .iter()
                .filter(|name| **name == FIRST_PATRON)
                .count(),
            1
        );
        assert_eq!(
            GLORIOUS_MESSAGE,
            "thanks to our glorious contributers! you guys keep Q0E alive."
        );
    }

    #[test]
    fn contributor_names_reveal_in_staggered_order() {
        let first = staggered_progress(0.45, 0, CONTRIBUTORS.len());
        let middle = staggered_progress(0.45, 2, CONTRIBUTORS.len());
        let last = staggered_progress(0.45, CONTRIBUTORS.len() - 1, CONTRIBUTORS.len());
        assert!(first > middle);
        assert!(middle >= last);
        assert!((0.0..=1.0).contains(&first));
        assert!((0.0..=1.0).contains(&last));
    }
}
