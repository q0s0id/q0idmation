use std::time::Duration;

use egui::{pos2, vec2, Color32, FontId, Id, Pos2, Rect, Response, Rounding, Sense, Stroke, Ui};

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

const HERO_FONT_SIZE: f32 = 24.0;
const MIN_HERO_FONT_SIZE: f32 = 17.0;
const NAME_FONT_SIZE: f32 = 29.0;
const NAME_ROW_HEIGHT: f32 = 104.0;
const NAME_GAP: f32 = 18.0;
const CONTENT_MARGIN: f32 = 32.0;

#[derive(Clone, Copy)]
struct NameStyle {
    line_ratio: f32,
    constellation_flip: bool,
    constellation_lift: f32,
}

const NAME_STYLES: [NameStyle; 6] = [
    NameStyle {
        line_ratio: 0.72,
        constellation_flip: false,
        constellation_lift: -4.0,
    },
    NameStyle {
        line_ratio: 0.88,
        constellation_flip: true,
        constellation_lift: 5.0,
    },
    NameStyle {
        line_ratio: 0.64,
        constellation_flip: false,
        constellation_lift: 7.0,
    },
    NameStyle {
        line_ratio: 0.81,
        constellation_flip: true,
        constellation_lift: -6.0,
    },
    NameStyle {
        line_ratio: 0.58,
        constellation_flip: false,
        constellation_lift: 2.0,
    },
    NameStyle {
        line_ratio: 0.94,
        constellation_flip: true,
        constellation_lift: -1.0,
    },
];

pub fn show(ui: &mut Ui, palette: &Palette, elapsed_seconds: f32) {
    let ctx = ui.ctx().clone();
    let reveal = ease_out_cubic((elapsed_seconds / 0.72).clamp(0.0, 1.0));
    let width = ui.available_width().max(360.0);
    let columns = if width >= 620.0 { 2 } else { 1 };
    let rows = CONTRIBUTORS.len().div_ceil(columns);
    let content_height = 178.0 + rows as f32 * (NAME_ROW_HEIGHT + NAME_GAP) + 34.0;
    let desired_height = content_height.max(ui.available_height());
    let (rect, _) = ui.allocate_exact_size(vec2(width, desired_height), Sense::hover());
    let painter = ui.painter_at(rect);
    let time = ctx.input(|input| input.time) as f32;

    paint_node_background(&painter, rect, palette, reveal, time);

    let inner = rect.shrink2(vec2(CONTENT_MARGIN, 24.0));
    paint_main_message(&painter, inner, palette, reveal);

    let grid_top = inner.top() + 148.0;
    let column_width = if columns == 2 {
        (inner.width() - NAME_GAP) * 0.5
    } else {
        inner.width()
    };

    for (index, name) in CONTRIBUTORS.iter().enumerate() {
        let column = index % columns;
        let row = index / columns;
        let x = inner.left() + column as f32 * (column_width + NAME_GAP);
        let y = grid_top + row as f32 * (NAME_ROW_HEIGHT + NAME_GAP);
        let progress = staggered_progress(reveal, index, CONTRIBUTORS.len());
        let slide = (1.0 - progress) * 18.0;
        let name_rect =
            Rect::from_min_size(pos2(x, y + slide), vec2(column_width, NAME_ROW_HEIGHT));
        let response = ui.interact(
            name_rect,
            Id::new(("q0player_credit_name", *name)),
            Sense::hover(),
        );
        paint_name(
            ui,
            &painter,
            name_rect,
            palette,
            name,
            NAME_STYLES[index],
            progress,
            response,
            *name == FIRST_PATRON,
        );
    }

    ctx.request_repaint_after(Duration::from_millis(16));
}

fn paint_node_background(
    painter: &egui::Painter,
    rect: Rect,
    palette: &Palette,
    reveal: f32,
    time: f32,
) {
    let window = palette.window;
    let deep = palette.deep_bg;
    let panel = palette.panel;
    let accent = palette.accent;
    let dim = palette.text_dim;

    painter.rect_filled(rect, Rounding::same(10.0), mix(window, deep, 0.22));

    const BASE_NODES: [(f32, f32); 17] = [
        (0.07, 0.12),
        (0.20, 0.08),
        (0.34, 0.18),
        (0.49, 0.09),
        (0.66, 0.16),
        (0.84, 0.10),
        (0.94, 0.27),
        (0.76, 0.34),
        (0.55, 0.29),
        (0.31, 0.38),
        (0.10, 0.31),
        (0.15, 0.61),
        (0.38, 0.72),
        (0.61, 0.63),
        (0.86, 0.72),
        (0.72, 0.91),
        (0.27, 0.90),
    ];

    let nodes: Vec<Pos2> = BASE_NODES
        .iter()
        .enumerate()
        .map(|(index, &(x, y))| {
            let phase = index as f32 * 0.73;
            let drift_x = (time * 0.18 + phase).sin() * 5.0;
            let drift_y = (time * 0.14 + phase * 1.31).cos() * 4.0;
            pos2(
                rect.left() + rect.width() * x + drift_x,
                rect.top() + rect.height() * y + drift_y,
            )
        })
        .collect();

    let line_color = alpha(mix(dim, panel, 0.45), (34.0 * reveal) as u8);
    for (index, &a) in nodes.iter().enumerate() {
        for &b in &nodes[index + 1..] {
            let distance = a.distance(b);
            if distance < 176.0 {
                let closeness = 1.0 - distance / 176.0;
                painter.line_segment(
                    [a, b],
                    Stroke::new(
                        0.7_f32,
                        alpha(line_color, (line_color.a() as f32 * closeness) as u8),
                    ),
                );
            }
        }
    }

    for (index, &node) in nodes.iter().enumerate() {
        let emphasized = index % 5 == 0;
        let color = if emphasized {
            mix(dim, accent, 0.18)
        } else {
            dim
        };
        painter.circle_filled(
            node,
            if emphasized { 2.7 } else { 1.8 },
            alpha(
                color,
                ((54.0 + if emphasized { 24.0 } else { 0.0 }) * reveal) as u8,
            ),
        );
    }
}

fn paint_main_message(painter: &egui::Painter, rect: Rect, palette: &Palette, reveal: f32) {
    let text = palette.text;
    let dim = palette.text_dim;
    let y = rect.top() + (1.0 - reveal) * 12.0;

    // This is the user's complete sentence, on one visual line. It scales
    // down only when the dialog is narrow; no alternate heading or extra copy.
    let full_color = alpha(text, (255.0 * reveal) as u8);
    let initial = painter.layout_no_wrap(
        GLORIOUS_MESSAGE.to_owned(),
        FontId::proportional(HERO_FONT_SIZE),
        full_color,
    );
    let fitted_size = fitted_hero_font_size(initial.size().x, rect.width());
    let galley = painter.layout_no_wrap(
        GLORIOUS_MESSAGE.to_owned(),
        FontId::proportional(fitted_size),
        full_color,
    );
    painter.galley(
        pos2(rect.center().x - galley.size().x * 0.5, y),
        galley,
        full_color,
    );

    let line_y = y + 58.0;
    let half = rect.width().min(360.0) * 0.5 * reveal;
    painter.line_segment(
        [
            pos2(rect.center().x - half, line_y),
            pos2(rect.center().x + half, line_y),
        ],
        Stroke::new(0.8_f32, alpha(dim, (76.0 * reveal) as u8)),
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_name(
    ui: &Ui,
    painter: &egui::Painter,
    rect: Rect,
    palette: &Palette,
    name: &str,
    style: NameStyle,
    progress: f32,
    response: Response,
    featured: bool,
) {
    let hover = ui.ctx().animate_bool_with_time(
        Id::new(("q0player_credit_name_hover", name)),
        response.hovered(),
        0.20,
    );
    let text = palette.text;
    let dim = palette.text_dim;
    let accent = palette.accent;
    let alpha_scale = progress.clamp(0.0, 1.0);
    if featured {
        let card = rect.shrink2(vec2(4.0, 3.0));
        painter.rect_filled(
            card,
            Rounding::same(8.0),
            alpha(
                mix(palette.panel, accent, 0.16),
                (118.0 * alpha_scale) as u8,
            ),
        );
        painter.rect_stroke(
            card,
            Rounding::same(8.0),
            Stroke::new(
                1.2_f32 + hover * 0.7,
                alpha(
                    mix(dim, accent, 0.52),
                    ((122.0 + hover * 70.0) * alpha_scale) as u8,
                ),
            ),
        );
        let crown_y = rect.top() + 17.0;
        let crown = [
            pos2(rect.center().x - 13.0, crown_y + 5.0),
            pos2(rect.center().x - 6.5, crown_y - 2.0),
            pos2(rect.center().x, crown_y + 4.0),
            pos2(rect.center().x + 6.5, crown_y - 2.0),
            pos2(rect.center().x + 13.0, crown_y + 5.0),
        ];
        for points in crown.windows(2) {
            painter.line_segment(
                [points[0], points[1]],
                Stroke::new(1.2_f32, alpha(accent, (190.0 * alpha_scale) as u8)),
            );
        }
    }
    let center = rect.center() + vec2(0.0, if featured { -7.0 } else { 0.0 } - hover * 3.0);
    let name_color = alpha(
        mix(
            text,
            accent,
            if featured {
                0.34 + hover * 0.16
            } else {
                hover * 0.12
            },
        ),
        (255.0 * alpha_scale) as u8,
    );
    let font =
        FontId::proportional(NAME_FONT_SIZE + if featured { 5.0 } else { 0.0 } + hover * 2.0);
    let galley = painter.layout_no_wrap(name.to_owned(), font, name_color);
    let text_pos = pos2(
        center.x - galley.size().x * 0.5,
        center.y - galley.size().y * 0.5 - 5.0,
    );

    if hover > 0.0 {
        painter.galley(
            text_pos + vec2(0.0, 2.0),
            galley.clone(),
            alpha(Color32::BLACK, (28.0 * hover * alpha_scale) as u8),
        );
    }
    if featured {
        let glow = alpha(accent, ((24.0 + hover * 26.0) * alpha_scale) as u8);
        for offset in [
            vec2(-1.5, 0.0),
            vec2(1.5, 0.0),
            vec2(0.0, -1.5),
            vec2(0.0, 1.5),
        ] {
            painter.galley(text_pos + offset, galley.clone(), glow);
        }
    }
    painter.galley(text_pos, galley.clone(), name_color);

    let underline_width = (galley.size().x * style.line_ratio).max(42.0);
    let underline_y = center.y + galley.size().y * 0.5 + 9.0;
    let line_start = pos2(center.x - underline_width * 0.5, underline_y);
    let line_end = pos2(center.x + underline_width * 0.5, underline_y);
    painter.line_segment(
        [line_start, line_end],
        Stroke::new(
            1.0_f32 + hover * 0.6,
            alpha(
                mix(dim, text, 0.18 + hover * 0.18),
                (130.0 * alpha_scale) as u8,
            ),
        ),
    );
    painter.circle_filled(
        line_start,
        2.2 + hover,
        alpha(mix(dim, accent, 0.10), (150.0 * alpha_scale) as u8),
    );
    painter.circle_filled(
        line_end,
        2.2 + hover,
        alpha(mix(dim, accent, 0.10), (150.0 * alpha_scale) as u8),
    );

    if featured {
        let badge_color = alpha(mix(dim, accent, 0.45), (210.0 * alpha_scale) as u8);
        let badge_galley = painter.layout_no_wrap(
            "first patron".to_owned(),
            FontId::proportional(11.0 + hover * 0.5),
            badge_color,
        );
        painter.galley(
            pos2(
                rect.center().x - badge_galley.size().x * 0.5,
                rect.bottom() - badge_galley.size().y - 8.0,
            ),
            badge_galley,
            badge_color,
        );
    }
    paint_name_constellation(
        painter,
        rect,
        style,
        alpha_scale,
        hover,
        mix(dim, accent, 0.08),
    );

    if response.hovered() {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}

fn paint_name_constellation(
    painter: &egui::Painter,
    rect: Rect,
    style: NameStyle,
    alpha_scale: f32,
    hover: f32,
    color: Color32,
) {
    let side = if style.constellation_flip { -1.0 } else { 1.0 };
    let anchor = pos2(
        rect.center().x + side * (rect.width() * 0.37),
        rect.center().y + style.constellation_lift,
    );
    let points = [
        anchor,
        anchor + vec2(side * 13.0, -10.0),
        anchor + vec2(side * 24.0, 4.0),
    ];
    let constellation_color = alpha(color, ((82.0 + hover * 54.0) * alpha_scale) as u8);
    painter.line_segment(
        [points[0], points[1]],
        Stroke::new(0.8_f32, constellation_color),
    );
    painter.line_segment(
        [points[1], points[2]],
        Stroke::new(0.8_f32, constellation_color),
    );
    for (index, point) in points.into_iter().enumerate() {
        painter.circle_filled(
            point,
            1.8 + index as f32 * 0.45 + hover * 0.4,
            constellation_color,
        );
    }
}

fn fitted_hero_font_size(initial_width: f32, available_width: f32) -> f32 {
    if initial_width > available_width {
        (HERO_FONT_SIZE * available_width / initial_width).max(MIN_HERO_FONT_SIZE)
    } else {
        HERO_FONT_SIZE
    }
}

fn staggered_progress(reveal: f32, index: usize, count: usize) -> f32 {
    let delay = if count <= 1 {
        0.0
    } else {
        index as f32 / (count - 1) as f32 * 0.30
    };
    ease_out_cubic(((reveal - delay) / (1.0 - delay)).clamp(0.0, 1.0))
}

fn ease_out_cubic(value: f32) -> f32 {
    1.0 - (1.0 - value.clamp(0.0, 1.0)).powi(3)
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
    fn full_user_message_is_the_main_heading() {
        assert_eq!(
            GLORIOUS_MESSAGE,
            "thanks to our glorious contributers! you guys keep Q0E alive."
        );
        assert!(!GLORIOUS_MESSAGE.contains('\n'));
        assert_eq!(fitted_hero_font_size(500.0, 700.0), HERO_FONT_SIZE);
        assert!(fitted_hero_font_size(900.0, 600.0) < HERO_FONT_SIZE);
        assert_eq!(fitted_hero_font_size(9_000.0, 100.0), MIN_HERO_FONT_SIZE);
    }

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
                "5feetman",
            ]
        );
        assert_eq!(
            CONTRIBUTORS.iter().copied().collect::<HashSet<_>>().len(),
            CONTRIBUTORS.len()
        );
        assert_eq!(NAME_STYLES.len(), CONTRIBUTORS.len());
        assert_eq!(FIRST_PATRON, "karma");
        assert_eq!(
            CONTRIBUTORS
                .iter()
                .filter(|name| **name == FIRST_PATRON)
                .count(),
            1
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
