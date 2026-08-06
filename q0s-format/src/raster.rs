//! Software rasteriser for `ProjectV2` РІР‚вЂќ turns a single frame of a q0rg
//! into a flat RGBA buffer. Shared between:
//!   * q0editor's `.q0s` export path (rasterise + ship if you want a v1
//!     bitmap-only `.q0s`, or skip rasterisation entirely for v2 vector
//!     `.q0s`),
//!   * q0player's playback loop (when fed a v2 vector `.q0s`, rasterise
//!     each frame on demand instead of blitting pre-baked bitmaps).
//!
//! Dependency-free (`std` only) so the player doesn't pick up GUI deps.
//!
//! Two fill rules:
//!   * `EvenOdd` for closed filled paths (Flash convention).
//!   * `NonZero` for stroke outlines (`brush_outline` self-intersects on
//!     sharp turns; even-odd would punch holes).
//!
//! `rasterize_q0rg_frame` does optional supersampling: pass `ss > 1` and
//! the function rasterises at `ss Р“вЂ” ss` and box-filters down. Caller
//! controls quality/cost trade-off РІР‚вЂќ players default to `1` (live), the
//! exporter could use `2`.

use crate::geom::{brush_outline, flatten_path};
use crate::transform::Affine;
use crate::v2::{
    Asset, BitmapAsset, ProjectV2, Rgba, Target, Transform2D, Vec2, VectorAsset,
    MAX_Q0RG_NESTING_DEPTH,
};

/// Render one frame of `q0rg_id` (resolved against `project`) into an
/// RGBA8 buffer of size `w Р“вЂ” h`. `ss` controls supersample factor:
///   * `ss = 1`: no AA, fast РІР‚вЂќ fine for live playback.
///   * `ss = 2`: ~4Р“вЂ” slower, smooth diagonals РІР‚вЂќ sane default for export.
///   * `ss >= 3`: diminishing returns, lots of memory.
///
/// `bg_rgba` is the canvas clear colour. Use `[255, 255, 255, 255]` to
/// match the editor's stage-paper white.
pub fn rasterize_q0rg_frame(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    w: u32,
    h: u32,
    ss: u32,
    bg_rgba: [u8; 4],
) -> Vec<u8> {
    rasterize_q0rg_frame_with_root_transform(
        project,
        q0rg_id,
        frame,
        w,
        h,
        ss,
        bg_rgba,
        Affine::IDENTITY,
    )
}

/// Render a project frame while mapping logical stage coordinates into an
/// arbitrary output size. This is the canonical offscreen export path: the
/// q0editor viewport is not involved, and q0player keeps using the identity
/// wrapper above for native-size playback.
pub fn rasterize_q0rg_frame_scaled(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    w: u32,
    h: u32,
    ss: u32,
    bg_rgba: [u8; 4],
) -> Vec<u8> {
    let stage_w = f32::from(project.meta.stage_width.max(1));
    let stage_h = f32::from(project.meta.stage_height.max(1));
    let root = Affine::from_scale(w as f32 / stage_w, h as f32 / stage_h);
    rasterize_q0rg_frame_with_root_transform(project, q0rg_id, frame, w, h, ss, bg_rgba, root)
}

#[allow(clippy::too_many_arguments)]
fn rasterize_q0rg_frame_with_root_transform(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    w: u32,
    h: u32,
    ss: u32,
    bg_rgba: [u8; 4],
    root: Affine,
) -> Vec<u8> {
    let ss = ss.max(1);
    let sw = w.saturating_mul(ss);
    let sh = h.saturating_mul(ss);
    let mut hi = vec![0u8; (sw as usize).saturating_mul(sh as usize).saturating_mul(4)];
    for px in hi.chunks_exact_mut(4) {
        px.copy_from_slice(&bg_rgba);
    }
    render_to_buffer(
        project,
        q0rg_id,
        frame,
        Affine::compose(Affine::from_uniform_scale(ss as f32), root),
        &mut hi,
        sw,
        sh,
        0,
    );
    if ss == 1 {
        return hi;
    }
    downscale_box(&hi, sw, w, h, ss)
}

fn downscale_box(src: &[u8], sw: u32, dw: u32, dh: u32, ss: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw as usize).saturating_mul(dh as usize).saturating_mul(4)];
    let s = ss as usize;
    let n = (s * s) as u64;
    for y in 0..dh as usize {
        for x in 0..dw as usize {
            let mut premul_r = 0u64;
            let mut premul_g = 0u64;
            let mut premul_b = 0u64;
            let mut alpha = 0u64;
            for dy in 0..s {
                for dx in 0..s {
                    let sx = x * s + dx;
                    let sy = y * s + dy;
                    let idx = (sy * sw as usize + sx) * 4;
                    let a = u64::from(src[idx + 3]);
                    premul_r += u64::from(src[idx]) * a;
                    premul_g += u64::from(src[idx + 1]) * a;
                    premul_b += u64::from(src[idx + 2]) * a;
                    alpha += a;
                }
            }
            let oi = (y * dw as usize + x) * 4;
            match std::num::NonZeroU64::new(alpha) {
                None => out[oi..oi + 4].copy_from_slice(&[0, 0, 0, 0]),
                Some(nonzero_alpha) => {
                    let alpha = nonzero_alpha.get();
                    out[oi] = ((premul_r + alpha / 2) / alpha) as u8;
                    out[oi + 1] = ((premul_g + alpha / 2) / alpha) as u8;
                    out[oi + 2] = ((premul_b + alpha / 2) / alpha) as u8;
                    out[oi + 3] = ((alpha + n / 2) / n) as u8;
                }
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn render_to_buffer(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    parent: Affine,
    buffer: &mut [u8],
    w: u32,
    h: u32,
    depth: u8,
) {
    if depth > MAX_Q0RG_NESTING_DEPTH {
        return;
    }
    let Some(q) = project.q0rgs.iter().find(|q| q.q0rg_id == q0rg_id) else {
        return;
    };
    let local_frame = if q.frame_count > 0 {
        frame % q.frame_count
    } else {
        0
    };
    for layer in &q.layers {
        for (idx, interp) in active_placements_at(layer, local_frame) {
            let Some(placement) = layer.placements.get(idx) else {
                continue;
            };
            let composed = Affine::compose(parent, Affine::from_transform(interp));
            match placement.target {
                Target::Asset(id) => {
                    if let Some(asset) = project.assets.iter().find(|a| a.id() == id) {
                        rasterize_asset(
                            asset,
                            composed,
                            buffer,
                            w,
                            h,
                            local_frame.saturating_sub(placement.frame),
                            project.meta.fps,
                        );
                    }
                }
                Target::Q0rg(child_id) if child_id != q0rg_id => {
                    render_to_buffer(
                        project,
                        child_id,
                        local_frame,
                        composed,
                        buffer,
                        w,
                        h,
                        depth + 1,
                    );
                }
                _ => {}
            }
        }
    }
}

/// Return every placement from the latest complete layer keyframe at or
/// before `frame`, with tween transforms baked for the requested playhead.
pub fn active_placements_at(layer: &crate::v2::Layer, frame: u16) -> Vec<(usize, Transform2D)> {
    let Some(keyframe) = layer
        .keyframe_frames()
        .into_iter()
        .filter(|candidate| *candidate <= frame)
        .max()
    else {
        return Vec::new();
    };

    let source_indices: Vec<usize> = layer
        .placements
        .iter()
        .enumerate()
        .filter_map(|(index, placement)| (placement.frame == keyframe).then_some(index))
        .collect();

    source_indices
        .iter()
        .enumerate()
        .map(|(source_order, index)| {
            let placement = &layer.placements[*index];
            let interp = match placement.tween.to_frame() {
                Some(to_frame) if to_frame > keyframe && frame >= keyframe => {
                    let occurrence = source_indices[..source_order]
                        .iter()
                        .filter(|candidate| {
                            layer.placements[**candidate].target == placement.target
                        })
                        .count();
                    let target_transform = layer
                        .placements
                        .iter()
                        .filter(|candidate| {
                            candidate.frame == to_frame && candidate.target == placement.target
                        })
                        .nth(occurrence)
                        .map(|candidate| candidate.transform)
                        .unwrap_or(placement.transform);
                    let denominator = f32::from(to_frame - keyframe);
                    let raw_t = (f32::from(frame - keyframe) / denominator).clamp(0.0, 1.0);
                    let t = placement.tween.easing().sample(raw_t);
                    lerp_transform(placement.transform, target_transform, t)
                }
                Some(_) | None => placement.transform,
            };
            (*index, interp)
        })
        .collect()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn lerp_transform(a: Transform2D, b: Transform2D, t: f32) -> Transform2D {
    Transform2D {
        tx: lerp(a.tx, b.tx, t),
        ty: lerp(a.ty, b.ty, t),
        sx: lerp(a.sx, b.sx, t),
        sy: lerp(a.sy, b.sy, t),
        rotation: lerp(a.rotation, b.rotation, t),
        skew_x: lerp(a.skew_x, b.skew_x, t),
        skew_y: lerp(a.skew_y, b.skew_y, t),
    }
}

const FLATTEN_SAMPLES: usize = 16;

fn rasterize_asset(
    asset: &Asset,
    t: Affine,
    buffer: &mut [u8],
    w: u32,
    h: u32,
    elapsed_host_frames: u16,
    host_fps: u16,
) {
    match asset {
        Asset::Bitmap(b) => rasterize_bitmap(b, t, buffer, w, h),
        Asset::Vector(v) => rasterize_vector(v, t, buffer, w, h),
        Asset::Q0v(v) => {
            let Ok(media) = q0video::q0v::Q0vFile::parse(v.bytes.clone()) else {
                return;
            };
            let Some(frame_index) = media.spec.video_frame_for_host_frame(
                u32::from(elapsed_host_frames),
                u32::from(host_fps.max(1)),
            ) else {
                return;
            };
            let Ok(rgba) = media.decode_frame_rgba(frame_index) else {
                return;
            };
            let Ok(width) = u16::try_from(media.spec.width) else {
                return;
            };
            let Ok(height) = u16::try_from(media.spec.height) else {
                return;
            };
            rasterize_bitmap(
                &BitmapAsset {
                    asset_id: v.asset_id,
                    width,
                    height,
                    rgba,
                },
                t,
                buffer,
                w,
                h,
            );
        }
    }
}

fn rasterize_vector(v: &VectorAsset, t: Affine, buffer: &mut [u8], w: u32, h: u32) {
    if let Some(fill) = v.fill {
        let contours: Vec<Vec<Vec2>> = v
            .paths
            .iter()
            .filter(|path| path.closed)
            .map(|path| {
                flatten_path(path, FLATTEN_SAMPLES)
                    .iter()
                    .map(|point| t.apply(*point))
                    .collect()
            })
            .collect();
        scanline_fill_multi(&contours, fill, buffer, w, h);
    }
    for path in &v.paths {
        let polyline_local = flatten_path(path, FLATTEN_SAMPLES);
        if polyline_local.len() < 2 {
            continue;
        }
        let world: Vec<Vec2> = polyline_local.iter().map(|p| t.apply(*p)).collect();

        if let Some(stroke) = &v.stroke {
            let half = (stroke.width * 0.5 * t.uniform_scale()).max(0.5);
            let outline = brush_outline(&world, half, 8);
            if outline.len() >= 3 {
                scanline_fill(&outline, stroke.color, buffer, w, h);
            }
        }
    }
}

fn rasterize_bitmap(b: &BitmapAsset, t: Affine, buffer: &mut [u8], w: u32, h: u32) {
    if b.width == 0 || b.height == 0 {
        return;
    }
    let bw = b.width as f32;
    let bh = b.height as f32;
    let corners = [
        Vec2::new(0.0, 0.0),
        Vec2::new(bw, 0.0),
        Vec2::new(bw, bh),
        Vec2::new(0.0, bh),
    ];
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for c in &corners {
        let p = t.apply(*c);
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }
    let Some(inv) = t.inverse() else {
        return;
    };
    let x_start = (min_x.floor() as i32).max(0);
    let x_end = (max_x.ceil() as i32).min(w as i32 - 1);
    let y_start = (min_y.floor() as i32).max(0);
    let y_end = (max_y.ceil() as i32).min(h as i32 - 1);
    for yi in y_start..=y_end {
        for xi in x_start..=x_end {
            let local = inv.apply(Vec2::new(xi as f32 + 0.5, yi as f32 + 0.5));
            if local.x < 0.0 || local.y < 0.0 || local.x >= bw || local.y >= bh {
                continue;
            }
            let lx = local.x as u32;
            let ly = local.y as u32;
            let src = ((ly * b.width as u32 + lx) * 4) as usize;
            let dst = ((yi as u32 * w + xi as u32) * 4) as usize;
            blend_pixel(
                buffer,
                dst,
                Rgba {
                    r: b.rgba[src],
                    g: b.rgba[src + 1],
                    b: b.rgba[src + 2],
                    a: b.rgba[src + 3],
                },
            );
        }
    }
}

fn scanline_fill(polygon: &[Vec2], color: Rgba, buffer: &mut [u8], w: u32, h: u32) {
    scanline_fill_multi(&[polygon.to_vec()], color, buffer, w, h);
}

fn scanline_fill_multi(polygons: &[Vec<Vec2>], color: Rgba, buffer: &mut [u8], w: u32, h: u32) {
    if polygons.is_empty() || color.a == 0 {
        return;
    }
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for polygon in polygons {
        for p in polygon {
            if p.y < min_y {
                min_y = p.y;
            }
            if p.y > max_y {
                max_y = p.y;
            }
        }
    }
    if !min_y.is_finite() {
        return;
    }
    let y_start = (min_y.floor() as i32).max(0);
    let y_end = (max_y.ceil() as i32).min(h as i32 - 1);
    if y_start > y_end {
        return;
    }
    let mut crossings: Vec<(f32, i32)> = Vec::with_capacity(8);
    for yi in y_start..=y_end {
        let y = yi as f32 + 0.5;
        crossings.clear();
        for polygon in polygons {
            if polygon.len() < 3 {
                continue;
            }
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                let cross = (a.y <= y && b.y > y) || (b.y <= y && a.y > y);
                if !cross {
                    continue;
                }
                let denom = b.y - a.y;
                if denom.abs() < 1e-6 {
                    continue;
                }
                let t = (y - a.y) / denom;
                let x = a.x + t * (b.x - a.x);
                let sign = if b.y > a.y { 1 } else { -1 };
                crossings.push((x, sign));
            }
        }
        crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut winding = 0i32;
        let mut last_x: Option<f32> = None;
        for (x, sign) in &crossings {
            if winding != 0 {
                if let Some(lx) = last_x {
                    paint_span(buffer, w, yi, lx, *x, color);
                }
            }
            winding += sign;
            last_x = Some(*x);
        }
    }
}

fn paint_span(buffer: &mut [u8], w: u32, yi: i32, x0: f32, x1: f32, color: Rgba) {
    if yi < 0 {
        return;
    }
    let xi0 = x0.max(0.0).round() as i32;
    let xi1 = x1.min((w as i32 - 1) as f32).round() as i32;
    if xi0 > xi1 {
        return;
    }
    for xi in xi0..=xi1 {
        if xi < 0 || xi >= w as i32 {
            continue;
        }
        let dst = ((yi as u32 * w + xi as u32) * 4) as usize;
        blend_pixel(buffer, dst, color);
    }
}

fn blend_pixel(buffer: &mut [u8], idx: usize, c: Rgba) {
    if c.a == 0 {
        return;
    }
    if c.a == 255 {
        buffer[idx] = c.r;
        buffer[idx + 1] = c.g;
        buffer[idx + 2] = c.b;
        buffer[idx + 3] = 255;
        return;
    }

    // Straight-alpha source-over. The old path blended RGB as though it were
    // premultiplied, which was invisible on the editor's opaque white paper
    // but produced dark fringes in transparent PNG exports.
    let sa = u32::from(c.a);
    let da = u32::from(buffer[idx + 3]);
    let inv_sa = 255 - sa;
    let out_a_numerator = sa * 255 + da * inv_sa;
    if out_a_numerator == 0 {
        buffer[idx..idx + 4].copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    let blend_channel = |src: u8, dst: u8| {
        let numerator = u32::from(src) * sa * 255 + u32::from(dst) * da * inv_sa;
        ((numerator + out_a_numerator / 2) / out_a_numerator) as u8
    };
    buffer[idx] = blend_channel(c.r, buffer[idx]);
    buffer[idx + 1] = blend_channel(c.g, buffer[idx + 1]);
    buffer[idx + 2] = blend_channel(c.b, buffer[idx + 2]);
    buffer[idx + 3] = ((out_a_numerator + 127) / 255) as u8;
}

// (Affine moved to crate::transform РІР‚вЂќ shared with the editor / player
// renderers so q0rg parent transforms compose identically across all
// three rasterise/draw paths.)

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use crate::v2::{Layer, Placement, Tween};

    fn placement(frame: u16, asset: u16, tx: f32, tween: Tween) -> Placement {
        Placement {
            frame,
            target: Target::Asset(asset),
            transform: Transform2D {
                tx,
                ..Transform2D::IDENTITY
            },
            tween,
        }
    }

    #[test]
    fn raster_resolver_uses_complete_layer_keyframes() {
        let layer = Layer {
            layer_id: 1,
            name: "layer".into(),
            explicit_keyframes: Vec::new(),
            placements: vec![
                placement(0, 1, 0.0, Tween::None),
                placement(0, 2, 0.0, Tween::None),
                placement(4, 3, 0.0, Tween::None),
            ],
        };
        assert_eq!(
            active_placements_at(&layer, 3)
                .into_iter()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            active_placements_at(&layer, 4)
                .into_iter()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn raster_resolver_stops_at_blank_keyframe() {
        let layer = Layer {
            layer_id: 1,
            name: "layer".into(),
            explicit_keyframes: vec![2],
            placements: vec![
                placement(0, 1, 0.0, Tween::None),
                placement(5, 2, 0.0, Tween::None),
            ],
        };

        assert_eq!(active_placements_at(&layer, 1).len(), 1);
        assert!(active_placements_at(&layer, 2).is_empty());
        assert!(active_placements_at(&layer, 4).is_empty());
        assert_eq!(active_placements_at(&layer, 5).len(), 1);
    }

    #[test]
    fn raster_resolver_keeps_duplicate_instances_and_tweens_by_order() {
        let layer = Layer {
            layer_id: 1,
            name: "layer".into(),
            explicit_keyframes: Vec::new(),
            placements: vec![
                placement(0, 1, 0.0, Tween::Linear { to_frame: 10 }),
                placement(0, 1, 100.0, Tween::Linear { to_frame: 10 }),
                placement(10, 1, 10.0, Tween::None),
                placement(10, 1, 200.0, Tween::None),
            ],
        };
        let active = active_placements_at(&layer, 5);
        assert_eq!(active.len(), 2);
        assert!((active[0].1.tx - 5.0).abs() < 1.0e-5);
        assert!((active[1].1.tx - 150.0).abs() < 1.0e-5);
    }

    #[test]
    fn transparent_source_over_keeps_straight_rgb() {
        let mut pixel = vec![0, 0, 0, 0];
        blend_pixel(
            &mut pixel,
            0,
            Rgba {
                r: 255,
                g: 40,
                b: 10,
                a: 128,
            },
        );
        assert_eq!(pixel, vec![255, 40, 10, 128]);
    }

    #[test]
    fn transparent_supersampling_does_not_create_dark_fringes() {
        let src = vec![255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let downscaled = downscale_box(&src, 2, 1, 1, 2);
        assert_eq!(&downscaled[..3], &[255, 0, 0]);
        assert_eq!(downscaled[3], 64);
    }

    #[test]
    fn scaled_offscreen_render_maps_the_whole_stage_to_output() {
        use crate::v2::{BitmapAsset, ProjectMeta, Q0rg};

        let project = ProjectV2 {
            meta: ProjectMeta {
                name: "scaled".into(),
                fps: 24,
                stage_width: 2,
                stage_height: 2,
                entry_q0rg_id: 1,
            },
            assets: vec![Asset::Bitmap(BitmapAsset {
                asset_id: 1,
                width: 2,
                height: 2,
                rgba: [255, 0, 0, 255].repeat(4),
            })],
            asset_names: std::collections::HashMap::new(),
            layer_metadata: std::collections::HashMap::new(),
            q0rgs: vec![Q0rg {
                q0rg_id: 1,
                name: "stage".into(),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "art".into(),
                    explicit_keyframes: Vec::new(),
                    placements: vec![placement(0, 1, 0.0, Tween::None)],
                }],
            }],
        };

        let frame = rasterize_q0rg_frame_scaled(&project, 1, 0, 4, 4, 1, [0, 0, 0, 0]);
        assert_eq!(frame.len(), 4 * 4 * 4);
        assert_eq!(&frame[((3 * 4 + 3) * 4)..][..4], &[255, 0, 0, 255]);
    }
}
