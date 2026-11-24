//! The Studio pack: pointers drawn the way painting and animation programs
//! draw theirs.
//!
//! Every pointer is its own line-art pictogram: one light line over a dark
//! halo, so it reads on any colour, a faint fill, and the accent only on
//! the working parts (a nib's slit, the paint on a brush). Tools put their
//! working point on the hotspot: the pen's nib, the pencil's lead, the
//! eraser's edge, the eyedropper's tip. Precise shape tools are a fine
//! crosshair with the shape beside it.
//!
//! Motion is restrained: a pointer fades and slides in over a fifth of a
//! second, a press taps the tool a pixel toward its point, the crosshair
//! draws in, the rotate arc turns with the drag. Nothing sways or bounces.

use super::*;

/// The lean of a tool held in the hand: its body up and to the right.
const TILT: f32 = 0.72;

/// Lines, collected so every halo goes under every light line: parts that
/// meet read as one drawing.
struct Ink {
    at: Pos2,
    scale: f32,
    xf: Xf,
    light: Color32,
    dark: Color32,
    accent: Color32,
    fills: Vec<Shape>,
    halos: Vec<Shape>,
    lines: Vec<Shape>,
}

impl Ink {
    fn new(at: Pos2, scale: f32, pal: Palette, alpha: f32) -> Self {
        let fade = |c: Color32| c.gamma_multiply(alpha.clamp(0.0, 1.0));
        Self {
            at,
            scale,
            xf: Xf::ID,
            light: fade(pal.fill),
            dark: fade(pal.outline),
            accent: fade(pal.accent),
            fills: Vec::new(),
            halos: Vec::new(),
            lines: Vec::new(),
        }
    }

    fn finish(self, shapes: &mut Vec<Shape>) {
        shapes.extend(self.fills);
        shapes.extend(self.halos);
        shapes.extend(self.lines);
    }

    fn map(&self, points: &[Vec2]) -> Vec<Pos2> {
        points
            .iter()
            .map(|p| self.at + self.xf.apply(*p) * self.scale)
            .collect()
    }

    fn with(&mut self, xf: Xf, draw: impl FnOnce(&mut Self)) {
        let outer = self.xf;
        self.xf = xf.then(outer);
        draw(self);
        self.xf = outer;
    }

    fn halo_width(&self) -> f32 {
        (2.7 * self.scale).max(2.2)
    }

    fn line_width(&self) -> f32 {
        (1.05 * self.scale).max(0.9)
    }

    /// An open line in `color` over the dark halo.
    fn stroke_in(&mut self, points: &[Vec2], closed: bool, color: Color32) {
        if points.len() < 2 {
            return;
        }
        let mapped = self.map(points);
        let halo = Stroke::new(self.halo_width(), self.dark);
        let ink = Stroke::new(self.line_width(), color);
        if closed {
            self.halos.push(Shape::closed_line(mapped.clone(), halo));
            self.lines.push(Shape::closed_line(mapped, ink));
        } else {
            for end in [mapped[0], mapped[mapped.len() - 1]] {
                self.halos.push(Shape::circle_filled(
                    end,
                    self.halo_width() * 0.5,
                    self.dark,
                ));
            }
            self.halos.push(Shape::line(mapped.clone(), halo));
            self.lines.push(Shape::line(mapped, ink));
        }
    }

    fn line(&mut self, points: &[Vec2]) {
        self.stroke_in(points, false, self.light);
    }

    fn accent_line(&mut self, points: &[Vec2]) {
        self.stroke_in(points, false, self.accent);
    }

    /// A closed outline with a faint fill.
    fn outline(&mut self, points: &[Vec2]) {
        let mapped = self.map(points);
        fill_polygon(&mut self.fills, &mapped, self.light.gamma_multiply(0.16));
        self.stroke_in(points, true, self.light);
    }

    /// A closed outline filled solid: the slim arrow's body.
    fn solid(&mut self, points: &[Vec2], body: Color32, rim: Color32) {
        let mapped = self.map(points);
        fill_polygon(&mut self.fills, &mapped, body);
        self.lines.push(Shape::closed_line(
            mapped,
            Stroke::new(self.line_width() * 1.15, rim),
        ));
    }

    fn ring(&mut self, centre: Vec2, rx: f32, ry: f32) {
        let points = arc(centre, rx, ry, 0.0, TAU, 28);
        self.outline(&points);
    }

    fn dot(&mut self, centre: Vec2, radius: f32, color: Color32) {
        let at = self.map(&[centre])[0];
        self.halos.push(Shape::circle_filled(
            at,
            radius * self.scale + self.halo_width() * 0.3,
            self.dark,
        ));
        self.lines
            .push(Shape::circle_filled(at, radius * self.scale, color));
    }
}

fn v2(x: f32, y: f32) -> Vec2 {
    vec2(x, y)
}

/// A small V at `tip`, pointing along `way`.
fn head(ink: &mut Ink, tip: Vec2, way: Vec2, size: f32) {
    let way = way.normalized();
    let side = vec2(-way.y, way.x);
    ink.line(&[
        tip - way * size + side * size * 0.8,
        tip,
        tip - way * size - side * size * 0.8,
    ]);
}

/// A line with a head at each end, through the hotspot, along `angle`.
fn double_arrow(ink: &mut Ink, angle: f32, reach: f32) {
    let way = vec2(angle.cos(), angle.sin());
    ink.line(&[-way * reach, way * reach]);
    head(ink, way * reach, way, 2.6);
    head(ink, -way * reach, -way, 2.6);
}

const SLIM_ARROW: [Vec2; 7] = [
    vec2(0.0, 0.0),
    vec2(0.0, 15.5),
    vec2(3.7, 12.0),
    vec2(6.3, 17.4),
    vec2(8.4, 16.4),
    vec2(5.9, 11.2),
    vec2(10.8, 11.0),
];

/// The slim arrow: dark with a light rim (light with a dark rim when
/// `inverse`).
fn slim_arrow(ink: &mut Ink, inverse: bool, press: f32) {
    let shrink = 1.0 - 0.07 * press;
    ink.with(Xf::scale(shrink, shrink), |ink| {
        let (body, rim) = if inverse {
            (ink.light, ink.dark)
        } else {
            (ink.dark, ink.light)
        };
        // A thin dark edge outside the rim, so a light rim still shows on
        // white.
        let outer = ink.map(&SLIM_ARROW);
        ink.halos.push(Shape::closed_line(
            outer,
            Stroke::new(ink.line_width() * 2.4, ink.dark.gamma_multiply(0.55)),
        ));
        ink.solid(&SLIM_ARROW, body, rim);
    });
}

/// A fine crosshair: four ticks and a dot. A press draws the ticks in.
fn crosshair(ink: &mut Ink, press: f32) {
    let gap = 3.2 - 1.3 * press;
    for way in [v2(1.0, 0.0), v2(-1.0, 0.0), v2(0.0, 1.0), v2(0.0, -1.0)] {
        ink.line(&[way * gap, way * 9.0]);
    }
    let accent = ink.accent;
    ink.dot(Vec2::ZERO, 0.6, accent);
}

/// Badges: small pictures beside the arrow or crosshair, centred on 0.
fn badge(ink: &mut Ink, at: Vec2, draw: impl FnOnce(&mut Ink)) {
    ink.with(Xf::scale(0.8, 0.8).then(Xf::translate(at)), draw);
}

fn plus_badge(ink: &mut Ink) {
    ink.ring(Vec2::ZERO, 3.8, 3.8);
    ink.accent_line(&[v2(-2.0, 0.0), v2(2.0, 0.0)]);
    ink.accent_line(&[v2(0.0, -2.0), v2(0.0, 2.0)]);
}

fn minus_badge(ink: &mut Ink) {
    ink.ring(Vec2::ZERO, 3.8, 3.8);
    ink.accent_line(&[v2(-2.0, 0.0), v2(2.0, 0.0)]);
}

fn copy_badge(ink: &mut Ink) {
    ink.outline(&[v2(-3.4, -3.4), v2(1.2, -3.4), v2(1.2, 1.2), v2(-3.4, 1.2)]);
    ink.outline(&[v2(-1.2, -1.2), v2(3.4, -1.2), v2(3.4, 3.4), v2(-1.2, 3.4)]);
}

fn lock_badge(ink: &mut Ink) {
    ink.line(&arc(v2(0.0, -1.2), 2.3, 2.6, PI, TAU, 10));
    ink.outline(&[v2(-3.4, -1.2), v2(3.4, -1.2), v2(3.4, 3.6), v2(-3.4, 3.6)]);
    let accent = ink.accent;
    ink.dot(v2(0.0, 1.2), 0.6, accent);
}

fn card_badge(ink: &mut Ink) {
    ink.outline(&[
        v2(-3.6, -3.0),
        v2(1.8, -3.0),
        v2(3.6, -1.2),
        v2(3.6, 3.0),
        v2(-3.6, 3.0),
    ]);
    ink.line(&[v2(1.8, -3.0), v2(1.8, -1.2), v2(3.6, -1.2)]);
}

fn link_badge(ink: &mut Ink, apart: f32) {
    ink.with(Xf::rotate(-FRAC_PI_4), |ink| {
        ink.ring(v2(-1.8 - apart, 0.0), 2.6, 1.5);
        ink.ring(v2(1.8 + apart, 0.0), 2.6, 1.5);
    });
    if apart > 0.0 {
        ink.accent_line(&[v2(-0.6, -2.6), v2(0.6, -3.8)]);
        ink.accent_line(&[v2(0.6, 2.6), v2(-0.6, 3.8)]);
    }
}

// The tools, drawn upright with their working point on 0 and their body
// up; `tool` leans them into the hand.

fn tool(ink: &mut Ink, tap: f32, draw: impl FnOnce(&mut Ink)) {
    ink.with(Xf::translate(v2(0.0, tap)).then(Xf::rotate(TILT)), draw);
}

fn pen_nib(ink: &mut Ink) {
    ink.outline(&[
        v2(0.0, 0.0),
        v2(-3.0, -6.4),
        v2(-3.0, -9.2),
        v2(3.0, -9.2),
        v2(3.0, -6.4),
    ]);
    ink.accent_line(&[v2(0.0, -1.6), v2(0.0, -5.4)]);
    ink.ring(v2(0.0, -6.3), 0.8, 0.8);
    ink.outline(&[
        v2(-2.2, -9.2),
        v2(-2.2, -18.6),
        v2(2.2, -18.6),
        v2(2.2, -9.2),
    ]);
}

fn pencil(ink: &mut Ink) {
    ink.outline(&[v2(0.0, 0.0), v2(-2.5, -5.4), v2(2.5, -5.4)]);
    let accent = ink.accent;
    let lead = ink.map(&[v2(0.0, 0.0), v2(-0.9, -2.0), v2(0.9, -2.0)]);
    fill_polygon(&mut ink.lines, &lead, accent);
    ink.outline(&[
        v2(-2.5, -5.4),
        v2(-2.5, -15.4),
        v2(2.5, -15.4),
        v2(2.5, -5.4),
    ]);
    ink.line(&[v2(0.85, -5.8), v2(0.85, -15.0)]);
    ink.outline(&[
        v2(-2.5, -15.4),
        v2(-2.5, -17.0),
        v2(2.5, -17.0),
        v2(2.5, -15.4),
    ]);
    ink.outline(&[
        v2(-2.5, -17.0),
        v2(-2.5, -18.5),
        v2(-1.6, -19.4),
        v2(1.6, -19.4),
        v2(2.5, -18.5),
        v2(2.5, -17.0),
    ]);
}

fn round_brush(ink: &mut Ink) {
    let tuft = [
        v2(0.0, 0.0),
        v2(-1.9, -3.0),
        v2(-2.3, -5.6),
        v2(2.3, -5.6),
        v2(1.9, -3.0),
    ];
    let accent = ink.accent;
    let mapped = ink.map(&tuft);
    fill_polygon(&mut ink.fills, &mapped, accent.gamma_multiply(0.45));
    ink.stroke_in(&tuft, true, accent);
    ink.outline(&[v2(-2.0, -5.6), v2(-2.0, -8.8), v2(2.0, -8.8), v2(2.0, -5.6)]);
    ink.outline(&[
        v2(-1.5, -8.8),
        v2(-1.0, -19.8),
        v2(0.0, -20.8),
        v2(1.0, -19.8),
        v2(1.5, -8.8),
    ]);
}

fn rubber(ink: &mut Ink) {
    ink.outline(&[v2(-3.4, 0.0), v2(3.4, 0.0), v2(3.4, -5.8), v2(-3.4, -5.8)]);
    ink.outline(&[
        v2(-3.6, -5.8),
        v2(3.6, -5.8),
        v2(3.6, -15.0),
        v2(-3.6, -15.0),
    ]);
    ink.accent_line(&[v2(-3.6, -10.2), v2(3.6, -10.2)]);
}

fn finger(ink: &mut Ink) {
    let mut body = vec![v2(2.7, -15.4), v2(2.7, -2.6)];
    body.extend(arc(v2(0.0, -2.6), 2.7, 2.6, 0.0, PI, 12));
    body.push(v2(-2.7, -15.4));
    ink.outline(&body);
    ink.outline(&[v2(-1.5, -3.4), v2(1.5, -3.4), v2(1.3, -6.8), v2(-1.3, -6.8)]);
    ink.line(&[v2(-2.0, -11.2), v2(2.0, -11.6)]);
}

fn dropper(ink: &mut Ink, sample: Option<Color32>) {
    ink.outline(&[
        v2(-0.7, 0.0),
        v2(-1.0, -2.0),
        v2(-1.0, -10.0),
        v2(1.0, -10.0),
        v2(1.0, -2.0),
        v2(0.7, 0.0),
    ]);
    let liquid = sample.unwrap_or(ink.accent);
    ink.stroke_in(&[v2(0.0, -1.0), v2(0.0, -5.6)], false, liquid);
    ink.outline(&[
        v2(-2.0, -10.0),
        v2(2.0, -10.0),
        v2(2.0, -11.6),
        v2(-2.0, -11.6),
    ]);
    ink.ring(v2(0.0, -15.2), 2.6, 3.6);
}

fn blade(ink: &mut Ink, cutting: bool) {
    ink.outline(&[
        v2(0.0, 0.0),
        v2(1.6, -11.4),
        v2(-1.6, -11.4),
        v2(-1.6, -3.2),
    ]);
    if cutting {
        ink.accent_line(&[v2(1.0, -2.0), v2(1.4, -9.0)]);
    }
    ink.outline(&[
        v2(-1.9, -11.4),
        v2(1.9, -11.4),
        v2(1.9, -19.4),
        v2(-1.9, -19.4),
    ]);
    let light = ink.light;
    ink.dot(v2(0.0, -13.6), 0.45, light);
    ink.dot(v2(0.0, -17.0), 0.45, light);
}

fn pail(ink: &mut Ink, press: f32, sample: Color32) {
    let tilt = -0.8 - 0.12 * press;
    let centre = v2(8.2, -6.8);
    let lip = centre + Xf::rotate(tilt).apply(v2(-4.4, -4.2));
    ink.with(Xf::rotate(tilt).then(Xf::translate(centre)), |ink| {
        ink.line(&arc(v2(0.0, -4.2), 4.4, 4.4, PI, TAU, 12));
        ink.outline(&[v2(-4.4, -4.2), v2(4.4, -4.2), v2(3.5, 4.2), v2(-3.5, 4.2)]);
        let mapped = ink.map(&arc(v2(0.0, -4.2), 4.4, 1.4, 0.0, TAU, 16));
        fill_polygon(&mut ink.lines, &mapped, sample);
    });
    ink.stroke_in(&[lip, v2(0.0, -2.8)], false, sample);
    let mut drop = arc(v2(0.0, -1.3 + press), 1.3, 1.3, -0.25 * PI, 1.25 * PI, 10);
    drop.push(v2(0.0, -3.4 + press));
    let mapped = ink.map(&drop);
    fill_polygon(&mut ink.lines, &mapped, sample);
}

fn rope(ink: &mut Ink, cinch: f32) {
    let centre = v2(6.0, -6.4);
    let tilted: Vec<Vec2> = arc(centre, 5.4 - cinch, 3.4 - cinch * 0.6, 0.0, TAU, 28)
        .into_iter()
        .map(|p| centre + Xf::rotate(-0.45).apply(p - centre))
        .collect();
    let light = ink.light;
    ink.stroke_in(&tilted, true, light);
    ink.line(&[v2(2.4, -3.6), v2(1.6, -1.4), v2(0.4, -0.3)]);
    let accent = ink.accent;
    ink.dot(Vec2::ZERO, 0.75, accent);
}

fn open_hand(ink: &mut Ink, curl: f32) {
    let fingers = [(-3.9, 6.6), (-1.3, 8.2), (1.3, 8.2), (3.9, 7.0)];
    for (x, height) in fingers {
        let h = height * (1.0 - curl);
        let hw = 1.15;
        let mut path = vec![v2(x - hw, 0.0), v2(x - hw, -h + hw)];
        path.extend(arc(v2(x, -h + hw), hw, hw, PI, TAU, 8));
        path.push(v2(x + hw, 0.0));
        ink.line(&path);
    }
    ink.line(&[
        v2(-5.05, 0.0),
        v2(-5.05, 6.4),
        v2(-3.6, 9.4),
        v2(3.6, 9.4),
        v2(5.05, 6.4),
        v2(5.05, 0.0),
    ]);
    ink.line(&[v2(-5.05, 2.6), v2(-8.0, 0.4), v2(-8.9, 1.8), v2(-5.05, 6.0)]);
}

fn fist(ink: &mut Ink) {
    for x in [-3.9, -1.3, 1.3, 3.9] {
        ink.line(&arc(v2(x, 0.4), 1.3, 1.6, PI, TAU, 8));
    }
    ink.line(&[
        v2(-5.2, 0.4),
        v2(-5.2, 6.4),
        v2(-3.6, 9.0),
        v2(3.6, 9.0),
        v2(5.2, 6.4),
        v2(5.2, 0.4),
    ]);
    ink.line(&[v2(-5.2, 3.0), v2(-1.0, 3.4), v2(0.8, 2.4)]);
}

fn pointing_hand(ink: &mut Ink, press: f32) {
    ink.with(Xf::translate(v2(0.0, press * 0.8)), |ink| {
        let mut outline = arc(v2(0.0, 1.2), 1.5, 1.5, PI, TAU, 8);
        outline.extend([
            v2(1.5, 6.4),
            v2(2.6, 5.8),
            v2(4.0, 6.0),
            v2(4.4, 7.2),
            v2(5.6, 6.8),
            v2(6.6, 7.6),
            v2(6.8, 8.6),
            v2(7.6, 8.4),
            v2(8.6, 9.2),
            v2(8.6, 10.5),
            v2(8.4, 13.5),
            v2(7.0, 16.2),
            v2(1.6, 16.2),
            v2(-1.2, 13.0),
            v2(-3.6, 10.0),
            v2(-3.9, 8.9),
            v2(-3.0, 8.4),
            v2(-1.5, 9.6),
        ]);
        ink.outline(&outline);
        ink.line(&[v2(4.4, 7.2), v2(4.2, 9.4)]);
        ink.line(&[v2(6.8, 8.6), v2(6.6, 10.2)]);
    });
}

fn ibeam(ink: &mut Ink) {
    ink.line(&[v2(0.0, -8.0), v2(0.0, 8.0)]);
    ink.line(&[
        v2(-3.0, -9.6),
        v2(-1.0, -8.6),
        v2(0.0, -8.0),
        v2(1.0, -8.6),
        v2(3.0, -9.6),
    ]);
    ink.line(&[
        v2(-3.0, 9.6),
        v2(-1.0, 8.6),
        v2(0.0, 8.0),
        v2(1.0, 8.6),
        v2(3.0, 9.6),
    ]);
    ink.line(&[v2(-1.6, 0.0), v2(1.6, 0.0)]);
}

fn four_way(ink: &mut Ink) {
    for angle in [0.0, FRAC_PI_2] {
        double_arrow(ink, angle, 8.4);
    }
    ink.ring(Vec2::ZERO, 1.6, 1.6);
}

fn turn(ink: &mut Ink, spin: f32) {
    ink.with(Xf::rotate(spin), |ink| {
        let from = -0.55 * PI;
        let to = 0.95 * PI;
        ink.line(&arc(Vec2::ZERO, 6.6, 6.6, from, to, 24));
        let tip = v2(to.cos(), to.sin()) * 6.6;
        let way = v2(-to.sin(), to.cos());
        head(ink, tip, way, 2.6);
    });
    let accent = ink.accent;
    ink.dot(Vec2::ZERO, 0.7, accent);
}

fn shear(ink: &mut Ink, angle: f32) {
    ink.with(Xf::rotate(angle), |ink| {
        ink.line(&[v2(-7.0, -2.6), v2(7.0, -2.6)]);
        head(ink, v2(7.0, -2.6), v2(1.0, 0.0), 2.4);
        ink.line(&[v2(7.0, 2.6), v2(-7.0, 2.6)]);
        head(ink, v2(-7.0, 2.6), v2(-1.0, 0.0), 2.4);
    });
}

fn number_drag(ink: &mut Ink, lean: f32) {
    let shift = (lean * 0.004).clamp(-1.6, 1.6);
    ink.line(&[v2(0.0, -4.0), v2(0.0, 4.0)]);
    ink.with(Xf::translate(v2(shift, 0.0)), |ink| {
        ink.line(&[v2(-9.0, 0.0), v2(-2.6, 0.0)]);
        ink.line(&[v2(2.6, 0.0), v2(9.0, 0.0)]);
        head(ink, v2(-9.0, 0.0), v2(-1.0, 0.0), 2.6);
        head(ink, v2(9.0, 0.0), v2(1.0, 0.0), 2.6);
    });
}

fn diamond(ink: &mut Ink) {
    let accent = ink.accent;
    let points = [v2(0.0, -3.6), v2(3.6, 0.0), v2(0.0, 3.6), v2(-3.6, 0.0)];
    let mapped = ink.map(&points);
    fill_polygon(&mut ink.fills, &mapped, accent.gamma_multiply(0.6));
    ink.stroke_in(&points, true, ink.light);
    ink.with(Xf::translate(v2(0.0, 7.5)), |ink| {
        double_arrow(ink, 0.0, 5.0)
    });
}

fn target(ink: &mut Ink, press: f32) {
    ink.ring(Vec2::ZERO, 4.4 - press, 4.4 - press);
    for way in [v2(1.0, 0.0), v2(-1.0, 0.0), v2(0.0, 1.0), v2(0.0, -1.0)] {
        ink.line(&[way * 5.6, way * 8.4]);
    }
    let accent = ink.accent;
    ink.dot(Vec2::ZERO, 1.0, accent);
}

fn loupe(ink: &mut Ink) {
    // The middle stays clear: the picker reads the pixel under it.
    ink.stroke_in(&arc(Vec2::ZERO, 7.0, 7.0, 0.0, TAU, 32), true, ink.light);
    for way in [v2(1.0, 0.0), v2(-1.0, 0.0), v2(0.0, 1.0), v2(0.0, -1.0)] {
        ink.line(&[way * 2.2, way * 4.4]);
    }
    ink.line(&[v2(5.0, 5.0), v2(10.0, 10.0)]);
}

fn shape_badge(ink: &mut Ink, kind: CursorKind) {
    match kind {
        CursorKind::Line => ink.line(&[v2(-3.0, 3.0), v2(3.0, -3.0)]),
        CursorKind::Rect => {
            ink.outline(&[v2(-3.4, -2.6), v2(3.4, -2.6), v2(3.4, 2.6), v2(-3.4, 2.6)])
        }
        CursorKind::Oval => ink.ring(Vec2::ZERO, 3.6, 2.6),
        CursorKind::Snap => {
            ink.line(&arc(v2(0.0, 0.4), 3.0, 3.0, 0.0, PI, 10));
            ink.line(&[v2(-3.0, 0.4), v2(-3.0, -2.8)]);
            ink.line(&[v2(3.0, 0.4), v2(3.0, -2.8)]);
            ink.accent_line(&[v2(-3.0, -2.0), v2(-3.0, -2.8)]);
            ink.accent_line(&[v2(3.0, -2.0), v2(3.0, -2.8)]);
        }
        _ => {}
    }
}

/// Draws `kind` in the Studio look.
pub(super) fn paint(
    shapes: &mut Vec<Shape>,
    kind: CursorKind,
    at: Pos2,
    scale: f32,
    pal: Palette,
    m: &Motion,
) {
    let appear = ease_out(m.enter / 0.18);
    let slide = (1.0 - appear) * 2.2;
    let press = m.press().clamp(0.0, 1.0);
    let tap = press * 0.9;
    let mut ink = Ink::new(at + vec2(slide, slide) * scale, scale, pal, appear);
    let badge_at = v2(10.0, 13.5);
    match kind {
        CursorKind::Arrow => slim_arrow(&mut ink, false, press),
        CursorKind::Subselect => {
            slim_arrow(&mut ink, true, press);
            let accent = ink.accent;
            ink.dot(v2(9.6, 16.0), 1.3, accent);
        }
        CursorKind::Link => pointing_hand(&mut ink, press),
        CursorKind::Text => ibeam(&mut ink),
        CursorKind::Grab => open_hand(&mut ink, 0.15 * press),
        CursorKind::Grabbing => fist(&mut ink),
        CursorKind::Move => four_way(&mut ink),
        CursorKind::Crosshair => crosshair(&mut ink, press),
        CursorKind::ResizeH => double_arrow(&mut ink, 0.0, 8.0),
        CursorKind::ResizeV => double_arrow(&mut ink, FRAC_PI_2, 8.0),
        CursorKind::ResizeNwSe => double_arrow(&mut ink, FRAC_PI_4, 7.6),
        CursorKind::ResizeNeSw => double_arrow(&mut ink, -FRAC_PI_4, 7.6),
        CursorKind::Rotate => {
            let spin = if m.held { m.sweep * 0.012 } else { 0.0 };
            turn(&mut ink, spin);
        }
        CursorKind::SkewH => shear(&mut ink, 0.0),
        CursorKind::SkewV => shear(&mut ink, FRAC_PI_2),
        CursorKind::Scrub => number_drag(&mut ink, m.lean.x),
        CursorKind::Pen => tool(&mut ink, tap, pen_nib),
        CursorKind::PenClose => {
            tool(&mut ink, tap, pen_nib);
            badge(&mut ink, v2(8.0, 9.0), |ink| ink.ring(Vec2::ZERO, 2.4, 2.4));
        }
        CursorKind::Pencil => tool(&mut ink, tap, pencil),
        CursorKind::RasterBrush => tool(&mut ink, tap, round_brush),
        CursorKind::RasterEraser => tool(&mut ink, tap, rubber),
        CursorKind::Smudge => tool(&mut ink, tap, finger),
        CursorKind::Eyedropper => {
            let sample = m.sample;
            tool(&mut ink, tap, |ink| dropper(ink, sample));
        }
        CursorKind::Knife => tool(&mut ink, tap, |ink| blade(ink, false)),
        CursorKind::KnifeCut => tool(&mut ink, tap, |ink| blade(ink, true)),
        CursorKind::Bucket => {
            let colour = m.sample.unwrap_or(ink.accent);
            pail(&mut ink, press, colour);
        }
        CursorKind::Lasso => rope(&mut ink, if m.held { 0.6 } else { 0.0 }),
        CursorKind::Line | CursorKind::Rect | CursorKind::Oval | CursorKind::Snap => {
            crosshair(&mut ink, press);
            badge(&mut ink, v2(9.0, 9.0), |ink| shape_badge(ink, kind));
        }
        CursorKind::ScreenPick => loupe(&mut ink),
        CursorKind::FrameDrag => diamond(&mut ink),
        CursorKind::Pivot => target(&mut ink, press),
        CursorKind::LibraryDrop => {
            slim_arrow(&mut ink, false, press);
            badge(&mut ink, badge_at, card_badge);
        }
        CursorKind::ParentLink => {
            slim_arrow(&mut ink, false, press);
            badge(&mut ink, badge_at, |ink| link_badge(ink, 0.0));
        }
        CursorKind::ParentUnlink => {
            slim_arrow(&mut ink, false, press);
            badge(&mut ink, badge_at, |ink| link_badge(ink, 1.2));
        }
        CursorKind::SelectAdd => {
            slim_arrow(&mut ink, false, press);
            badge(&mut ink, badge_at, plus_badge);
        }
        CursorKind::SelectRemove => {
            slim_arrow(&mut ink, false, press);
            badge(&mut ink, badge_at, minus_badge);
        }
        CursorKind::CopyDrag => {
            slim_arrow(&mut ink, false, press);
            badge(&mut ink, badge_at, copy_badge);
        }
        CursorKind::Locked => {
            slim_arrow(&mut ink, false, 0.0);
            badge(&mut ink, badge_at, lock_badge);
        }
    }
    ink.finish(shapes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_draws_its_own_picture_for_every_pointer() {
        let pal = Palette::from_theme(&crate::settings::Theme::default());
        for kind in super::super::own::EVERY {
            let mut shapes = Vec::new();
            paint(&mut shapes, kind, pos2(40.0, 40.0), 1.0, pal, &Motion::REST);
            assert!(shapes.len() >= 2, "{kind:?}");
            // Everything stays near the hotspot.
            for shape in &shapes {
                let bounds = shape.visual_bounding_rect();
                if bounds.is_finite() && bounds.is_positive() {
                    assert!(
                        bounds.expand(1.0).intersects(egui::Rect::from_center_size(
                            pos2(40.0, 40.0),
                            vec2(64.0, 64.0)
                        )),
                        "{kind:?} {bounds:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_screen_picker_leaves_its_middle_clear() {
        let pal = Palette::from_theme(&crate::settings::Theme::default());
        let mut shapes = Vec::new();
        let at = pos2(40.0, 40.0);
        paint(
            &mut shapes,
            CursorKind::ScreenPick,
            at,
            1.0,
            pal,
            &Motion::REST,
        );
        let mut tessellator =
            egui::epaint::Tessellator::new(1.0, Default::default(), [1, 1], Vec::new());
        for shape in shapes {
            let mut mesh = Mesh::default();
            tessellator.tessellate_shape(shape, &mut mesh);
            for triangle in mesh.indices.chunks(3) {
                let [a, b, c] = [0, 1, 2].map(|k| mesh.vertices[triangle[k] as usize].pos);
                let area = (b - a).x * (c - a).y - (b - a).y * (c - a).x;
                if area.abs() < 1e-6 {
                    continue;
                }
                let w0 = ((b - at).x * (c - at).y - (b - at).y * (c - at).x) / area;
                let w1 = ((c - at).x * (a - at).y - (c - at).y * (a - at).x) / area;
                let inside = w0 > 0.0 && w1 > 0.0 && 1.0 - w0 - w1 > 0.0;
                assert!(!inside, "a triangle covers the sampled pixel");
            }
        }
    }
}
