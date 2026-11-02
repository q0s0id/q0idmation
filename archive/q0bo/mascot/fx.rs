//! What q0bo draws over the editor: his own mouse pointer, the keys he
//! presses, marching ants around what he talks about and a bit of confetti.

use egui::{
    epaint::{Mesh, PathShape},
    pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2,
};

use super::face::{INK, RED};

/// q0bo's mouse pointer: where it is, where it is going and whether its
/// button is down.
#[derive(Clone, Debug)]
pub(crate) struct Hand {
    pub pos: Pos2,
    pub down: bool,
    /// 0 hidden, 1 fully shown.
    pub shown: f32,
    /// Where and when the button last went down, for the ripple.
    pub clicked: Option<(Pos2, f64)>,
}

impl Default for Hand {
    fn default() -> Self {
        Self {
            pos: Pos2::ZERO,
            down: false,
            shown: 0.0,
            clicked: None,
        }
    }
}

impl Hand {
    pub(crate) fn press(&mut self, now: f64) {
        if !self.down {
            self.down = true;
            self.clicked = Some((self.pos, now));
        }
    }

    pub(crate) fn paint(&self, painter: &Painter, now: f64) {
        if let Some((at, when)) = self.clicked {
            let age = (now - when) as f32;
            if age < 0.45 {
                let k = age / 0.45;
                painter.circle_stroke(
                    at,
                    4.0 + k * 18.0,
                    Stroke::new(2.5 * (1.0 - k), RED.gamma_multiply(1.0 - k)),
                );
            }
        }
        if self.shown <= 0.01 {
            return;
        }
        let alpha = self.shown;
        let scale = if self.down { 0.88 } else { 1.0 };
        // The classic arrow, in points with its tip at the origin.
        let outline = [
            vec2(0.0, 0.0),
            vec2(0.0, 17.0),
            vec2(4.2, 13.2),
            vec2(7.0, 19.8),
            vec2(9.8, 18.6),
            vec2(7.0, 12.2),
            vec2(12.2, 12.2),
        ];
        let at = |v: Vec2| self.pos + v * scale * 1.15;
        let shadow = |v: Vec2| at(v) + vec2(2.0, 2.5);
        let mut mesh = Mesh::default();
        let fan = |mesh: &mut Mesh, place: &dyn Fn(Vec2) -> Pos2, colour: Color32| {
            let first = mesh.vertices.len() as u32;
            mesh.colored_vertex(place(vec2(3.6, 10.6)), colour);
            for v in outline {
                mesh.colored_vertex(place(v), colour);
            }
            for i in 0..outline.len() as u32 {
                let next = (i + 1) % outline.len() as u32;
                mesh.add_triangle(first, first + 1 + i, first + 1 + next);
            }
        };
        fan(&mut mesh, &shadow, Color32::from_black_alpha((70.0 * alpha) as u8));
        fan(&mut mesh, &at, RED.gamma_multiply(alpha));
        painter.add(Shape::mesh(mesh));
        painter.add(Shape::Path(PathShape::closed_line(
            outline.iter().map(|v| at(*v)).collect(),
            Stroke::new(1.6_f32, INK.gamma_multiply(alpha)),
        )));
        // A name tag, like the other cursors in a shared document.
        let tag_at = at(vec2(13.0, 18.0));
        let galley = painter.layout_no_wrap(
            "q0bo".to_owned(),
            FontId::monospace(11.0),
            Color32::WHITE.gamma_multiply(alpha),
        );
        let tag = Rect::from_min_size(tag_at, galley.size() + vec2(8.0, 4.0));
        painter.rect(
            tag,
            3.0,
            INK.gamma_multiply(alpha),
            Stroke::new(1.0_f32, RED.gamma_multiply(alpha)),
        );
        painter.galley(tag.min + vec2(4.0, 2.0), galley, Color32::WHITE);
    }
}

/// A keyboard key being pressed, drawn like a chunky old keycap.
pub(crate) fn keycap(painter: &Painter, centre: Pos2, label: &str, age: f32) {
    let pop = (age / 0.12).clamp(0.3, 1.0);
    let fade = if age > 0.9 { (1.3 - age).max(0.0) / 0.4 } else { 1.0 };
    if fade <= 0.0 {
        return;
    }
    let pressed = (0.12..0.32).contains(&age);
    let galley = painter.layout_no_wrap(
        label.to_owned(),
        FontId::monospace(18.0 * pop),
        INK.gamma_multiply(fade),
    );
    let size = galley.size() + vec2(22.0, 14.0);
    let depth = if pressed { 2.0 } else { 6.0 };
    let top = Rect::from_center_size(centre + vec2(0.0, 6.0 - depth), size);
    let base = top.translate(vec2(0.0, depth));
    painter.rect(
        base,
        5.0,
        Color32::from_rgb(0x8C, 0x86, 0x7C).gamma_multiply(fade),
        Stroke::new(1.5_f32, INK.gamma_multiply(fade)),
    );
    painter.rect(
        top,
        5.0,
        Color32::from_rgb(0xF2, 0xEE, 0xE4).gamma_multiply(fade),
        Stroke::new(1.5_f32, INK.gamma_multiply(fade)),
    );
    painter.galley(
        top.center() - galley.size() * 0.5,
        galley,
        INK.gamma_multiply(fade),
    );
}

/// Marching ants and a bobbing arrow around something on screen.
pub(crate) fn highlight(painter: &Painter, rect: Rect, now: f64, screen: Rect) {
    let rect = rect.expand(4.0);
    let offset = (now * 22.0) as f32 % 12.0;
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    painter.add(Shape::closed_line(
        corners[..4].to_vec(),
        Stroke::new(3.0_f32, Color32::WHITE.gamma_multiply(0.85)),
    ));
    painter.extend(Shape::dashed_line_with_offset(
        &corners,
        Stroke::new(3.0_f32, RED),
        &[6.0],
        &[6.0],
        offset,
    ));
    // The arrow points down at the rectangle, or up when there is no room above.
    let bob = ((now * 5.0).sin() as f32) * 4.0;
    let above = rect.top() > screen.top() + 40.0;
    let x = super::within(rect.center().x, screen.left() + 16.0, screen.right() - 16.0);
    let (tip, dir) = if above {
        (pos2(x, rect.top() - 6.0 + bob), 1.0)
    } else {
        (pos2(x, rect.bottom() + 6.0 - bob), -1.0)
    };
    let arrow = vec![
        tip,
        tip + vec2(-11.0, -14.0 * dir),
        tip + vec2(-4.5, -14.0 * dir),
        tip + vec2(-4.5, -26.0 * dir),
        tip + vec2(4.5, -26.0 * dir),
        tip + vec2(4.5, -14.0 * dir),
        tip + vec2(11.0, -14.0 * dir),
    ];
    // Head and shaft are each convex; the outline goes over both.
    painter.add(Shape::convex_polygon(
        vec![arrow[0], arrow[1], arrow[6]],
        RED,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![arrow[2], arrow[3], arrow[4], arrow[5]],
        RED,
        Stroke::NONE,
    ));
    painter.add(Shape::closed_line(arrow, Stroke::new(1.6_f32, INK)));
}

/// One bit of confetti.
#[derive(Clone, Debug)]
pub(crate) struct Bit {
    pos: Pos2,
    vel: Vec2,
    born: f64,
    colour: Color32,
    size: f32,
}

/// Confetti and floating "+xp" text, thrown when the user does a thing.
#[derive(Clone, Debug, Default)]
pub(crate) struct Party {
    bits: Vec<Bit>,
    texts: Vec<(Pos2, f64, &'static str)>,
    last: Option<f64>,
}

impl Party {
    pub(crate) fn burst(&mut self, at: Pos2, now: f64, text: &'static str) {
        let colours = [RED, INK, Color32::WHITE, Color32::from_rgb(0xFF, 0xC8, 0x3D)];
        // A cheap, deterministic scatter: confetti does not need real randomness.
        let mut seed = (now * 1000.0) as u32 | 1;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 10_000) as f32 / 10_000.0
        };
        for i in 0..36 {
            let angle = -std::f32::consts::FRAC_PI_2 + (next() - 0.5) * 2.4;
            let speed = 160.0 + next() * 260.0;
            self.bits.push(Bit {
                pos: at,
                vel: vec2(angle.cos(), angle.sin()) * speed,
                born: now,
                colour: colours[i % colours.len()],
                size: 3.0 + next() * 3.5,
            });
        }
        self.texts.push((at, now, text));
    }

    pub(crate) fn busy(&self) -> bool {
        !self.bits.is_empty() || !self.texts.is_empty()
    }

    pub(crate) fn paint(&mut self, painter: &Painter, now: f64) {
        let dt = self
            .last
            .map_or(0.0, |last| ((now - last) as f32).clamp(0.0, 0.05));
        self.last = Some(now);
        self.bits.retain(|bit| now - bit.born < 1.6);
        for bit in &mut self.bits {
            bit.vel.y += 620.0 * dt;
            bit.vel *= 1.0 - 0.9 * dt;
            bit.pos += bit.vel * dt;
            let age = (now - bit.born) as f32;
            let fade = (1.0 - (age - 1.0).max(0.0) / 0.6).clamp(0.0, 1.0);
            let wobble = (age * 14.0 + bit.size).sin();
            painter.rect_filled(
                Rect::from_center_size(bit.pos, vec2(bit.size, bit.size * (0.35 + 0.65 * wobble.abs()))),
                0.0,
                bit.colour.gamma_multiply(fade),
            );
        }
        self.texts.retain(|(_, born, _)| now - born < 1.4);
        for (at, born, text) in &self.texts {
            let age = (now - born) as f32;
            let fade = (1.0 - (age - 0.8).max(0.0) / 0.6).clamp(0.0, 1.0);
            let point = *at + vec2(0.0, -40.0 - age * 50.0);
            for (offset, colour) in [(vec2(2.0, 2.0), INK), (Vec2::ZERO, RED)] {
                painter.text(
                    point + offset,
                    Align2::CENTER_CENTER,
                    *text,
                    FontId::monospace(20.0),
                    colour.gamma_multiply(fade),
                );
            }
        }
    }
}

/// Eases in and out, for flights and drags.
pub(crate) fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A point on a gentle arc from `a` to `b`, so the pointer swoops instead of
/// sliding along a ruler.
pub(crate) fn swoop(a: Pos2, b: Pos2, t: f32) -> Pos2 {
    let t = smooth(t);
    let d = b - a;
    let bulge = vec2(-d.y, d.x) * 0.12;
    let control = a + d * 0.5 + bulge;
    let p = a.lerp(control, t);
    p.lerp(control.lerp(b, t), t)
}
