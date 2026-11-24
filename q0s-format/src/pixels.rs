//! Raster drawings: the pixels a raster layer's keyframes hold.
//!
//! A raster drawing is an asset like a vector drawing, placed on its keyframe
//! by an identity placement, so the timeline, symbols, tweens, masks, export
//! and the player treat it as they treat any other drawing. Only the pixels
//! are new.
//!
//! Pixels live in 64 x 64 tiles keyed by tile coordinates; only tiles with
//! something in them exist, so a drawing costs what is drawn, not the canvas
//! it could cover. Pixel `(x, y)` covers the unit square at `(x, y)` in the
//! asset's own space, which is stage space at an identity placement. Pixels
//! are premultiplied RGBA8.
//!
//! Tiles sit behind `Arc`s and are copied on write: undo steps, duplicated
//! keyframes and copies of a drawing share every tile until one of them
//! paints over it, and then only that tile is copied.
//!
//! On disk (q1s v49) each tile is row-delta filtered and deflated; a tile
//! that is the same as one written earlier in the file is stored as a
//! reference to it, so held and duplicated drawings cost a few bytes a tile.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use crate::error::Error;
use crate::io::Cursor;

/// Side of a tile in pixels.
pub const TILE: usize = 64;
/// [`TILE`] as a signed pixel count, for coordinate maths.
pub const TILE_I32: i32 = TILE as i32;
/// Bytes in one tile's pixels.
pub const TILE_BYTES: usize = TILE * TILE * 4;
/// How hard a tile is deflated. Tiles are packed once and the result is
/// kept, so this can afford to be thorough.
const TILE_PACK_LEVEL: u8 = 6;
/// The most tiles one drawing may hold: 8192 tiles is a 5792 px square
/// painted edge to edge, 128 MiB of pixels.
pub const MAX_TILES_PER_DRAWING: usize = 8192;
/// The most distinct tiles one file may hold (2 GiB of pixels).
const MAX_TILES_PER_FILE: usize = 131_072;

const TILE_DATA: u8 = 0;
const TILE_REPEAT: u8 = 1;

static STAMPS: AtomicU64 = AtomicU64::new(1);

fn next_stamp() -> u64 {
    STAMPS.fetch_add(1, Ordering::Relaxed)
}

/// Tile coordinates of the tile holding pixel coordinate `value`, and the
/// pixel's place inside it.
pub fn tile_of(value: i32) -> (i32, usize) {
    let tile = value.div_euclid(TILE_I32);
    (tile, value.rem_euclid(TILE_I32) as usize)
}

/// One 64 x 64 block of premultiplied pixels.
#[derive(Debug)]
pub struct RasterTile {
    rgba: Vec<u8>,
    /// Changes whenever the pixels do, and is unique to that content across
    /// the process: textures compare stamps to know what to upload again.
    /// Copies keep it, since their pixels are the same.
    stamp: u64,
    /// The tile as it is written to disk, once someone has asked for it.
    packed: OnceLock<Arc<[u8]>>,
    /// Where its visible pixels are (x0, y0, x1, y1 inclusive), once asked.
    bounds: OnceLock<Option<[u8; 4]>>,
}

impl Clone for RasterTile {
    fn clone(&self) -> Self {
        let packed = OnceLock::new();
        if let Some(bytes) = self.packed.get() {
            let _ = packed.set(Arc::clone(bytes));
        }
        let bounds = OnceLock::new();
        if let Some(found) = self.bounds.get() {
            let _ = bounds.set(*found);
        }
        Self {
            rgba: self.rgba.clone(),
            stamp: self.stamp,
            packed,
            bounds,
        }
    }
}

impl PartialEq for RasterTile {
    fn eq(&self, other: &Self) -> bool {
        self.stamp == other.stamp || self.rgba == other.rgba
    }
}

impl Eq for RasterTile {}

impl Default for RasterTile {
    fn default() -> Self {
        Self::blank()
    }
}

impl RasterTile {
    /// A fully transparent tile.
    pub fn blank() -> Self {
        Self {
            rgba: vec![0; TILE_BYTES],
            stamp: next_stamp(),
            packed: OnceLock::new(),
            bounds: OnceLock::new(),
        }
    }

    /// A tile from [`TILE_BYTES`] premultiplied bytes.
    pub fn from_pixels(rgba: Vec<u8>) -> Result<Self, Error> {
        if rgba.len() != TILE_BYTES {
            return Err(Error::Validation("raster tile has the wrong size"));
        }
        Ok(Self {
            rgba,
            stamp: next_stamp(),
            packed: OnceLock::new(),
            bounds: OnceLock::new(),
        })
    }

    pub fn pixels(&self) -> &[u8] {
        &self.rgba
    }

    /// The pixels, to change. Gives the tile a new stamp and forgets its
    /// packed form.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        self.stamp = next_stamp();
        self.packed = OnceLock::new();
        self.bounds = OnceLock::new();
        &mut self.rgba
    }

    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    /// One pixel, premultiplied.
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * TILE + x) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    /// Nothing visible in it.
    pub fn is_clear(&self) -> bool {
        self.visible_bounds().is_none()
    }

    /// The smallest box (x0, y0, x1, y1, inclusive) holding its visible
    /// pixels. Worked out once per content.
    pub fn visible_bounds(&self) -> Option<[u8; 4]> {
        *self.bounds.get_or_init(|| {
            let mut found: Option<[usize; 4]> = None;
            for y in 0..TILE {
                let row = &self.rgba[y * TILE * 4..(y + 1) * TILE * 4];
                let Some(first) = row.as_chunks::<4>().0.iter().position(|p| p[3] != 0) else {
                    continue;
                };
                let last = TILE
                    - 1
                    - row
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .rev()
                        .position(|p| p[3] != 0)?;
                found = Some(match found {
                    None => [first, y, last, y],
                    Some([x0, y0, x1, _]) => [x0.min(first), y0, x1.max(last), y],
                });
            }
            found.map(|b| b.map(|v| v as u8))
        })
    }

    /// The tile as stored on disk: each row's bytes minus the pixel to their
    /// left, deflated. Worked out once per content and kept.
    pub fn packed(&self) -> Arc<[u8]> {
        Arc::clone(self.packed.get_or_init(|| {
            let mut filtered = self.rgba.clone();
            for row in filtered.as_chunks_mut::<{ TILE * 4 }>().0.iter_mut() {
                for i in (4..row.len()).rev() {
                    row[i] = row[i].wrapping_sub(row[i - 4]);
                }
            }
            miniz_oxide::deflate::compress_to_vec(&filtered, TILE_PACK_LEVEL).into()
        }))
    }

    /// A tile read back from [`packed`](Self::packed) bytes.
    pub fn from_packed(bytes: &[u8]) -> Result<Self, Error> {
        let mut rgba = vec![0; TILE_BYTES];
        let mut state =
            miniz_oxide::inflate::stream::InflateState::new_boxed(miniz_oxide::DataFormat::Raw);
        let result = miniz_oxide::inflate::stream::inflate(
            &mut state,
            bytes,
            &mut rgba,
            miniz_oxide::MZFlush::Finish,
        );
        if result.status != Ok(miniz_oxide::MZStatus::StreamEnd)
            || result.bytes_written != TILE_BYTES
            || result.bytes_consumed != bytes.len()
        {
            return Err(Error::Validation("raster tile is damaged"));
        }
        for row in rgba.as_chunks_mut::<{ TILE * 4 }>().0.iter_mut() {
            for i in 4..row.len() {
                row[i] = row[i].wrapping_add(row[i - 4]);
            }
        }
        // Premultiplied colour never exceeds its alpha. A file that says
        // otherwise would blend wrong; clamp rather than refuse it.
        for pixel in rgba.as_chunks_mut::<4>().0.iter_mut() {
            let alpha = pixel[3];
            for channel in &mut pixel[..3] {
                *channel = (*channel).min(alpha);
            }
        }
        let tile = Self {
            rgba,
            stamp: next_stamp(),
            packed: OnceLock::new(),
            bounds: OnceLock::new(),
        };
        let _ = tile.packed.set(bytes.into());
        Ok(tile)
    }

    fn content_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.rgba.hash(&mut hasher);
        hasher.finish()
    }
}

/// Pixel rectangle: `x0..x1` by `y0..y1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl PixelRect {
    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn width(&self) -> i32 {
        (self.x1 - self.x0).max(0)
    }

    pub fn height(&self) -> i32 {
        (self.y1 - self.y0).max(0)
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    pub fn intersect(self, other: Self) -> Self {
        Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        }
    }

    /// The tiles this rectangle touches, inclusive.
    pub fn tiles(&self) -> (i32, i32, i32, i32) {
        (
            self.x0.div_euclid(TILE_I32),
            self.y0.div_euclid(TILE_I32),
            (self.x1 - 1).div_euclid(TILE_I32),
            (self.y1 - 1).div_euclid(TILE_I32),
        )
    }
}

/// A raster drawing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RasterAsset {
    pub asset_id: u16,
    /// Non-empty tiles by tile coordinates (x, y).
    pub tiles: BTreeMap<(i16, i16), Arc<RasterTile>>,
}

impl RasterAsset {
    pub fn new(asset_id: u16) -> Self {
        Self {
            asset_id,
            tiles: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// The tiles' extent in pixels, or `None` for an empty drawing.
    pub fn tile_bounds(&self) -> Option<PixelRect> {
        let mut rect: Option<PixelRect> = None;
        for &(tx, ty) in self.tiles.keys() {
            let x0 = i32::from(tx) * TILE_I32;
            let y0 = i32::from(ty) * TILE_I32;
            let tile = PixelRect::new(x0, y0, x0 + TILE_I32, y0 + TILE_I32);
            rect = Some(rect.map_or(tile, |rect| rect.union(tile)));
        }
        rect
    }

    /// The smallest rectangle holding every visible pixel.
    pub fn exact_bounds(&self) -> Option<PixelRect> {
        let mut rect: Option<PixelRect> = None;
        for (&(tx, ty), tile) in &self.tiles {
            if let Some([x0, y0, x1, y1]) = tile.visible_bounds() {
                let ox = i32::from(tx) * TILE_I32;
                let oy = i32::from(ty) * TILE_I32;
                let found = PixelRect::new(
                    ox + i32::from(x0),
                    oy + i32::from(y0),
                    ox + i32::from(x1) + 1,
                    oy + i32::from(y1) + 1,
                );
                rect = Some(rect.map_or(found, |rect| rect.union(found)));
            }
        }
        rect
    }

    /// One pixel, premultiplied; transparent outside the drawing.
    pub fn pixel(&self, x: i32, y: i32) -> [u8; 4] {
        let (tx, px) = tile_of(x);
        let (ty, py) = tile_of(y);
        let (Ok(tx), Ok(ty)) = (i16::try_from(tx), i16::try_from(ty)) else {
            return [0; 4];
        };
        self.tiles
            .get(&(tx, ty))
            .map_or([0; 4], |tile| tile.pixel(px, py))
    }

    /// The tile at tile coordinates `(tx, ty)`, to change: made if missing,
    /// copied first if anything else shares it. `None` past the edge of the
    /// coordinate space or the tile limit.
    pub fn tile_mut(&mut self, tx: i32, ty: i32) -> Option<&mut RasterTile> {
        let key = (i16::try_from(tx).ok()?, i16::try_from(ty).ok()?);
        if !self.tiles.contains_key(&key) && self.tiles.len() >= MAX_TILES_PER_DRAWING {
            return None;
        }
        let tile = self
            .tiles
            .entry(key)
            .or_insert_with(|| Arc::new(RasterTile::blank()));
        Some(Arc::make_mut(tile))
    }

    /// Drops tiles nothing is left on.
    pub fn prune(&mut self) {
        self.tiles.retain(|_, tile| !tile.is_clear());
    }

    /// Memory this copy holds of its own: a shared tile counts its share.
    pub fn held_bytes(&self) -> usize {
        self.tiles
            .values()
            .map(|tile| TILE_BYTES / Arc::strong_count(tile).max(1))
            .sum()
    }

    /// Premultiplied pixels of `rect`, row by row.
    pub fn compose(&self, rect: PixelRect) -> Vec<u8> {
        let width = rect.width() as usize;
        let height = rect.height() as usize;
        let mut out = vec![0_u8; width * height * 4];
        if rect.is_empty() {
            return out;
        }
        let (tx0, ty0, tx1, ty1) = rect.tiles();
        for (&(tx, ty), tile) in self.tiles.range((clamp16(tx0), i16::MIN)..) {
            let (tx, ty) = (i32::from(tx), i32::from(ty));
            if tx > tx1 {
                break;
            }
            if ty < ty0 || ty > ty1 {
                continue;
            }
            let ox = tx * TILE_I32;
            let oy = ty * TILE_I32;
            let part = rect.intersect(PixelRect::new(ox, oy, ox + TILE_I32, oy + TILE_I32));
            if part.is_empty() {
                continue;
            }
            let run = part.width() as usize * 4;
            for y in part.y0..part.y1 {
                let src = (((y - oy) as usize) * TILE + (part.x0 - ox) as usize) * 4;
                let dst = (((y - rect.y0) as usize) * width + (part.x0 - rect.x0) as usize) * 4;
                out[dst..dst + run].copy_from_slice(&tile.rgba[src..src + run]);
            }
        }
        out
    }

    /// Straight (not premultiplied) pixels of `rect`, as bitmaps hold them.
    pub fn compose_straight(&self, rect: PixelRect) -> Vec<u8> {
        let mut out = self.compose(rect);
        unpremultiply(&mut out);
        out
    }

    /// Writes premultiplied `rgba` (`rect`-sized) over the drawing,
    /// replacing what was there. Fully transparent source tiles that do not
    /// exist yet are not made.
    pub fn put(&mut self, rect: PixelRect, rgba: &[u8]) {
        if rect.is_empty() || rgba.len() != rect.width() as usize * rect.height() as usize * 4 {
            return;
        }
        let width = rect.width() as usize;
        let (tx0, ty0, tx1, ty1) = rect.tiles();
        let mut touched = Vec::new();
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let ox = tx * TILE_I32;
                let oy = ty * TILE_I32;
                let part = rect.intersect(PixelRect::new(ox, oy, ox + TILE_I32, oy + TILE_I32));
                let run = part.width() as usize * 4;
                let exists = i16::try_from(tx)
                    .ok()
                    .zip(i16::try_from(ty).ok())
                    .is_some_and(|key| self.tiles.contains_key(&key));
                if !exists {
                    let visible = (part.y0..part.y1).any(|y| {
                        let src =
                            (((y - rect.y0) as usize) * width + (part.x0 - rect.x0) as usize) * 4;
                        rgba[src..src + run]
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .any(|p| p[3] != 0)
                    });
                    if !visible {
                        continue;
                    }
                }
                let Some(tile) = self.tile_mut(tx, ty) else {
                    continue;
                };
                let pixels = tile.pixels_mut();
                for y in part.y0..part.y1 {
                    let src = (((y - rect.y0) as usize) * width + (part.x0 - rect.x0) as usize) * 4;
                    let dst = (((y - oy) as usize) * TILE + (part.x0 - ox) as usize) * 4;
                    pixels[dst..dst + run].copy_from_slice(&rgba[src..src + run]);
                }
                touched.push((tx as i16, ty as i16));
            }
        }
        self.prune_tiles(touched);
    }

    /// Drops those of `keys` nothing is left on.
    pub fn prune_tiles(&mut self, keys: impl IntoIterator<Item = (i16, i16)>) {
        for key in keys {
            if self.tiles.get(&key).is_some_and(|tile| tile.is_clear()) {
                self.tiles.remove(&key);
            }
        }
    }

    /// Changes whenever any pixel does: a key for caches of the drawing.
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.asset_id.hash(&mut hasher);
        for (key, tile) in &self.tiles {
            key.hash(&mut hasher);
            tile.stamp.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Paints the drawing through `transform` (its space to the buffer's
    /// pixels) over a straight-alpha RGBA `buffer`, as the software
    /// rasteriser does for every other asset. Bilinear, and pre-shrunk by
    /// halves when drawn much smaller, so it stays smooth at any size.
    pub fn draw_straight(
        &self,
        transform: crate::transform::Affine,
        buffer: &mut [u8],
        width: u32,
        height: u32,
    ) {
        let Some(rect) = self.exact_bounds() else {
            return;
        };
        if width == 0 || height == 0 || buffer.len() < width as usize * height as usize * 4 {
            return;
        }
        let mut pixels = self.compose(rect);
        let (mut pw, mut ph) = (rect.width() as usize, rect.height() as usize);
        // A unit step in the drawing covers this many buffer pixels.
        let scale = ((transform.a11 * transform.a22 - transform.a12 * transform.a21).abs())
            .sqrt()
            .max(f32::EPSILON);
        let mut step = 1.0_f32;
        while scale * step < 0.5 && pw > 1 && ph > 1 {
            let (next, w, h) = halve(&pixels, pw, ph);
            pixels = next;
            pw = w;
            ph = h;
            step *= 2.0;
        }
        let exact_copy = step == 1.0
            && (transform.a11 - 1.0).abs() < 1e-4
            && (transform.a22 - 1.0).abs() < 1e-4
            && transform.a12.abs() < 1e-4
            && transform.a21.abs() < 1e-4
            && (transform.tx - transform.tx.round()).abs() < 1e-3
            && (transform.ty - transform.ty.round()).abs() < 1e-3;
        let local = crate::transform::Affine::compose(
            transform,
            crate::transform::Affine {
                a11: step,
                a12: 0.0,
                a21: 0.0,
                a22: step,
                tx: rect.x0 as f32,
                ty: rect.y0 as f32,
            },
        );
        let corners = [
            (0.0, 0.0),
            (pw as f32, 0.0),
            (pw as f32, ph as f32),
            (0.0, ph as f32),
        ]
        .map(|(x, y)| local.apply(crate::v2::Vec2::new(x, y)));
        let min_x = corners.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let max_x = corners
            .iter()
            .map(|p| p.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = corners.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let max_y = corners
            .iter()
            .map(|p| p.y)
            .fold(f32::NEG_INFINITY, f32::max);
        if !(min_x.is_finite() && max_x.is_finite() && min_y.is_finite() && max_y.is_finite()) {
            return;
        }
        let Some(inverse) = local.inverse() else {
            return;
        };
        let x_start = (min_x.floor() as i64).max(0) as u32;
        let y_start = (min_y.floor() as i64).max(0) as u32;
        let x_end = (max_x.ceil() as i64).min(i64::from(width)) as u32;
        let y_end = (max_y.ceil() as i64).min(i64::from(height)) as u32;
        for y in y_start..y_end {
            for x in x_start..x_end {
                let source = if exact_copy {
                    let sx = x as i64 - local.tx.round() as i64;
                    let sy = y as i64 - local.ty.round() as i64;
                    if sx < 0 || sy < 0 || sx >= pw as i64 || sy >= ph as i64 {
                        continue;
                    }
                    let i = (sy as usize * pw + sx as usize) * 4;
                    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
                } else {
                    let p = inverse.apply(crate::v2::Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
                    bilinear(&pixels, pw, ph, p.x - 0.5, p.y - 0.5)
                };
                if source[3] == 0 {
                    continue;
                }
                let i = (y as usize * width as usize + x as usize) * 4;
                over_straight(&mut buffer[i..i + 4], source);
            }
        }
    }

    /// A drawing of premultiplied `rgba` covering `rect`.
    pub fn from_pixels(asset_id: u16, rect: PixelRect, rgba: &[u8]) -> Self {
        let mut asset = Self::new(asset_id);
        asset.put(rect, rgba);
        asset
    }

    /// A drawing from a straight-alpha bitmap whose top left sits at
    /// `(x, y)`.
    pub fn from_straight(
        asset_id: u16,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Self {
        let mut pixels = rgba.to_vec();
        premultiply(&mut pixels);
        Self::from_pixels(
            asset_id,
            PixelRect::new(x, y, x + width as i32, y + height as i32),
            &pixels,
        )
    }
}

/// Premultiplied pixels shrunk to half size, box filtered.
pub fn halve(pixels: &[u8], width: usize, height: usize) -> (Vec<u8>, usize, usize) {
    let w = width.div_ceil(2);
    let h = height.div_ceil(2);
    let mut out = vec![0_u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            // Pixels past an odd edge count as clear.
            let mut sum = [0_u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                if sx < width && sy < height {
                    let i = (sy * width + sx) * 4;
                    for c in 0..4 {
                        sum[c] += u32::from(pixels[i + c]);
                    }
                }
            }
            let i = (y * w + x) * 4;
            for c in 0..4 {
                out[i + c] = ((sum[c] + 2) / 4) as u8;
            }
        }
    }
    (out, w, h)
}

/// Premultiplied bilinear sample at pixel-centre coordinates `(x, y)`;
/// clear outside.
pub fn bilinear(pixels: &[u8], width: usize, height: usize, x: f32, y: f32) -> [u8; 4] {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let (x0, y0) = (x0 as i64, y0 as i64);
    let at = |sx: i64, sy: i64| -> [f32; 4] {
        if sx < 0 || sy < 0 || sx >= width as i64 || sy >= height as i64 {
            return [0.0; 4];
        }
        let i = (sy as usize * width + sx as usize) * 4;
        [
            f32::from(pixels[i]),
            f32::from(pixels[i + 1]),
            f32::from(pixels[i + 2]),
            f32::from(pixels[i + 3]),
        ]
    };
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1, y0),
        at(x0, y0 + 1),
        at(x0 + 1, y0 + 1),
    );
    let mut out = [0_u8; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        out[i] = (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// A premultiplied pixel over a straight-alpha one.
pub fn over_straight(destination: &mut [u8], source: [u8; 4]) {
    let sa = u32::from(source[3]);
    if sa == 255 {
        destination[..3].copy_from_slice(&source[..3]);
        destination[3] = 255;
        return;
    }
    let da = u32::from(destination[3]);
    let keep = da * (255 - sa);
    let out_a = sa * 255 + keep;
    if out_a == 0 {
        return;
    }
    for c in 0..3 {
        let value = u32::from(source[c]) * 255 * 255 + u32::from(destination[c]) * keep;
        destination[c] = ((value + out_a / 2) / out_a).min(255) as u8;
    }
    destination[3] = ((out_a + 127) / 255) as u8;
}

fn clamp16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Straight alpha to premultiplied, in place.
pub fn premultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0.iter_mut() {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
}

/// Premultiplied to straight alpha, in place.
pub fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0.iter_mut() {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            pixel[..3].fill(0);
        } else if alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// q1s

/// Tiles written so far in one file, so a repeat is stored as a reference.
#[derive(Default)]
pub(crate) struct TileWriteMemory {
    by_pointer: HashMap<usize, u32>,
    by_content: HashMap<u64, Vec<(u32, Arc<RasterTile>)>>,
    written: u32,
}

impl TileWriteMemory {
    fn find(&mut self, tile: &Arc<RasterTile>) -> Result<u32, Arc<RasterTile>> {
        let pointer = Arc::as_ptr(tile) as usize;
        if let Some(&index) = self.by_pointer.get(&pointer) {
            return Ok(index);
        }
        let hash = tile.content_hash();
        let bucket = self.by_content.entry(hash).or_default();
        if let Some((index, _)) = bucket.iter().find(|(_, known)| known.rgba == tile.rgba) {
            let index = *index;
            self.by_pointer.insert(pointer, index);
            return Ok(index);
        }
        Err(Arc::clone(tile))
    }

    fn remember(&mut self, tile: Arc<RasterTile>) -> u32 {
        let index = self.written;
        self.written += 1;
        self.by_pointer.insert(Arc::as_ptr(&tile) as usize, index);
        let hash = tile.content_hash();
        // The Arc is kept so its address cannot be reused by another tile
        // while this file is written.
        self.by_content.entry(hash).or_default().push((index, tile));
        index
    }
}

/// A raster asset's payload, after its id, kind and name.
pub(crate) fn write_raster(
    out: &mut Vec<u8>,
    asset: &RasterAsset,
    memory: &mut TileWriteMemory,
) -> Result<(), Error> {
    let count = u32::try_from(asset.tiles.len())
        .map_err(|_| Error::Overflow("raster tile count exceeds u32"))?;
    if asset.tiles.len() > MAX_TILES_PER_DRAWING {
        return Err(Error::Validation("raster drawing holds too many tiles"));
    }
    out.extend_from_slice(&count.to_le_bytes());
    for (&(tx, ty), tile) in &asset.tiles {
        out.extend_from_slice(&tx.to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
        match memory.find(tile) {
            Ok(index) => {
                out.push(TILE_REPEAT);
                out.extend_from_slice(&index.to_le_bytes());
            }
            Err(tile) => {
                let packed = tile.packed();
                let len = u32::try_from(packed.len())
                    .map_err(|_| Error::Overflow("raster tile exceeds u32"))?;
                out.push(TILE_DATA);
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&packed);
                memory.remember(tile);
            }
        }
    }
    Ok(())
}

/// Reads what [`write_raster`] wrote. `seen` holds every tile read so far
/// from this file, so repeats come back as the same shared tile.
pub(crate) fn read_raster(
    c: &mut Cursor<'_>,
    asset_id: u16,
    seen: &mut Vec<Arc<RasterTile>>,
) -> Result<RasterAsset, Error> {
    let count = c.read_u32()? as usize;
    if count > MAX_TILES_PER_DRAWING {
        return Err(Error::Validation("raster drawing holds too many tiles"));
    }
    let mut asset = RasterAsset::new(asset_id);
    for _ in 0..count {
        let tx = c.read_i16()?;
        let ty = c.read_i16()?;
        let tile = match c.read_u8()? {
            TILE_DATA => {
                if seen.len() >= MAX_TILES_PER_FILE {
                    return Err(Error::Validation("file holds too many raster tiles"));
                }
                let len = c.read_u32()? as usize;
                let tile = Arc::new(RasterTile::from_packed(c.read_exact(len)?)?);
                seen.push(Arc::clone(&tile));
                tile
            }
            TILE_REPEAT => {
                let index = c.read_u32()? as usize;
                Arc::clone(
                    seen.get(index)
                        .ok_or(Error::Validation("raster tile repeats one not yet read"))?,
                )
            }
            _ => return Err(Error::Validation("invalid raster tile tag")),
        };
        if asset.tiles.insert((tx, ty), tile).is_some() {
            return Err(Error::Validation("raster drawing repeats a tile position"));
        }
    }
    Ok(asset)
}

// ---------------------------------------------------------------------------
// JSON (plugins, online rooms)

#[cfg(feature = "serde")]
mod json {
    use super::{RasterAsset, RasterTile};
    use base64::Engine;
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;

    #[derive(Serialize, Deserialize)]
    struct Shape {
        asset_id: u16,
        /// `[x, y, base64 of the packed tile]`.
        tiles: Vec<(i16, i16, String)>,
    }

    impl Serialize for RasterAsset {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let engine = base64::engine::general_purpose::STANDARD;
            Shape {
                asset_id: self.asset_id,
                tiles: self
                    .tiles
                    .iter()
                    .map(|(&(x, y), tile)| (x, y, engine.encode(tile.packed())))
                    .collect(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for RasterAsset {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let shape = Shape::deserialize(deserializer)?;
            if shape.tiles.len() > super::MAX_TILES_PER_DRAWING {
                return Err(D::Error::custom("raster drawing holds too many tiles"));
            }
            let engine = base64::engine::general_purpose::STANDARD;
            let mut asset = RasterAsset::new(shape.asset_id);
            for (x, y, text) in shape.tiles {
                let bytes = engine.decode(text.as_bytes()).map_err(D::Error::custom)?;
                let tile = RasterTile::from_packed(&bytes).map_err(D::Error::custom)?;
                asset.tiles.insert((x, y), Arc::new(tile));
            }
            Ok(asset)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(asset: &mut RasterAsset, x: i32, y: i32, rgba: [u8; 4]) {
        let (tx, px) = tile_of(x);
        let (ty, py) = tile_of(y);
        let tile = asset.tile_mut(tx, ty).unwrap();
        let i = (py * TILE + px) * 4;
        tile.pixels_mut()[i..i + 4].copy_from_slice(&rgba);
    }

    #[test]
    fn tiles_are_made_where_painted_and_read_back() {
        let mut asset = RasterAsset::new(3);
        dot(&mut asset, -1, -1, [10, 20, 30, 255]);
        dot(&mut asset, 130, 5, [0, 0, 128, 128]);
        assert_eq!(asset.tiles.len(), 2);
        assert!(asset.tiles.contains_key(&(-1, -1)));
        assert!(asset.tiles.contains_key(&(2, 0)));
        assert_eq!(asset.pixel(-1, -1), [10, 20, 30, 255]);
        assert_eq!(asset.pixel(130, 5), [0, 0, 128, 128]);
        assert_eq!(asset.pixel(0, 0), [0; 4]);
        assert_eq!(asset.exact_bounds(), Some(PixelRect::new(-1, -1, 131, 6)));
        assert_eq!(asset.tile_bounds(), Some(PixelRect::new(-64, -64, 192, 64)));
    }

    #[test]
    fn copies_share_tiles_until_one_paints() {
        let mut first = RasterAsset::new(1);
        dot(&mut first, 3, 3, [255, 0, 0, 255]);
        dot(&mut first, 100, 3, [255, 0, 0, 255]);
        let mut second = first.clone();
        assert!(Arc::ptr_eq(&first.tiles[&(0, 0)], &second.tiles[&(0, 0)]));
        dot(&mut second, 4, 4, [0, 255, 0, 255]);
        assert!(!Arc::ptr_eq(&first.tiles[&(0, 0)], &second.tiles[&(0, 0)]));
        assert!(Arc::ptr_eq(&first.tiles[&(1, 0)], &second.tiles[&(1, 0)]));
        assert_eq!(first.pixel(4, 4), [0; 4]);
        assert_eq!(second.pixel(4, 4), [0, 255, 0, 255]);
        assert_ne!(first.tiles[&(0, 0)].stamp(), second.tiles[&(0, 0)].stamp());
    }

    #[test]
    fn packed_tiles_unpack_to_the_same_pixels() {
        let mut tile = RasterTile::blank();
        for (i, byte) in tile
            .pixels_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            let alpha = (i % 256) as u8;
            byte.copy_from_slice(&[alpha / 2, alpha / 3, alpha, alpha]);
        }
        let packed = tile.packed();
        assert!(packed.len() < TILE_BYTES / 4, "packed to {}", packed.len());
        let back = RasterTile::from_packed(&packed).unwrap();
        assert_eq!(back.pixels(), tile.pixels());
        assert!(RasterTile::from_packed(&packed[..packed.len() - 1]).is_err());
    }

    #[test]
    fn compose_and_put_round_trip_across_tiles() {
        let rect = PixelRect::new(-10, 50, 90, 70);
        let mut rgba = vec![0_u8; rect.width() as usize * rect.height() as usize * 4];
        for (i, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let alpha = if i % 3 == 0 { 0 } else { 200 };
            pixel.copy_from_slice(&[alpha / 2, 0, alpha, alpha]);
        }
        let asset = RasterAsset::from_pixels(9, rect, &rgba);
        assert_eq!(asset.compose(rect), rgba);
        assert_eq!(asset.tiles.len(), 6);
        let mut cleared = asset.clone();
        cleared.put(rect, &vec![0; rgba.len()]);
        assert!(cleared.is_empty());
    }

    #[test]
    fn premultiplying_round_trips_opaque_and_clears_hidden_colour() {
        let mut rgba = vec![200, 100, 50, 255, 200, 100, 50, 0, 255, 255, 255, 128];
        premultiply(&mut rgba);
        assert_eq!(&rgba[..8], &[200, 100, 50, 255, 0, 0, 0, 0]);
        assert_eq!(&rgba[8..], &[128, 128, 128, 128]);
        unpremultiply(&mut rgba);
        assert_eq!(&rgba[8..], &[255, 255, 255, 128]);
    }

    #[test]
    fn repeated_tiles_are_written_once() {
        let mut first = RasterAsset::new(1);
        dot(&mut first, 3, 3, [255, 0, 0, 255]);
        let shared = first.clone();
        // Same pixels, painted separately: found by content.
        let mut twin = RasterAsset::new(3);
        dot(&mut twin, 3, 3, [255, 0, 0, 255]);
        let mut memory = TileWriteMemory::default();
        let mut out = Vec::new();
        for asset in [&first, &shared, &twin] {
            write_raster(&mut out, asset, &mut memory).unwrap();
        }
        let single = first.tiles[&(0, 0)].packed().len();
        assert!(out.len() < single + 3 * 9 + 3 * 4 + 16, "{}", out.len());
        let mut seen = Vec::new();
        let mut c = Cursor::new(&out);
        let a = read_raster(&mut c, 1, &mut seen).unwrap();
        let b = read_raster(&mut c, 2, &mut seen).unwrap();
        let t = read_raster(&mut c, 3, &mut seen).unwrap();
        c.finish().unwrap();
        assert_eq!(a.pixel(3, 3), [255, 0, 0, 255]);
        assert!(Arc::ptr_eq(&a.tiles[&(0, 0)], &b.tiles[&(0, 0)]));
        assert!(Arc::ptr_eq(&a.tiles[&(0, 0)], &t.tiles[&(0, 0)]));
        assert_eq!(seen.len(), 1);
    }
}
