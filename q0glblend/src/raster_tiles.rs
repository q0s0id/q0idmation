//! Raster drawings on the GPU, for the editor's stage and the player.
//!
//! A drawing is cut into 512 px chunks, each its own texture with a one
//! pixel apron copied from its neighbours, so linear filtering never shows
//! a seam where two chunks meet. Zoomed out, a chunk is drawn from a smaller
//! copy (halved up to three times, box filtered on the CPU), so thin lines
//! stay smooth instead of sparkling.
//!
//! Textures follow the tiles' stamps: when a stroke changes a tile, only that
//! tile's pixels go up again, into the chunks and levels that show it.

use std::collections::HashMap;

use egui::epaint::{Mesh, Vertex};
use egui::{pos2, Color32, ColorImage, Context, Pos2, Rect, TextureHandle, TextureOptions};
use q0s_format::pixels::{PixelRect, RasterAsset, TILE_I32};
use q0s_format::v2::Vec2;

/// Side of a chunk, in drawing pixels.
const CHUNK: i32 = 512;
/// Smaller copies kept for zoomed-out views.
const LEVELS: usize = 4;

/// The textures of every raster drawing drawn lately.
pub struct RasterTextures {
    drawings: HashMap<u16, DrawingTextures>,
    held: usize,
    budget: usize,
    name: &'static str,
}

struct DrawingTextures {
    fingerprint: u64,
    chunks: HashMap<(i32, i32), Chunk>,
    used: u64,
}

#[derive(Default)]
struct Chunk {
    levels: [Option<Level>; LEVELS],
}

struct Level {
    texture: TextureHandle,
    /// The drawing's fingerprint when this level last caught up with it.
    synced: u64,
    /// The stamp of every tile as it was uploaded, 0 for none.
    stamps: HashMap<(i16, i16), u64>,
    bytes: usize,
}

impl Default for RasterTextures {
    fn default() -> Self {
        // Phones end an app that holds much more.
        let budget = if cfg!(target_os = "android") {
            192 << 20
        } else {
            768 << 20
        };
        Self::new("q0_raster", budget)
    }
}

impl RasterTextures {
    /// `budget` is how many bytes of textures to keep before forgetting the
    /// drawings drawn longest ago.
    pub fn new(name: &'static str, budget: usize) -> Self {
        Self {
            drawings: HashMap::new(),
            held: 0,
            budget,
            name,
        }
    }

    /// Bytes of texture held.
    pub fn held_bytes(&self) -> usize {
        self.held
    }

    pub fn clear(&mut self) {
        self.drawings.clear();
        self.held = 0;
    }

    pub fn forget(&mut self, asset_id: u16) {
        if let Some(drawing) = self.drawings.remove(&asset_id) {
            self.held = self.held.saturating_sub(drawing.bytes());
        }
    }

    /// Meshes that draw `raster`, its space mapped to the screen by
    /// `to_screen`, at `screen_scale` screen pixels per drawing pixel.
    /// `clip` skips chunks that are off screen.
    pub fn meshes(
        &mut self,
        ctx: &Context,
        raster: &RasterAsset,
        to_screen: impl Fn(Vec2) -> Pos2,
        screen_scale: f32,
        tint: Color32,
        clip: Option<Rect>,
    ) -> Vec<Mesh> {
        let frame = ctx.frame_nr();
        let level = level_for(screen_scale);
        let fingerprint = raster.fingerprint();
        let name = self.name;
        let mut freed = 0;
        let drawing = self
            .drawings
            .entry(raster.asset_id)
            .or_insert_with(|| DrawingTextures {
                fingerprint: 0,
                chunks: HashMap::new(),
                used: frame,
            });
        drawing.used = frame;
        let wanted = chunks_of(raster);
        if drawing.fingerprint != fingerprint {
            drawing.chunks.retain(|key, chunk| {
                let keep = wanted.contains(key);
                if !keep {
                    freed += chunk.bytes();
                }
                keep
            });
        }
        let mut grown = 0;
        let mut meshes = Vec::new();
        for key in &wanted {
            let corners = chunk_corners(*key).map(&to_screen);
            if let Some(clip) = clip {
                let bounds = Rect::from_points(&corners);
                if !bounds.intersects(clip) {
                    continue;
                }
            }
            let chunk = drawing.chunks.entry(*key).or_default();
            let (texture, added) = chunk.level(ctx, name, raster, *key, level, fingerprint);
            grown += added;
            meshes.push(chunk_mesh(texture, corners, level, tint));
        }
        drawing.fingerprint = fingerprint;
        self.held = (self.held + grown).saturating_sub(freed);
        self.trim(frame);
        meshes
    }

    /// Forgets the drawings drawn longest ago while over budget. Nothing
    /// drawn this frame goes.
    fn trim(&mut self, frame: u64) {
        while self.held > self.budget {
            let Some((&oldest, _)) = self
                .drawings
                .iter()
                .filter(|(_, drawing)| drawing.used < frame)
                .min_by_key(|(_, drawing)| drawing.used)
            else {
                break;
            };
            self.forget(oldest);
        }
    }
}

impl DrawingTextures {
    fn bytes(&self) -> usize {
        self.chunks.values().map(Chunk::bytes).sum()
    }
}

impl Chunk {
    fn bytes(&self) -> usize {
        self.levels.iter().flatten().map(|level| level.bytes).sum()
    }

    /// This chunk's texture at `level`, made or brought up to date with the
    /// drawing whose fingerprint is `fingerprint`. Returns it with the bytes
    /// it added.
    fn level(
        &mut self,
        ctx: &Context,
        name: &str,
        raster: &RasterAsset,
        key: (i32, i32),
        level: usize,
        fingerprint: u64,
    ) -> (&TextureHandle, usize) {
        let region = level_region(key, level);
        let tiles = tiles_seen(key);
        let mut added = 0;
        match &mut self.levels[level] {
            Some(existing) => {
                if existing.synced != fingerprint {
                    existing.synced = fingerprint;
                    for (tx, ty) in tiles {
                        let stamp = tile_stamp(raster, tx, ty);
                        let known = existing.stamps.get(&(tx, ty)).copied().unwrap_or(0);
                        if stamp == known {
                            continue;
                        }
                        // The tile's pixels at this level, inside the chunk.
                        let step = 1 << level;
                        let tile = PixelRect::new(
                            i32::from(tx) * TILE_I32 / step,
                            i32::from(ty) * TILE_I32 / step,
                            (i32::from(tx) + 1) * TILE_I32 / step,
                            (i32::from(ty) + 1) * TILE_I32 / step,
                        );
                        let part = tile.intersect(region);
                        if !part.is_empty() {
                            let image = level_pixels(raster, part, level);
                            existing.texture.set_partial(
                                [
                                    (part.x0 - region.x0) as usize,
                                    (part.y0 - region.y0) as usize,
                                ],
                                image,
                                TextureOptions::LINEAR,
                            );
                        }
                        if stamp == 0 {
                            existing.stamps.remove(&(tx, ty));
                        } else {
                            existing.stamps.insert((tx, ty), stamp);
                        }
                    }
                }
            }
            slot @ None => {
                let image = level_pixels(raster, region, level);
                let bytes = image.pixels.len() * 4;
                let texture = ctx.load_texture(
                    format!("{name}_{}_{}_{}_{}", raster.asset_id, key.0, key.1, level),
                    image,
                    TextureOptions::LINEAR,
                );
                let stamps = tiles
                    .into_iter()
                    .filter_map(|(tx, ty)| {
                        let stamp = tile_stamp(raster, tx, ty);
                        (stamp != 0).then_some(((tx, ty), stamp))
                    })
                    .collect();
                added += bytes;
                *slot = Some(Level {
                    texture,
                    synced: fingerprint,
                    stamps,
                    bytes,
                });
            }
        }
        let texture = &self.levels[level]
            .as_ref()
            .expect("level was just made")
            .texture;
        (texture, added)
    }
}

/// Which copy suits `screen_scale` screen pixels per drawing pixel.
fn level_for(screen_scale: f32) -> usize {
    if !screen_scale.is_finite() || screen_scale >= 0.75 {
        return 0;
    }
    let mut level = 0;
    let mut scale = screen_scale;
    while scale < 0.75 && level + 1 < LEVELS {
        scale *= 2.0;
        level += 1;
    }
    level
}

fn tile_stamp(raster: &RasterAsset, tx: i16, ty: i16) -> u64 {
    raster.tiles.get(&(tx, ty)).map_or(0, |tile| tile.stamp())
}

/// Chunks holding any of the drawing's tiles.
fn chunks_of(raster: &RasterAsset) -> Vec<(i32, i32)> {
    let per = CHUNK / TILE_I32;
    let mut chunks: Vec<(i32, i32)> = raster
        .tiles
        .keys()
        .map(|&(tx, ty)| (i32::from(tx).div_euclid(per), i32::from(ty).div_euclid(per)))
        .collect();
    chunks.sort_unstable();
    chunks.dedup();
    chunks
}

/// Tiles whose pixels a chunk's texture shows, apron included.
fn tiles_seen(key: (i32, i32)) -> Vec<(i16, i16)> {
    let per = CHUNK / TILE_I32;
    let mut tiles = Vec::with_capacity(((per + 2) * (per + 2)) as usize);
    for ty in key.1 * per - 1..=(key.1 + 1) * per {
        for tx in key.0 * per - 1..=(key.0 + 1) * per {
            if let (Ok(x), Ok(y)) = (i16::try_from(tx), i16::try_from(ty)) {
                tiles.push((x, y));
            }
        }
    }
    tiles
}

/// The pixels a chunk's texture covers at `level`, in that level's pixels:
/// the chunk plus one pixel all round.
fn level_region(key: (i32, i32), level: usize) -> PixelRect {
    let side = CHUNK >> level;
    PixelRect::new(
        key.0 * side - 1,
        key.1 * side - 1,
        (key.0 + 1) * side + 1,
        (key.1 + 1) * side + 1,
    )
}

/// The chunk's corners in drawing space, clockwise from the top left.
fn chunk_corners(key: (i32, i32)) -> [Vec2; 4] {
    let x0 = (key.0 * CHUNK) as f32;
    let y0 = (key.1 * CHUNK) as f32;
    let x1 = x0 + CHUNK as f32;
    let y1 = y0 + CHUNK as f32;
    [
        Vec2::new(x0, y0),
        Vec2::new(x1, y0),
        Vec2::new(x1, y1),
        Vec2::new(x0, y1),
    ]
}

/// The drawing's pixels in `rect` (in `level`'s pixels), each the average
/// of the 2^level square of full-size pixels under it.
fn level_pixels(raster: &RasterAsset, rect: PixelRect, level: usize) -> ColorImage {
    let width = rect.width() as usize;
    let height = rect.height() as usize;
    if level == 0 {
        let rgba = raster.compose(rect);
        return ColorImage::from_rgba_premultiplied([width, height], &rgba);
    }
    let step = 1_i32 << level;
    let full = PixelRect::new(
        rect.x0 * step,
        rect.y0 * step,
        rect.x1 * step,
        rect.y1 * step,
    );
    let source = raster.compose(full);
    let source_width = full.width() as usize;
    let count = (step * step) as u32;
    let step = step as usize;
    let mut rgba = vec![0_u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0_u32; 4];
            for sy in 0..step {
                let row = (y * step + sy) * source_width;
                for sx in 0..step {
                    let i = (row + x * step + sx) * 4;
                    for c in 0..4 {
                        sum[c] += u32::from(source[i + c]);
                    }
                }
            }
            let o = (y * width + x) * 4;
            for c in 0..4 {
                rgba[o + c] = ((sum[c] + count / 2) / count) as u8;
            }
        }
    }
    ColorImage::from_rgba_premultiplied([width, height], &rgba)
}

fn chunk_mesh(texture: &TextureHandle, corners: [Pos2; 4], level: usize, tint: Color32) -> Mesh {
    // Inset by the apron, so the quad shows exactly the chunk.
    let side = ((CHUNK >> level) + 2) as f32;
    let (lo, hi) = (1.0 / side, (side - 1.0) / side);
    let uvs = [pos2(lo, lo), pos2(hi, lo), pos2(hi, hi), pos2(lo, hi)];
    let mut mesh = Mesh::with_texture(texture.id());
    for (pos, uv) in corners.into_iter().zip(uvs) {
        mesh.vertices.push(Vertex {
            pos,
            uv,
            color: tint,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(raster: &mut RasterAsset, x: i32, y: i32) {
        raster.put(PixelRect::new(x, y, x + 1, y + 1), &[255, 255, 255, 255]);
    }

    #[test]
    fn levels_follow_the_zoom() {
        assert_eq!(level_for(4.0), 0);
        assert_eq!(level_for(0.8), 0);
        assert_eq!(level_for(0.5), 1);
        assert_eq!(level_for(0.3), 2);
        assert_eq!(level_for(0.01), LEVELS - 1);
    }

    #[test]
    fn chunks_cover_the_tiles_and_the_apron_sees_neighbours() {
        let mut raster = RasterAsset::new(1);
        dot(&mut raster, -1, 0);
        dot(&mut raster, 600, 10);
        assert_eq!(chunks_of(&raster), vec![(-1, 0), (1, 0)]);
        let region = level_region((1, 0), 0);
        assert_eq!(region, PixelRect::new(511, -1, 1025, 513));
        // Its texture shows the left neighbour's last column, so it must
        // follow the tiles there too.
        assert!(tiles_seen((1, 0)).contains(&(7, 0)));
    }

    #[test]
    fn a_level_averages_the_pixels_under_it() {
        let mut raster = RasterAsset::new(1);
        dot(&mut raster, 0, 0);
        let image = level_pixels(&raster, PixelRect::new(0, 0, 1, 1), 1);
        assert_eq!(
            image.pixels[0],
            Color32::from_rgba_premultiplied(64, 64, 64, 64)
        );
    }

    #[test]
    fn stage_textures_are_made_updated_and_trimmed() {
        let ctx = Context::default();
        let mut textures = RasterTextures::new("test_raster", 1);
        let mut raster = RasterAsset::new(1);
        dot(&mut raster, 3, 3);
        let to_screen = |p: Vec2| pos2(p.x, p.y);
        let meshes = textures.meshes(&ctx, &raster, to_screen, 1.0, Color32::WHITE, None);
        assert_eq!(meshes.len(), 1);
        assert!(textures.held_bytes() >= 514 * 514 * 4);
        // A change goes up as a partial update: the texture stays.
        dot(&mut raster, 4, 3);
        let again = textures.meshes(&ctx, &raster, to_screen, 1.0, Color32::WHITE, None);
        assert_eq!(again[0].texture_id, meshes[0].texture_id);
        // Erased away: nothing to draw, nothing held.
        let mut empty = raster.clone();
        empty.put(PixelRect::new(0, 0, 8, 8), &[0; 8 * 8 * 4]);
        assert!(empty.is_empty());
        let none = textures.meshes(&ctx, &empty, to_screen, 1.0, Color32::WHITE, None);
        assert!(none.is_empty());
        assert_eq!(textures.held_bytes(), 0);
    }
}
