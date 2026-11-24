//! The raster fill bucket.
//!
//! It floods the area under the click on a reference image: the current
//! drawing, or everything visible on the frame (so colour can go on its own
//! layer under the line art). Lines with small gaps still hold the paint:
//! the lines are thickened by the gap size for the flood, then the fill
//! creeps back out to them. Last it grows a little under the lines' soft
//! edges, so no pale halo is left between line and colour.

use std::collections::VecDeque;

use egui::{Context, Response};
use q0s_format::pixels::{PixelRect, RasterAsset};
use q0s_format::transform::Affine;
use q0s_format::v2::Rgba;
use serde::{Deserialize, Serialize};

use crate::app::EditorApp;
use crate::render::StageView;

/// The largest area one fill looks at, in pixels per side.
const MAX_SIDE: i32 = 6144;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FillSettings {
    /// How different a colour may be and still be filled, 0..=1.
    pub tolerance: f32,
    /// Gaps in the lines up to this many pixels across still hold the fill.
    pub gap: u8,
    /// How far the fill reaches under the lines' edges, in pixels.
    pub grow: u8,
    /// Looks at every visible layer, not only the drawing being filled.
    pub all_layers: bool,
}

impl Default for FillSettings {
    fn default() -> Self {
        Self {
            tolerance: 0.12,
            gap: 4,
            grow: 2,
            all_layers: true,
        }
    }
}

pub const MAX_GAP: u8 = 24;
pub const MAX_GROW: u8 = 8;

impl FillSettings {
    pub fn sanitized(mut self) -> Self {
        self.tolerance = if self.tolerance.is_finite() {
            self.tolerance.clamp(0.0, 1.0)
        } else {
            0.12
        };
        self.gap = self.gap.min(MAX_GAP);
        self.grow = self.grow.min(MAX_GROW);
        self
    }
}

/// A straight-alpha image of the area a fill looks at.
pub struct Reference {
    pub rect: PixelRect,
    pub rgba: Vec<u8>,
}

impl Reference {
    fn at(&self, x: i32, y: i32) -> [u8; 4] {
        let i = (((y - self.rect.y0) * self.rect.width() + (x - self.rect.x0)) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// Which pixels of `reference` a click at `(x, y)` fills. Empty when the
/// click is outside it.
pub fn region(reference: &Reference, x: i32, y: i32, settings: FillSettings) -> Vec<bool> {
    let rect = reference.rect;
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    if x < rect.x0 || y < rect.y0 || x >= rect.x1 || y >= rect.y1 || w == 0 || h == 0 {
        return Vec::new();
    }
    let settings = settings.sanitized();
    let seed = reference.at(x, y);
    let limit = (settings.tolerance * 255.0).round() as i32;
    // Fully clear pixels match each other whatever colour they hide.
    let similar: Vec<bool> = reference
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            if seed[3] == 0 && p[3] == 0 {
                return true;
            }
            (0..4).all(|c| (i32::from(p[c]) - i32::from(seed[c])).abs() <= limit)
        })
        .collect();
    let (sx, sy) = ((x - rect.x0) as usize, (y - rect.y0) as usize);
    let index = |x: usize, y: usize| y * w + x;
    let mut open = similar.clone();
    let gap = usize::from(settings.gap);
    if gap > 0 {
        // Lines thickened by half the gap on each side meet across it.
        let distance = distance_to_walls(&similar, w, h);
        let reach = (gap as u32 * 3).div_ceil(2);
        let closed: Vec<bool> = distance.iter().map(|d| *d > reach).collect();
        if closed[index(sx, sy)] {
            open = closed;
        }
    }
    let mut filled = flood(&open, w, h, sx, sy);
    if gap > 0 {
        // Back out to the real lines, through what the thickening hid.
        grow(&mut filled, w, h, gap + 1, Some(&similar));
    }
    if settings.grow > 0 {
        grow(&mut filled, w, h, usize::from(settings.grow), None);
    }
    filled
}

/// Distance to the nearest pixel that is not `open`, in thirds of a pixel
/// (a 3-4 chamfer: close enough to round for thickening lines).
fn distance_to_walls(open: &[bool], w: usize, h: usize) -> Vec<u32> {
    const FAR: u32 = u32::MAX / 4;
    let mut d: Vec<u32> = open.iter().map(|o| if *o { FAR } else { 0 }).collect();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut v = d[i];
            if x > 0 {
                v = v.min(d[i - 1] + 3);
            }
            if y > 0 {
                v = v.min(d[i - w] + 3);
                if x > 0 {
                    v = v.min(d[i - w - 1] + 4);
                }
                if x + 1 < w {
                    v = v.min(d[i - w + 1] + 4);
                }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            let mut v = d[i];
            if x + 1 < w {
                v = v.min(d[i + 1] + 3);
            }
            if y + 1 < h {
                v = v.min(d[i + w] + 3);
                if x + 1 < w {
                    v = v.min(d[i + w + 1] + 4);
                }
                if x > 0 {
                    v = v.min(d[i + w - 1] + 4);
                }
            }
            d[i] = v;
        }
    }
    d
}

/// The `open` pixels joined to `(sx, sy)` through their four neighbours.
fn flood(open: &[bool], w: usize, h: usize, sx: usize, sy: usize) -> Vec<bool> {
    let mut filled = vec![false; w * h];
    if !open[sy * w + sx] {
        return filled;
    }
    let mut stack = vec![(sx, sy)];
    while let Some((x, y)) = stack.pop() {
        let row = y * w;
        if filled[row + x] || !open[row + x] {
            continue;
        }
        let mut left = x;
        while left > 0 && open[row + left - 1] && !filled[row + left - 1] {
            left -= 1;
        }
        let mut right = x;
        while right + 1 < w && open[row + right + 1] && !filled[row + right + 1] {
            right += 1;
        }
        filled[row + left..=row + right].fill(true);
        for px in left..=right {
            if y > 0 && open[row - w + px] && !filled[row - w + px] {
                stack.push((px, y - 1));
            }
            if y + 1 < h && open[row + w + px] && !filled[row + w + px] {
                stack.push((px, y + 1));
            }
        }
    }
    filled
}

/// Spreads `filled` up to `steps` pixels, only into `allowed` when given.
fn grow(filled: &mut [bool], w: usize, h: usize, steps: usize, allowed: Option<&[bool]>) {
    let mut front: VecDeque<(usize, usize)> = VecDeque::new();
    let mut step = vec![0_u16; w * h];
    for y in 0..h {
        for x in 0..w {
            if !filled[y * w + x] {
                continue;
            }
            let edge = (x > 0 && !filled[y * w + x - 1])
                || (x + 1 < w && !filled[y * w + x + 1])
                || (y > 0 && !filled[(y - 1) * w + x])
                || (y + 1 < h && !filled[(y + 1) * w + x]);
            if edge {
                front.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = front.pop_front() {
        let here = step[y * w + x];
        if usize::from(here) >= steps {
            continue;
        }
        let neighbours = [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ];
        for (nx, ny) in neighbours {
            if nx >= w || ny >= h {
                continue;
            }
            let i = ny * w + nx;
            if filled[i] || allowed.is_some_and(|allowed| !allowed[i]) {
                continue;
            }
            filled[i] = true;
            step[i] = here + 1;
            front.push_back((nx, ny));
        }
    }
}

/// Pours `color` into `drawing` over `filled` (a `rect`-sized mask). Pixels
/// the click matched are replaced; the edges the fill grew into get the
/// colour behind them, so lines keep their own edges on top.
pub fn pour(
    drawing: &mut RasterAsset,
    reference: &Reference,
    filled: &[bool],
    seed: (i32, i32),
    color: Rgba,
    settings: FillSettings,
) -> bool {
    let rect = reference.rect;
    if filled.len() != (rect.width() * rect.height()) as usize {
        return false;
    }
    let Some(bounds) = mask_bounds(filled, rect) else {
        return false;
    };
    let seed_color = reference.at(seed.0, seed.1);
    let limit = (settings.sanitized().tolerance * 255.0).round() as i32;
    let alpha = f32::from(color.a) / 255.0;
    let src = [
        f32::from(color.r) * alpha,
        f32::from(color.g) * alpha,
        f32::from(color.b) * alpha,
        255.0 * alpha,
    ];
    let mut pixels = drawing.compose(bounds);
    let width = bounds.width();
    let mut changed = false;
    for y in bounds.y0..bounds.y1 {
        for x in bounds.x0..bounds.x1 {
            let m = ((y - rect.y0) * rect.width() + (x - rect.x0)) as usize;
            if !filled[m] {
                continue;
            }
            let reference_pixel = reference.at(x, y);
            let matched = (seed_color[3] == 0 && reference_pixel[3] == 0)
                || (0..4).all(|c| {
                    (i32::from(reference_pixel[c]) - i32::from(seed_color[c])).abs() <= limit
                });
            let i = (((y - bounds.y0) * width + (x - bounds.x0)) * 4) as usize;
            let p = &mut pixels[i..i + 4];
            let before = [p[0], p[1], p[2], p[3]];
            let base: [f32; 4] = std::array::from_fn(|c| f32::from(p[c]));
            let out: [f32; 4] = if matched {
                // Replace what was there, as a refill should.
                std::array::from_fn(|c| src[c] + base[c] * (1.0 - alpha))
            } else {
                let room = 1.0 - base[3] / 255.0;
                std::array::from_fn(|c| base[c] + src[c] * room)
            };
            let a = out[3].round().clamp(0.0, 255.0);
            p[3] = a as u8;
            for c in 0..3 {
                p[c] = out[c].round().clamp(0.0, a) as u8;
            }
            changed |= before != [p[0], p[1], p[2], p[3]];
        }
    }
    if changed {
        drawing.put(bounds, &pixels);
    }
    changed
}

fn mask_bounds(mask: &[bool], rect: PixelRect) -> Option<PixelRect> {
    let w = rect.width() as usize;
    let mut found: Option<PixelRect> = None;
    for (y, row) in mask.chunks_exact(w.max(1)).enumerate() {
        let Some(first) = row.iter().position(|f| *f) else {
            continue;
        };
        let last = w - 1 - row.iter().rev().position(|f| *f)?;
        let line = PixelRect::new(
            rect.x0 + first as i32,
            rect.y0 + y as i32,
            rect.x0 + last as i32 + 1,
            rect.y0 + y as i32 + 1,
        );
        found = Some(found.map_or(line, |f| f.union(line)));
    }
    found
}

/// The area a fill at `(x, y)` (drawing pixels) looks at: the stage and
/// everything drawn, as seen in the drawing's pixels, kept to a size.
fn reference_rect(
    app: &EditorApp,
    canvas: &super::CanvasRef,
    drawing: Option<&RasterAsset>,
    x: i32,
    y: i32,
) -> PixelRect {
    let meta = &app.state.project.meta;
    let corners = [
        (0.0, 0.0),
        (f32::from(meta.stage_width), 0.0),
        (f32::from(meta.stage_width), f32::from(meta.stage_height)),
        (0.0, f32::from(meta.stage_height)),
    ]
    .map(|(sx, sy)| canvas.local(q0s_format::v2::Vec2::new(sx, sy)));
    let min_x = corners.iter().map(|c| c.0).fold(f32::INFINITY, f32::min);
    let min_y = corners.iter().map(|c| c.1).fold(f32::INFINITY, f32::min);
    let max_x = corners
        .iter()
        .map(|c| c.0)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = corners
        .iter()
        .map(|c| c.1)
        .fold(f32::NEG_INFINITY, f32::max);
    let mut rect = PixelRect::new(
        min_x.floor() as i32 - 1,
        min_y.floor() as i32 - 1,
        max_x.ceil() as i32 + 1,
        max_y.ceil() as i32 + 1,
    );
    if let Some(bounds) = drawing.and_then(RasterAsset::exact_bounds) {
        rect = rect.union(PixelRect::new(
            bounds.x0 - 1,
            bounds.y0 - 1,
            bounds.x1 + 1,
            bounds.y1 + 1,
        ));
    }
    rect = rect.union(PixelRect::new(x - 1, y - 1, x + 2, y + 2));
    // Huge drawings: look at a window round the click.
    let half = MAX_SIDE / 2;
    if rect.width() > MAX_SIDE {
        let x0 = (x - half).max(rect.x0).min(rect.x1 - MAX_SIDE);
        rect.x0 = x0;
        rect.x1 = x0 + MAX_SIDE;
    }
    if rect.height() > MAX_SIDE {
        let y0 = (y - half).max(rect.y0).min(rect.y1 - MAX_SIDE);
        rect.y0 = y0;
        rect.y1 = y0 + MAX_SIDE;
    }
    rect
}

/// The image a fill looks at.
fn reference(
    app: &EditorApp,
    canvas: &super::CanvasRef,
    rect: PixelRect,
    all_layers: bool,
) -> Reference {
    let project = &app.state.project;
    let rgba = if all_layers {
        // Every raster drawing shown on the frame, laid into the fill's
        // pixels. Straight from the tiles: a render of the whole frame can
        // take seconds on heavy vector art.
        let (w, h) = (rect.width() as u32, rect.height() as u32);
        let mut buffer = vec![0_u8; (w * h * 4) as usize];
        let to_reference = Affine::compose(
            Affine {
                tx: -rect.x0 as f32,
                ty: -rect.y0 as f32,
                ..Affine::IDENTITY
            },
            canvas.to_local,
        );
        for (asset_id, world) in super::shown_drawings(app) {
            if let Some(drawing) = super::raster_ref(project, asset_id) {
                drawing.draw_straight(Affine::compose(to_reference, world), &mut buffer, w, h);
            }
        }
        buffer
    } else {
        super::raster_ref(project, canvas.asset_id)
            .map(|drawing| drawing.compose_straight(rect))
            .unwrap_or_else(|| vec![0; (rect.width() * rect.height() * 4) as usize])
    };
    Reference { rect, rgba }
}

/// Stage input for the fill tool.
pub fn handle(app: &mut EditorApp, response: &Response, view: &StageView, ctx: &Context) {
    crate::cursors::request(ctx, crate::cursors::CursorKind::Bucket);
    if !response.clicked() {
        return;
    }
    let Some(pointer) = response.interact_pointer_pos() else {
        return;
    };
    if let Err(why) = super::check_layer(app) {
        app.session.status = why.message().to_string();
        return;
    }
    let settings = app.settings.raster.fill.sanitized();
    let color = super::fill_color(app);
    let started = std::time::Instant::now();
    // Look first, so a click that would fill nothing changes nothing.
    let stage = view.screen_to_stage(pointer);
    let look = match super::find_canvas(app) {
        Ok(Some(canvas)) => canvas,
        Ok(None) => {
            // Nothing drawn on this frame yet: look through where the new
            // drawing will go.
            let mut world = Affine::IDENTITY;
            if let Some(parent) = q0s_format::parenting::parent_world(
                &app.state.project,
                app.session.current_q0rg_id,
                app.session.current_layer_id,
                app.session.current_frame,
            ) {
                world = parent;
            }
            super::CanvasRef {
                q0rg_id: app.session.current_q0rg_id,
                layer_id: app.session.current_layer_id,
                asset_id: 0,
                to_local: world.inverse().unwrap_or(Affine::IDENTITY),
            }
        }
        Err(why) => {
            app.session.status = why.message().to_string();
            return;
        }
    };
    let (lx, ly) = look.local(stage);
    let (x, y) = (lx.floor() as i32, ly.floor() as i32);
    let drawing = super::raster_ref(&app.state.project, look.asset_id);
    let rect = reference_rect(app, &look, drawing, x, y);
    let reference = reference(app, &look, rect, settings.all_layers);
    let filled = region(&reference, x, y, settings);
    if !filled.iter().any(|f| *f) {
        app.session.status = "nothing to fill there".to_string();
        return;
    }
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let canvas = match super::prepare_canvas(app) {
        Ok(canvas) => canvas,
        Err(why) => {
            app.session.status = why.message().to_string();
            return;
        }
    };
    let Some(drawing) = super::raster_mut(&mut app.state.project, canvas.asset_id) else {
        return;
    };
    let changed = pour(drawing, &reference, &filled, (x, y), color, settings);
    app.state.mark_dirty();
    let leaked = filled
        .chunks_exact(rect.width() as usize)
        .enumerate()
        .any(|(row, cells)| {
            (row == 0 || row + 1 == rect.height() as usize) && cells.iter().any(|f| *f)
        });
    app.session.status = if !changed {
        "already that colour".to_string()
    } else if leaked {
        format!(
            "filled out to the edge: close the lines, or raise Close gaps ({} ms)",
            started.elapsed().as_millis()
        )
    } else {
        format!("filled ({} ms)", started.elapsed().as_millis())
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ring of black line `thickness` wide round (50, 50), with a gap of
    /// `gap` pixels on its right.
    fn ring(gap: i32) -> Reference {
        let rect = PixelRect::new(0, 0, 100, 100);
        let mut rgba = vec![0_u8; 100 * 100 * 4];
        for y in 0..100_i32 {
            for x in 0..100_i32 {
                let d = (((x - 50) * (x - 50) + (y - 50) * (y - 50)) as f32).sqrt();
                let in_gap = x > 70 && (y - 50).abs() * 2 < gap;
                if (28.0..32.0).contains(&d) && !in_gap {
                    let i = (y * 100 + x) as usize * 4;
                    rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
                }
            }
        }
        Reference { rect, rgba }
    }

    fn count(mask: &[bool]) -> usize {
        mask.iter().filter(|f| **f).count()
    }

    #[test]
    fn a_closed_ring_fills_inside_only() {
        let reference = ring(0);
        let settings = FillSettings {
            gap: 0,
            grow: 0,
            ..FillSettings::default()
        };
        let filled = region(&reference, 50, 50, settings);
        let inside = count(&filled);
        // About pi * 28^2 = 2463 pixels.
        assert!((2300..2600).contains(&inside), "{inside}");
        assert!(!filled[5 * 100 + 5]);
    }

    #[test]
    fn small_gaps_hold_the_fill_and_it_still_reaches_the_lines() {
        let reference = ring(5);
        let leaky = region(
            &reference,
            50,
            50,
            FillSettings {
                gap: 0,
                grow: 0,
                ..FillSettings::default()
            },
        );
        assert!(leaky[5 * 100 + 5], "without gap closing the fill escapes");
        let held = region(
            &reference,
            50,
            50,
            FillSettings {
                gap: 6,
                grow: 0,
                ..FillSettings::default()
            },
        );
        assert!(!held[5 * 100 + 5], "gap closing keeps it in");
        // It creeps back to the line: right next to the inner edge is filled.
        assert!(held[50 * 100 + 23], "reaches the line");
        assert!(count(&held) > 2200, "{}", count(&held));
    }

    #[test]
    fn growing_tucks_the_fill_under_the_line_edge() {
        let reference = ring(0);
        let plain = region(
            &reference,
            50,
            50,
            FillSettings {
                gap: 0,
                grow: 0,
                ..FillSettings::default()
            },
        );
        let grown = region(
            &reference,
            50,
            50,
            FillSettings {
                gap: 0,
                grow: 2,
                ..FillSettings::default()
            },
        );
        assert!(count(&grown) > count(&plain));
        // Under the line, not past it.
        assert!(!grown[50 * 100 + 15]);
    }

    #[test]
    fn pouring_goes_behind_the_line_and_replaces_the_inside() {
        let reference = ring(0);
        let mut drawing = RasterAsset::new(1);
        let mut line = reference.rgba.clone();
        q0s_format::pixels::premultiply(&mut line);
        drawing.put(reference.rect, &line);
        let settings = FillSettings {
            gap: 0,
            grow: 2,
            all_layers: false,
            ..FillSettings::default()
        };
        let filled = region(&reference, 50, 50, settings);
        let red = Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        };
        assert!(pour(
            &mut drawing,
            &reference,
            &filled,
            (50, 50),
            red,
            settings
        ));
        assert_eq!(drawing.pixel(50, 50), [255, 0, 0, 255]);
        // The line stays black on top.
        assert_eq!(drawing.pixel(50, 21), [0, 0, 0, 255]);
        assert_eq!(drawing.pixel(5, 5), [0, 0, 0, 0]);
        // Filling again with the same colour changes nothing.
        let again = Reference {
            rect: reference.rect,
            rgba: drawing.compose_straight(reference.rect),
        };
        let filled = region(&again, 50, 50, settings);
        assert!(!pour(
            &mut drawing,
            &again,
            &filled,
            (50, 50),
            red,
            settings
        ));
    }
}
