//! What q0bo looks like: a red gumdrop with stubby black arms, black feet and
//! an old black screen for a face. The screen is a 17 by 10 grid of LEDs and
//! every mood is a little bitmap on it, so a new face is a few lines of dots.

use std::f32::consts::TAU;

use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2};

pub(crate) const RED: Color32 = Color32::from_rgb(0xE2, 0x26, 0x32);
pub(crate) const RED_DARK: Color32 = Color32::from_rgb(0x9C, 0x12, 0x1D);
pub(crate) const INK: Color32 = Color32::from_rgb(0x14, 0x11, 0x13);
const SCREEN: Color32 = Color32::from_rgb(0x0C, 0x09, 0x0B);
const BEZEL: Color32 = Color32::from_rgb(0x3A, 0x2A, 0x30);
const LED: Color32 = Color32::from_rgb(0xFF, 0x52, 0x5E);
const LED_DIM: Color32 = Color32::from_rgb(0x9A, 0x2A, 0x3C);
const LED_OFF: Color32 = Color32::from_rgb(0x2A, 0x11, 0x17);
/// The pale edge around him, like a sticker's.
const STICKER: Color32 = Color32::from_rgb(0xF6, 0xEF, 0xE6);

const COLS: usize = 17;
const ROWS: usize = 10;

/// How he feels, which is mostly which face is on his screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mood {
    #[default]
    Happy,
    Talk,
    Excited,
    Wink,
    Surprised,
    Thinking,
    Cool,
    Sad,
    Love,
    Sleepy,
    Smug,
    Focus,
    Dizzy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Arm {
    #[default]
    Rest,
    Wave,
    /// Reaching toward a point, given as a direction from his body.
    Point(Vec2),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Pose {
    pub mood: Mood,
    pub talking: bool,
    /// Where he looks, each axis from -1 to 1.
    pub look: Vec2,
    pub arm: Arm,
    /// How high he is hopping, 0 when standing.
    pub hop: f32,
    /// His size while popping in or out, 1 once he is there.
    pub scale: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            mood: Mood::Happy,
            talking: false,
            look: Vec2::ZERO,
            arm: Arm::Rest,
            hop: 0.0,
            scale: 1.0,
        }
    }
}

// Faces: `#` is a lit LED, `+` a dim one (blush, tears), `.` an unlit one.
// Eyes take the top six rows and mouths the bottom four.
type Eyes = [&'static str; 6];
type Mouth = [&'static str; 4];

const EYES_OPEN: Eyes = [
    ".................",
    "....##.....##....",
    "....##.....##....",
    "....##.....##....",
    ".................",
    ".................",
];
const EYES_BLINK: Eyes = [
    ".................",
    ".................",
    ".................",
    "...###.....###...",
    ".................",
    ".................",
];
const EYES_HAPPY: Eyes = [
    ".................",
    ".................",
    "....##.....##....",
    "...#..#...#..#...",
    ".................",
    ".................",
];
const EYES_STARS: Eyes = [
    "....#.......#....",
    "..#####...#####..",
    "...###.....###...",
    "...#.#.....#.#...",
    ".................",
    ".................",
];
const EYES_WINK: Eyes = [
    ".................",
    "...........##....",
    "....##.....##....",
    "...#..#....##....",
    ".................",
    ".................",
];
const EYES_RINGS: Eyes = [
    "....##.....##....",
    "...#..#...#..#...",
    "...#..#...#..#...",
    "....##.....##....",
    ".................",
    ".................",
];
const EYES_UP: Eyes = [
    ".....##.....##...",
    ".....##.....##...",
    ".................",
    ".................",
    ".................",
    ".................",
];
const EYES_SHADES: Eyes = [
    ".................",
    ".###############.",
    "..#####...#####..",
    "..##.##...##.##..",
    "...###.....###...",
    ".................",
];
const EYES_SAD: Eyes = [
    ".................",
    "...##.......##...",
    "....##.....##....",
    ".................",
    "....+.......+....",
    ".................",
];
const EYES_HEARTS: Eyes = [
    "...#.#.....#.#...",
    "..#####...#####..",
    "...###.....###...",
    "....#.......#....",
    ".................",
    ".................",
];
const EYES_SLEEPY: Eyes = [
    ".................",
    ".................",
    "...###.....###...",
    "....#.......#....",
    ".................",
    ".................",
];
const EYES_SMUG: Eyes = [
    ".................",
    "...####....####..",
    ".....##......##..",
    ".................",
    ".................",
    ".................",
];
const EYES_FOCUS: Eyes = [
    ".................",
    "...#.........#...",
    "....##.....##....",
    "...#.........#...",
    ".................",
    ".................",
];
const EYES_X: Eyes = [
    ".................",
    "...#.#.....#.#...",
    "....#.......#....",
    "...#.#.....#.#...",
    ".................",
    ".................",
];

const MOUTH_SMILE: Mouth = [
    "..++.........++..",
    "......#...#......",
    ".......###.......",
    ".................",
];
const MOUTH_OPEN: Mouth = [
    "..++.........++..",
    "......#####......",
    "......#...#......",
    ".......###.......",
];
const MOUTH_CAT: Mouth = [
    "..++.........++..",
    "......#.#.#......",
    ".......#.#.......",
    ".................",
];
const MOUTH_GRIN: Mouth = [
    "..++.........++..",
    ".....#######.....",
    "......#...#......",
    ".......###.......",
];
const MOUTH_O: Mouth = [
    ".................",
    ".......###.......",
    "......#...#......",
    ".......###.......",
];
const MOUTH_DOT: Mouth = [
    ".................",
    "........#........",
    ".................",
    ".................",
];
const MOUTH_HMM: Mouth = [
    ".................",
    ".........###.....",
    ".................",
    ".................",
];
const MOUTH_FROWN: Mouth = [
    ".................",
    ".......###.......",
    "......#...#......",
    ".................",
];
const MOUTH_SMIRK: Mouth = [
    ".................",
    "..........#......",
    "......####.......",
    ".................",
];
const MOUTH_TONGUE: Mouth = [
    ".................",
    "......#####......",
    "........++.......",
    ".................",
];
const MOUTH_WAVY: Mouth = [
    ".................",
    ".....#.#.#.#.....",
    "......#.#.#......",
    ".................",
];

/// A mood's eyes, its resting mouth and its talking mouth, and whether its
/// eyes are plain dots that blink and follow what he looks at.
fn face(mood: Mood) -> (&'static Eyes, &'static Mouth, &'static Mouth, bool) {
    match mood {
        Mood::Happy => (&EYES_HAPPY, &MOUTH_SMILE, &MOUTH_OPEN, false),
        Mood::Talk => (&EYES_OPEN, &MOUTH_SMILE, &MOUTH_OPEN, true),
        Mood::Excited => (&EYES_STARS, &MOUTH_GRIN, &MOUTH_OPEN, false),
        Mood::Wink => (&EYES_WINK, &MOUTH_CAT, &MOUTH_OPEN, false),
        Mood::Surprised => (&EYES_RINGS, &MOUTH_O, &MOUTH_DOT, false),
        Mood::Thinking => (&EYES_UP, &MOUTH_HMM, &MOUTH_DOT, false),
        Mood::Cool => (&EYES_SHADES, &MOUTH_SMIRK, &MOUTH_OPEN, false),
        Mood::Sad => (&EYES_SAD, &MOUTH_FROWN, &MOUTH_O, false),
        Mood::Love => (&EYES_HEARTS, &MOUTH_CAT, &MOUTH_OPEN, false),
        Mood::Sleepy => (&EYES_SLEEPY, &MOUTH_DOT, &MOUTH_O, false),
        Mood::Smug => (&EYES_SMUG, &MOUTH_SMIRK, &MOUTH_OPEN, false),
        Mood::Focus => (&EYES_FOCUS, &MOUTH_TONGUE, &MOUTH_TONGUE, false),
        Mood::Dizzy => (&EYES_X, &MOUTH_WAVY, &MOUTH_WAVY, false),
    }
}

/// Which LEDs are on: 0 off, 1 dim, 2 lit.
fn screen_pixels(pose: &Pose, time: f32) -> [[u8; COLS]; ROWS] {
    let (eyes, mouth, talk_mouth, plain) = face(pose.mood);
    // A blink every few seconds, never in step with the breathing.
    let blinking = plain && (time * 0.37).fract() > 0.965;
    let eyes = if blinking { &EYES_BLINK } else { eyes };
    let mouth = if pose.talking && (time * 9.0) as i32 % 2 == 0 {
        talk_mouth
    } else {
        mouth
    };
    let (dx, dy) = if plain && !blinking {
        (
            (pose.look.x * 1.4).round().clamp(-1.0, 1.0) as i32,
            (pose.look.y * 1.4).round().clamp(-1.0, 1.0) as i32,
        )
    } else {
        (0, 0)
    };
    let mut pixels = [[0u8; COLS]; ROWS];
    let mut put = |row: i32, col: i32, value: u8| {
        if (0..ROWS as i32).contains(&row) && (0..COLS as i32).contains(&col) {
            let cell = &mut pixels[row as usize][col as usize];
            *cell = (*cell).max(value);
        }
    };
    let value = |c: char| match c {
        '#' => 2,
        '+' => 1,
        _ => 0,
    };
    for (row, line) in eyes.iter().enumerate() {
        for (col, c) in line.chars().enumerate() {
            put(row as i32 + dy, col as i32 + dx, value(c));
        }
    }
    for (row, line) in mouth.iter().enumerate() {
        for (col, c) in line.chars().enumerate() {
            put(row as i32 + 6, col as i32, value(c));
        }
    }
    pixels
}

/// The outline of his body: a rounded gumdrop, a little wider at the feet.
fn body_outline(centre: Pos2, rx: f32, ry: f32) -> Vec<Pos2> {
    const POINTS: usize = 48;
    (0..POINTS)
        .map(|i| {
            let (sin, cos) = (i as f32 / POINTS as f32 * TAU).sin_cos();
            let x = cos.signum() * cos.abs().powf(0.82);
            let y = sin.signum() * sin.abs().powf(0.9);
            let flare = 1.0 + 0.07 * y;
            pos2(centre.x + x * rx * flare, centre.y + y * ry)
        })
        .collect()
}

/// The rectangle he takes up when drawn with his feet at `feet`.
pub(crate) fn bounds(feet: Pos2, height: f32) -> Rect {
    Rect::from_min_max(
        pos2(feet.x - height * 0.48, feet.y - height),
        pos2(feet.x + height * 0.48, feet.y),
    )
}

/// Draws q0bo standing on `feet`, about `height` points tall.
pub(crate) fn paint(painter: &Painter, feet: Pos2, height: f32, pose: &Pose, time: f64) {
    let u = height / 100.0 * pose.scale;
    if u <= 0.02 {
        return;
    }
    let t = time as f32;
    let breathe = (t * 2.6).sin();
    let hop = pose.hop.max(0.0);
    let stretch = 1.0 + hop * 0.07 + breathe * 0.012;
    let ground = pos2(feet.x, feet.y);
    let base = pos2(feet.x, feet.y - hop * 24.0 * u);
    let rx = 38.0 * u / stretch.sqrt();
    let ry = 36.0 * u * stretch;
    let centre = pos2(base.x, base.y - 7.0 * u - ry);

    // Where everything goes.
    let feet_at = [-1.0, 1.0].map(|side| pos2(base.x + side * 14.0 * u, base.y - 4.5 * u));
    let ears_at = [-1.0, 1.0].map(|side| pos2(centre.x + side * rx * 0.97, centre.y - 6.0 * u));
    let sway = (t * 2.1).sin() * 3.0 * u + pose.look.x * 2.5 * u;
    let root = pos2(centre.x, centre.y - ry * 0.9);
    let tip = pos2(centre.x + sway, centre.y - ry - 15.0 * u);
    let bend = pos2(centre.x + sway * 0.2, centre.y - ry - 6.0 * u);
    let stalk: Vec<Pos2> = (0..=8)
        .map(|i| {
            let s = i as f32 / 8.0;
            let a = root.lerp(bend, s);
            a.lerp(bend.lerp(tip, s), s)
        })
        .collect();
    let outline = body_outline(centre, rx, ry);
    let arms = arms(pose, centre, rx, u, t);

    painter.add(Shape::ellipse_filled(
        pos2(ground.x, ground.y - u),
        vec2(29.0 * u * (1.0 - hop * 0.35), 4.5 * u),
        Color32::from_black_alpha(60),
    ));

    // A pale sticker edge around all of him, so his black bits show on a
    // dark editor too.
    let edge = 2.2 * u;
    for at in feet_at {
        painter.add(Shape::ellipse_filled(at, vec2(10.5 * u + edge, 5.0 * u + edge), STICKER));
    }
    painter.add(Shape::line(stalk.clone(), Stroke::new(2.4 * u + edge * 2.0, STICKER)));
    painter.circle_filled(tip, 4.6 * u + edge + 0.8 * u, STICKER);
    for at in ears_at {
        painter.add(Shape::ellipse_filled(at, vec2(7.5 * u + edge, 10.5 * u + edge), STICKER));
    }
    painter.add(Shape::closed_line(outline.clone(), Stroke::new(2.6 * u + edge * 2.0, STICKER)));
    for (shoulder, hand) in arms {
        painter.line_segment([shoulder, hand], Stroke::new(6.5 * u + edge * 2.0, STICKER));
        painter.circle_filled(hand, 4.4 * u + edge, STICKER);
    }

    for at in feet_at {
        painter.add(Shape::ellipse_filled(at, vec2(10.5 * u, 5.0 * u), INK));
    }
    // Antenna and ear cups go behind the body.
    painter.add(Shape::line(stalk, Stroke::new(2.4 * u, INK)));
    let glow = 0.5 + 0.5 * (t * 3.3).sin();
    painter.circle_filled(tip, 7.5 * u, LED.gamma_multiply(0.18 + 0.2 * glow));
    painter.circle(tip, 4.6 * u, RED, Stroke::new(1.6 * u, INK));
    painter.circle_filled(tip + vec2(-1.4, -1.4) * u, 1.3 * u, Color32::WHITE);
    for (side, at) in [-1.0, 1.0].into_iter().zip(ears_at) {
        painter.add(Shape::ellipse_filled(at, vec2(7.5 * u, 10.5 * u), INK));
        painter.circle_filled(at + vec2(side * 3.0 * u, 0.0), 1.6 * u, LED_DIM);
    }

    // Body: a dark rim, the red face of it, a shine and the ink outline.
    painter.add(Shape::convex_polygon(
        outline.clone(),
        RED_DARK,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        body_outline(centre + vec2(-3.0, -3.5) * u, rx * 0.92, ry * 0.91),
        RED,
        Stroke::NONE,
    ));
    painter.add(Shape::ellipse_filled(
        centre + vec2(-19.0, -21.0) * u,
        vec2(8.0 * u, 4.6 * u),
        Color32::from_rgba_unmultiplied(255, 170, 175, 110),
    ));
    painter.circle_filled(centre + vec2(-24.0, -19.0) * u, 1.8 * u, Color32::WHITE);
    painter.add(Shape::closed_line(outline, Stroke::new(2.6 * u, INK)));
    painter.text(
        centre + vec2(0.0, ry * 0.66),
        Align2::CENTER_CENTER,
        "q0",
        FontId::monospace(10.0 * u),
        RED_DARK,
    );

    // The screen.
    let screen = Rect::from_center_size(centre + vec2(0.0, -6.0 * u), vec2(56.0 * u, 38.0 * u));
    painter.rect(screen, 8.0 * u, SCREEN, Stroke::new(2.0 * u, INK));
    painter.rect_stroke(screen.shrink(1.6 * u), 7.0 * u, Stroke::new(u, BEZEL));
    let inner = screen.shrink(4.5 * u);
    let cell = (inner.width() / COLS as f32).min(inner.height() / ROWS as f32);
    let grid = Rect::from_center_size(
        inner.center(),
        vec2(cell * COLS as f32, cell * ROWS as f32),
    );
    let pixels = screen_pixels(pose, t);
    let cell_rect = |row: usize, col: usize| {
        Rect::from_min_size(
            grid.min + vec2(col as f32 * cell, row as f32 * cell),
            vec2(cell, cell),
        )
    };
    for (row, line) in pixels.iter().enumerate() {
        for (col, value) in line.iter().enumerate() {
            let rect = cell_rect(row, col);
            if *value == 2 {
                painter.rect_filled(rect.expand(cell * 0.3), cell * 0.5, LED.gamma_multiply(0.16));
            } else {
                painter.rect_filled(rect.shrink(cell * 0.32), 0.0, LED_OFF);
            }
        }
    }
    for (row, line) in pixels.iter().enumerate() {
        for (col, value) in line.iter().enumerate() {
            let colour = match value {
                2 => LED,
                1 => LED_DIM,
                _ => continue,
            };
            painter.rect_filled(cell_rect(row, col).shrink(cell * 0.08), cell * 0.22, colour);
        }
    }
    // An old screen's glare across the top corner.
    painter.add(Shape::convex_polygon(
        vec![
            screen.left_top() + vec2(5.0, 3.0) * u,
            screen.left_top() + vec2(20.0, 3.0) * u,
            screen.left_top() + vec2(9.0, 17.0) * u,
            screen.left_top() + vec2(3.0, 17.0) * u,
        ],
        Color32::from_white_alpha(14),
        Stroke::NONE,
    ));

    // Arms in front.
    for (shoulder, hand) in arms {
        painter.line_segment([shoulder, hand], Stroke::new(6.5 * u, INK));
        painter.circle_filled(hand, 4.4 * u, INK);
    }

    extras(painter, centre, rx, ry, u, pose.mood, t);
}

/// Shoulder and hand of each arm: one may wave or reach, the other rests.
fn arms(pose: &Pose, centre: Pos2, rx: f32, u: f32, t: f32) -> [(Pos2, Pos2); 2] {
    let reach = match pose.arm {
        Arm::Point(direction) if direction.length() > 0.01 => Some(direction.normalized()),
        _ => None,
    };
    let reaching_side = reach.map_or(1.0, |direction| if direction.x < 0.0 { -1.0 } else { 1.0 });
    [-1.0_f32, 1.0].map(|side| {
        let shoulder = pos2(centre.x + side * rx * 0.93, centre.y + 9.0 * u);
        let rest = vec2(side * 0.55, 0.84);
        let active = side == reaching_side;
        let direction = match pose.arm {
            Arm::Wave if active => {
                let angle = (t * 11.0).sin() * 0.5 - 0.15;
                let up = vec2(side * 0.55, -0.84);
                vec2(
                    up.x * angle.cos() - up.y * angle.sin(),
                    up.x * angle.sin() + up.y * angle.cos(),
                )
            }
            Arm::Point(_) if active => reach.unwrap_or(rest),
            _ => rest,
        };
        let length = if matches!(pose.arm, Arm::Point(_)) && active {
            19.0
        } else {
            15.0
        };
        (shoulder, shoulder + direction * length * u)
    })
}

/// The little things floating around a mood: hearts, zzz, sparkles.
fn extras(painter: &Painter, centre: Pos2, rx: f32, ry: f32, u: f32, mood: Mood, t: f32) {
    let head = pos2(centre.x + rx * 0.8, centre.y - ry * 0.95);
    match mood {
        Mood::Thinking => {
            for i in 0..3 {
                let rise = ((t * 1.6 - i as f32 * 0.35).sin() * 0.5 + 0.5) * 1.5 * u;
                let point = head + vec2(4.0 + i as f32 * 7.0, -2.0 - i as f32 * 6.0 - rise) * u;
                painter.circle(
                    point,
                    (2.0 + i as f32) * u,
                    Color32::WHITE,
                    Stroke::new(1.2 * u, INK),
                );
            }
        }
        Mood::Sleepy => {
            for i in 0..2 {
                let phase = (t * 0.6 + i as f32 * 0.5).fract();
                let point = head + vec2(4.0 + phase * 10.0, -phase * 20.0) * u;
                painter.text(
                    point,
                    Align2::CENTER_CENTER,
                    if i == 0 { "z" } else { "Z" },
                    FontId::monospace((8.0 + phase * 6.0) * u),
                    INK.gamma_multiply(1.0 - phase),
                );
            }
        }
        Mood::Love => {
            for i in 0..2 {
                let phase = (t * 0.7 + i as f32 * 0.5).fract();
                let x = if i == 0 { 1.0 } else { -1.0 };
                let point = pos2(centre.x + x * rx * 0.9, centre.y - ry * 0.6)
                    + vec2(x * phase * 6.0, -phase * 26.0) * u;
                heart(painter, point, 4.0 * u, RED.gamma_multiply(1.0 - phase * 0.8));
            }
        }
        Mood::Excited => {
            for i in 0..3 {
                let phase = (t * 1.3 + i as f32 / 3.0).fract();
                let angle = i as f32 * 2.1 + 0.6;
                let point = centre + vec2(angle.cos() * rx * 1.25, angle.sin() * ry * 0.9 - 14.0 * u);
                sparkle(painter, point, (1.0 - (phase * 2.0 - 1.0).abs()) * 5.0 * u, RED);
            }
        }
        Mood::Dizzy => {
            for i in 0..3 {
                let angle = t * 4.0 + i as f32 * TAU / 3.0;
                let point = pos2(centre.x, centre.y - ry - 8.0 * u)
                    + vec2(angle.cos() * 16.0, angle.sin() * 4.0) * u;
                sparkle(painter, point, 3.5 * u, Color32::from_rgb(0xFF, 0xC8, 0x3D));
            }
        }
        Mood::Surprised => {
            painter.text(
                head + vec2(6.0, -6.0) * u,
                Align2::CENTER_CENTER,
                "!",
                FontId::monospace(16.0 * u),
                RED,
            );
        }
        _ => {}
    }
}

/// A pixel-ish heart.
pub(crate) fn heart(painter: &Painter, centre: Pos2, size: f32, colour: Color32) {
    painter.circle_filled(centre + vec2(-0.5, -0.3) * size, size * 0.55, colour);
    painter.circle_filled(centre + vec2(0.5, -0.3) * size, size * 0.55, colour);
    painter.add(Shape::convex_polygon(
        vec![
            centre + vec2(-1.02, -0.1) * size,
            centre + vec2(1.02, -0.1) * size,
            centre + vec2(0.0, 1.0) * size,
        ],
        colour,
        Stroke::NONE,
    ));
}

/// A four-pointed twinkle.
pub(crate) fn sparkle(painter: &Painter, centre: Pos2, size: f32, colour: Color32) {
    if size <= 0.1 {
        return;
    }
    let thin = size * 0.28;
    painter.add(Shape::convex_polygon(
        vec![
            centre + vec2(0.0, -size),
            centre + vec2(thin, 0.0),
            centre + vec2(0.0, size),
            centre + vec2(-thin, 0.0),
        ],
        colour,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![
            centre + vec2(-size, 0.0),
            centre + vec2(0.0, -thin),
            centre + vec2(size, 0.0),
            centre + vec2(0.0, thin),
        ],
        colour,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_MOOD: [Mood; 13] = [
        Mood::Happy,
        Mood::Talk,
        Mood::Excited,
        Mood::Wink,
        Mood::Surprised,
        Mood::Thinking,
        Mood::Cool,
        Mood::Sad,
        Mood::Love,
        Mood::Sleepy,
        Mood::Smug,
        Mood::Focus,
        Mood::Dizzy,
    ];

    #[test]
    fn every_face_fits_the_screen() {
        for mood in EVERY_MOOD {
            let (eyes, mouth, talking, _) = face(mood);
            for line in eyes.iter().chain(mouth.iter()).chain(talking.iter()) {
                assert_eq!(line.chars().count(), COLS, "{mood:?}: {line:?}");
                assert!(line.chars().all(|c| matches!(c, '#' | '+' | '.')));
            }
        }
        for line in EYES_BLINK {
            assert_eq!(line.chars().count(), COLS);
        }
    }

    #[test]
    fn every_face_lights_some_leds() {
        for mood in EVERY_MOOD {
            for talking in [false, true] {
                let pose = Pose {
                    mood,
                    talking,
                    ..Pose::default()
                };
                let lit = screen_pixels(&pose, 0.0)
                    .iter()
                    .flatten()
                    .filter(|value| **value == 2)
                    .count();
                assert!(lit >= 4, "{mood:?} lights only {lit} LEDs");
            }
        }
    }
}
