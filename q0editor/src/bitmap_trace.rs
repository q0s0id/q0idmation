//! Trace Bitmap: a bitmap's pixels turned into filled vector shapes.
//!
//! The picture is reduced to a few colours, patches too small to matter join
//! a neighbour, and each colour's outline is walked along the pixel edges and
//! fitted with curves. The shapes are stacked, biggest first, and each
//! colour's shape reaches a pixel or two under the colours stacked above it
//! where they meet, so two neighbours never show a crack between their
//! separately fitted curves.

use std::sync::Arc;

use q0s_format::v2::{Path as VPath, Rgba, Vec2};

/// The most colours a trace keeps. Labels fit in a byte with room for
/// [`CLEAR`].
pub const MAX_COLORS: u8 = 255;
/// The label of a pixel left out for being too transparent.
const CLEAR: u8 = u8::MAX;

/// What the Trace Bitmap dialog asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceSettings {
    /// How many colours the picture is reduced to.
    pub colors: u8,
    /// Pixels less opaque than this are left out.
    pub alpha_threshold: u8,
    /// Patches smaller than this many pixels join a neighbouring colour.
    pub min_area: u32,
    /// How far, in pixels, curves may stray from the pixel outline; 0 keeps
    /// the exact pixel edges.
    pub curve_fit: f32,
    /// Straight pixel edges at least this long meet in a sharp corner; 0
    /// rounds every corner.
    pub corner_run: u32,
}

impl Default for TraceSettings {
    fn default() -> Self {
        Self {
            colors: 8,
            alpha_threshold: 128,
            min_area: 8,
            curve_fit: 1.0,
            corner_run: 3,
        }
    }
}

/// One traced colour: its fill and outlines, in bitmap pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct TracedShape {
    pub color: Rgba,
    pub paths: Vec<VPath>,
}

/// One 15-bit colour bucket: the mean colour of its pixels, and how many.
#[derive(Debug, Clone, Copy)]
pub struct Bin {
    color: [f64; 3],
    weight: f64,
}

fn bucket(r: u8, g: u8, b: u8) -> usize {
    (usize::from(r >> 3) << 10) | (usize::from(g >> 3) << 5) | usize::from(b >> 3)
}

fn kept(alpha: u8, settings: &TraceSettings) -> bool {
    alpha >= settings.alpha_threshold.max(1)
}

/// The picture's kept pixels sorted into 15-bit colour buckets.
pub fn histogram(rgba: &[u8], settings: &TraceSettings) -> Arc<Vec<Bin>> {
    let mut count = vec![0u32; 1 << 15];
    let mut sum = vec![[0u64; 3]; 1 << 15];
    for pixel in rgba.as_chunks::<4>().0 {
        if !kept(pixel[3], settings) {
            continue;
        }
        let bin = bucket(pixel[0], pixel[1], pixel[2]);
        count[bin] += 1;
        for (total, &value) in sum[bin].iter_mut().zip(pixel) {
            *total += u64::from(value);
        }
    }
    let bins = count
        .iter()
        .zip(&sum)
        .filter(|(count, _)| **count > 0)
        .map(|(&count, sum)| {
            let weight = f64::from(count);
            Bin {
                color: sum.map(|total| total as f64 / weight),
                weight,
            }
        })
        .collect();
    Arc::new(bins)
}

/// The colours a trace uses: at most `colors`, the most used first.
pub fn palette(bins: &[Bin], colors: u8) -> Vec<Rgba> {
    let wanted = usize::from(colors.clamp(1, MAX_COLORS));
    let Some(first) = bins.iter().max_by(|a, b| a.weight.total_cmp(&b.weight)) else {
        return Vec::new();
    };
    // Seeds: the busiest bucket, then each time the bucket that is both busy
    // and far from every seed so far.
    let mut centers = vec![first.color];
    let mut gaps: Vec<f64> = bins
        .iter()
        .map(|bin| distance2(bin.color, first.color))
        .collect();
    while centers.len() < wanted {
        let Some((index, score)) = bins
            .iter()
            .zip(&gaps)
            .map(|(bin, gap)| bin.weight * gap)
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
        else {
            break;
        };
        if score <= 0.0 {
            break;
        }
        let seed = bins[index].color;
        centers.push(seed);
        for (gap, bin) in gaps.iter_mut().zip(bins) {
            *gap = gap.min(distance2(bin.color, seed));
        }
    }
    // Then k-means: each centre moves to the mean of the buckets nearest it.
    let mut weights = vec![0.0; centers.len()];
    for _ in 0..16 {
        let mut sums = vec![[0.0; 3]; centers.len()];
        weights.fill(0.0);
        for bin in bins {
            let slot = nearest(&centers, bin.color);
            for (total, value) in sums[slot].iter_mut().zip(bin.color) {
                *total += value * bin.weight;
            }
            weights[slot] += bin.weight;
        }
        let mut moved = false;
        for ((center, sum), &weight) in centers.iter_mut().zip(&sums).zip(&weights) {
            if weight > 0.0 {
                let mean = sum.map(|total| total / weight);
                moved |= distance2(mean, *center) > 0.25;
                *center = mean;
            }
        }
        if !moved {
            break;
        }
    }
    let mut ranked: Vec<([f64; 3], f64)> = centers
        .into_iter()
        .zip(weights)
        .filter(|&(_, weight)| weight > 0.0)
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked
        .into_iter()
        .map(|([r, g, b], _)| Rgba {
            r: channel(r),
            g: channel(g),
            b: channel(b),
            a: 255,
        })
        .collect()
}

fn distance2(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(a, b)| (a - b) * (a - b)).sum()
}

fn nearest(centers: &[[f64; 3]], color: [f64; 3]) -> usize {
    centers
        .iter()
        .map(|&center| distance2(center, color))
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0, |(index, _)| index)
}

fn channel(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// Each pixel's colour as an index into `palette`, or [`CLEAR`].
fn labels(rgba: &[u8], palette: &[Rgba], settings: &TraceSettings) -> Vec<u8> {
    let centers: Vec<[f64; 3]> = palette
        .iter()
        .map(|color| [color.r, color.g, color.b].map(f64::from))
        .collect();
    // Every pixel in a bucket gets the same colour, so look each bucket up once.
    let mut lookup = vec![CLEAR; 1 << 15];
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| {
            if !kept(pixel[3], settings) {
                return CLEAR;
            }
            let bin = bucket(pixel[0], pixel[1], pixel[2]);
            if lookup[bin] == CLEAR {
                let color = [pixel[0], pixel[1], pixel[2]].map(f64::from);
                lookup[bin] = nearest(&centers, color) as u8;
            }
            lookup[bin]
        })
        .collect()
}

/// Patches smaller than `min_area` pixels take on a neighbouring colour.
fn despeckle(labels: &mut [u8], width: usize, height: usize, min_area: u32) {
    if min_area <= 1 {
        return;
    }
    let min_area = min_area as usize;
    let mut patch = Vec::new();
    let mut stack = Vec::new();
    // A patch that joins another can leave that one still too small, so go
    // round again while anything changes.
    for _ in 0..4 {
        let mut seen = vec![false; labels.len()];
        let mut changed = false;
        for start in 0..labels.len() {
            if seen[start] {
                continue;
            }
            let own = labels[start];
            patch.clear();
            seen[start] = true;
            stack.push(start);
            while let Some(index) = stack.pop() {
                patch.push(index);
                for next in neighbours(index, width, height) {
                    if labels[next] == own && !seen[next] {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
            if patch.len() >= min_area {
                continue;
            }
            let mut borders = [0u32; 256];
            for &index in &patch {
                for next in neighbours(index, width, height) {
                    if labels[next] != own {
                        borders[usize::from(labels[next])] += 1;
                    }
                }
            }
            if let Some(into) = absorber(own, &borders).filter(|&into| into != own) {
                for &index in &patch {
                    labels[index] = into;
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// The colour a patch that is too small takes on, or `None` to keep it.
/// `own` is the patch's colour and `borders[label]` how many pixel edges it
/// shares with each neighbouring colour; `borders[usize::from(CLEAR)]` counts
/// edges against left-out pixels, and a patch whose `own` is `CLEAR` is a
/// pinhole in the picture.
fn absorber(own: u8, borders: &[u32; 256]) -> Option<u8> {
    // The colour it shares the most edge with, ties going to the more used
    // colour (palette labels are ranked busiest first). Left-out pixels only
    // win when nothing else touches the patch, so a stray fleck floating in a
    // transparent area is dropped while a speck on a shape's edge stays solid.
    (0..CLEAR)
        .filter(|&label| label != own && borders[usize::from(label)] > 0)
        .max_by_key(|&label| (borders[usize::from(label)], std::cmp::Reverse(label)))
        .or((own != CLEAR && borders[usize::from(CLEAR)] > 0).then_some(CLEAR))
}

/// The 4-connected neighbours of pixel `index`.
fn neighbours(index: usize, width: usize, height: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (index % width, index / width);
    [
        (x > 0, index.wrapping_sub(1)),
        (x + 1 < width, index + 1),
        (y > 0, index.wrapping_sub(width)),
        (y + 1 < height, index + width),
    ]
    .into_iter()
    .filter_map(|(inside, next)| inside.then_some(next))
}

/// Traces a `width` × `height` RGBA picture into one shape per colour,
/// bottom shape first. Paths are in the bitmap's own pixels.
pub fn trace(width: u16, height: u16, rgba: &[u8], settings: &TraceSettings) -> Vec<TracedShape> {
    let (width, height) = (usize::from(width), usize::from(height));
    if width == 0 || height == 0 || rgba.len() != width * height * 4 {
        return Vec::new();
    }
    let palette = palette(&histogram(rgba, settings), settings.colors);
    let mut labels = labels(rgba, &palette, settings);
    despeckle(&mut labels, width, height, settings.min_area);

    // The most used colour goes at the bottom, where its shape is least cut up.
    let mut area = vec![0usize; palette.len()];
    for &label in labels.iter().filter(|&&label| label != CLEAR) {
        area[usize::from(label)] += 1;
    }
    let mut order: Vec<usize> = (0..palette.len())
        .filter(|&color| area[color] > 0)
        .collect();
    order.sort_by_key(|&color| std::cmp::Reverse(area[color]));
    let mut level = vec![usize::MAX; 256];
    for (rank, &color) in order.iter().enumerate() {
        level[color] = rank;
    }
    // Each colour reaches a little way under the colours stacked above it
    // where they meet, so their separately fitted outlines never leave a
    // crack; fitting strays up to `curve_fit`, so the reach is one pixel more.
    // Reaching under every colour above instead let a middle colour's own
    // fitted edge show as a hairline between two others.
    let reach = 1 + settings.curve_fit.max(0.0).ceil() as usize;
    let mut by_rank: Vec<Vec<usize>> = vec![Vec::new(); order.len()];
    for (index, &label) in labels.iter().enumerate() {
        if label != CLEAR {
            by_rank[level[usize::from(label)]].push(index);
        }
    }
    order
        .iter()
        .enumerate()
        .filter_map(|(rank, &color)| {
            let mask = reach_under(&labels, &level, &by_rank[rank], rank, reach, width, height);
            let paths: Vec<VPath> = outlines(&mask, width, height)
                .iter()
                .filter_map(|corners| fit_outline(corners, settings))
                .collect();
            (!paths.is_empty()).then_some(TracedShape {
                color: palette[color],
                paths,
            })
        })
        .collect()
}

/// The pixels `own` of the colour ranked `rank`, and those of colours
/// stacked above it up to `reach` steps (4-connected) away from them.
fn reach_under(
    labels: &[u8],
    level: &[usize],
    own: &[usize],
    rank: usize,
    reach: usize,
    width: usize,
    height: usize,
) -> Vec<bool> {
    let above = |index: usize| {
        let label = labels[index];
        label != CLEAR && level[usize::from(label)] > rank
    };
    let mut mask = vec![false; labels.len()];
    for &index in own {
        mask[index] = true;
    }
    let mut frontier = own.to_vec();
    for _ in 0..reach {
        let mut next = Vec::new();
        for &index in &frontier {
            for neighbour in neighbours(index, width, height) {
                if !mask[neighbour] && above(neighbour) {
                    mask[neighbour] = true;
                    next.push(neighbour);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    mask
}

const RIGHT: u8 = 0;
const DOWN: u8 = 1;
const LEFT: u8 = 2;
const UP: u8 = 3;

/// Every closed outline of `mask` along the pixel edges, as the pixel
/// corners it passes, walked with the filled side on the right (y down).
fn outlines(mask: &[bool], width: usize, height: usize) -> Vec<Vec<(usize, usize)>> {
    let filled = |x: usize, y: usize| x < width && y < height && mask[y * width + x];
    let stride = width + 1;
    // Bit `d` of `exits[corner]`: an outline edge leaves that corner heading `d`.
    let mut exits = vec![0u8; stride * (height + 1)];
    for y in 0..height {
        for x in 0..width {
            if !mask[y * width + x] {
                continue;
            }
            if y == 0 || !filled(x, y - 1) {
                exits[y * stride + x] |= 1 << RIGHT;
            }
            if !filled(x + 1, y) {
                exits[y * stride + x + 1] |= 1 << DOWN;
            }
            if !filled(x, y + 1) {
                exits[(y + 1) * stride + x + 1] |= 1 << LEFT;
            }
            if x == 0 || !filled(x - 1, y) {
                exits[(y + 1) * stride + x] |= 1 << UP;
            }
        }
    }
    let mut walked = vec![0u8; exits.len()];
    let mut outlines = Vec::new();
    for start in 0..exits.len() {
        while exits[start] & !walked[start] != 0 {
            let start_heading = (exits[start] & !walked[start]).trailing_zeros() as u8;
            let (mut corner, mut heading) = (start, start_heading);
            let mut corners = Vec::new();
            loop {
                walked[corner] |= 1 << heading;
                corners.push((corner % stride, corner / stride));
                corner = match heading {
                    RIGHT => corner + 1,
                    DOWN => corner + stride,
                    LEFT => corner - 1,
                    _ => corner - stride,
                };
                heading = turn(exits[corner], heading);
                if corner == start && heading == start_heading {
                    break;
                }
            }
            outlines.push(corners);
        }
    }
    outlines
}

/// Which way an outline goes on from a corner it reached heading `heading`.
/// Right turns come first, so patches that touch only at a corner keep
/// separate outlines.
fn turn(exits: u8, heading: u8) -> u8 {
    [1, 0, 3]
        .into_iter()
        .map(|turn| (heading + turn) % 4)
        .find(|&next| exits & (1 << next) != 0)
        .unwrap_or(heading)
}

/// One outline as a closed path. Stair steps become slopes (the path runs
/// through the middle of each pixel edge), straight edges of `corner_run`
/// pixels or more meet in a sharp corner, and curves are fitted within
/// `curve_fit` pixels.
fn fit_outline(corners: &[(usize, usize)], settings: &TraceSettings) -> Option<VPath> {
    let count = corners.len();
    if count < 4 {
        return None;
    }
    let point = |index: usize| {
        let (x, y) = corners[index % count];
        Vec2::new(x as f32, y as f32)
    };
    let headings: Vec<(isize, isize)> = (0..count)
        .map(|index| {
            let (a, b) = (corners[index], corners[(index + 1) % count]);
            (b.0 as isize - a.0 as isize, b.1 as isize - a.1 as isize)
        })
        .collect();
    let turns_at = |index: usize| headings[index] != headings[(index + count - 1) % count];
    if settings.curve_fit <= 0.0 {
        // Exact pixel edges: only the corners where the outline turns.
        let turns: Vec<Vec2> = (0..count)
            .filter(|&index| turns_at(index))
            .map(point)
            .collect();
        return (turns.len() >= 3).then(|| crate::brush::linear_ring_path(&turns));
    }
    // How long the straight run each edge belongs to is.
    let first = (0..count).find(|&index| turns_at(index)).unwrap_or(0);
    let mut run = vec![0u32; count];
    let mut offset = 0;
    while offset < count {
        let start = (first + offset) % count;
        let mut length = 1;
        while offset + length < count && headings[(start + length) % count] == headings[start] {
            length += 1;
        }
        for step in 0..length {
            run[(start + step) % count] = length as u32;
        }
        offset += length;
    }
    let mut points = Vec::with_capacity(count * 2);
    for index in 0..count {
        let previous = (index + count - 1) % count;
        if settings.corner_run > 0
            && turns_at(index)
            && run[previous] >= settings.corner_run
            && run[index] >= settings.corner_run
        {
            points.push(point(index));
        }
        let (a, b) = (point(index), point(index + 1));
        points.push(Vec2::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5));
    }
    crate::brush::fit_closed_ring_path(&points, settings.curve_fit)
        .or_else(|| (points.len() >= 3).then(|| crate::brush::linear_ring_path(&points)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [220, 30, 40, 255];
    const BLUE: [u8; 4] = [30, 60, 200, 255];

    const GREEN: [u8; 4] = [40, 180, 60, 255];

    /// A picture drawn as text: `r` red, `b` blue, `.` transparent.
    fn picture(rows: &[&str]) -> (u16, u16, Vec<u8>) {
        let rgba = rows
            .iter()
            .flat_map(|row| row.chars())
            .flat_map(|pixel| match pixel {
                'r' => RED,
                'b' => BLUE,
                'g' => GREEN,
                _ => [0; 4],
            })
            .collect();
        (rows[0].len() as u16, rows.len() as u16, rgba)
    }

    /// No speck clean-up, so every pixel counts.
    fn every_pixel() -> TraceSettings {
        TraceSettings {
            min_area: 1,
            ..TraceSettings::default()
        }
    }

    fn pixel_edges() -> TraceSettings {
        TraceSettings {
            curve_fit: 0.0,
            ..every_pixel()
        }
    }

    /// The smallest box round a shape's anchor points: min x, min y, max x, max y.
    fn bounds(shape: &TracedShape) -> [f32; 4] {
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for anchor in shape.paths.iter().flat_map(|path| &path.anchors) {
            bounds[0] = bounds[0].min(anchor.point.x);
            bounds[1] = bounds[1].min(anchor.point.y);
            bounds[2] = bounds[2].max(anchor.point.x);
            bounds[3] = bounds[3].max(anchor.point.y);
        }
        bounds
    }

    fn assert_near(actual: [f32; 4], expected: [f32; 4]) {
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 1.0e-3),
            "{actual:?} is not {expected:?}"
        );
    }

    #[test]
    fn a_square_on_a_background_becomes_two_stacked_shapes() {
        let (width, height, rgba) = picture(&[
            "bbbbbbbb", "bbbbbbbb", "bbrrrrbb", "bbrrrrbb", "bbrrrrbb", "bbrrrrbb", "bbbbbbbb",
            "bbbbbbbb",
        ]);
        let shapes = trace(width, height, &rgba, &every_pixel());
        assert_eq!(shapes.len(), 2);
        // The background is at the bottom and fills under the square too.
        assert_eq!(
            shapes[0].color,
            Rgba {
                r: 30,
                g: 60,
                b: 200,
                a: 255
            }
        );
        assert_eq!(shapes[0].paths.len(), 1);
        assert_near(bounds(&shapes[0]), [0.0, 0.0, 8.0, 8.0]);
        assert_eq!(
            shapes[1].color,
            Rgba {
                r: 220,
                g: 30,
                b: 40,
                a: 255
            }
        );
        assert_near(bounds(&shapes[1]), [2.0, 2.0, 6.0, 6.0]);
    }

    #[test]
    fn a_middle_colour_stays_away_from_a_border_between_two_others() {
        // Blue is the most used, then green (left), then red (right): red
        // meets blue far from any green.
        let rows = [
            "bbbbbbbbbbbbbbbb",
            "bgggbbbbbbbbbbbb",
            "bgggbbbbbbbrrrbb",
            "bgggbbbbbbbrrrbb",
            "bbbbbbbbbbbbbbbb",
        ];
        let (width, height, rgba) = picture(&rows);
        let shapes = trace(width, height, &rgba, &pixel_edges());
        assert_eq!(shapes.len(), 3);
        let green = bounds(&shapes[1]);
        assert!(
            green[2] <= 5.0,
            "green reached across to red's border: {green:?}"
        );
    }

    #[test]
    fn transparent_pixels_are_left_out() {
        let (width, height, rgba) = picture(&["..rr", "..rr", "..rr", "..rr"]);
        let shapes = trace(width, height, &rgba, &pixel_edges());
        assert_eq!(shapes.len(), 1);
        assert_near(bounds(&shapes[0]), [2.0, 0.0, 4.0, 4.0]);
    }

    #[test]
    fn a_speck_joins_the_colour_round_it() {
        let (width, height, rgba) = picture(&["bbbb", "brbb", "bbbb", "bbbb"]);
        let settings = TraceSettings {
            min_area: 2,
            ..TraceSettings::default()
        };
        let shapes = trace(width, height, &rgba, &settings);
        assert_eq!(shapes.len(), 1, "the red speck should have turned blue");
        assert_eq!(shapes[0].color.b, 200);
    }

    #[test]
    fn pixels_touching_only_at_a_corner_get_separate_outlines() {
        let (width, height, rgba) = picture(&["r.", ".r"]);
        let shapes = trace(width, height, &rgba, &pixel_edges());
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].paths.len(), 2);
    }

    #[test]
    fn a_hole_stays_open() {
        let (width, height, rgba) = picture(&["rrr", "r.r", "rrr"]);
        let shapes = trace(width, height, &rgba, &pixel_edges());
        assert_eq!(shapes[0].paths.len(), 2, "the outside and the hole");
    }

    #[test]
    fn a_pinhole_smaller_than_the_smallest_patch_is_filled() {
        let (width, height, rgba) = picture(&["rrr", "r.r", "rrr"]);
        let settings = TraceSettings {
            min_area: 2,
            ..pixel_edges()
        };
        let shapes = trace(width, height, &rgba, &settings);
        assert_eq!(shapes[0].paths.len(), 1, "no hole left");
    }

    #[test]
    fn a_speck_with_only_transparency_round_it_is_dropped() {
        let (width, height, rgba) = picture(&["...", ".r.", "..."]);
        let settings = TraceSettings {
            min_area: 2,
            ..pixel_edges()
        };
        assert!(trace(width, height, &rgba, &settings).is_empty());
    }

    #[test]
    fn curve_fit_zero_keeps_the_exact_pixel_edges() {
        let (width, height, rgba) = picture(&["r..", "r..", "rrr"]);
        let shapes = trace(width, height, &rgba, &pixel_edges());
        let path = &shapes[0].paths[0];
        assert_eq!(path.anchors.len(), 6, "an L has six corners");
        assert!(path.anchors.iter().all(|anchor| anchor.in_handle.is_none()
            && anchor.point.x.fract() == 0.0
            && anchor.point.y.fract() == 0.0));
    }
}
