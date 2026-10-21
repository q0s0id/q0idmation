//! The outline of a liquid: every particle spreads a soft blob, and the
//! outline is where the blobs add up to a threshold (metaballs), traced by
//! marching squares. Drops merge smoothly into pools and split off again.

use std::collections::HashMap;

/// The blobs' level where the surface is drawn.
const LEVEL: f32 = 0.45;
/// The field grid never grows past this many cells; a wider spread makes the
/// cells bigger instead.
const MAX_CELLS: usize = 1_500_000;

/// Closed outlines around `points`, each a list of stage points. Outer
/// outlines and holes wind opposite ways, so either fill rule fills them
/// right.
pub fn liquid_outline(points: &[[f32; 2]], spacing: f32, smoothness: f32) -> Vec<Vec<[f32; 2]>> {
    if points.is_empty() {
        return Vec::new();
    }
    let radius = spacing.max(0.5) * 1.55 * smoothness.clamp(0.3, 4.0);
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for point in points
        .iter()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
    {
        for axis in 0..2 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    if !min[0].is_finite() {
        return Vec::new();
    }
    let margin = radius * 1.1;
    let origin = [min[0] - margin, min[1] - margin];
    let extent = [
        max[0] - min[0] + margin * 2.0,
        max[1] - min[1] + margin * 2.0,
    ];
    let mut cell = (spacing * 0.5).max(0.5);
    while ((extent[0] / cell) as usize + 2) * ((extent[1] / cell) as usize + 2) > MAX_CELLS {
        cell *= 1.5;
    }
    let columns = (extent[0] / cell).ceil() as usize + 1;
    let rows = (extent[1] / cell).ceil() as usize + 1;
    let field = Field::splat(points, origin, cell, columns, rows, radius);
    let loops = field.trace();
    loops
        .into_iter()
        .map(|outline| simplify(round_off(&outline), cell * 0.12))
        .filter(|outline| outline.len() >= 3 && signed_area(outline).abs() > cell * cell)
        .collect()
}

struct Field {
    origin: [f32; 2],
    cell: f32,
    columns: usize,
    rows: usize,
    values: Vec<f32>,
}

impl Field {
    fn splat(
        points: &[[f32; 2]],
        origin: [f32; 2],
        cell: f32,
        columns: usize,
        rows: usize,
        radius: f32,
    ) -> Self {
        let mut values = vec![0.0; columns * rows];
        let reach = (radius / cell).ceil() as isize;
        let inverse = 1.0 / (radius * radius);
        for point in points
            .iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
        {
            let cx = ((point[0] - origin[0]) / cell).round() as isize;
            let cy = ((point[1] - origin[1]) / cell).round() as isize;
            for y in (cy - reach).max(0)..=(cy + reach).min(rows as isize - 1) {
                let dy = origin[1] + y as f32 * cell - point[1];
                for x in (cx - reach).max(0)..=(cx + reach).min(columns as isize - 1) {
                    let dx = origin[0] + x as f32 * cell - point[0];
                    let falloff = 1.0 - (dx * dx + dy * dy) * inverse;
                    if falloff > 0.0 {
                        values[y as usize * columns + x as usize] += falloff * falloff;
                    }
                }
            }
        }
        Self {
            origin,
            cell,
            columns,
            rows,
            values,
        }
    }

    fn value(&self, x: usize, y: usize) -> f32 {
        self.values[y * self.columns + x]
    }

    fn corner(&self, x: usize, y: usize) -> [f32; 2] {
        [
            self.origin[0] + x as f32 * self.cell,
            self.origin[1] + y as f32 * self.cell,
        ]
    }

    /// Where the surface crosses the edge between two grid corners.
    fn crossing(&self, a: (usize, usize), b: (usize, usize)) -> [f32; 2] {
        let va = self.value(a.0, a.1);
        let vb = self.value(b.0, b.1);
        let t = if (vb - va).abs() > 1.0e-9 {
            ((LEVEL - va) / (vb - va)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let pa = self.corner(a.0, a.1);
        let pb = self.corner(b.0, b.1);
        [pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t]
    }

    /// Marching squares. Each cell's corners are walked clockwise; the
    /// surface enters where the walk goes from outside to inside and leaves
    /// where it goes back out, so every segment keeps the liquid on the same
    /// side and the segments chain into closed loops.
    fn trace(&self) -> Vec<Vec<[f32; 2]>> {
        // Edge ids: 0 horizontal from corner (x, y), 1 vertical from it.
        let edge = |x: usize, y: usize, vertical: bool| -> u64 {
            ((y as u64 * self.columns as u64 + x as u64) << 1) | u64::from(vertical)
        };
        let mut next = HashMap::<u64, (u64, [f32; 2])>::new();
        for y in 0..self.rows - 1 {
            for x in 0..self.columns - 1 {
                // Corners clockwise from the top left, and the edge after each.
                let corners = [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)];
                let inside = corners.map(|(cx, cy)| self.value(cx, cy) >= LEVEL);
                if inside.iter().all(|i| *i) || inside.iter().all(|i| !*i) {
                    continue;
                }
                let edges = [
                    edge(x, y, false),
                    edge(x + 1, y, true),
                    edge(x, y + 1, false),
                    edge(x, y, true),
                ];
                let mut entries = Vec::with_capacity(2);
                let mut exits = Vec::with_capacity(2);
                for side in 0..4 {
                    let from = inside[side];
                    let to = inside[(side + 1) % 4];
                    if from != to {
                        let point = self.crossing(corners[side], corners[(side + 1) % 4]);
                        if to {
                            entries.push((side, edges[side], point));
                        } else {
                            exits.push((side, edges[side], point));
                        }
                    }
                }
                if entries.len() == 1 {
                    let (_, into, at) = entries[0];
                    let (_, out, _) = exits[0];
                    next.insert(into, (out, at));
                    continue;
                }
                // A saddle: two inside corners facing each other. The middle
                // decides whether they join through the cell or stay apart.
                let middle = corners
                    .iter()
                    .map(|(cx, cy)| self.value(*cx, *cy))
                    .sum::<f32>()
                    * 0.25;
                let joined = middle >= LEVEL;
                for &(side, into, at) in &entries {
                    let exit = if joined {
                        exits.iter().find(|(s, _, _)| *s == (side + 3) % 4)
                    } else {
                        exits.iter().find(|(s, _, _)| *s == (side + 1) % 4)
                    }
                    .or_else(|| exits.first());
                    if let Some(&(_, out, _)) = exit {
                        next.insert(into, (out, at));
                    }
                }
            }
        }
        let mut loops = Vec::new();
        while let Some(&start) = next.keys().next() {
            let mut outline = Vec::new();
            let mut at = start;
            while let Some((out, point)) = next.remove(&at) {
                outline.push(point);
                at = out;
                if at == start {
                    break;
                }
            }
            if outline.len() >= 3 {
                loops.push(outline);
            }
        }
        loops
    }
}

/// One pass of Chaikin's corner cutting: the stair steps of the grid turn
/// into rounded curves.
fn round_off(outline: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut rounded = Vec::with_capacity(outline.len() * 2);
    for (index, a) in outline.iter().enumerate() {
        let b = outline[(index + 1) % outline.len()];
        rounded.push([a[0] * 0.75 + b[0] * 0.25, a[1] * 0.75 + b[1] * 0.25]);
        rounded.push([a[0] * 0.25 + b[0] * 0.75, a[1] * 0.25 + b[1] * 0.75]);
    }
    rounded
}

/// Drops points that barely bend the outline.
/// A closed loop with fewer points that strays no more than `tolerance`
/// from `points`.
pub fn simplify_loop(points: &[[f32; 2]], tolerance: f32) -> Vec<[f32; 2]> {
    simplify(points.to_vec(), tolerance)
}

fn simplify(outline: Vec<[f32; 2]>, tolerance: f32) -> Vec<[f32; 2]> {
    if outline.len() <= 8 {
        return outline;
    }
    // Douglas-Peucker on a closed loop: split it at the point farthest from
    // the first one and simplify both halves, so no stretch strays more than
    // `tolerance` from the curve it replaces.
    let far = (1..outline.len())
        .max_by(|a, b| {
            let da = distance_squared(outline[0], outline[*a]);
            let db = distance_squared(outline[0], outline[*b]);
            da.total_cmp(&db)
        })
        .unwrap_or(outline.len() / 2);
    let mut keep = vec![false; outline.len()];
    keep[0] = true;
    keep[far] = true;
    let closed = outline
        .iter()
        .copied()
        .chain(std::iter::once(outline[0]))
        .collect::<Vec<_>>();
    let mut stack = vec![(0, far), (far, outline.len())];
    while let Some((from, to)) = stack.pop() {
        if to <= from + 1 {
            continue;
        }
        let (a, b) = (closed[from], closed[to]);
        let (worst, off) = (from + 1..to)
            .map(|index| (index, off_line(closed[index], a, b)))
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .expect("a point between");
        if off > tolerance {
            keep[worst] = true;
            stack.push((from, worst));
            stack.push((worst, to));
        }
    }
    outline
        .into_iter()
        .zip(keep)
        .filter_map(|(point, kept)| kept.then_some(point))
        .collect()
}

fn distance_squared(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

/// How far `point` is from the segment `a`-`b`.
fn off_line(point: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let line = [b[0] - a[0], b[1] - a[1]];
    let length = line[0].hypot(line[1]);
    if length <= 1.0e-6 {
        return distance_squared(point, a).sqrt();
    }
    ((point[0] - a[0]) * line[1] - (point[1] - a[1]) * line[0]).abs() / length
}

fn signed_area(outline: &[[f32; 2]]) -> f32 {
    let mut area = 0.0;
    for (index, a) in outline.iter().enumerate() {
        let b = outline[(index + 1) % outline.len()];
        area += a[0] * b[1] - b[0] * a[1];
    }
    area * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_particle_draws_one_round_drop() {
        let outlines = liquid_outline(&[[50.0, 50.0]], 8.0, 1.0);
        assert_eq!(outlines.len(), 1);
        let radius = outlines[0]
            .iter()
            .map(|p| (p[0] - 50.0).hypot(p[1] - 50.0))
            .fold(0.0_f32, f32::max);
        assert!((5.0..12.0).contains(&radius), "drop radius {radius}");
    }

    #[test]
    fn far_drops_stay_apart_and_close_ones_merge() {
        let apart = liquid_outline(&[[0.0, 0.0], [100.0, 0.0]], 8.0, 1.0);
        assert_eq!(apart.len(), 2);
        let close = liquid_outline(&[[0.0, 0.0], [8.0, 0.0], [16.0, 0.0]], 8.0, 1.0);
        assert_eq!(close.len(), 1);
    }

    #[test]
    fn a_ring_of_liquid_has_a_hole_winding_the_other_way() {
        let mut points = Vec::new();
        for step in 0..80 {
            let angle = step as f32 / 80.0 * std::f32::consts::TAU;
            for radius in [40.0, 46.0, 52.0] {
                points.push([radius * angle.cos(), radius * angle.sin()]);
            }
        }
        let outlines = liquid_outline(&points, 6.0, 1.0);
        assert_eq!(outlines.len(), 2, "outside and hole");
        let areas = outlines.iter().map(|o| signed_area(o)).collect::<Vec<_>>();
        assert!(areas[0].signum() != areas[1].signum(), "areas {areas:?}");
    }
}
