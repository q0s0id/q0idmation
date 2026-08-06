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
        CapShape::Butt => (Vec::new(), Vec::new()),
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
