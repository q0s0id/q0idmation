//! The timeline's look: how spans, keyframes, the playhead and the ruler are
//! painted, and the springs and fades that move them.
//!
//! Nothing here loops on its own. Every motion is a glide or a fade started
//! by something the user did (a click, a key, a toggle), and each one asks
//! for repaints only until it settles, so an idle timeline costs nothing.

use std::cell::Cell;

use egui::{pos2, vec2, Color32, Context, FontId, Id, Mesh, Painter, Pos2, Rect, Shape, Stroke};

thread_local! {
    /// The Motion switch in the Timeline settings, set as each timeline is
    /// drawn.
    static MOTION: Cell<bool> = const { Cell::new(true) };
    /// The Keyframe corners and Dot shine settings, the same way.
    static KEYS: Cell<KeyLook> = const { Cell::new(KeyLook { round: 4.0, shine: true }) };
}

pub(super) fn set_motion(on: bool) {
    MOTION.with(|motion| motion.set(on));
}

/// How keyframes are drawn, from the Timeline settings.
#[derive(Clone, Copy)]
pub(super) struct KeyLook {
    /// How round a keyframe block's corners are, in points.
    pub round: f32,
    /// Whether filled dots carry their white shine.
    pub shine: bool,
}

pub(super) fn set_key_look(look: KeyLook) {
    KEYS.with(|keys| keys.set(look));
}

fn key_look() -> KeyLook {
    KEYS.with(Cell::get)
}

fn motion() -> bool {
    MOTION.with(Cell::get)
}

/// A 0..1 fade toward `on`, over `secs`; snaps when motion is off.
pub(super) fn fade(ctx: &Context, id: Id, on: bool, secs: f32) -> f32 {
    ctx.animate_bool_with_time(id, on, if motion() { secs } else { 1.0e-4 })
}

#[derive(Clone, Copy)]
struct Spring {
    value: f32,
    velocity: f32,
    time: f64,
}

/// Follows `target` like a weight on a critically damped spring: quick, no
/// overshoot, and a target that moves mid-glide bends the path instead of
/// restarting it. `period` is about how long a glide takes to settle.
/// Calling it twice in one frame with the same id gives the same value, so
/// the ruler and the rows can share one glide.
pub(super) fn glide(ctx: &Context, id: Id, target: f32, period: f32) -> f32 {
    let now = ctx.input(|input| input.time);
    let id = id.with("q0editor.timeline.glide");
    let fresh = Spring {
        value: target,
        velocity: 0.0,
        time: now,
    };
    let mut spring = ctx
        .data(|data| data.get_temp::<Spring>(id))
        .unwrap_or(fresh);
    if !motion() || period <= 0.0 || !spring.value.is_finite() {
        spring = fresh;
    } else if now > spring.time {
        let dt = ((now - spring.time) as f32).min(0.1);
        let omega = 6.0 / period;
        let offset = spring.value - target;
        let push = spring.velocity + omega * offset;
        let decay = (-omega * dt).exp();
        let offset = (offset + push * dt) * decay;
        spring.velocity = (spring.velocity - omega * push * dt) * decay;
        spring.value = target + offset;
        spring.time = now;
        if offset.abs() < 0.002 && spring.velocity.abs() < 0.05 {
            spring = fresh;
        }
    }
    if spring.value != target {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(id, spring));
    spring.value
}

/// How long a new keyframe takes to pop in.
const POP_SECS: f32 = 0.36;

#[derive(Clone, Default)]
struct Seen {
    items: Vec<u16>,
    arrivals: Vec<(u16, f64)>,
}

/// Remembers a list (a row's keyframes, a timeline's layers) from the last
/// frame drawn and when each new item turned up, and gives the ones that
/// came less than `secs` ago with their age in seconds. A list seen for the
/// first time brings nothing in, and neither does a change of more than
/// `most` items at once (pasting a whole stretch, opening a symbol), which
/// would only flicker.
pub(super) fn arrivals(
    ctx: &Context,
    id: Id,
    items: &[u16],
    secs: f32,
    most: usize,
) -> Vec<(u16, f32)> {
    let id = id.with("q0editor.timeline.arrivals");
    let now = ctx.input(|input| input.time);
    let Some(mut seen) = ctx.data(|data| data.get_temp::<Seen>(id)) else {
        ctx.data_mut(|data| {
            data.insert_temp(
                id,
                Seen {
                    items: items.to_vec(),
                    arrivals: Vec::new(),
                },
            )
        });
        return Vec::new();
    };
    if seen.items != items {
        let fresh: Vec<u16> = items
            .iter()
            .copied()
            .filter(|item| !seen.items.contains(item))
            .collect();
        if motion() && fresh.len() <= most {
            seen.arrivals
                .extend(fresh.into_iter().map(|item| (item, now)));
        }
        seen.items = items.to_vec();
    }
    seen.arrivals
        .retain(|(item, born)| now - born < f64::from(secs) && items.contains(item));
    let ages = seen
        .arrivals
        .iter()
        .map(|(item, born)| (*item, (now - born) as f32))
        .collect::<Vec<_>>();
    if !ages.is_empty() {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(id, seen));
    ages
}

#[derive(Clone, Default)]
struct Shown {
    /// The rows drawn last frame, the leaving ones among them, in order.
    rows: Vec<u16>,
    /// Rows on their way out, and when they set off.
    leaving: Vec<(u16, f64)>,
}

/// The rows to draw: `items`, with the ones that have just gone (a folder
/// or mask closed over them) still among them where they were, closing.
/// Each closing row comes with the share of its height still open, 1 down
/// to 0 over `secs`. Only rows `kept` still has (hidden, not deleted) close
/// this way, and only up to `most` at a time.
pub(super) fn with_leaving(
    ctx: &Context,
    id: Id,
    items: &[u16],
    kept: impl Fn(u16) -> bool,
    secs: f32,
    most: usize,
) -> (Vec<u16>, Vec<(u16, f32)>) {
    let id = id.with("q0editor.timeline.leaving");
    let now = ctx.input(|input| input.time);
    let Some(mut shown) = ctx.data(|data| data.get_temp::<Shown>(id)) else {
        ctx.data_mut(|data| {
            data.insert_temp(
                id,
                Shown {
                    rows: items.to_vec(),
                    leaving: Vec::new(),
                },
            )
        });
        return (items.to_vec(), Vec::new());
    };
    let gone: Vec<u16> = shown
        .rows
        .iter()
        .copied()
        .filter(|row| {
            !items.contains(row) && !shown.leaving.iter().any(|(id, _)| id == row) && kept(*row)
        })
        .collect();
    if motion() && gone.len() <= most {
        shown.leaving.extend(gone.into_iter().map(|row| (row, now)));
    }
    shown
        .leaving
        .retain(|(row, since)| now - since < f64::from(secs) && !items.contains(row) && kept(*row));
    // Each leaving row goes back in after the row it followed last time.
    let mut rows = items.to_vec();
    for (index, row) in shown.rows.iter().enumerate() {
        if !shown.leaving.iter().any(|(id, _)| id == row) {
            continue;
        }
        let at = shown.rows[..index]
            .iter()
            .rev()
            .find_map(|before| rows.iter().position(|row| row == before))
            .map_or(0, |position| position + 1);
        rows.insert(at, *row);
    }
    let shares = shown
        .leaving
        .iter()
        .map(|(row, since)| {
            let t = ((now - since) as f32 / secs).clamp(0.0, 1.0);
            (*row, 1.0 - t * t * (3.0 - 2.0 * t))
        })
        .collect::<Vec<_>>();
    if !shares.is_empty() {
        ctx.request_repaint();
    }
    shown.rows = rows.clone();
    ctx.data_mut(|data| data.insert_temp(id, shown));
    (rows, shares)
}

/// A wave across the rows when another project's timeline takes this one's
/// place: each row opens a moment after the one above it. Gives each row's
/// open share by its place in `rows`, or nothing once the wave has passed.
pub(super) fn switch_wave(ctx: &Context, id: Id, project: u64, rows: &[u16]) -> Vec<(u16, f32)> {
    const OPEN: f32 = 0.24;
    let id = id.with("q0editor.timeline.wave");
    let now = ctx.input(|input| input.time);
    let last: Option<(u64, f64)> = ctx.data(|data| data.get_temp(id));
    let since = match last {
        Some((seen, since)) if seen == project => since,
        // The first timeline drawn opens as it is; a switch starts a wave.
        None => f64::NEG_INFINITY,
        Some(_) => now,
    };
    ctx.data_mut(|data| data.insert_temp(id, (project, since)));
    if !motion() || !since.is_finite() {
        return Vec::new();
    }
    let age = (now - since) as f32;
    let stagger = (0.4 / rows.len().max(1) as f32).min(0.035);
    if age > OPEN + stagger * rows.len() as f32 {
        return Vec::new();
    }
    ctx.request_repaint();
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let t = (age - stagger * index as f32) / OPEN;
            (*row, ease_out(t.max(0.0)))
        })
        .collect()
}

/// The keyframes of a row still popping in, with their age in seconds.
pub(super) fn key_pops(ctx: &Context, id: Id, keys: &[u16]) -> Vec<(u16, f32)> {
    arrivals(ctx, id, keys, POP_SECS, 16)
}

/// Fast, then settling: 0 at 0, 1 at 1 and after.
pub(super) fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// `a` toward `b` by `t`, alpha included.
pub(super) fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

/// Text that reads on top of `fill`.
pub(super) fn ink_on(fill: Color32) -> Color32 {
    let light =
        0.299 * f32::from(fill.r()) + 0.587 * f32::from(fill.g()) + 0.114 * f32::from(fill.b());
    if light > 150.0 {
        Color32::from_gray(22)
    } else {
        Color32::from_gray(250)
    }
}

/// A left-to-right gradient over `rect`.
pub(super) fn sideways(rect: Rect, [left, right]: [Color32; 2]) -> Shape {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    Shape::mesh(mesh)
}

/// One held stretch of frames: a rounded block lit a little at the top,
/// with the small square at its end when it holds without a tween.
pub(super) fn paint_span(
    painter: &Painter,
    cells: Rect,
    fill: Color32,
    edge: Color32,
    hold_end: bool,
) {
    let block = Rect::from_min_max(cells.min + vec2(0.5, 0.5), cells.max - vec2(0.5, 0.5));
    if block.width() <= 0.5 || block.height() <= 1.0 {
        return;
    }
    // Never rounder than a pill: half the block's height or width.
    let round = key_look()
        .round
        .max(0.0)
        .min(block.height() * 0.5)
        .min(block.width() * 0.5);
    painter.add(crate::ui::gradient_shape(
        painter.ctx(),
        block,
        round,
        [
            mix(fill, Color32::WHITE, 0.1),
            mix(fill, Color32::BLACK, 0.05),
        ],
    ));
    if block.width() > round * 2.0 + 2.0 {
        painter.line_segment(
            [
                pos2(block.left() + round, block.top() + 0.75),
                pos2(block.right() - round, block.top() + 0.75),
            ],
            Stroke::new(1.0_f32, Color32::from_white_alpha(24)),
        );
    }
    painter.rect_stroke(
        block,
        round,
        Stroke::new(1.0_f32, edge.gamma_multiply(0.55)),
    );
    if hold_end && block.width() > 11.0 && block.height() > 9.0 {
        let glyph = Rect::from_min_size(
            pos2(block.right() - 6.5, block.bottom() - 6.5),
            vec2(4.0, 4.0),
        );
        painter.rect_stroke(glyph, 1.0, Stroke::new(1.0_f32, edge));
    }
}

/// A tween's arrow from one key to the next. While the playhead is inside
/// the tween a bead rides the arrow with it, showing how far along it is.
pub(super) fn paint_tween_arrow(
    painter: &Painter,
    (from_x, to_x): (f32, f32),
    y: f32,
    color: Color32,
    bead: Option<f32>,
) {
    if to_x <= from_x + 2.0 {
        return;
    }
    let head = ((to_x - from_x) * 0.5).min(5.0);
    painter.line_segment(
        [pos2(from_x, y), pos2(to_x - head + 0.5, y)],
        Stroke::new(1.25_f32, color),
    );
    painter.add(Shape::convex_polygon(
        vec![
            pos2(to_x, y),
            pos2(to_x - head, y - head * 0.62),
            pos2(to_x - head, y + head * 0.62),
        ],
        color,
        Stroke::NONE,
    ));
    if let Some(x) = bead.filter(|x| *x >= from_x && *x <= to_x - head) {
        painter.circle_filled(pos2(x, y), 4.4, color.gamma_multiply(0.22));
        painter.circle_filled(pos2(x, y), 2.1, color);
    }
}

/// How a keyframe dot is drawn.
pub(super) struct Key {
    pub center: Pos2,
    pub radius: f32,
    /// A blank keyframe: a ring with the cell showing through.
    pub blank: bool,
    pub color: Color32,
    /// The dark rim that keeps it readable on any span colour.
    pub rim: Color32,
    /// What shows through a blank one.
    pub hole: Color32,
    /// Seconds since it appeared, while it is still popping in.
    pub age: Option<f32>,
    /// A ring round it: the playhead is on it.
    pub ring: Option<Color32>,
}

pub(super) fn paint_key(painter: &Painter, key: &Key) {
    let (scale, ripple) = match key.age {
        Some(age) => {
            let p = (age / POP_SECS).clamp(0.0, 1.0);
            // Ease out with a little overshoot, like something set down.
            let (c1, q) = (2.2_f32, p - 1.0);
            (1.0 + (c1 + 1.0) * q * q * q + c1 * q * q, Some(p))
        }
        None => (1.0, None),
    };
    let r = key.radius * scale.max(0.0);
    if let Some(p) = ripple {
        let fade = 1.0 - p;
        painter.circle_stroke(
            key.center,
            key.radius * (1.0 + 2.4 * p),
            Stroke::new(1.6 * fade, key.color.gamma_multiply(fade)),
        );
    }
    if let Some(ring) = key.ring {
        painter.circle_stroke(key.center, key.radius + 2.6, Stroke::new(1.3_f32, ring));
    }
    if r <= 0.2 {
        return;
    }
    painter.circle_filled(key.center, r + 0.9, key.rim);
    if key.blank {
        painter.circle_filled(key.center, r, key.hole);
        painter.circle_stroke(key.center, r - 0.55, Stroke::new(1.3_f32, key.color));
    } else {
        painter.circle_filled(key.center, r, key.color);
        if key_look().shine {
            painter.circle_filled(
                key.center + vec2(-r * 0.3, -r * 0.32),
                r * 0.36,
                Color32::from_white_alpha(90),
            );
        }
    }
}

/// The playhead's head on the ruler: a tab with the frame number in it and
/// a point that meets the line below.
pub(super) fn paint_playhead_head(
    painter: &Painter,
    x: f32,
    ruler: Rect,
    label: String,
    color: Color32,
) {
    let ink = ink_on(color);
    let galley = painter.layout_no_wrap(label, FontId::proportional(10.0), ink);
    let width = (galley.size().x + 9.0).max(15.0);
    let head = Rect::from_center_size(pos2(x, ruler.bottom() - 11.5), vec2(width, 14.0));
    painter.rect_filled(
        head.translate(vec2(0.0, 1.2)).expand(0.6),
        4.6,
        Color32::from_black_alpha(70),
    );
    painter.add(crate::ui::gradient_shape(
        painter.ctx(),
        head,
        4.0,
        [mix(color, Color32::WHITE, 0.2), color],
    ));
    painter.add(Shape::convex_polygon(
        vec![
            pos2(x - 4.0, head.bottom() - 0.5),
            pos2(x + 4.0, head.bottom() - 0.5),
            pos2(x, ruler.bottom()),
        ],
        color,
        Stroke::NONE,
    ));
    painter.galley(head.center() - galley.size() * 0.5, galley, ink);
}

/// The frame under the pointer, on the ruler: an outlined tab that says
/// which frame a click there would go to.
pub(super) fn paint_ghost_head(
    painter: &Painter,
    x: f32,
    ruler: Rect,
    label: String,
    (fill, line, ink): (Color32, Color32, Color32),
    shown: f32,
) {
    if shown <= 0.01 {
        return;
    }
    let galley =
        painter.layout_no_wrap(label, FontId::proportional(10.0), ink.gamma_multiply(shown));
    let width = (galley.size().x + 9.0).max(15.0);
    let head = Rect::from_center_size(pos2(x, ruler.bottom() - 11.5), vec2(width, 14.0));
    painter.line_segment(
        [pos2(x, head.bottom()), pos2(x, ruler.bottom())],
        Stroke::new(1.0_f32, line.gamma_multiply(shown)),
    );
    painter.rect_filled(head, 4.0, fill.gamma_multiply(shown));
    painter.rect_stroke(head, 4.0, Stroke::new(1.0_f32, line.gamma_multiply(shown)));
    painter.galley(
        head.center() - galley.size() * 0.5,
        galley,
        ink.gamma_multiply(shown),
    );
}

/// The playhead's line down through the rows, with a soft glow, and while
/// it plays a fading trail over the frames it just left.
pub(super) fn paint_playhead_line(
    painter: &Painter,
    x: f32,
    (top, bottom): (f32, f32),
    color: Color32,
    trail: Option<f32>,
) {
    if let Some(width) = trail.filter(|width| *width > 1.0) {
        painter.add(sideways(
            Rect::from_min_max(pos2(x - width, top), pos2(x, bottom)),
            [Color32::TRANSPARENT, color.gamma_multiply(0.16)],
        ));
    }
    painter.line_segment(
        [pos2(x, top), pos2(x, bottom)],
        Stroke::new(5.0_f32, color.gamma_multiply(0.16)),
    );
    painter.line_segment([pos2(x, top), pos2(x, bottom)], Stroke::new(1.5_f32, color));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_reaches_both_ends() {
        let a = Color32::from_rgb(10, 20, 30);
        let b = Color32::from_rgb(210, 220, 230);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), Color32::from_rgb(110, 120, 130));
    }

    #[test]
    fn ink_reads_on_light_and_dark() {
        assert_eq!(ink_on(Color32::WHITE), Color32::from_gray(22));
        assert_eq!(
            ink_on(Color32::from_rgb(200, 40, 40)),
            Color32::from_gray(250)
        );
    }

    #[test]
    fn a_glide_settles_on_its_target_and_new_keys_pop() {
        let ctx = Context::default();
        let id = Id::new("glide-test");
        let mut value = 0.0;
        let mut pops = Vec::new();
        for frame in 0..90 {
            let mut input = egui::RawInput {
                time: Some(f64::from(frame) / 60.0),
                ..Default::default()
            };
            input.predicted_dt = 1.0 / 60.0;
            let _ = ctx.run(input, |ctx| {
                set_motion(true);
                let target = if frame == 0 { 0.0 } else { 10.0 };
                value = glide(ctx, id, target, 0.2);
                let keys: &[u16] = if frame < 3 { &[0] } else { &[0, 5] };
                let ages = key_pops(ctx, id, keys);
                if frame == 3 {
                    pops = ages;
                }
            });
        }
        assert_eq!(value, 10.0);
        assert_eq!(pops.len(), 1);
        assert_eq!(pops[0].0, 5);
    }

    #[test]
    fn rows_a_folder_closes_over_shrink_where_they_were() {
        set_motion(true);
        let ctx = Context::default();
        let id = Id::new("rows");
        let (rows, _) = with_leaving(&ctx, id, &[1, 2, 3, 4], |_| true, 0.24, 64);
        assert_eq!(rows, vec![1, 2, 3, 4]);
        let (rows, closing) = with_leaving(&ctx, id, &[1, 4], |_| true, 0.24, 64);
        assert_eq!(rows, vec![1, 2, 3, 4], "still in their places");
        assert_eq!(closing.len(), 2);
        // A deleted row just goes.
        let (rows, _) = with_leaving(&ctx, id, &[1], |row| row != 4, 0.24, 64);
        assert_eq!(rows, vec![1, 2, 3]);
    }
}
