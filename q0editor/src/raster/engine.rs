//! The raster brush engine.
//!
//! A stroke stamps dabs along the pointer's path into a coverage mask of its
//! own, then lays the mask over the drawing as it was when the stroke began.
//! So one stroke never darkens where it crosses itself unless its brush
//! builds up, its opacity is a ceiling, and painting behind, erasing and
//! keeping transparency are just different ways of laying the same mask.
//!
//! Smudge and blur work on the pixels directly instead: they move what is
//! there rather than adding paint.
//!
//! Everything here is in the drawing's own pixels; the caller maps the
//! pointer from the stage.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use q0s_format::pixels::{tile_of, PixelRect, RasterAsset, RasterTile, TILE, TILE_I32};
use serde::{Deserialize, Serialize};

/// The shape a dab is stamped with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Tip {
    /// A round or oval nib.
    #[default]
    Round,
    /// A row of hairs across the stroke: streaky, dry paint.
    Bristles,
    /// A cloud of fine drops.
    Spray,
}

/// A paper the brush drags over: the texture stays put on the drawing, so
/// strokes over it catch the same tooth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Grain {
    #[default]
    None,
    Paper,
    Canvas,
    Chalk,
    Charcoal,
}

impl Grain {
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Paper,
        Self::Canvas,
        Self::Chalk,
        Self::Charcoal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Paper => "Paper",
            Self::Canvas => "Canvas",
            Self::Chalk => "Chalk",
            Self::Charcoal => "Charcoal",
        }
    }
}

/// How a stroke's paint meets what is already there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Blend {
    #[default]
    Normal,
    /// Paint goes under what is there: colouring under line art.
    Behind,
    /// Takes away.
    Erase,
    /// Pulls the colour under the brush along the stroke.
    Smudge,
    /// Softens what is under the brush.
    Blur,
}

/// Everything a brush is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushSettings {
    /// Nib diameter in drawing pixels.
    pub size: f32,
    /// The most one stroke can lay down, 0..=1.
    pub opacity: f32,
    /// What each dab lays down, 0..=1.
    pub flow: f32,
    /// 1 = crisp edge, 0 = soft all the way in.
    pub hardness: f32,
    /// Gap between dabs as a share of the diameter.
    pub spacing: f32,
    /// How much a light touch thins the stroke, 0..=1.
    pub pressure_size: f32,
    /// How much a light touch lightens it, 0..=1.
    pub pressure_opacity: f32,
    /// The size left at no pressure (and at the tip of a taper), 0..=1.
    pub min_size: f32,
    /// Height of the nib over its width, 0.05..=1.
    pub roundness: f32,
    /// Nib angle in degrees.
    pub angle: f32,
    /// The nib turns with the stroke.
    pub follow: bool,
    /// How far dabs stray from the path, in diameters.
    pub scatter: f32,
    pub size_jitter: f32,
    pub flow_jitter: f32,
    /// Random turn of each dab, as a share of a half turn.
    pub angle_jitter: f32,
    pub tip: Tip,
    /// Hairs for bristles, drops for spray: 0..=1.
    pub density: f32,
    pub grain: Grain,
    pub grain_amount: f32,
    /// Size of the grain, 0.25..=4.
    pub grain_scale: f32,
    /// Darker rims, as watercolour dries: 0..=1.
    pub wet_edges: f32,
    /// Keeps spraying while the pointer rests, like an airbrush.
    pub build_up: bool,
    /// Dabs add up inside a stroke; off, a stroke is as dark as its darkest
    /// dab, as a marker is.
    pub accumulate: bool,
    /// Lags the nib behind the pointer for steadier lines, 0..=1.
    pub stabilizer: f32,
    /// Length of the thin start and end, in drawing pixels.
    pub taper: f32,
    pub blend: Blend,
    /// Paints only where there is paint already.
    pub lock_alpha: bool,
    /// For smudge and blur: how strongly they act, 0..=1.
    pub strength: f32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            size: 12.0,
            opacity: 1.0,
            flow: 1.0,
            hardness: 0.85,
            spacing: 0.12,
            pressure_size: 0.6,
            pressure_opacity: 0.0,
            min_size: 0.15,
            roundness: 1.0,
            angle: 0.0,
            follow: false,
            scatter: 0.0,
            size_jitter: 0.0,
            flow_jitter: 0.0,
            angle_jitter: 0.0,
            tip: Tip::Round,
            density: 0.5,
            grain: Grain::None,
            grain_amount: 0.0,
            grain_scale: 1.0,
            wet_edges: 0.0,
            build_up: false,
            accumulate: false,
            stabilizer: 0.0,
            taper: 0.0,
            blend: Blend::Normal,
            lock_alpha: false,
            strength: 0.6,
        }
    }
}

pub const SIZE_RANGE: std::ops::RangeInclusive<f32> = 0.5..=600.0;

impl BrushSettings {
    /// The same brush with every number brought into range.
    pub fn sanitized(mut self) -> Self {
        let unit = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        self.size = if self.size.is_finite() {
            self.size.clamp(*SIZE_RANGE.start(), *SIZE_RANGE.end())
        } else {
            12.0
        };
        self.opacity = unit(self.opacity);
        self.flow = unit(self.flow);
        self.hardness = unit(self.hardness);
        self.spacing = if self.spacing.is_finite() {
            self.spacing.clamp(0.02, 2.0)
        } else {
            0.12
        };
        self.pressure_size = unit(self.pressure_size);
        self.pressure_opacity = unit(self.pressure_opacity);
        self.min_size = unit(self.min_size).max(0.02);
        self.roundness = unit(self.roundness).max(0.05);
        self.angle = if self.angle.is_finite() {
            self.angle
        } else {
            0.0
        };
        self.scatter = if self.scatter.is_finite() {
            self.scatter.clamp(0.0, 3.0)
        } else {
            0.0
        };
        self.size_jitter = unit(self.size_jitter);
        self.flow_jitter = unit(self.flow_jitter);
        self.angle_jitter = unit(self.angle_jitter);
        self.density = unit(self.density);
        self.grain_amount = unit(self.grain_amount);
        self.grain_scale = if self.grain_scale.is_finite() {
            self.grain_scale.clamp(0.25, 4.0)
        } else {
            1.0
        };
        self.wet_edges = unit(self.wet_edges);
        self.stabilizer = unit(self.stabilizer);
        self.taper = if self.taper.is_finite() {
            self.taper.clamp(0.0, 400.0)
        } else {
            0.0
        };
        self.strength = unit(self.strength);
        self
    }
}

/// One pointer position, in the drawing's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub x: f32,
    pub y: f32,
    /// Pen pressure 0..=1, or `None` for a mouse.
    pub pressure: Option<f32>,
    pub time: f64,
}

impl Sample {
    pub fn new(x: f32, y: f32, pressure: Option<f32>, time: f64) -> Self {
        Self {
            x,
            y,
            pressure,
            time,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Dab {
    x: f32,
    y: f32,
    radius: f32,
    alpha: f32,
    angle: f32,
    roundness: f32,
    /// Distance along the stroke, for tapers.
    along: f32,
    /// Seed of its spray drops or hairs' wobble.
    seed: u64,
}

/// One stroke in progress.
pub struct Stroke {
    settings: BrushSettings,
    /// Straight colour, 0..=255.
    color: [f32; 3],
    /// Drawing pixels per screen pixel: the stabilizer's reach is in screen
    /// pixels, so it feels the same at any zoom.
    pixels_per_screen: f32,
    /// The tiles as they were when the stroke began.
    base: HashMap<(i16, i16), Arc<RasterTile>>,
    mask: HashMap<(i16, i16), Vec<f32>>,
    dabs: Vec<Dab>,
    /// Where the nib is (after the stabilizer).
    nib: Option<(f32, f32)>,
    /// Pointer the nib follows.
    pointer: (f32, f32),
    pressure: f32,
    /// Distance left to the next dab.
    gap: f32,
    length: f32,
    heading: f32,
    rng: u64,
    /// Hairs across a bristle nib: offset across (-1..=1), thickness, ink.
    hairs: Vec<(f32, f32, f32)>,
    /// Paint a smudge carries, premultiplied, per pixel of the nib's box.
    carry: Vec<[f32; 4]>,
    carry_side: usize,
    dirty: HashSet<(i16, i16)>,
    touched: Option<PixelRect>,
    last_time: f64,
    changed: bool,
}

/// Pressure as the brush sees it.
fn pressure_of(sample: &Sample) -> f32 {
    sample
        .pressure
        .filter(|p| p.is_finite())
        .map_or(1.0, |p| p.clamp(0.0, 1.0))
}

fn next_random(state: &mut u64) -> f32 {
    // xorshift64*
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    ((x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32) / (1u64 << 24) as f32
}

impl Stroke {
    /// Starts a stroke at `sample` on `drawing`. `color` is straight RGB.
    pub fn begin(
        settings: BrushSettings,
        color: [u8; 3],
        pixels_per_screen: f32,
        drawing: &mut RasterAsset,
        sample: Sample,
    ) -> Self {
        let settings = settings.sanitized();
        let seed = (sample.time.to_bits() ^ 0x9e37_79b9_7f4a_7c15) | 1;
        let mut rng = seed;
        let hairs = if settings.tip == Tip::Bristles {
            let count = 5 + (settings.density * 27.0) as usize;
            (0..count)
                .map(|i| {
                    let across = (i as f32 + 0.5) / count as f32 * 2.0 - 1.0;
                    let wobble = (next_random(&mut rng) - 0.5) * 1.4 / count as f32;
                    let thickness = 0.55 + next_random(&mut rng) * 0.9;
                    let ink = 0.35 + next_random(&mut rng) * 0.65;
                    (across + wobble, thickness, ink)
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut stroke = Self {
            settings,
            color: color.map(f32::from),
            pixels_per_screen: if pixels_per_screen.is_finite() && pixels_per_screen > 0.0 {
                pixels_per_screen
            } else {
                1.0
            },
            base: drawing
                .tiles
                .iter()
                .map(|(key, tile)| (*key, Arc::clone(tile)))
                .collect(),
            mask: HashMap::new(),
            dabs: Vec::new(),
            nib: None,
            pointer: (sample.x, sample.y),
            pressure: pressure_of(&sample),
            gap: 0.0,
            length: 0.0,
            heading: 0.0,
            rng,
            hairs,
            carry: Vec::new(),
            carry_side: 0,
            dirty: HashSet::new(),
            touched: None,
            last_time: sample.time,
            changed: false,
        };
        stroke.nib = Some((sample.x, sample.y));
        stroke.place_dab(drawing, sample.x, sample.y);
        stroke.gap = stroke.step();
        stroke.flush(drawing);
        stroke
    }

    pub fn settings(&self) -> &BrushSettings {
        &self.settings
    }

    /// Whether the stroke has changed the drawing at all.
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// The pixels the stroke has reached so far.
    pub fn touched(&self) -> Option<PixelRect> {
        self.touched
    }

    /// Where the nib is now.
    pub fn nib(&self) -> Option<(f32, f32)> {
        self.nib
    }

    /// The pointer moved to `sample`.
    pub fn add(&mut self, drawing: &mut RasterAsset, sample: Sample) {
        self.pointer = (sample.x, sample.y);
        let pressure = pressure_of(&sample);
        // Pens report pressure in steps; ease between them.
        self.pressure += (pressure - self.pressure) * 0.6;
        self.last_time = sample.time;
        self.follow_pointer(drawing, false);
        self.flush(drawing);
    }

    /// Time passes with the pointer held still: an airbrush keeps spraying,
    /// a stabilized nib keeps closing in.
    pub fn hold(&mut self, drawing: &mut RasterAsset, time: f64) {
        let dt = (time - self.last_time).clamp(0.0, 0.25) as f32;
        self.last_time = time;
        if self.settings.stabilizer > 0.0 {
            self.follow_pointer(drawing, false);
        }
        if self.settings.build_up && dt > 0.0 {
            let count = ((dt * 40.0).round() as usize).clamp(1, 10);
            if let Some((x, y)) = self.nib {
                for _ in 0..count {
                    self.place_dab(drawing, x, y);
                }
            }
        }
        self.flush(drawing);
    }

    /// The pointer came up: the nib catches up with it, tapers end, and
    /// empty tiles are dropped.
    pub fn finish(mut self, drawing: &mut RasterAsset) -> bool {
        self.follow_pointer(drawing, true);
        self.taper_end(drawing);
        self.flush(drawing);
        let touched: Vec<(i16, i16)> = self.mask.keys().copied().collect();
        drawing.prune_tiles(touched);
        self.changed
    }

    /// Moves the nib toward the pointer, laying dabs along the way.
    fn follow_pointer(&mut self, drawing: &mut RasterAsset, catch_up: bool) {
        let Some((nx, ny)) = self.nib else {
            return;
        };
        let (px, py) = self.pointer;
        let (tx, ty) = if catch_up || self.settings.stabilizer <= 0.0 {
            (px, py)
        } else {
            // A rope: the nib only moves once the pointer pulls it taut.
            let rope = self.settings.stabilizer * 48.0 * self.pixels_per_screen;
            let dx = px - nx;
            let dy = py - ny;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance <= rope {
                return;
            }
            let pull = (distance - rope) / distance;
            (nx + dx * pull, ny + dy * pull)
        };
        self.line_to(drawing, tx, ty);
    }

    fn line_to(&mut self, drawing: &mut RasterAsset, x: f32, y: f32) {
        let Some((sx, sy)) = self.nib else {
            return;
        };
        let dx = x - sx;
        let dy = y - sy;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance < 1e-4 {
            return;
        }
        if distance > 0.5 {
            let heading = dy.atan2(dx);
            // Turn the nib smoothly, not dab by dab.
            let mut turn = heading - self.heading;
            while turn > std::f32::consts::PI {
                turn -= std::f32::consts::TAU;
            }
            while turn < -std::f32::consts::PI {
                turn += std::f32::consts::TAU;
            }
            self.heading += turn * if self.dabs.len() <= 1 { 1.0 } else { 0.5 };
        }
        // `gap` is how far along the next dab still is.
        let start = self.length;
        let mut travelled = 0.0;
        while travelled + self.gap <= distance {
            travelled += self.gap;
            self.length = start + travelled;
            let t = travelled / distance;
            self.place_dab(drawing, sx + dx * t, sy + dy * t);
            self.gap = self.step();
        }
        self.gap -= distance - travelled;
        self.length = start + distance;
        self.nib = Some((x, y));
    }

    /// Distance between dabs at the current size.
    fn step(&self) -> f32 {
        let diameter = self.current_radius() * 2.0;
        let spacing = match self.settings.blend {
            Blend::Smudge | Blend::Blur => self.settings.spacing.min(0.15),
            _ => self.settings.spacing,
        };
        (diameter * spacing).max(0.35)
    }

    fn current_radius(&self) -> f32 {
        let s = &self.settings;
        let thin = 1.0 - s.pressure_size * (1.0 - self.pressure);
        let mut radius = s.size * 0.5 * thin.max(s.min_size);
        if s.taper > 0.0 {
            let ramp = (self.length / s.taper).clamp(0.0, 1.0);
            radius *= s.min_size + (1.0 - s.min_size) * smoothstep(ramp);
        }
        radius.max(0.25)
    }

    fn place_dab(&mut self, drawing: &mut RasterAsset, x: f32, y: f32) {
        let s = self.settings;
        let mut radius = self.current_radius();
        if s.size_jitter > 0.0 {
            radius *= 1.0 - s.size_jitter * next_random(&mut self.rng) * 0.8;
        }
        let (mut x, mut y) = (x, y);
        if s.scatter > 0.0 {
            let angle = next_random(&mut self.rng) * std::f32::consts::TAU;
            let reach = next_random(&mut self.rng).sqrt() * s.scatter * radius * 2.0;
            x += angle.cos() * reach;
            y += angle.sin() * reach;
        }
        let mut alpha = s.flow * (1.0 - s.pressure_opacity * (1.0 - self.pressure));
        if s.flow_jitter > 0.0 {
            alpha *= 1.0 - s.flow_jitter * next_random(&mut self.rng);
        }
        let mut angle = s.angle.to_radians();
        if s.follow {
            angle += self.heading;
        }
        if s.angle_jitter > 0.0 {
            angle += (next_random(&mut self.rng) - 0.5) * s.angle_jitter * std::f32::consts::PI;
        }
        let dab = Dab {
            x,
            y,
            radius,
            alpha: alpha.clamp(0.0, 1.0),
            angle,
            roundness: s.roundness,
            along: self.length,
            seed: (self.rng ^ self.dabs.len() as u64).wrapping_mul(0x9e37_79b9) | 1,
        };
        match s.blend {
            Blend::Smudge => self.smudge_dab(drawing, dab),
            Blend::Blur => self.blur_dab(drawing, dab),
            _ => {
                self.stamp(dab, None);
                self.dabs.push(dab);
            }
        }
    }

    /// Lays `dab` into the mask; with `only`, just into those tiles.
    fn stamp(&mut self, dab: Dab, only: Option<&HashSet<(i16, i16)>>) {
        let s = self.settings;
        match s.tip {
            Tip::Round => self.stamp_nib(dab.x, dab.y, dab, dab.radius, dab.alpha, only),
            Tip::Bristles => {
                let (sin, cos) = (dab.angle + std::f32::consts::FRAC_PI_2).sin_cos();
                let hairs = std::mem::take(&mut self.hairs);
                let hair = (dab.radius / hairs.len().max(1) as f32 * 1.6).max(0.5);
                for &(across, thickness, ink) in &hairs {
                    let reach = across * dab.radius;
                    self.stamp_nib(
                        dab.x + cos * reach,
                        dab.y + sin * reach,
                        Dab {
                            roundness: 1.0,
                            ..dab
                        },
                        hair * thickness,
                        dab.alpha * ink,
                        only,
                    );
                }
                self.hairs = hairs;
            }
            Tip::Spray => {
                let mut rng = dab.seed;
                let drops = 4 + (dab.radius * dab.radius * 0.05 * (0.2 + s.density * 1.8)) as usize;
                let drop = (s.size * 0.012).clamp(0.45, 3.0);
                for _ in 0..drops.min(600) {
                    let angle = next_random(&mut rng) * std::f32::consts::TAU;
                    // Thicker toward the middle, as a can sprays.
                    let reach = next_random(&mut rng).powf(0.8) * dab.radius;
                    let size = drop * (0.5 + next_random(&mut rng));
                    self.stamp_nib(
                        dab.x + angle.cos() * reach,
                        dab.y + angle.sin() * reach * dab.roundness,
                        Dab {
                            roundness: 1.0,
                            ..dab
                        },
                        size,
                        dab.alpha * (0.5 + next_random(&mut rng) * 0.5),
                        only,
                    );
                }
            }
        }
    }

    /// One nib-shaped mark at (x, y).
    fn stamp_nib(
        &mut self,
        x: f32,
        y: f32,
        dab: Dab,
        radius: f32,
        alpha: f32,
        only: Option<&HashSet<(i16, i16)>>,
    ) {
        if alpha <= 0.0 || radius <= 0.0 {
            return;
        }
        let s = self.settings;
        // Tiny nibs fade instead of shrinking below a pixel.
        let (radius, alpha) = if radius < 0.6 {
            (0.6, alpha * (radius / 0.6) * (radius / 0.6))
        } else {
            (radius, alpha)
        };
        let reach = radius + 1.0;
        let rect = PixelRect::new(
            (x - reach).floor() as i32,
            (y - reach).floor() as i32,
            (x + reach).ceil() as i32 + 1,
            (y + reach).ceil() as i32 + 1,
        );
        let (sin, cos) = dab.angle.sin_cos();
        let inv_round = 1.0 / dab.roundness.max(0.05);
        let soft = (1.0 - s.hardness).max(1.2 / radius).min(1.0);
        let grain = grain_texture(s.grain);
        let (tx0, ty0, tx1, ty1) = rect.tiles();
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let (Ok(kx), Ok(ky)) = (i16::try_from(tx), i16::try_from(ty)) else {
                    continue;
                };
                if only.is_some_and(|only| !only.contains(&(kx, ky))) {
                    continue;
                }
                let ox = tx * TILE_I32;
                let oy = ty * TILE_I32;
                let part = rect.intersect(PixelRect::new(ox, oy, ox + TILE_I32, oy + TILE_I32));
                if part.is_empty() {
                    continue;
                }
                let mut wrote = false;
                let mask = self
                    .mask
                    .entry((kx, ky))
                    .or_insert_with(|| vec![0.0; TILE * TILE]);
                for py in part.y0..part.y1 {
                    let dy = py as f32 + 0.5 - y;
                    for px in part.x0..part.x1 {
                        let dx = px as f32 + 0.5 - x;
                        let u = dx * cos + dy * sin;
                        let v = (-dx * sin + dy * cos) * inv_round;
                        let d = (u * u + v * v).sqrt() / radius;
                        if d >= 1.0 {
                            continue;
                        }
                        let mut f = 1.0 - smooth_range(1.0 - soft, 1.0, d);
                        if s.wet_edges > 0.0 {
                            f *= 1.0 - s.wet_edges * 0.7 * (1.0 - d * d);
                        }
                        if let Some(grain) = grain {
                            let g = grain.sample(px, py, s.grain_scale);
                            // A firm press fills the tooth in.
                            let tooth = g + (1.0 - g) * self.pressure * self.pressure * 0.35;
                            f *= 1.0 - s.grain_amount * (1.0 - tooth);
                        }
                        let a = (f * alpha).clamp(0.0, 1.0);
                        if a <= 0.0 {
                            continue;
                        }
                        let i = ((py - oy) as usize) * TILE + (px - ox) as usize;
                        let m = &mut mask[i];
                        *m = if s.accumulate || s.build_up {
                            *m + (1.0 - *m) * a
                        } else {
                            m.max(a)
                        };
                        wrote = true;
                    }
                }
                if wrote {
                    self.dirty.insert((kx, ky));
                    let tile = PixelRect::new(part.x0, part.y0, part.x1, part.y1);
                    self.touched = Some(self.touched.map_or(tile, |t| t.union(tile)));
                }
            }
        }
    }

    /// Shrinks the last stretch of the stroke into a point.
    fn taper_end(&mut self, drawing: &mut RasterAsset) {
        let s = self.settings;
        if s.taper <= 0.0 || self.dabs.len() < 3 || matches!(s.blend, Blend::Smudge | Blend::Blur) {
            return;
        }
        let total = self.length;
        let reach = s.taper.min(total * 0.5);
        if reach <= 0.5 {
            return;
        }
        let start = total - reach;
        let mut tiles = HashSet::new();
        for dab in self.dabs.iter().filter(|dab| dab.along > start) {
            let r = dab.radius * 1.5 + 2.0;
            let rect = PixelRect::new(
                (dab.x - r).floor() as i32,
                (dab.y - r).floor() as i32,
                (dab.x + r).ceil() as i32 + 1,
                (dab.y + r).ceil() as i32 + 1,
            );
            let (tx0, ty0, tx1, ty1) = rect.tiles();
            for ty in ty0..=ty1 {
                for tx in tx0..=tx1 {
                    if let (Ok(x), Ok(y)) = (i16::try_from(tx), i16::try_from(ty)) {
                        tiles.insert((x, y));
                    }
                }
            }
        }
        for key in &tiles {
            if let Some(mask) = self.mask.get_mut(key) {
                mask.fill(0.0);
            }
        }
        let dabs = std::mem::take(&mut self.dabs);
        for dab in &dabs {
            let mut dab = *dab;
            if dab.along > start {
                let left = ((total - dab.along) / reach).clamp(0.0, 1.0);
                dab.radius *= s.min_size + (1.0 - s.min_size) * smoothstep(left);
            }
            self.stamp(dab, Some(&tiles));
        }
        self.dabs = dabs;
        self.dirty
            .extend(tiles.into_iter().filter(|key| self.mask.contains_key(key)));
        let _ = drawing;
    }

    /// Lays the mask of every changed tile over the stroke's base.
    fn flush(&mut self, drawing: &mut RasterAsset) {
        if self.dirty.is_empty() {
            return;
        }
        let s = self.settings;
        let opacity = s.opacity;
        let [r, g, b] = self.color;
        for key in std::mem::take(&mut self.dirty) {
            let Some(mask) = self.mask.get(&key) else {
                continue;
            };
            let base = self.base.get(&key);
            if base.is_none() && (s.blend == Blend::Erase || s.lock_alpha) {
                // Nothing to take away or recolour here.
                continue;
            }
            let Some(tile) = drawing.tile_mut(i32::from(key.0), i32::from(key.1)) else {
                continue;
            };
            let out = tile.pixels_mut();
            match base {
                Some(base) => out.copy_from_slice(base.pixels()),
                None => out.fill(0),
            }
            for (i, &m) in mask.iter().enumerate() {
                if m <= 0.0 {
                    continue;
                }
                let sa = m * opacity;
                let p = &mut out[i * 4..i * 4 + 4];
                let ba = f32::from(p[3]) / 255.0;
                let (mut pr, mut pg, mut pb, mut pa) = (
                    f32::from(p[0]),
                    f32::from(p[1]),
                    f32::from(p[2]),
                    f32::from(p[3]),
                );
                if s.lock_alpha {
                    let target = [r * ba, g * ba, b * ba];
                    pr += (target[0] - pr) * sa;
                    pg += (target[1] - pg) * sa;
                    pb += (target[2] - pb) * sa;
                } else {
                    match s.blend {
                        Blend::Normal | Blend::Smudge | Blend::Blur => {
                            let keep = 1.0 - sa;
                            pr = r * sa + pr * keep;
                            pg = g * sa + pg * keep;
                            pb = b * sa + pb * keep;
                            pa = 255.0 * sa + pa * keep;
                        }
                        Blend::Behind => {
                            let add = sa * (1.0 - ba);
                            pr += r * add;
                            pg += g * add;
                            pb += b * add;
                            pa += 255.0 * add;
                        }
                        Blend::Erase => {
                            let keep = 1.0 - sa;
                            pr *= keep;
                            pg *= keep;
                            pb *= keep;
                            pa *= keep;
                        }
                    }
                }
                let a = pa.round().clamp(0.0, 255.0);
                p[3] = a as u8;
                p[0] = pr.round().clamp(0.0, a) as u8;
                p[1] = pg.round().clamp(0.0, a) as u8;
                p[2] = pb.round().clamp(0.0, a) as u8;
            }
            self.changed = true;
        }
    }

    /// The nib's box: corner and side, in drawing pixels.
    fn nib_box(dab: &Dab) -> (i32, i32, usize) {
        let r = dab.radius.max(0.5).ceil() as i32;
        let x0 = dab.x.floor() as i32 - r;
        let y0 = dab.y.floor() as i32 - r;
        (x0, y0, (r * 2 + 1) as usize)
    }

    fn coverage(&self, dab: &Dab, px: i32, py: i32) -> f32 {
        let dx = px as f32 + 0.5 - dab.x;
        let dy = py as f32 + 0.5 - dab.y;
        let d = (dx * dx + dy * dy).sqrt() / dab.radius.max(0.5);
        if d >= 1.0 {
            return 0.0;
        }
        let soft = (1.0 - self.settings.hardness).max(1.2 / dab.radius.max(0.5));
        1.0 - smooth_range(1.0 - soft.min(1.0), 1.0, d)
    }

    fn smudge_dab(&mut self, drawing: &mut RasterAsset, dab: Dab) {
        let (x0, y0, side) = Self::nib_box(&dab);
        let strength = self.settings.strength * dab.alpha.max(0.05);
        if self.carry_side != side || self.carry.is_empty() {
            // Pick up what is under the first dab.
            self.carry = (0..side * side)
                .map(|i| {
                    let p = drawing.pixel(x0 + (i % side) as i32, y0 + (i / side) as i32);
                    p.map(f32::from)
                })
                .collect();
            self.carry_side = side;
            return;
        }
        for j in 0..side {
            for i in 0..side {
                let (px, py) = (x0 + i as i32, y0 + j as i32);
                let f = self.coverage(&dab, px, py) * strength;
                if f <= 0.0 {
                    continue;
                }
                let k = j * side + i;
                let here = drawing.pixel(px, py).map(f32::from);
                let carried = self.carry[k];
                let mixed: [f32; 4] = std::array::from_fn(|c| here[c] + (carried[c] - here[c]) * f);
                // The brush soaks up some of what it passes over.
                self.carry[k] = std::array::from_fn(|c| {
                    carried[c] + (here[c] - carried[c]) * (1.0 - strength) * 0.5
                });
                self.write_pixel(drawing, px, py, mixed);
            }
        }
    }

    fn blur_dab(&mut self, drawing: &mut RasterAsset, dab: Dab) {
        let (x0, y0, side) = Self::nib_box(&dab);
        let strength = self.settings.strength * dab.alpha.max(0.05);
        // Read the box with a margin first, so the blur reads the old pixels.
        let margin = 2;
        let wide = side + margin * 2;
        let read: Vec<[f32; 4]> = (0..wide * wide)
            .map(|i| {
                let px = x0 - margin as i32 + (i % wide) as i32;
                let py = y0 - margin as i32 + (i / wide) as i32;
                drawing.pixel(px, py).map(f32::from)
            })
            .collect();
        const WEIGHTS: [f32; 5] = [1.0, 4.0, 6.0, 4.0, 1.0];
        for j in 0..side {
            for i in 0..side {
                let (px, py) = (x0 + i as i32, y0 + j as i32);
                let f = self.coverage(&dab, px, py) * strength;
                if f <= 0.0 {
                    continue;
                }
                let mut sum = [0.0_f32; 4];
                for (dy, wy) in WEIGHTS.iter().enumerate() {
                    for (dx, wx) in WEIGHTS.iter().enumerate() {
                        let w = wx * wy;
                        let p = read[(j + dy) * wide + i + dx];
                        for c in 0..4 {
                            sum[c] += p[c] * w;
                        }
                    }
                }
                let here = read[(j + margin) * wide + i + margin];
                let mixed: [f32; 4] =
                    std::array::from_fn(|c| here[c] + (sum[c] / 256.0 - here[c]) * f);
                self.write_pixel(drawing, px, py, mixed);
            }
        }
    }

    fn write_pixel(&mut self, drawing: &mut RasterAsset, x: i32, y: i32, value: [f32; 4]) {
        let (tx, px) = tile_of(x);
        let (ty, py) = tile_of(y);
        let (Ok(kx), Ok(ky)) = (i16::try_from(tx), i16::try_from(ty)) else {
            return;
        };
        let exists = drawing.tiles.contains_key(&(kx, ky));
        if !exists && value[3] < 0.5 {
            return;
        }
        let Some(tile) = drawing.tile_mut(tx, ty) else {
            return;
        };
        let i = (py * TILE + px) * 4;
        let a = value[3].round().clamp(0.0, 255.0);
        let new = [
            value[0].round().clamp(0.0, a) as u8,
            value[1].round().clamp(0.0, a) as u8,
            value[2].round().clamp(0.0, a) as u8,
            a as u8,
        ];
        if tile.pixels()[i..i + 4] == new {
            return;
        }
        tile.pixels_mut()[i..i + 4].copy_from_slice(&new);
        self.mask.entry((kx, ky)).or_default();
        self.changed = true;
        let rect = PixelRect::new(x, y, x + 1, y + 1);
        self.touched = Some(self.touched.map_or(rect, |t| t.union(rect)));
    }
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn smooth_range(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    smoothstep((x - edge0) / (edge1 - edge0))
}

// ---------------------------------------------------------------------------
// Grain

const GRAIN_SIDE: usize = 256;

/// A tileable texture the brush's paint catches on.
pub struct GrainTexture {
    values: Vec<f32>,
}

impl GrainTexture {
    fn sample(&self, x: i32, y: i32, scale: f32) -> f32 {
        let fx = x as f32 / scale;
        let fy = y as f32 / scale;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let (tx, ty) = (fx - x0, fy - y0);
        let at = |x: f32, y: f32| {
            let xi = (x as i64).rem_euclid(GRAIN_SIDE as i64) as usize;
            let yi = (y as i64).rem_euclid(GRAIN_SIDE as i64) as usize;
            self.values[yi * GRAIN_SIDE + xi]
        };
        let top = at(x0, y0) + (at(x0 + 1.0, y0) - at(x0, y0)) * tx;
        let bottom = at(x0, y0 + 1.0) + (at(x0 + 1.0, y0 + 1.0) - at(x0, y0 + 1.0)) * tx;
        top + (bottom - top) * ty
    }
}

/// Smooth tileable value noise at `period` cells across the texture.
fn value_noise(seed: u64, period: usize, stretch: (f32, f32)) -> Vec<f32> {
    let mut rng = seed | 1;
    let cells: Vec<f32> = (0..period * period)
        .map(|_| next_random(&mut rng))
        .collect();
    let mut out = vec![0.0; GRAIN_SIDE * GRAIN_SIDE];
    for y in 0..GRAIN_SIDE {
        for x in 0..GRAIN_SIDE {
            let fx = x as f32 / GRAIN_SIDE as f32 * period as f32 * stretch.0;
            let fy = y as f32 / GRAIN_SIDE as f32 * period as f32 * stretch.1;
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (tx, ty) = (smoothstep(fx.fract()), smoothstep(fy.fract()));
            let at = |cx: usize, cy: usize| cells[(cy % period) * period + (cx % period)];
            let top = at(x0, y0) + (at(x0 + 1, y0) - at(x0, y0)) * tx;
            let bottom = at(x0, y0 + 1) + (at(x0 + 1, y0 + 1) - at(x0, y0 + 1)) * tx;
            out[y * GRAIN_SIDE + x] = top + (bottom - top) * ty;
        }
    }
    out
}

fn build_grain(grain: Grain) -> GrainTexture {
    let octaves: &[(u64, usize, (f32, f32), f32)] = match grain {
        Grain::None => &[],
        Grain::Paper => &[
            (11, 128, (1.0, 1.0), 0.55),
            (12, 64, (1.0, 1.0), 0.3),
            (13, 32, (1.0, 1.0), 0.15),
        ],
        Grain::Canvas => &[(21, 128, (1.0, 1.0), 0.35), (22, 16, (1.0, 1.0), 0.15)],
        Grain::Chalk => &[(31, 256, (1.0, 1.0), 0.6), (32, 64, (1.0, 1.0), 0.4)],
        Grain::Charcoal => &[
            (41, 256, (1.0, 0.125), 0.55),
            (42, 128, (1.0, 0.25), 0.3),
            (43, 256, (1.0, 1.0), 0.15),
        ],
    };
    let mut values = vec![0.0; GRAIN_SIDE * GRAIN_SIDE];
    for &(seed, period, stretch, weight) in octaves {
        for (out, value) in values.iter_mut().zip(value_noise(seed, period, stretch)) {
            *out += value * weight;
        }
    }
    if grain == Grain::Canvas {
        // A weave: threads across and down.
        for y in 0..GRAIN_SIDE {
            for x in 0..GRAIN_SIDE {
                let wx = ((x as f32 / 4.0) * std::f32::consts::PI).sin().abs();
                let wy = ((y as f32 / 4.0) * std::f32::consts::PI).sin().abs();
                let thread = if ((x / 4) + (y / 4)) % 2 == 0 { wx } else { wy };
                values[y * GRAIN_SIDE + x] += thread * 0.5;
            }
        }
    }
    // Spread to 0..=1 with some contrast: tooth is mostly peaks and pits.
    let (min, max) = values
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| {
            (a.min(v), b.max(v))
        });
    let span = (max - min).max(1e-6);
    let contrast = match grain {
        Grain::Chalk | Grain::Charcoal => 2.2,
        _ => 1.5,
    };
    for value in &mut values {
        let t = (*value - min) / span;
        *value = ((t - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
    }
    GrainTexture { values }
}

fn grain_texture(grain: Grain) -> Option<&'static GrainTexture> {
    static TEXTURES: [OnceLock<GrainTexture>; 5] = [
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
    ];
    let index = Grain::ALL.iter().position(|g| *g == grain)?;
    if grain == Grain::None {
        return None;
    }
    Some(TEXTURES[index].get_or_init(|| build_grain(grain)))
}

// ---------------------------------------------------------------------------
// One-off marks

/// Paints a whole stroke through `points` (drawing pixels, pressure) in one
/// go: brush previews and tests.
pub fn paint_path(
    drawing: &mut RasterAsset,
    settings: BrushSettings,
    color: [u8; 3],
    points: &[(f32, f32, Option<f32>)],
) -> bool {
    let Some(&(x, y, pressure)) = points.first() else {
        return false;
    };
    let mut stroke = Stroke::begin(
        settings,
        color,
        1.0,
        drawing,
        Sample::new(x, y, pressure, 0.0),
    );
    for (index, &(x, y, pressure)) in points.iter().enumerate().skip(1) {
        stroke.add(drawing, Sample::new(x, y, pressure, index as f64 / 120.0));
    }
    stroke.finish(drawing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(drawing: &mut RasterAsset, settings: BrushSettings, from: (f32, f32), to: (f32, f32)) {
        let points: Vec<_> = (0..=40)
            .map(|i| {
                let t = i as f32 / 40.0;
                (
                    from.0 + (to.0 - from.0) * t,
                    from.1 + (to.1 - from.1) * t,
                    None,
                )
            })
            .collect();
        paint_path(drawing, settings, [200, 40, 10], &points);
    }

    fn hard(size: f32) -> BrushSettings {
        BrushSettings {
            size,
            hardness: 1.0,
            pressure_size: 0.0,
            ..BrushSettings::default()
        }
    }

    #[test]
    fn a_hard_brush_paints_solid_colour_along_its_path_and_nothing_beside_it() {
        let mut drawing = RasterAsset::new(1);
        line(&mut drawing, hard(10.0), (10.0, 20.0), (150.0, 20.0));
        assert_eq!(drawing.pixel(80, 20), [200, 40, 10, 255]);
        assert_eq!(drawing.pixel(80, 32), [0, 0, 0, 0]);
        let bounds = drawing.exact_bounds().unwrap();
        assert!(bounds.x0 >= 3 && bounds.x1 <= 157, "{bounds:?}");
        assert!(bounds.y0 >= 13 && bounds.y1 <= 27, "{bounds:?}");
        // Only tiles that hold paint exist.
        assert!(drawing.tiles.keys().all(|(_, y)| *y == 0));
    }

    #[test]
    fn opacity_caps_a_stroke_that_crosses_itself() {
        let mut drawing = RasterAsset::new(1);
        let settings = BrushSettings {
            opacity: 0.5,
            ..hard(12.0)
        };
        let points = [
            (10.0, 10.0, None),
            (60.0, 10.0, None),
            (60.0, 40.0, None),
            (35.0, 0.0, None),
        ];
        paint_path(&mut drawing, settings, [0, 0, 0], &points);
        let crossing = drawing.pixel(36, 10);
        assert!((126..=129).contains(&crossing[3]), "{crossing:?}");
    }

    #[test]
    fn the_eraser_takes_paint_away_and_behind_paints_only_where_it_is_clear() {
        let mut drawing = RasterAsset::new(1);
        line(&mut drawing, hard(10.0), (10.0, 20.0), (150.0, 20.0));
        let eraser = BrushSettings {
            blend: Blend::Erase,
            ..hard(20.0)
        };
        line(&mut drawing, eraser, (80.0, 0.0), (80.0, 40.0));
        assert_eq!(drawing.pixel(80, 20)[3], 0);
        assert_eq!(drawing.pixel(30, 20)[3], 255);
        let behind = BrushSettings {
            blend: Blend::Behind,
            ..hard(30.0)
        };
        let points = [(30.0, 20.0, None), (31.0, 20.0, None)];
        paint_path(&mut drawing, behind, [0, 0, 255], &points);
        // The line stays on top; the clear pixels round it get blue.
        assert_eq!(drawing.pixel(30, 20), [200, 40, 10, 255]);
        assert_eq!(drawing.pixel(30, 30), [0, 0, 255, 255]);
    }

    #[test]
    fn an_eraser_on_an_empty_drawing_makes_no_tiles() {
        let mut drawing = RasterAsset::new(1);
        let eraser = BrushSettings {
            blend: Blend::Erase,
            ..hard(20.0)
        };
        line(&mut drawing, eraser, (0.0, 0.0), (200.0, 200.0));
        assert!(drawing.is_empty());
    }

    #[test]
    fn pressure_thins_the_stroke() {
        let settings = BrushSettings {
            pressure_size: 1.0,
            min_size: 0.1,
            ..hard(20.0)
        };
        let mut light = RasterAsset::new(1);
        let mut firm = RasterAsset::new(2);
        let path = |p: f32| -> Vec<(f32, f32, Option<f32>)> {
            (0..30)
                .map(|i| (10.0 + i as f32 * 4.0, 50.0, Some(p)))
                .collect()
        };
        paint_path(&mut light, settings, [0, 0, 0], &path(0.2));
        paint_path(&mut firm, settings, [0, 0, 0], &path(1.0));
        let height = |d: &RasterAsset| d.exact_bounds().unwrap().height();
        assert!(
            height(&light) * 2 < height(&firm),
            "{} {}",
            height(&light),
            height(&firm)
        );
    }

    #[test]
    fn a_stroke_shares_the_tiles_it_does_not_touch() {
        let mut drawing = RasterAsset::new(1);
        line(&mut drawing, hard(8.0), (10.0, 10.0), (20.0, 10.0));
        line(&mut drawing, hard(8.0), (300.0, 300.0), (310.0, 300.0));
        let before = drawing.clone();
        line(&mut drawing, hard(8.0), (10.0, 30.0), (30.0, 30.0));
        let far = (4, 4);
        assert!(Arc::ptr_eq(&before.tiles[&far], &drawing.tiles[&far]));
        assert!(!Arc::ptr_eq(
            &before.tiles[&(0, 0)],
            &drawing.tiles[&(0, 0)]
        ));
    }

    #[test]
    fn smudge_drags_colour_into_clear_pixels() {
        let mut drawing = RasterAsset::new(1);
        line(&mut drawing, hard(16.0), (20.0, 10.0), (20.0, 60.0));
        let smudge = BrushSettings {
            blend: Blend::Smudge,
            strength: 0.8,
            ..hard(12.0)
        };
        line(&mut drawing, smudge, (20.0, 35.0), (60.0, 35.0));
        assert!(drawing.pixel(36, 35)[3] > 20, "{:?}", drawing.pixel(36, 35));
    }

    #[test]
    fn a_taper_thins_both_ends() {
        let settings = BrushSettings {
            taper: 40.0,
            min_size: 0.1,
            ..hard(16.0)
        };
        let mut drawing = RasterAsset::new(1);
        line(&mut drawing, settings, (10.0, 50.0), (210.0, 50.0));
        let column = |x: i32| (0..100).filter(|&y| drawing.pixel(x, y)[3] > 128).count();
        assert!(column(110) >= 14, "{}", column(110));
        assert!(column(14) < 8, "{}", column(14));
        assert!(column(206) < 8, "{}", column(206));
    }

    #[test]
    fn every_grain_and_tip_paints_something() {
        for grain in Grain::ALL {
            for tip in [Tip::Round, Tip::Bristles, Tip::Spray] {
                let settings = BrushSettings {
                    grain,
                    grain_amount: 0.8,
                    tip,
                    ..BrushSettings::default()
                };
                let mut drawing = RasterAsset::new(1);
                line(&mut drawing, settings, (10.0, 10.0), (90.0, 40.0));
                assert!(!drawing.is_empty(), "{grain:?} {tip:?}");
            }
        }
    }
}
