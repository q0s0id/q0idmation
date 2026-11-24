//! Cursor packs: other looks for the same pointers.
//!
//! - **Studio**: calm and precise, as drawing programs have it. Drawing
//!   tools are a fine crosshair with the tool's picture beside it; nothing
//!   sways or bounces, presses only tighten.
//! - **Retro**: every pointer is pixel art, hard edged and outlined, in a
//!   ramp of the accent colour. It moves in steps, a few frames a second,
//!   and the main arrow's edge glows in a cycle.
//! - **Critters**: the pointers are alive. The arrow is a round little
//!   thing with a face, tools wear googly eyes that jiggle behind every
//!   move, everyone blinks, and left alone for a while they doze off.
//! - **Your own**: pixel pointers drawn in the cursor maker (`own.rs`);
//!   the ones not drawn yet are the standard ones.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use super::*;

/// A look for the pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CursorPack {
    #[default]
    Standard,
    Studio,
    Retro,
    Critters,
    Own,
}

impl CursorPack {
    pub(crate) const ALL: [Self; 5] = [
        Self::Standard,
        Self::Studio,
        Self::Retro,
        Self::Critters,
        Self::Own,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Studio => "Studio",
            Self::Retro => "Retro",
            Self::Critters => "Critters",
            Self::Own => "Your own",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Standard => "The editor's own pointers: springy, they lean into moves and settle.",
            Self::Studio => "Calm and precise, like a painting program: fine crosshairs with the tool beside them, thin outlines, no sway.",
            Self::Retro => "Old-school pixel art: hard edges, a heavy outline, a few frames a second, and a glowing arrow.",
            Self::Critters => "Alive: a round little pointer with a face, googly eyes on the tools, blinks and naps. Keeps moving while you rest, a few redraws a second.",
            Self::Own => "The pixel pointers you draw below. The ones you have not drawn are the standard ones.",
        }
    }

    /// Squash at edges, stretch at speed and the trailing lean.
    pub(crate) fn bends(self) -> bool {
        matches!(self, Self::Standard | Self::Critters)
    }

    /// Click rings.
    pub(crate) fn ripples(self) -> bool {
        matches!(self, Self::Standard | Self::Critters)
    }

    /// A short cross-fade when the pointer changes.
    pub(crate) fn fades(self) -> bool {
        matches!(self, Self::Standard | Self::Studio | Self::Critters)
    }

    /// How often a resting pointer is drawn again, for packs that move
    /// while nothing happens.
    pub(crate) fn idle_frame(self) -> Option<Duration> {
        match self {
            Self::Retro => Some(Duration::from_millis(125)),
            Self::Critters => Some(Duration::from_millis(50)),
            _ => None,
        }
    }
}

thread_local! {
    static PIXELS_PER_POINT: Cell<f32> = const { Cell::new(1.0) };
}

/// Pixel packs line their pixels up with the screen's.
pub(super) fn set_pixels_per_point(ppp: f32) {
    if ppp.is_finite() && ppp > 0.0 {
        PIXELS_PER_POINT.with(|cell| cell.set(ppp));
    }
}

pub(super) fn pixels_per_point() -> f32 {
    PIXELS_PER_POINT.with(Cell::get)
}

/// `point` moved onto the screen's pixel grid.
pub(super) fn snap(point: Pos2) -> Pos2 {
    let ppp = pixels_per_point();
    pos2((point.x * ppp).round() / ppp, (point.y * ppp).round() / ppp)
}

/// One drawing pass with its shadow, at `line` points of outline per unit
/// of scale.
fn canvas_pass(
    shapes: &mut Vec<Shape>,
    at: Pos2,
    scale: f32,
    xf: Xf,
    pal: Palette,
    line: f32,
    draw: impl Fn(&mut Canvas),
) {
    if pal.shadow.a() > 0 {
        let mut shadow = Canvas::new(at + vec2(0.5, 1.4) * scale, scale, pal, true);
        shadow.xf = xf;
        shadow.line = (line * scale).max(0.75);
        draw(&mut shadow);
        shapes.extend(shadow.shapes);
    }
    let mut canvas = Canvas::new(at, scale, pal, false);
    canvas.xf = xf;
    canvas.line = (line * scale).max(0.75);
    draw(&mut canvas);
    shapes.extend(canvas.shapes);
}

// ---------------------------------------------------------------------------
// Retro

/// A pointer as pixels, with its hotspot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sprite {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) hotspot: (i32, i32),
    /// Row by row; `None` is clear.
    pub(crate) cells: Vec<Option<Color32>>,
    /// Cells that glow in a cycle, by their place in the glow ramp.
    pub(crate) glow: Vec<Option<u8>>,
}

/// The colours a retro pointer is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ramp {
    outline: Color32,
    body: Color32,
    body_shade: Color32,
    dark: Color32,
    mid: Color32,
    light: Color32,
    highlight: Color32,
}

impl Ramp {
    fn of(pal: Palette) -> Self {
        let accent = pal.accent;
        let scale = |k: f32| {
            Color32::from_rgb(
                (f32::from(accent.r()) * k) as u8,
                (f32::from(accent.g()) * k) as u8,
                (f32::from(accent.b()) * k) as u8,
            )
        };
        Self {
            outline: Color32::from_rgb(
                pal.outline.r() / 2,
                pal.outline.g() / 2,
                pal.outline.b() / 2,
            ),
            body: pal.fill,
            body_shade: mix(pal.fill, pal.outline, 0.3),
            dark: scale(0.38),
            mid: scale(0.68),
            light: accent,
            highlight: mix(accent, Color32::WHITE, 0.55),
        }
    }

    /// The glow cycle, darkest to brightest and back.
    fn glow(&self, at: u8) -> Color32 {
        [
            self.dark,
            self.mid,
            self.light,
            self.highlight,
            self.light,
            self.mid,
        ][usize::from(at) % 6]
    }

    fn colours(&self) -> [Color32; 7] {
        [
            self.outline,
            self.body,
            self.body_shade,
            self.dark,
            self.mid,
            self.light,
            self.highlight,
        ]
    }
}

/// The main arrow, drawn by hand. `K` outline, `h` highlight, `l` light,
/// `m` mid, `d` dark, `g` the edge that glows.
const RETRO_ARROW: [&str; 19] = [
    "K...........",
    "KK..........",
    "KgK.........",
    "KhgK........",
    "KhlgK.......",
    "KhllgK......",
    "KhlllgK.....",
    "KhllllgK....",
    "KhlllllgK...",
    "KhllllllgK..",
    "KhlllllllgK.",
    "KhllllmKKKKK",
    "KhlmKmmK....",
    "KhmKKdmK....",
    "KmK..KdmK...",
    "KK...KmdK...",
    "K.....KmdK..",
    "......KddK..",
    ".......KK...",
];

fn hand_drawn(rows: &[&str], ramp: &Ramp) -> Sprite {
    let height = rows.len();
    let width = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let mut cells = vec![None; width * height];
    let mut glow = vec![None; width * height];
    for (y, row) in rows.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let i = y * width + x;
            cells[i] = match ch {
                'K' => Some(ramp.outline),
                'h' => Some(ramp.highlight),
                'l' => Some(ramp.light),
                'm' => Some(ramp.mid),
                'd' => Some(ramp.dark),
                'g' => {
                    // Down the edge, each pixel a step on in the cycle.
                    glow[i] = Some((y % 6) as u8);
                    Some(ramp.light)
                }
                _ => None,
            };
        }
    }
    Sprite {
        width,
        height,
        hotspot: (0, 0),
        cells,
        glow,
    }
}

/// Points of the standard drawing that make one retro pixel.
const RETRO_SAMPLE: f32 = 1.25;
/// Points a retro pixel is drawn at, at size 1: chunky, about two screen
/// pixels each.
const RETRO_CELL: f32 = 1.7;
const GRID: usize = 48;

/// The standard drawing of `kind`, made into pixel art: drawn at one cell
/// per pixel, every pixel made solid or clear, every colour brought onto
/// the ramp, and the whole given a hard outline.
pub(super) fn rasterize(kind: CursorKind, pal: Palette, m: &Motion) -> Sprite {
    let ramp = Ramp::of(pal);
    // Only the fills: the hard outline comes after, a pixel all round.
    let flat = Palette {
        shadow: Color32::TRANSPARENT,
        outline: Color32::TRANSPARENT,
        ..pal
    };
    let origin = pos2(GRID as f32 * 0.5, GRID as f32 * 0.5);
    let mut canvas = Canvas::new(origin, 1.0 / RETRO_SAMPLE, flat, false);
    canvas.line = 0.6;
    draw(&mut canvas, kind, m);
    let mut tessellator = egui::epaint::Tessellator::new(
        1.0,
        egui::epaint::TessellationOptions {
            feathering: false,
            ..Default::default()
        },
        [1, 1],
        Vec::new(),
    );
    let mut mesh = Mesh::default();
    for shape in canvas.shapes {
        tessellator.tessellate_shape(shape, &mut mesh);
    }
    // Cover each cell's centre with the topmost triangle there.
    let mut coverage: Vec<[f32; 4]> = vec![[0.0; 4]; GRID * GRID];
    for triangle in mesh.indices.chunks(3) {
        if triangle.len() < 3 {
            continue;
        }
        let [a, b, c] = [0, 1, 2].map(|k| mesh.vertices[triangle[k] as usize]);
        let min_x = a.pos.x.min(b.pos.x).min(c.pos.x).floor().max(0.0) as usize;
        let max_x = (a.pos.x.max(b.pos.x).max(c.pos.x).ceil() as usize).min(GRID);
        let min_y = a.pos.y.min(b.pos.y).min(c.pos.y).floor().max(0.0) as usize;
        let max_y = (a.pos.y.max(b.pos.y).max(c.pos.y).ceil() as usize).min(GRID);
        let area = (b.pos - a.pos).x * (c.pos - a.pos).y - (b.pos - a.pos).y * (c.pos - a.pos).x;
        if area.abs() < 1e-6 {
            continue;
        }
        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = pos2(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((b.pos - p).x * (c.pos - p).y - (b.pos - p).y * (c.pos - p).x) / area;
                let w1 = ((c.pos - p).x * (a.pos - p).y - (c.pos - p).y * (a.pos - p).x) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let colour = [a.color, b.color, c.color].map(|v| v.to_array().map(f32::from));
                let src: [f32; 4] = std::array::from_fn(|k| {
                    colour[0][k] * w0 + colour[1][k] * w1 + colour[2][k] * w2
                });
                let dst = &mut coverage[y * GRID + x];
                let keep = 1.0 - src[3] / 255.0;
                for k in 0..4 {
                    dst[k] = src[k] + dst[k] * keep;
                }
            }
        }
    }
    // Fills only: details land on the body or accent colours.
    let mut colours = ramp.colours()[1..].to_vec();
    if let Some(sample) = m.sample {
        colours.push(sample);
    }
    let nearest = |rgb: [f32; 3]| {
        *colours
            .iter()
            .min_by(|a, b| {
                let d = |c: &Color32| {
                    (f32::from(c.r()) - rgb[0]).powi(2)
                        + (f32::from(c.g()) - rgb[1]).powi(2)
                        + (f32::from(c.b()) - rgb[2]).powi(2)
                };
                d(a).total_cmp(&d(b))
            })
            .expect("the ramp has colours")
    };
    let mut cells: Vec<Option<Color32>> = coverage
        .iter()
        .map(|c| {
            (c[3] >= 110.0).then(|| {
                let a = c[3] / 255.0;
                nearest([c[0] / a, c[1] / a, c[2] / a])
            })
        })
        .collect();
    // A hard outline wherever paint meets clear.
    let solid = cells.clone();
    for y in 0..GRID {
        for x in 0..GRID {
            if solid[y * GRID + x].is_some() {
                continue;
            }
            let neighbours = [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ];
            let touches = neighbours
                .iter()
                .any(|&(nx, ny)| nx < GRID && ny < GRID && solid[ny * GRID + nx].is_some());
            if touches {
                cells[y * GRID + x] = Some(ramp.outline);
            }
        }
    }
    crop(cells, ramp)
}

/// The sprite cut down to its painted cells, its hotspot moved with it.
fn crop(cells: Vec<Option<Color32>>, ramp: Ramp) -> Sprite {
    let (mut x0, mut y0, mut x1, mut y1) = (GRID, GRID, 0, 0);
    for y in 0..GRID {
        for x in 0..GRID {
            if cells[y * GRID + x].is_some() {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    let centre = (GRID / 2) as i32;
    if x0 >= x1 {
        return Sprite {
            width: 0,
            height: 0,
            hotspot: (0, 0),
            cells: Vec::new(),
            glow: Vec::new(),
        };
    }
    let width = x1 - x0;
    let height = y1 - y0;
    let mut out = Vec::with_capacity(width * height);
    let mut glow = Vec::with_capacity(width * height);
    for y in y0..y1 {
        for x in x0..x1 {
            let cell = cells[y * GRID + x];
            glow.push(match cell {
                Some(c) if c == ramp.light => Some(((x + y) % 6) as u8),
                _ => None,
            });
            out.push(cell);
        }
    }
    Sprite {
        width,
        height,
        hotspot: (centre - x0 as i32, centre - y0 as i32),
        cells: out,
        glow,
    }
}

/// Time cut into steps: animation as frames.
fn steps(t: f32, per_second: f32, settle: f32) -> f32 {
    if t >= settle {
        100.0
    } else {
        (t * per_second).floor() / per_second
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SpriteKey {
    kind: u8,
    enter: i32,
    pressed: i32,
    release: i32,
    held: bool,
    travel: i32,
    sample: Option<[u8; 4]>,
    ramp: [u8; 12],
}

thread_local! {
    static SPRITES: RefCell<HashMap<SpriteKey, Rc<Sprite>>> = RefCell::new(HashMap::new());
}

fn kind_index(kind: CursorKind) -> u8 {
    super::own::EVERY
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(0) as u8
}

/// The retro sprite for `kind` at this moment, made once per frame of its
/// animation.
pub(super) fn retro_sprite(kind: CursorKind, pal: Palette, m: &Motion) -> Rc<Sprite> {
    let ramp = Ramp::of(pal);
    let stepped = Motion {
        enter: steps(m.enter, 8.0, kind.entry_secs().max(0.01)),
        pressed: steps(m.pressed, 8.0, 0.75),
        release: steps(m.release, 8.0, 0.75),
        lean: Vec2::ZERO,
        travel: (m.travel / 12.0).floor() * 12.0 % 96.0,
        sweep: 0.0,
        drift: 0.0,
        ..*m
    };
    let colour = |c: Color32| [c.r(), c.g(), c.b()];
    let mut ramp_key = [0_u8; 12];
    ramp_key[..3].copy_from_slice(&colour(ramp.outline));
    ramp_key[3..6].copy_from_slice(&colour(ramp.body));
    ramp_key[6..9].copy_from_slice(&colour(ramp.light));
    ramp_key[9..12].copy_from_slice(&colour(pal.fill));
    let key = SpriteKey {
        kind: kind_index(kind),
        enter: (stepped.enter * 8.0) as i32,
        pressed: (stepped.pressed * 8.0) as i32,
        release: (stepped.release * 8.0) as i32,
        held: stepped.held,
        travel: stepped.travel as i32,
        sample: m.sample.map(|c| c.to_array()),
        ramp: ramp_key,
    };
    SPRITES.with(|sprites| {
        let mut sprites = sprites.borrow_mut();
        if let Some(sprite) = sprites.get(&key) {
            return Rc::clone(sprite);
        }
        if sprites.len() > 400 {
            sprites.clear();
        }
        let sprite = Rc::new(if kind == CursorKind::Arrow {
            hand_drawn(&RETRO_ARROW, &ramp)
        } else {
            rasterize(kind, pal, &stepped)
        });
        sprites.insert(key, Rc::clone(&sprite));
        sprite
    })
}

pub(super) fn retro(
    shapes: &mut Vec<Shape>,
    kind: CursorKind,
    at: Pos2,
    scale: f32,
    pal: Palette,
    m: &Motion,
) {
    let sprite = retro_sprite(kind, pal, m);
    let ramp = Ramp::of(pal);
    // The glow runs while the pointer is busy and, on the arrow, always.
    let busy = m.held || m.enter < 1.0 || m.release < 0.5 || kind == CursorKind::Arrow;
    let step = (m.now * 8.0).floor() as i64;
    let flash = m.held && m.pressed < 0.25;
    let glow = |cell: usize| -> Option<Color32> {
        let place = sprite.glow.get(cell).copied().flatten()?;
        if !busy {
            return None;
        }
        let at = (i64::from(place) - step).rem_euclid(6) as u8;
        Some(ramp.glow(if flash { at.saturating_add(1) } else { at }))
    };
    let cell = cell_size(RETRO_CELL * scale);
    paint_cells(
        shapes,
        &sprite,
        at,
        cell,
        pal.shadow.a() > 0,
        |index, colour| glow(index).unwrap_or(colour),
    );
}

/// A cell `points` wide, made a whole number of screen pixels.
pub(super) fn cell_size(points: f32) -> f32 {
    let ppp = pixels_per_point();
    (points * ppp).round().max(1.0) / ppp
}

/// Draws `sprite` with its hotspot on `at`, `cell` points to a pixel, with
/// a hard shadow a pixel down and right when `shadow`.
pub(super) fn paint_cells(
    shapes: &mut Vec<Shape>,
    sprite: &Sprite,
    at: Pos2,
    cell: f32,
    shadow: bool,
    colour: impl Fn(usize, Color32) -> Color32,
) {
    if sprite.width == 0 {
        return;
    }
    let origin = snap(
        at - vec2(
            sprite.hotspot.0 as f32 * cell,
            sprite.hotspot.1 as f32 * cell,
        ),
    );
    let mut mesh = Mesh::default();
    let mut add = |x: usize, y: usize, offset: Vec2, c: Color32| {
        let min = origin + vec2(x as f32 * cell, y as f32 * cell) + offset;
        mesh.add_colored_rect(egui::Rect::from_min_size(min, vec2(cell, cell)), c);
    };
    if shadow {
        let dark = Color32::from_black_alpha(110);
        for y in 0..sprite.height {
            for x in 0..sprite.width {
                if sprite.cells[y * sprite.width + x].is_some() {
                    add(x, y, vec2(cell, cell), dark);
                }
            }
        }
    }
    for y in 0..sprite.height {
        for x in 0..sprite.width {
            let index = y * sprite.width + x;
            if let Some(c) = sprite.cells[index] {
                add(x, y, Vec2::ZERO, colour(index, c));
            }
        }
    }
    shapes.push(Shape::mesh(mesh));
}

// ---------------------------------------------------------------------------
// Critters

/// A gentle hash of a number into 0..1, for blink times.
fn jitter(n: u32) -> f32 {
    let x = n.wrapping_mul(2_654_435_761) ^ 0x9e37_79b9;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// How shut the eyes are now, 0 open to 1 shut: a blink every few seconds,
/// asleep after a long rest.
fn lids(m: &Motion, seed: u32) -> f32 {
    if m.still > 9.0 {
        return 1.0;
    }
    let period = 3.4 + jitter(seed) * 1.6;
    let t = (m.now + jitter(seed + 7) * period).rem_euclid(period);
    let blink = if t < 0.14 { (PI * t / 0.14).sin() } else { 0.0 };
    // Droopy just before nodding off.
    let drowsy = ((m.still - 6.5) / 2.5).clamp(0.0, 1.0) * 0.6;
    blink.max(drowsy)
}

/// Where the pupils look: behind fast moves (they lag, as googly eyes
/// do), and around when idle.
fn gaze(m: &Motion, seed: u32) -> Vec2 {
    let trail = -m.lean * 0.0016;
    let wander = if m.still > 1.5 {
        let t = m.now * 0.6 + jitter(seed) * 10.0;
        vec2((t * 1.3).sin(), (t * 0.9).cos() * 0.6) * 0.6
    } else {
        Vec2::ZERO
    };
    let look = trail + wander;
    if look.length() > 1.0 {
        look.normalized()
    } else {
        look
    }
}

/// A pair of eyes, centred on `centre`, `size` across each.
fn eyes(c: &mut Canvas, centre: Vec2, size: f32, m: &Motion, seed: u32, googly: bool, bias: Vec2) {
    let lids = lids(m, seed);
    let mut look = gaze(m, seed) + bias;
    if look.length() > 1.0 {
        look = look.normalized();
    }
    let happy = m.held && m.pressed < 0.6;
    let fill = c.pal.fill;
    let outline = c.pal.outline;
    for side in [-1.0, 1.0] {
        let at = centre + vec2(side * size * 0.62, 0.0);
        let r = size * 0.5;
        if happy {
            // ^ ^
            c.lines(&[(
                &[
                    at + vec2(-r * 0.7, r * 0.2),
                    at + vec2(0.0, -r * 0.5),
                    at + vec2(r * 0.7, r * 0.2),
                ],
                size * 0.22,
                outline,
            )]);
            continue;
        }
        if lids > 0.85 {
            c.lines(&[(
                &[at + vec2(-r * 0.7, 0.0), at + vec2(r * 0.7, 0.0)],
                size * 0.2,
                outline,
            )]);
            continue;
        }
        if googly {
            c.dot(at, r, fill);
        } else {
            c.spot(at, r * 0.95, fill);
        }
        let pupil = at + look * r * 0.45;
        c.spot(pupil, r * 0.48, outline);
        c.spot(pupil + vec2(-r * 0.16, -r * 0.16), r * 0.14, fill);
        if lids > 0.05 {
            // The lid comes down over the top of the eye.
            let lid = arc(at, r * 1.02, r * 1.02 * lids * 2.0, PI, TAU, 10);
            c.flat(&lid, c.pal.accent);
        }
    }
}

/// Zs drifting up from a sleeping critter.
fn snooze(c: &mut Canvas, from: Vec2, m: &Motion) {
    if m.still <= 9.0 {
        return;
    }
    let outline = c.pal.outline;
    for i in 0..2 {
        let t = (m.now * 0.55 + i as f32 * 0.5).rem_euclid(1.0);
        let size = 2.0 + 1.6 * t;
        let at = from + vec2(3.0 * t + i as f32 * 2.5, -7.0 * t);
        let colour = outline.gamma_multiply(1.0 - t);
        c.hairline(
            &[
                at,
                at + vec2(size, 0.0),
                at + vec2(0.0, size),
                at + vec2(size, size),
            ],
            0.9,
            colour,
        );
    }
}

pub(super) fn critters(
    shapes: &mut Vec<Shape>,
    kind: CursorKind,
    at: Pos2,
    (scale, deform): (f32, Xf),
    pal: Palette,
    m: &Motion,
) {
    let seed = u32::from(kind_index(kind));
    match kind {
        CursorKind::Arrow | CursorKind::Subselect | CursorKind::Link => {
            canvas_pass(shapes, at, scale, deform, pal, 1.2, |c| {
                blob(c, kind, m, seed)
            });
        }
        CursorKind::Crosshair | CursorKind::Snap => {
            canvas_pass(shapes, at, scale, deform, pal, 1.2, |c| peeper(c, m, seed));
        }
        CursorKind::Grab | CursorKind::Grabbing => {
            canvas_pass(shapes, at, scale, deform, pal, 1.2, |c| {
                paw(c, kind == CursorKind::Grabbing, m)
            });
        }
        _ => {
            // The standard drawing, alive: googly eyes stuck on it.
            canvas_pass(shapes, at, scale, deform, pal, 1.2, |c| {
                draw(c, kind, m);
                let centre = kind.preview_center() + vec2(0.0, -1.0);
                eyes(c, centre, 3.4, m, seed, true, Vec2::ZERO);
                snooze(c, centre + vec2(4.0, -4.0), m);
            });
        }
    }
}

/// The arrow as a soft teardrop. Its sharp tip is the hotspot, so it
/// points as precisely as an arrow; its round end holds the eyes. The round
/// end trails moves like jelly, swells as it breathes, squashes toward the
/// tip on a press, hops over buttons and dozes when left alone.
fn blob(c: &mut Canvas, kind: CursorKind, m: &Motion, seed: u32) {
    let breathe = 1.0 + 0.05 * (m.now * 2.1).sin() * if m.still > 9.0 { 1.5 } else { 1.0 };
    let press = m.press().max(0.0);
    let hop = if kind == CursorKind::Link {
        -(m.now * 7.0).sin().abs() * 1.4
    } else {
        0.0
    };
    // Jelly: the round end lags behind the pointer's moves.
    let mut trail = -m.lean * 0.0035;
    if trail.length() > 3.2 {
        trail = trail.normalized() * 3.2;
    }
    let centre = v(6.4, 7.6) * (1.0 - 0.16 * press) + trail + v(0.0, hop);
    let radius = 5.4 * breathe * (1.0 + 0.1 * press);
    let distance = centre.length().max(radius + 0.5);
    // Round the far side of the end, from one tangent to the tip to the
    // other.
    let toward_tip = (-centre.y).atan2(-centre.x);
    let spread = (radius / distance).clamp(-1.0, 1.0).acos();
    let mut body = vec![Vec2::ZERO];
    body.extend(arc(
        centre,
        radius,
        radius,
        toward_tip + spread,
        toward_tip + TAU - spread,
        26,
    ));
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let colour = if kind == CursorKind::Subselect {
        fill
    } else {
        accent
    };
    c.body(&body, colour);
    // A sheen along the upper rim.
    let sheen = arc(centre, radius * 0.7, radius * 0.7, PI * 1.08, PI * 1.42, 6);
    c.hairline(&sheen, 1.0, mix(colour, Color32::WHITE, 0.55));
    // The eyes watch what it points at, and look about when it rests.
    let size = if kind == CursorKind::Link { 2.7 } else { 2.35 };
    let at_tip = -centre / distance * 0.55;
    eyes(c, centre + v(0.0, -0.4), size, m, seed, false, at_tip);
    snooze(c, centre + v(radius * 0.5, -radius), m);
}

/// A crosshair that is one big eye, its pupil on the hotspot's side of
/// things: it peers about when idle and blinks.
fn peeper(c: &mut Canvas, m: &Motion, seed: u32) {
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let outline = c.pal.outline;
    let r = 5.4;
    for direction in [v(1.0, 0.0), v(-1.0, 0.0), v(0.0, 1.0), v(0.0, -1.0)] {
        c.line(&[direction * (r + 1.0), direction * (r + 3.6)], 1.0, fill);
    }
    let sclera = arc(
        Vec2::ZERO,
        r,
        r * (1.0 - lids(m, seed) * 0.92),
        0.0,
        TAU,
        24,
    );
    c.body(&sclera, fill);
    if lids(m, seed) < 0.85 {
        let look = gaze(m, seed) * 1.8;
        c.spot(look, 2.4, accent);
        c.spot(look, 1.2, outline);
        c.spot(look + v(-0.6, -0.6), 0.45, fill);
    }
    snooze(c, v(5.0, -6.0), m);
}

/// A paw: a pad and four beans; it curls when it holds something.
fn paw(c: &mut Canvas, holding: bool, m: &Motion) {
    let fill = c.pal.fill;
    let accent = c.pal.accent;
    let curl = if holding { 0.55 } else { 0.0 } + 0.1 * m.press().max(0.0);
    let pad = arc(v(0.0, 2.2), 4.6, 3.8 * (1.0 - curl * 0.3), 0.0, TAU, 20);
    c.body(&pad, fill);
    c.spot(v(0.0, 2.6), 2.2, mix(accent, fill, 0.35));
    for (i, x) in [-4.6, -1.6, 1.6, 4.6].into_iter().enumerate() {
        let lift = if i == 0 || i == 3 { 1.6 } else { 0.0 };
        let wiggle = (m.now * 3.0 + i as f32).sin() * 0.3 * if m.still > 1.0 { 1.0 } else { 0.0 };
        let toe = v(x * (1.0 - curl * 0.35), -3.6 + lift + curl * 3.0 + wiggle);
        c.dot(toe, 1.5, fill);
        c.spot(toe + v(0.0, 0.2), 0.8, mix(accent, fill, 0.35));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pal() -> Palette {
        Palette::from_theme(&crate::settings::Theme::default())
    }

    #[test]
    fn every_pointer_has_retro_pixels_with_a_hotspot_inside() {
        for kind in super::super::own::EVERY {
            let sprite = retro_sprite(kind, pal(), &Motion::REST);
            assert!(sprite.width > 3 && sprite.height > 3, "{kind:?}");
            assert_eq!(sprite.cells.len(), sprite.width * sprite.height);
            let (hx, hy) = sprite.hotspot;
            assert!(
                (-4..=sprite.width as i32 + 4).contains(&hx)
                    && (-4..=sprite.height as i32 + 4).contains(&hy),
                "{kind:?} hotspot {hx},{hy} in {}x{}",
                sprite.width,
                sprite.height
            );
        }
    }

    #[test]
    fn retro_pixels_are_solid_and_on_the_ramp() {
        let ramp = Ramp::of(pal());
        let sprite = rasterize(CursorKind::Pen, pal(), &Motion::REST);
        let colours = ramp.colours();
        for cell in sprite.cells.iter().flatten() {
            assert_eq!(cell.a(), 255);
            assert!(colours.contains(cell), "{cell:?}");
        }
    }

    #[test]
    fn critters_blink_and_sleep() {
        let awake = Motion {
            still: 0.0,
            now: 10.0,
            ..Motion::REST
        };
        let asleep = Motion {
            still: 20.0,
            ..awake
        };
        assert!(lids(&awake, 3) < 1.0);
        assert_eq!(lids(&asleep, 3), 1.0);
        // Over a few seconds, every critter blinks at least once.
        let blinked = (0..400).any(|i| {
            let m = Motion {
                now: i as f32 * 0.02,
                ..awake
            };
            lids(&m, 3) > 0.5
        });
        assert!(blinked);
    }

    #[test]
    fn every_pack_draws_every_pointer() {
        for pack in CursorPack::ALL {
            let prefs = CursorSettings {
                pack,
                ..CursorSettings::default()
            };
            for kind in super::super::own::EVERY {
                let mut shapes = Vec::new();
                super::super::paint(
                    &mut shapes,
                    &prefs,
                    kind,
                    pos2(50.0, 50.0),
                    (1.0, Xf::ID),
                    pal(),
                    &Motion::demo(1.3),
                );
                assert!(!shapes.is_empty(), "{pack:?} {kind:?}");
            }
        }
    }
}
