//! The credits every q0idmation app shows: a strip of film running along
//! the top and bottom, a title that writes itself in, and one frame per
//! contributor, each playing its own little bouncing-ball loop with onion
//! skins, the first thing anyone animates. Hovering a frame lifts it and
//! plays its loop faster.
//!
//! The "q0" mark in the corner can be dressed by the host (see
//! [`MarkLook`]): the editor squashes it as it is knocked on, warms it
//! toward purple and shakes it, for what hides behind it.

use egui::{
    pos2, vec2, Align2, Color32, FontId, Id, Painter, Pos2, Rect, Rounding, Sense, Stroke, Ui,
};

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
pub const CONTRIBUTORS: [&str; 6] = [
    "Walibite", "ykenomad", "PoBoi", "XENON", "Dodonse", "AerAgain",
];

/// How the "q0" mark looks this frame, for a host that plays with it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MarkLook {
    /// 0 at rest, 1 squashed flat under a knock.
    pub press: f32,
    /// 0 in the accent, 1 burning the demon's purple.
    pub heat: f32,
    /// How far it is shaken this frame, in points.
    pub shake: [f32; 2],
    /// How far the rest of the credits fade back, 0 to 1.
    pub dim: f32,
}

/// The demon's purple, which the mark warms toward.
pub const DEMON: Color32 = Color32::from_rgb(157, 78, 255);

const MARGIN: f32 = 28.0;
const STRIP: f32 = 24.0;
const HEADER: f32 = 196.0;
const CARD_HEIGHT: f32 = 112.0;
const GAP: f32 = 14.0;
const MARK: f32 = 44.0;

/// Draws the credits; returns where the "q0" mark sits.
pub fn show(ui: &mut Ui, palette: &Palette, elapsed_seconds: f32) -> Rect {
    show_with(ui, palette, elapsed_seconds, MarkLook::default())
}

/// [`show`], with the mark dressed as `look` says.
pub fn show_with(ui: &mut Ui, palette: &Palette, elapsed_seconds: f32, look: MarkLook) -> Rect {
    let time = ui.input(|input| input.time) as f32;
    let reveal = ease_out_cubic((elapsed_seconds / 0.9).clamp(0.0, 1.0));
    let width = ui.available_width().max(360.0);
    let columns = if width >= 600.0 { 2 } else { 1 };
    let rows = CONTRIBUTORS.len().div_ceil(columns);
    let grid_height = rows as f32 * CARD_HEIGHT + (rows - 1) as f32 * GAP;
    let height = STRIP + HEADER + grid_height + 74.0 + STRIP;
    let (rect, _) = ui.allocate_exact_size(
        vec2(width, height.max(ui.available_height())),
        Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    let inner = rect.shrink2(vec2(MARGIN, 0.0));

    paint_background(&painter, rect, palette, reveal, time);
    let mark = paint_header(
        &painter,
        inner,
        palette,
        reveal,
        elapsed_seconds,
        time,
        look,
    );

    let grid_top = rect.top() + STRIP + HEADER;
    let card_width = (inner.width() - (columns - 1) as f32 * GAP) / columns as f32;
    for (index, name) in CONTRIBUTORS.iter().enumerate() {
        let column = index % columns;
        let row = index / columns;
        let progress = staggered_progress(reveal, index, CONTRIBUTORS.len());
        let card = Rect::from_min_size(
            pos2(
                inner.left() + column as f32 * (card_width + GAP),
                grid_top + row as f32 * (CARD_HEIGHT + GAP) + (1.0 - progress) * 18.0,
            ),
            vec2(card_width, CARD_HEIGHT),
        );
        let hovered = ui
            .interact(card, Id::new(("q0credits_name", *name)), Sense::hover())
            .hovered();
        paint_card(
            ui, &painter, card, palette, name, index, progress, hovered, time,
        );
    }

    let footer_y = grid_top + grid_height + 24.0;
    let rule = mix(palette.text_dim, palette.window, 0.55);
    painter.line_segment(
        [pos2(inner.left(), footer_y), pos2(inner.right(), footer_y)],
        Stroke::new(1.0_f32, alpha(rule, (130.0 * reveal) as u8)),
    );
    // A playhead runs along the rule, the way a timeline's does.
    let run = (time * 0.12).fract();
    let head = pos2(inner.left() + inner.width() * run, footer_y);
    painter.line_segment(
        [head - vec2(0.0, 5.0), head + vec2(0.0, 5.0)],
        Stroke::new(1.5_f32, alpha(palette.accent, (200.0 * reveal) as u8)),
    );
    painter.text(
        pos2(inner.left(), footer_y + 14.0),
        Align2::LEFT_TOP,
        "Q0E  /  THANK YOU",
        FontId::proportional(11.0),
        alpha(palette.text_dim, (220.0 * reveal) as u8),
    );
    painter.text(
        pos2(inner.right(), footer_y + 14.0),
        Align2::RIGHT_TOP,
        format!("{:02} PEOPLE", CONTRIBUTORS.len()),
        FontId::proportional(11.0),
        alpha(palette.text_dim, (220.0 * reveal) as u8),
    );

    if look.dim > 0.0 {
        painter.rect_filled(
            rect,
            Rounding::same(12.0),
            Color32::from_black_alpha((150.0 * look.dim.clamp(0.0, 1.0)) as u8),
        );
    }

    // The film creeps and the balls bounce on every frame the screen
    // shows: a timer of its own came round unevenly and made them stutter.
    ui.ctx().request_repaint();
    mark
}

/// Film strips along the top and bottom, motes drifting up between them,
/// and a soft glow behind the title.
fn paint_background(painter: &Painter, rect: Rect, palette: &Palette, reveal: f32, time: f32) {
    let base = mix(palette.deep_bg, palette.window, 0.45);
    painter.rect_filled(rect, Rounding::same(12.0), base);

    let glow = pos2(rect.right() - 90.0, rect.top() + STRIP + 70.0);
    for (radius, strength) in [(210.0, 10.0), (150.0, 12.0), (95.0, 14.0), (52.0, 16.0)] {
        painter.circle_filled(
            glow,
            radius,
            alpha(palette.accent, (strength * reveal) as u8),
        );
    }

    // Motes: each its own speed and drift, wrapping round.
    let span = rect.height() - STRIP * 2.0;
    for index in 0..28 {
        let seed = hash(index as u64 + 17);
        let x = rect.left() + (seed % 1000) as f32 / 1000.0 * rect.width();
        let speed = 6.0 + ((seed >> 10) % 100) as f32 / 100.0 * 14.0;
        let start = ((seed >> 20) % 1000) as f32 / 1000.0 * span;
        let y = rect.bottom() - STRIP - (start + time * speed).rem_euclid(span);
        let sway = ((time * 0.6 + index as f32).sin()) * 6.0;
        let size = 0.8 + ((seed >> 30) % 100) as f32 / 100.0 * 1.4;
        painter.circle_filled(
            pos2(x + sway, y),
            size,
            alpha(palette.text, (26.0 * reveal) as u8),
        );
    }

    let strip = mix(palette.deep_bg, Color32::BLACK, 0.35);
    let holes = mix(base, palette.text_dim, 0.18);
    for (top, way) in [(rect.top(), 1.0), (rect.bottom() - STRIP, -1.0)] {
        let band = Rect::from_min_size(pos2(rect.left(), top), vec2(rect.width(), STRIP));
        let rounding = if way > 0.0 {
            Rounding {
                nw: 12.0,
                ne: 12.0,
                ..Rounding::ZERO
            }
        } else {
            Rounding {
                sw: 12.0,
                se: 12.0,
                ..Rounding::ZERO
            }
        };
        painter.rect_filled(band, rounding, alpha(strip, (235.0 * reveal) as u8));
        let pitch = 22.0;
        let offset = (time * 16.0 * way).rem_euclid(pitch);
        let clip = painter.with_clip_rect(band.shrink2(vec2(10.0, 0.0)));
        let mut x = band.left() - pitch + offset;
        while x < band.right() + pitch {
            clip.rect_filled(
                Rect::from_center_size(pos2(x, band.center().y), vec2(10.0, 8.0)),
                Rounding::same(2.0),
                alpha(holes, (200.0 * reveal) as u8),
            );
            x += pitch;
        }
    }
    painter.rect_filled(
        Rect::from_min_size(
            rect.min + vec2(0.0, STRIP),
            vec2(3.0, rect.height() - STRIP * 2.0),
        ),
        0.0,
        alpha(palette.accent, (170.0 * reveal) as u8),
    );
}

fn paint_header(
    painter: &Painter,
    inner: Rect,
    palette: &Palette,
    reveal: f32,
    elapsed: f32,
    time: f32,
    look: MarkLook,
) -> Rect {
    let top = inner.top() + STRIP;
    let mark = Rect::from_min_size(pos2(inner.left(), top + 22.0), vec2(MARK, MARK));

    // The mark breathes; a knock squashes it, heat turns it purple.
    let breathe = 1.0 + 0.025 * (time * 2.1).sin();
    let press = look.press.clamp(-0.6, 1.0);
    let size = vec2(
        MARK * breathe * (1.0 + 0.16 * press),
        MARK * breathe * (1.0 - 0.18 * press),
    );
    let shaken = mark.center() + vec2(look.shake[0], look.shake[1]);
    // Squashed, it stays on the floor it sits on.
    let drawn = Rect::from_center_size(pos2(shaken.x, shaken.y + (MARK - size.y) * 0.5), size);
    let heat = look.heat.clamp(0.0, 1.0);
    let fill = mix(palette.accent, DEMON, heat);
    if heat > 0.01 {
        for (grow, strength) in [(14.0, 18.0), (8.0, 30.0), (3.0, 50.0)] {
            painter.rect_filled(
                drawn.expand(grow * heat),
                Rounding::same(12.0 + grow),
                alpha(DEMON, (strength * heat) as u8),
            );
        }
    }
    painter.rect_filled(
        drawn.translate(vec2(0.0, 2.0)),
        Rounding::same(11.0),
        Color32::from_black_alpha((60.0 * reveal) as u8),
    );
    painter.rect_filled(
        drawn,
        Rounding::same(11.0),
        alpha(fill, (235.0 * reveal) as u8),
    );
    painter.text(
        drawn.center(),
        Align2::CENTER_CENTER,
        "q0",
        FontId::proportional(20.0 * (size.x / MARK).sqrt()),
        alpha(on_accent(fill), (255.0 * reveal) as u8),
    );
    painter.text(
        pos2(mark.right() + 14.0, mark.top() + 5.0),
        Align2::LEFT_TOP,
        "Q0E  /  CREDITS",
        FontId::proportional(12.0),
        alpha(palette.text_dim, (230.0 * reveal) as u8),
    );
    painter.text(
        pos2(mark.right() + 14.0, mark.top() + 22.0),
        Align2::LEFT_TOP,
        "The people behind the project",
        FontId::proportional(12.5),
        alpha(palette.text, (235.0 * reveal) as u8),
    );

    // The title writes itself in, letter by letter, each rising into place.
    let title = "Made possible by you.";
    let title_size = if inner.width() < 500.0 { 27.0 } else { 35.0 };
    let font = FontId::proportional(title_size);
    let mut x = inner.left();
    let count = title.chars().count() as f32;
    for (index, letter) in title.chars().enumerate() {
        let start = 0.15 + index as f32 / count * 0.55;
        let t = ease_out_cubic(((elapsed - start) / 0.35).clamp(0.0, 1.0));
        let galley = painter.layout_no_wrap(letter.to_string(), font.clone(), palette.text);
        let advance = galley.size().x;
        if t > 0.0 {
            painter.galley(
                pos2(x, top + 84.0 + (1.0 - t) * 12.0),
                galley,
                alpha(palette.text, (255.0 * t) as u8),
            );
        }
        x += advance;
    }
    // A pen stroke underlines it as it finishes.
    let underline = ease_out_cubic(((elapsed - 0.7) / 0.5).clamp(0.0, 1.0));
    if underline > 0.0 {
        let y = top + 84.0 + title_size + 8.0;
        let points: Vec<Pos2> = (0..=24)
            .map(|step| {
                let s = step as f32 / 24.0 * underline;
                pos2(
                    inner.left() + (x - inner.left()) * s,
                    y + (s * 9.0).sin() * 1.6,
                )
            })
            .collect();
        painter.add(egui::Shape::line(
            points,
            Stroke::new(2.4_f32, alpha(palette.accent, 220)),
        ));
    }

    let message_in = ease_out_cubic(((elapsed - 0.55) / 0.5).clamp(0.0, 1.0));
    let message_color = alpha(palette.text_dim, (235.0 * message_in) as u8);
    let message = painter.layout(
        GLORIOUS_MESSAGE.to_owned(),
        FontId::proportional(15.0),
        message_color,
        inner.width() - 8.0,
    );
    painter.galley(
        pos2(
            inner.left(),
            top + 84.0 + title_size + 22.0 + (1.0 - message_in) * 6.0,
        ),
        message,
        message_color,
    );
    mark
}

#[allow(clippy::too_many_arguments)]
fn paint_card(
    ui: &Ui,
    painter: &Painter,
    rect: Rect,
    palette: &Palette,
    name: &str,
    index: usize,
    progress: f32,
    hovered: bool,
    time: f32,
) {
    let hover = ui
        .ctx()
        .animate_bool_with_time(Id::new(("q0credits_hover", name)), hovered, 0.18);
    let card = rect.translate(vec2(0.0, -3.0 * hover));
    let hue = hue_shift(palette.accent, index as f32 * 0.16);
    let fade = |a: f32| (a * progress) as u8;

    painter.rect_filled(
        card.translate(vec2(0.0, 3.0 + 3.0 * hover)),
        Rounding::same(12.0),
        Color32::from_black_alpha(fade(40.0 + 40.0 * hover)),
    );
    let fill = mix(palette.panel, palette.window, 0.45 + hover * 0.15);
    painter.rect_filled(card, Rounding::same(12.0), alpha(fill, fade(255.0)));
    painter.rect_stroke(
        card,
        Rounding::same(12.0),
        Stroke::new(
            1.0_f32,
            alpha(
                mix(palette.text_dim, hue, hover * 0.8),
                fade(110.0 + 90.0 * hover),
            ),
        ),
    );
    // The frame's sprocket edge.
    for k in 0..3 {
        let y = card.top() + 24.0 + k as f32 * ((card.height() - 48.0) / 2.0);
        painter.rect_filled(
            Rect::from_center_size(pos2(card.left() + 11.0, y), vec2(7.0, 9.0)),
            Rounding::same(2.0),
            alpha(mix(palette.deep_bg, palette.window, 0.3), fade(255.0)),
        );
    }
    // Light sweeps across a frame as it is hovered.
    if hover > 0.01 && hover < 0.99 {
        let x = card.left() + card.width() * hover;
        painter.with_clip_rect(card).line_segment(
            [pos2(x - 30.0, card.bottom()), pos2(x + 10.0, card.top())],
            Stroke::new(
                18.0_f32,
                Color32::from_white_alpha(fade(14.0 * (1.0 - hover))),
            ),
        );
    }

    let left = card.left() + 26.0;
    painter.text(
        pos2(left, card.top() + 14.0),
        Align2::LEFT_TOP,
        format!("FRAME {:02} / {:02}", index + 1, CONTRIBUTORS.len()),
        FontId::monospace(10.5),
        alpha(mix(palette.text_dim, hue, hover), fade(210.0)),
    );
    let viewport = 76.0;
    let name_size = fitted_name_font_size(painter, name, card.width() - viewport - 60.0);
    painter.text(
        pos2(left, card.top() + 36.0 - hover * 1.5),
        Align2::LEFT_TOP,
        name,
        FontId::proportional(name_size),
        alpha(mix(palette.text, hue, hover * 0.3), fade(255.0)),
    );
    painter.text(
        pos2(left, card.bottom() - 24.0),
        Align2::LEFT_TOP,
        "CONTRIBUTOR",
        FontId::proportional(10.0),
        alpha(palette.text_dim, fade(215.0)),
    );

    // The little loop: a ball bouncing in its own viewport, onion skins
    // trailing it, its own colour and timing.
    let view = Rect::from_min_size(
        pos2(
            card.right() - viewport - 16.0,
            card.center().y - viewport * 0.5,
        ),
        vec2(viewport, viewport),
    );
    painter.rect_filled(
        view,
        Rounding::same(8.0),
        alpha(mix(palette.deep_bg, palette.window, 0.25), fade(255.0)),
    );
    let floor = view.bottom() - 12.0;
    painter.line_segment(
        [
            pos2(view.left() + 10.0, floor),
            pos2(view.right() - 10.0, floor),
        ],
        Stroke::new(1.0_f32, alpha(palette.text_dim, fade(90.0))),
    );
    // Each frame keeps its own steady tempo: the clock alone moves it, so
    // hovering or clicking around never hurries it along.
    let speed = 0.9 + 0.12 * (index % 3) as f32;
    let phase = index as f32 * 0.37;
    let ball_at = |t: f32| -> (Pos2, f32) {
        // One bounce a beat, and across and back every four: both repeat
        // exactly, so the loop has no seam.
        let height = (std::f32::consts::PI * t.rem_euclid(1.0)).sin();
        let x = view.left()
            + 18.0
            + (view.width() - 36.0)
                * (0.5 + 0.35 * (std::f32::consts::TAU * (t * 0.25 + phase)).sin());
        (
            pos2(x, floor - 7.0 - height * (view.height() - 34.0)),
            height,
        )
    };
    let now = time * speed + phase;
    let skins = if hover > 0.5 { 3 } else { 2 };
    for skin in (1..=skins).rev() {
        let (at, _) = ball_at(now - skin as f32 * 0.07);
        painter.circle_stroke(
            at,
            6.0,
            Stroke::new(1.0_f32, alpha(hue, fade(70.0 / skin as f32))),
        );
    }
    let (at, height) = ball_at(now);
    let shadow = 1.0 - height;
    painter.add(egui::Shape::ellipse_filled(
        pos2(at.x, floor + 1.0),
        vec2(7.0 * (0.5 + 0.5 * shadow), 2.0),
        Color32::from_black_alpha(fade(70.0 * shadow)),
    ));
    // Squashed against the floor, stretched in flight.
    let squash = (1.0 - height * 6.0).clamp(0.0, 1.0);
    let stretch = 1.0 + 0.25 * height.min(0.3);
    painter.add(egui::Shape::ellipse_filled(
        at + vec2(0.0, 2.0 * squash),
        vec2(
            6.0 * (1.0 + 0.35 * squash),
            6.0 * stretch * (1.0 - 0.32 * squash),
        ),
        alpha(hue, fade(255.0)),
    ));
}

fn fitted_name_font_size(painter: &Painter, name: &str, width: f32) -> f32 {
    let initial =
        painter.layout_no_wrap(name.to_owned(), FontId::proportional(26.0), Color32::WHITE);
    if initial.size().x > width {
        (26.0 * width / initial.size().x).max(16.0)
    } else {
        26.0
    }
}

fn staggered_progress(reveal: f32, index: usize, count: usize) -> f32 {
    let delay = if count <= 1 {
        0.0
    } else {
        0.25 + index as f32 / (count - 1) as f32 * 0.35
    };
    ease_out_cubic(((reveal - delay) / (1.0 - delay)).clamp(0.0, 1.0))
}

fn ease_out_cubic(value: f32) -> f32 {
    1.0 - (1.0 - value.clamp(0.0, 1.0)).powi(3)
}

/// `color` turned round the colour wheel by `turn` (1 is all the way).
fn hue_shift(color: Color32, turn: f32) -> Color32 {
    let mut hsva = egui::ecolor::Hsva::from(color);
    hsva.h = (hsva.h + turn).rem_euclid(1.0);
    Color32::from(hsva)
}

/// A cheap well-mixed number from `seed`, for the motes.
fn hash(seed: u64) -> u64 {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^ (x >> 29)
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
            ["Walibite", "ykenomad", "PoBoi", "XENON", "Dodonse", "AerAgain"]
        );
        assert_eq!(
            CONTRIBUTORS.iter().copied().collect::<HashSet<_>>().len(),
            CONTRIBUTORS.len()
        );
        assert_eq!(
            GLORIOUS_MESSAGE,
            "thanks to our glorious contributers! you guys keep Q0E alive."
        );
    }

    #[test]
    fn contributor_names_reveal_in_staggered_order() {
        let first = staggered_progress(0.55, 0, CONTRIBUTORS.len());
        let middle = staggered_progress(0.55, 2, CONTRIBUTORS.len());
        let last = staggered_progress(0.55, CONTRIBUTORS.len() - 1, CONTRIBUTORS.len());
        assert!(first > middle);
        assert!(middle >= last);
        assert!((0.0..=1.0).contains(&first));
        assert!((0.0..=1.0).contains(&last));
        assert_eq!(staggered_progress(1.0, 5, CONTRIBUTORS.len()), 1.0);
    }

    #[test]
    fn the_credits_draw_and_hand_back_the_mark() {
        let ctx = egui::Context::default();
        let palette = Palette {
            accent: Color32::from_rgb(90, 140, 255),
            panel: Color32::from_gray(40),
            window: Color32::from_gray(30),
            deep_bg: Color32::from_gray(15),
            text: Color32::WHITE,
            text_dim: Color32::GRAY,
        };
        let mut mark = Rect::NOTHING;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                mark = show_with(
                    ui,
                    &palette,
                    2.0,
                    MarkLook {
                        press: 0.5,
                        heat: 0.7,
                        shake: [1.0, -1.0],
                        dim: 0.2,
                    },
                );
            });
        });
        assert!(!output.shapes.is_empty());
        assert_eq!(
            mark.size(),
            vec2(MARK, MARK),
            "the mark answers knocks where it stands"
        );
    }
}
