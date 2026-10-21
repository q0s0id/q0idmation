//! Path geometry helpers shared between the editor (drawing tools), the
//! exporter (rasterising stroked paths to filled polygons), and the player
//! (rendering vector frames at playback time).
//!
//! Kept dependency-free so it can be reused without dragging eframe/egui
//! into the player.

use crate::v2::Vec2;

/// Cap shape applied to both ends of a stroked polyline when wrapping
/// it into a closed `brush_outline` polygon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapShape {
    /// Half-circle smooth caps. Looks like a felt-tip pen.
    Round,
    /// Flat cut perpendicular to the tangent at each endpoint. Looks
    /// like a paper-cut, a marker on its edge, or the way Animate's
    /// "Square" cap renders.
    Butt,
    /// Flat, but reaching half the stroke's width past each end, so a line
    /// ends in a square nib.
    Square,
}

/// How a stroke turns its corners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JoinShape {
    /// Sharp points; very sharp corners are cut off like a bevel.
    Miter,
    /// Rounded corners.
    Round,
    /// Corners cut flat. Strokes saved before joins could be chosen were
    /// drawn this way.
    #[default]
    Bevel,
}

/// How long a miter may get, in half stroke widths, before the corner is
/// cut flat instead: right angles stay sharp, hairpin turns in a shaky
/// freehand line cannot grow spikes. Animate uses the same limit.
pub const MITER_LIMIT: f32 = 3.0;

/// The stroke of `points` as polygons whose union, filled with the
/// non-zero rule, is the stroked shape: one quad per segment, a piece for
/// each corner and one for each end. Every polygon winds the same way, so
/// overlaps add up instead of cancelling.
pub fn stroke_polygons(
    points: &[Vec2],
    closed: bool,
    half_width: f32,
    cap: CapShape,
    join: JoinShape,
) -> Vec<Vec<Vec2>> {
    if !half_width.is_finite() || half_width <= 0.0 {
        return Vec::new();
    }
    let mut pts: Vec<Vec2> = Vec::with_capacity(points.len());
    for point in points {
        if !point.x.is_finite() || !point.y.is_finite() {
            continue;
        }
        if pts
            .last()
            .is_none_or(|last: &Vec2| (last.x - point.x).hypot(last.y - point.y) > 1.0e-4)
        {
            pts.push(*point);
        }
    }
    let closed = closed && pts.len() >= 3;
    if closed {
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        if (first.x - last.x).hypot(first.y - last.y) <= 1.0e-4 {
            pts.pop();
        }
    }
    let round_steps = |radius: f32| ((radius * 1.2) as usize).clamp(8, 48);
    let mut polygons = Vec::new();
    if pts.len() == 1 {
        // A dot: only a round or square nib shows anything.
        let c = pts[0];
        match cap {
            CapShape::Round => polygons.push(circle(c, half_width, round_steps(half_width))),
            CapShape::Square => polygons.push(vec![
                Vec2::new(c.x - half_width, c.y - half_width),
                Vec2::new(c.x + half_width, c.y - half_width),
                Vec2::new(c.x + half_width, c.y + half_width),
                Vec2::new(c.x - half_width, c.y + half_width),
            ]),
            CapShape::Butt => {}
        }
        return polygons;
    }
    let n = pts.len();
    let segments = if closed { n } else { n - 1 };
    let direction = |i: usize| {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let length = (b.x - a.x).hypot(b.y - a.y).max(1.0e-6);
        Vec2::new((b.x - a.x) / length, (b.y - a.y) / length)
    };
    let offset = |d: Vec2| Vec2::new(-d.y * half_width, d.x * half_width);
    let add = |a: Vec2, b: Vec2| Vec2::new(a.x + b.x, a.y + b.y);
    let sub = |a: Vec2, b: Vec2| Vec2::new(a.x - b.x, a.y - b.y);
    for i in 0..segments {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let o = offset(direction(i));
        polygons.push(vec![add(a, o), add(b, o), sub(b, o), sub(a, o)]);
    }
    // Corners: every inner point, and every point of a closed path.
    let corners: Box<dyn Iterator<Item = usize>> = if closed {
        Box::new(0..n)
    } else {
        Box::new(1..n - 1)
    };
    for i in corners {
        let incoming = direction((i + n - 1) % n);
        let outgoing = direction(i);
        let turn = incoming.x * outgoing.y - incoming.y * outgoing.x;
        if turn.abs() <= 1.0e-6 && incoming.x * outgoing.x + incoming.y * outgoing.y > 0.0 {
            continue;
        }
        let c = pts[i];
        // The gap opens on the outside of the turn.
        let side = if turn > 0.0 { -1.0 } else { 1.0 };
        let o_in = offset(incoming);
        let o_out = offset(outgoing);
        let a = Vec2::new(c.x + o_in.x * side, c.y + o_in.y * side);
        let b = Vec2::new(c.x + o_out.x * side, c.y + o_out.y * side);
        match join {
            JoinShape::Bevel => polygons.push(vec![c, a, b]),
            JoinShape::Round => {
                let start = (a.y - c.y).atan2(a.x - c.x);
                let mut sweep = (b.y - c.y).atan2(b.x - c.x) - start;
                while sweep > std::f32::consts::PI {
                    sweep -= std::f32::consts::TAU;
                }
                while sweep < -std::f32::consts::PI {
                    sweep += std::f32::consts::TAU;
                }
                let steps = ((sweep.abs() / std::f32::consts::PI) * round_steps(half_width) as f32)
                    .ceil()
                    .max(2.0) as usize;
                let mut fan = vec![c];
                for step in 0..=steps {
                    let angle = start + sweep * step as f32 / steps as f32;
                    fan.push(Vec2::new(
                        c.x + angle.cos() * half_width,
                        c.y + angle.sin() * half_width,
                    ));
                }
                polygons.push(fan);
            }
            JoinShape::Miter => {
                // The miter point sits on the bisector of the two offsets.
                let bisector = Vec2::new(a.x + b.x - 2.0 * c.x, a.y + b.y - 2.0 * c.y);
                let length = bisector.x.hypot(bisector.y);
                let cos_half = (length * 0.5 / half_width).clamp(1.0e-4, 1.0);
                let reach = half_width / cos_half;
                if length > 1.0e-6 && reach <= MITER_LIMIT * half_width {
                    let tip = Vec2::new(
                        c.x + bisector.x / length * reach,
                        c.y + bisector.y / length * reach,
                    );
                    polygons.push(vec![c, a, tip, b]);
                } else {
                    polygons.push(vec![c, a, b]);
                }
            }
        }
    }
    if !closed {
        for (end, d) in [(pts[0], direction(0)), (pts[n - 1], direction(n - 2))] {
            let outward = if (end.x - pts[0].x).abs() <= f32::EPSILON
                && (end.y - pts[0].y).abs() <= f32::EPSILON
            {
                Vec2::new(-d.x, -d.y)
            } else {
                d
            };
            match cap {
                CapShape::Butt => {}
                CapShape::Round => polygons.push(circle(end, half_width, round_steps(half_width))),
                CapShape::Square => {
                    let o = offset(outward);
                    let far = Vec2::new(
                        end.x + outward.x * half_width,
                        end.y + outward.y * half_width,
                    );
                    polygons.push(vec![add(end, o), add(far, o), sub(far, o), sub(end, o)]);
                }
            }
        }
    }
    for polygon in &mut polygons {
        let area: f32 = polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .map(|(p, q)| p.x * q.y - q.x * p.y)
            .sum();
        if area < 0.0 {
            polygon.reverse();
        }
    }
    polygons
}

fn circle(c: Vec2, radius: f32, steps: usize) -> Vec<Vec2> {
    (0..steps)
        .map(|i| {
            let angle = i as f32 * std::f32::consts::TAU / steps as f32;
            Vec2::new(c.x + angle.cos() * radius, c.y + angle.sin() * radius)
        })
        .collect()
}

/// Backwards-compatible wrapper: round caps, kept so existing call sites
/// don't have to thread a `CapShape` through. New code should call
/// `brush_outline_with_caps` directly.
pub fn brush_outline(centerline: &[Vec2], half_width: f32, cap_steps: usize) -> Vec<Vec2> {
    brush_outline_with_caps(centerline, half_width, cap_steps, CapShape::Round)
}

/// Build a closed polygon outline that wraps `centerline` at distance
/// `half_width` on either side, with caps determined by `cap_shape`.
///
/// The polygon is wound counter-clockwise: left side forward, end cap,
/// right side reversed, start cap. `cap_steps` controls how many
/// segments approximate each round cap (8 is plenty for typical brush
/// widths; ignored when `cap_shape == Butt`). Returns an empty vec when
/// the centerline is degenerate.
pub fn brush_outline_with_caps(
    centerline: &[Vec2],
    half_width: f32,
    cap_steps: usize,
    cap_shape: CapShape,
) -> Vec<Vec2> {
    if centerline.len() < 2 || half_width <= 0.0 || cap_steps < 2 {
        return Vec::new();
    }
    let n = centerline.len();
    let mut tangents = Vec::with_capacity(n);
    for i in 0..n {
        let prev = if i > 0 {
            centerline[i - 1]
        } else {
            centerline[i]
        };
        let next = if i + 1 < n {
            centerline[i + 1]
        } else {
            centerline[i]
        };
        let dx = next.x - prev.x;
        let dy = next.y - prev.y;
        let len = (dx * dx + dy * dy).sqrt().max(1e-5);
        tangents.push(Vec2::new(dx / len, dy / len));
    }
    let normals: Vec<Vec2> = tangents.iter().map(|t| Vec2::new(-t.y, t.x)).collect();
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    for i in 0..n {
        let c = centerline[i];
        let nv = normals[i];
        left.push(Vec2::new(c.x + nv.x * half_width, c.y + nv.y * half_width));
        right.push(Vec2::new(c.x - nv.x * half_width, c.y - nv.y * half_width));
    }
    let (end_cap, start_cap) = match cap_shape {
        CapShape::Butt | CapShape::Square => (Vec::new(), Vec::new()),
        CapShape::Round => {
            let mut end_cap = Vec::with_capacity(cap_steps - 1);
            {
                let c = centerline[n - 1];
                let t = tangents[n - 1];
                let nv = normals[n - 1];
                for i in 1..cap_steps {
                    let theta = std::f32::consts::PI * i as f32 / cap_steps as f32;
                    let (sin_t, cos_t) = theta.sin_cos();
                    end_cap.push(Vec2::new(
                        c.x + half_width * (cos_t * nv.x + sin_t * t.x),
                        c.y + half_width * (cos_t * nv.y + sin_t * t.y),
                    ));
                }
            }
            let mut start_cap = Vec::with_capacity(cap_steps - 1);
            {
                let c = centerline[0];
                let t = tangents[0];
                let nv = normals[0];
                for i in 1..cap_steps {
                    let theta = std::f32::consts::PI * i as f32 / cap_steps as f32;
                    let (sin_t, cos_t) = theta.sin_cos();
                    start_cap.push(Vec2::new(
                        c.x + half_width * (-cos_t * nv.x - sin_t * t.x),
                        c.y + half_width * (-cos_t * nv.y - sin_t * t.y),
                    ));
                }
            }
            (end_cap, start_cap)
        }
    };
    let mut out = Vec::with_capacity(2 * n + 2 * cap_steps);
    out.extend_from_slice(&left);
    out.extend(end_cap);
    for r in right.iter().rev() {
        out.push(*r);
    }
    out.extend(start_cap);
    out
}

/// Flatten a vector `Path` to a polyline by sampling each cubic-bezier
/// segment at `samples_per_segment` evenly-spaced steps. Open paths get N
/// straight-or-curved segments; closed paths add one extra back to the
/// first anchor.
pub fn flatten_path(path: &crate::v2::Path, samples_per_segment: usize) -> Vec<Vec2> {
    use crate::v2::Anchor;
    if path.anchors.is_empty() {
        return Vec::new();
    }
    if path.anchors.len() == 1 {
        return vec![path.anchors[0].point];
    }
    let mut out: Vec<Vec2> = Vec::new();
    out.push(path.anchors[0].point);
    let segment_count = if path.closed {
        path.anchors.len()
    } else {
        path.anchors.len() - 1
    };
    for i in 0..segment_count {
        let a: &Anchor = &path.anchors[i];
        let b: &Anchor = &path.anchors[(i + 1) % path.anchors.len()];
        sample_segment(a, b, samples_per_segment, &mut out);
    }
    out
}

fn sample_segment(
    a: &crate::v2::Anchor,
    b: &crate::v2::Anchor,
    samples: usize,
    out: &mut Vec<Vec2>,
) {
    let p0 = a.point;
    let p1 = a.out_handle.unwrap_or(a.point);
    let p2 = b.in_handle.unwrap_or(b.point);
    let p3 = b.point;
    let straight = a.out_handle.is_none() && b.in_handle.is_none();
    if straight {
        out.push(p3);
        return;
    }
    for i in 1..=samples {
        let t = i as f32 / samples as f32;
        out.push(cubic_bezier(p0, p1, p2, p3, t));
    }
}

fn cubic_bezier(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f32) -> Vec2 {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    Vec2::new(
        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `point` is inside the non-zero union of `polygons`.
    fn covered(polygons: &[Vec<Vec2>], point: Vec2) -> bool {
        let mut winding = 0;
        for polygon in polygons {
            for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
                let crosses_up = a.y <= point.y && b.y > point.y;
                let crosses_down = a.y > point.y && b.y <= point.y;
                if crosses_up || crosses_down {
                    let t = (point.y - a.y) / (b.y - a.y);
                    if a.x + t * (b.x - a.x) > point.x {
                        winding += if crosses_up { 1 } else { -1 };
                    }
                }
            }
        }
        winding != 0
    }

    /// An L: along the top, then down the right side.
    fn corner() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 100.0),
        ]
    }

    #[test]
    fn corners_are_sharp_round_or_cut() {
        // The outer corner of a 20 px wide L sits at (110, -10).
        let sharp_spot = Vec2::new(109.0, -9.0);
        let round_spot = Vec2::new(106.0, -6.0);
        let sharp = stroke_polygons(&corner(), false, 10.0, CapShape::Butt, JoinShape::Miter);
        let round = stroke_polygons(&corner(), false, 10.0, CapShape::Butt, JoinShape::Round);
        let cut = stroke_polygons(&corner(), false, 10.0, CapShape::Butt, JoinShape::Bevel);
        assert!(covered(&sharp, sharp_spot));
        assert!(!covered(&round, sharp_spot) && covered(&round, round_spot));
        assert!(!covered(&cut, round_spot));
        for polygons in [&sharp, &round, &cut] {
            assert!(covered(polygons, Vec2::new(50.0, 5.0)), "the stroke itself");
            assert!(covered(polygons, Vec2::new(99.0, 1.0)), "inside the corner");
        }
    }

    #[test]
    fn a_hairpin_miter_turns_flat_instead_of_spiking() {
        let hairpin = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(0.0, 2.0),
        ];
        let polygons = stroke_polygons(&hairpin, false, 5.0, CapShape::Butt, JoinShape::Miter);
        let far = polygons
            .iter()
            .flatten()
            .map(|point| point.x)
            .fold(f32::MIN, f32::max);
        assert!(far < 100.0 + 5.0 * MITER_LIMIT, "no spike: {far}");
    }

    #[test]
    fn ends_are_round_flat_or_square() {
        let line = vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)];
        let past_the_end = Vec2::new(104.0, 4.0);
        let just_past = Vec2::new(103.0, 0.0);
        let flat = stroke_polygons(&line, false, 5.0, CapShape::Butt, JoinShape::Round);
        let round = stroke_polygons(&line, false, 5.0, CapShape::Round, JoinShape::Round);
        let square = stroke_polygons(&line, false, 5.0, CapShape::Square, JoinShape::Round);
        assert!(!covered(&flat, just_past));
        assert!(covered(&round, just_past) && !covered(&round, past_the_end));
        assert!(covered(&square, past_the_end));
        // The start end too.
        assert!(covered(&square, Vec2::new(-4.0, -4.0)));
    }

    #[test]
    fn a_closed_square_has_four_sharp_corners() {
        let square = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(0.0, 100.0),
            Vec2::new(0.0, 0.0),
        ];
        let polygons = stroke_polygons(&square, true, 10.0, CapShape::Round, JoinShape::Miter);
        for corner in [
            Vec2::new(-9.0, -9.0),
            Vec2::new(109.0, -9.0),
            Vec2::new(109.0, 109.0),
            Vec2::new(-9.0, 109.0),
        ] {
            assert!(covered(&polygons, corner), "{corner:?}");
        }
        assert!(!covered(&polygons, Vec2::new(50.0, 50.0)), "hollow inside");
    }
}
