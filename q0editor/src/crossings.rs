//! Lines that cross are cut where they cross, the way Animate's merge
//! drawing does, so each piece between crossings can be picked, moved,
//! recoloured or deleted on its own. Double-clicking a piece picks every
//! line joined to it.
//!
//! Only plain lines are cut: open strokes, and closed outlines without a
//! fill. A filled shape's outline keeps being picked side by side.

use q0s_format::transform::Affine;
use q0s_format::v2::{Anchor, Asset, Path as VPath, ProjectV2, Target, Vec2};

use crate::state::{PathRef, Selection, StrokeSegmentRef};

/// Steps each curve is followed in to find where it crosses another.
const STEPS: usize = 24;
/// Pieces shorter than this, in path parameter, are not cut off.
const MIN_PIECE: f32 = 1.0e-3;
/// How near, in stage pixels, a line's end has to come to another line to
/// meet it.
const MEET: f32 = 0.35;
/// How near, in stage pixels, two lines have to come to count as joined.
const JOINED: f32 = 0.75;

type Cubic = [Vec2; 4];

fn handle_or(point: Vec2, handle: Option<Vec2>) -> Vec2 {
    handle.unwrap_or(point)
}

fn segment_count(path: &VPath) -> usize {
    match path.anchors.len() {
        0 | 1 => 0,
        count if path.closed => count,
        count => count - 1,
    }
}

fn cubic(path: &VPath, segment: usize) -> Cubic {
    let a = &path.anchors[segment];
    let b = &path.anchors[(segment + 1) % path.anchors.len()];
    [
        a.point,
        handle_or(a.point, a.out_handle),
        handle_or(b.point, b.in_handle),
        b.point,
    ]
}

fn lerp(a: Vec2, b: Vec2, t: f32) -> Vec2 {
    Vec2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn at(c: &Cubic, t: f32) -> Vec2 {
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    Vec2::new(
        w[0] * c[0].x + w[1] * c[1].x + w[2] * c[2].x + w[3] * c[3].x,
        w[0] * c[0].y + w[1] * c[1].y + w[2] * c[2].y + w[3] * c[3].y,
    )
}

/// `c` cut in two at `t`.
fn split_cubic(c: &Cubic, t: f32) -> (Cubic, Cubic) {
    let ab = lerp(c[0], c[1], t);
    let bc = lerp(c[1], c[2], t);
    let cd = lerp(c[2], c[3], t);
    let abc = lerp(ab, bc, t);
    let bcd = lerp(bc, cd, t);
    let middle = lerp(abc, bcd, t);
    ([c[0], ab, abc, middle], [middle, bcd, cd, c[3]])
}

fn overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32), pad: f32) -> bool {
    a.0 - pad <= b.2 && b.0 - pad <= a.2 && a.1 - pad <= b.3 && b.1 - pad <= a.3
}

fn distance(a: Vec2, b: Vec2) -> f32 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// The path followed in small straight steps: (parameter, point) pairs,
/// the parameter being segment index plus how far along it.
fn samples(path: &VPath, affine: Affine) -> Vec<Vec<(f32, Vec2)>> {
    (0..segment_count(path))
        .map(|segment| {
            let c = cubic(path, segment);
            (0..=STEPS)
                .map(|step| {
                    let t = step as f32 / STEPS as f32;
                    (segment as f32 + t, affine.apply(at(&c, t)))
                })
                .collect()
        })
        .collect()
}

/// Where segments `p0..p1` and `q0..q1` cross, as how far along each.
fn segments_cross(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> Option<(f32, f32)> {
    let r = Vec2::new(p1.x - p0.x, p1.y - p0.y);
    let s = Vec2::new(q1.x - q0.x, q1.y - q0.y);
    let denominator = r.x * s.y - r.y * s.x;
    // Lines this near parallel are taken as parallel: run along each other
    // and the noise in their sampling crosses them over and over.
    if denominator.abs() < 1.0e-9 || denominator.abs() <= r.x.hypot(r.y) * s.x.hypot(s.y) * 1.0e-5 {
        return None;
    }
    let qp = Vec2::new(q0.x - p0.x, q0.y - p0.y);
    let u = (qp.x * s.y - qp.y * s.x) / denominator;
    let v = (qp.x * r.y - qp.y * r.x) / denominator;
    ((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then_some((u, v))
}

/// The nearest place on a sampled path to `point`: its parameter and how
/// far away it is.
fn nearest_on(samples: &[Vec<(f32, Vec2)>], point: Vec2) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    for run in samples {
        for pair in run.windows(2) {
            let (t0, a) = pair[0];
            let (t1, b) = pair[1];
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let length_sq = dx * dx + dy * dy;
            let k = if length_sq <= 1.0e-12 {
                0.0
            } else {
                (((point.x - a.x) * dx + (point.y - a.y) * dy) / length_sq).clamp(0.0, 1.0)
            };
            let on = Vec2::new(a.x + dx * k, a.y + dy * k);
            let away = distance(on, point);
            if best.is_none_or(|(_, nearest)| away < nearest) {
                best = Some((t0 + (t1 - t0) * k, away));
            }
        }
    }
    best
}

fn ends(path: &VPath) -> Vec<(f32, Vec2)> {
    if path.closed || path.anchors.len() < 2 {
        return Vec::new();
    }
    vec![
        (0.0, path.anchors[0].point),
        (
            segment_count(path) as f32,
            path.anchors[path.anchors.len() - 1].point,
        ),
    ]
}

/// How near, in stage pixels, two cuts on one line are taken as one, and a
/// cut to an open line's end as no cut at all.
const SAME_CUT: f32 = 0.05;
/// Pieces shorter than this, in stage pixels, are dropped.
const MIN_LENGTH: f32 = 0.01;
/// The most pieces one new line may leave behind. Past it nothing is cut:
/// a pile of lines through one point must never freeze drawing.
const MAX_PIECES: usize = 2048;

/// `c` placed by `affine`, at `t`.
fn placed_at(c: &Cubic, affine: Affine, t: f32) -> Vec2 {
    affine.apply(at(c, t))
}

/// Whether `c` is a straight segment: one without handles.
fn is_straight(c: &Cubic) -> bool {
    same(c[1], c[0]) && same(c[2], c[3])
}

/// A straight segment is a cubic whose handles sit on its ends, so it
/// covers `3t^2 - 2t^3` of its length at `t`. The `t` covering `share`.
fn straight_param(share: f32) -> f32 {
    let share = share.clamp(0.0, 1.0);
    let (mut low, mut high) = (0.0_f32, 1.0_f32);
    for _ in 0..40 {
        let middle = (low + high) * 0.5;
        if middle * middle * (3.0 - 2.0 * middle) < share {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) * 0.5
}

/// Where two straight segments, in stage space, cross: how far along each
/// and the point. Parallel ones never cross; their ends may still meet.
fn straight_crossing(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> Option<(f32, f32, Vec2)> {
    let (p0, p1, q0, q1) = (
        (f64::from(p0.x), f64::from(p0.y)),
        (f64::from(p1.x), f64::from(p1.y)),
        (f64::from(q0.x), f64::from(q0.y)),
        (f64::from(q1.x), f64::from(q1.y)),
    );
    let r = (p1.0 - p0.0, p1.1 - p0.1);
    let s = (q1.0 - q0.0, q1.1 - q0.1);
    let denominator = r.0 * s.1 - r.1 * s.0;
    let lengths = (r.0.hypot(r.1)) * (s.0.hypot(s.1));
    if lengths <= 1.0e-12 || denominator.abs() <= lengths * 1.0e-6 {
        return None;
    }
    let qp = (q0.0 - p0.0, q0.1 - p0.1);
    let u = (qp.0 * s.1 - qp.1 * s.0) / denominator;
    let v = (qp.0 * r.1 - qp.1 * r.0) / denominator;
    const SLACK: f64 = 1.0e-9;
    if !(-SLACK..=1.0 + SLACK).contains(&u) || !(-SLACK..=1.0 + SLACK).contains(&v) {
        return None;
    }
    let point = Vec2::new((p0.0 + r.0 * u) as f32, (p0.1 + r.1 * u) as f32);
    Some((u.clamp(0.0, 1.0) as f32, v.clamp(0.0, 1.0) as f32, point))
}

/// Narrow down where the spans `ta` of `a` and `tb` of `b` (parameter
/// ranges of one segment each, known to cross near `guess`) cross, by
/// following both ever more finely. Returns the parameters on each.
fn refine_crossing(
    a: &Cubic,
    a_affine: Affine,
    mut ta: (f32, f32),
    b: &Cubic,
    b_affine: Affine,
    mut tb: (f32, f32),
    guess: (f32, f32),
) -> (f32, f32) {
    const SPLIT: usize = 6;
    let along = |c: &Cubic, affine: Affine, span: (f32, f32)| -> Vec<(f32, Vec2)> {
        (0..=SPLIT)
            .map(|step| {
                let t = span.0 + (span.1 - span.0) * step as f32 / SPLIT as f32;
                (t, placed_at(c, affine, t))
            })
            .collect()
    };
    let mut best = guess;
    for _ in 0..5 {
        let run_a = along(a, a_affine, ta);
        let run_b = along(b, b_affine, tb);
        let mut found = None;
        'search: for pa in run_a.windows(2) {
            for pb in run_b.windows(2) {
                if let Some((u, v)) = segments_cross(pa[0].1, pa[1].1, pb[0].1, pb[1].1) {
                    found = Some((
                        (pa[0].0, pa[1].0),
                        (pb[0].0, pb[1].0),
                        pa[0].0 + (pa[1].0 - pa[0].0) * u,
                        pb[0].0 + (pb[1].0 - pb[0].0) * v,
                    ));
                    break 'search;
                }
            }
        }
        let Some((next_a, next_b, guess_a, guess_b)) = found else {
            break;
        };
        best = (guess_a, guess_b);
        ta = next_a;
        tb = next_b;
    }
    best
}

/// The parameter on segment `c` nearest to `point`, searching one sampling
/// step either side of `guess`.
fn refine_nearest(c: &Cubic, affine: Affine, point: Vec2, guess: f32) -> f32 {
    if is_straight(c) {
        let (a, b) = (affine.apply(c[0]), affine.apply(c[3]));
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let length_sq = dx * dx + dy * dy;
        if length_sq <= 1.0e-12 {
            return 0.0;
        }
        let share = ((point.x - a.x) * dx + (point.y - a.y) * dy) / length_sq;
        return straight_param(share);
    }
    let step = 1.0 / STEPS as f32;
    let (mut low, mut high) = ((guess - step).max(0.0), (guess + step).min(1.0));
    for _ in 0..32 {
        let left = low + (high - low) / 3.0;
        let right = high - (high - low) / 3.0;
        if distance(placed_at(c, affine, left), point)
            <= distance(placed_at(c, affine, right), point)
        {
            high = right;
        } else {
            low = left;
        }
    }
    (low + high) * 0.5
}

/// Parameters along a line, each with the stage point it is cut at.
type Cuts = Vec<(f32, Vec2)>;

/// Where `a` and `b` (placed by their affines) cross or one ends on the
/// other: the parameters on `a` and on `b`, each with the stage point both
/// lines share there, so the pieces of the two meet exactly.
fn crossings(a: &VPath, a_affine: Affine, b: &VPath, b_affine: Affine) -> (Cuts, Cuts) {
    let mut on_a = Vec::new();
    let mut on_b = Vec::new();
    let a_samples = samples(a, a_affine);
    let b_samples = samples(b, b_affine);
    let box_of = |run: &Vec<(f32, Vec2)>| {
        run.iter()
            .fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, (_, point)| {
                (
                    b.0.min(point.x),
                    b.1.min(point.y),
                    b.2.max(point.x),
                    b.3.max(point.y),
                )
            })
    };
    let b_boxes: Vec<_> = b_samples.iter().map(box_of).collect();
    for (segment_a, run_a) in a_samples.iter().enumerate() {
        let box_a = box_of(run_a);
        let cubic_a = cubic(a, segment_a);
        let base_a = segment_a as f32;
        for (segment_b, (run_b, box_b)) in b_samples.iter().zip(&b_boxes).enumerate() {
            if !overlap(box_a, *box_b, MEET) {
                continue;
            }
            let cubic_b = cubic(b, segment_b);
            let base_b = segment_b as f32;
            if is_straight(&cubic_a) && is_straight(&cubic_b) {
                if let Some((u, v, point)) = straight_crossing(
                    a_affine.apply(cubic_a[0]),
                    a_affine.apply(cubic_a[3]),
                    b_affine.apply(cubic_b[0]),
                    b_affine.apply(cubic_b[3]),
                ) {
                    on_a.push((base_a + straight_param(u), point));
                    on_b.push((base_b + straight_param(v), point));
                }
                continue;
            }
            for pa in run_a.windows(2) {
                for pb in run_b.windows(2) {
                    let Some((u, v)) = segments_cross(pa[0].1, pa[1].1, pb[0].1, pb[1].1) else {
                        continue;
                    };
                    let (ta, tb) = refine_crossing(
                        &cubic_a,
                        a_affine,
                        (pa[0].0 - base_a, pa[1].0 - base_a),
                        &cubic_b,
                        b_affine,
                        (pb[0].0 - base_b, pb[1].0 - base_b),
                        (
                            pa[0].0 + (pa[1].0 - pa[0].0) * u - base_a,
                            pb[0].0 + (pb[1].0 - pb[0].0) * v - base_b,
                        ),
                    );
                    let shared = lerp(
                        placed_at(&cubic_a, a_affine, ta),
                        placed_at(&cubic_b, b_affine, tb),
                        0.5,
                    );
                    on_a.push((base_a + ta, shared));
                    on_b.push((base_b + tb, shared));
                }
            }
        }
    }
    // A line that ends on another cuts that one there, right at its end.
    let ends_on = |line: &VPath,
                   line_affine: Affine,
                   other: &VPath,
                   other_affine: Affine,
                   other_samples: &[Vec<(f32, Vec2)>],
                   into: &mut Cuts| {
        for (_, end) in ends(line) {
            let end = line_affine.apply(end);
            let Some((t, away)) = nearest_on(other_samples, end) else {
                continue;
            };
            if away > MEET {
                continue;
            }
            let segment = (t.floor().max(0.0) as usize).min(segment_count(other).saturating_sub(1));
            let local = refine_nearest(
                &cubic(other, segment),
                other_affine,
                end,
                t - segment as f32,
            );
            into.push((segment as f32 + local, end));
        }
    };
    ends_on(a, a_affine, b, b_affine, &b_samples, &mut on_b);
    ends_on(b, b_affine, a, a_affine, &a_samples, &mut on_a);
    (on_a, on_b)
}

/// The point `param` along `path`.
#[cfg(test)]
fn path_point(path: &VPath, param: f32) -> Vec2 {
    let count = segment_count(path);
    let segment = (param.floor().max(0.0) as usize).min(count.saturating_sub(1));
    at(
        &cubic(path, segment),
        (param - segment as f32).clamp(0.0, 1.0),
    )
}

/// `params` sorted, without ones at the ends of `path` or next to each
/// other.
#[cfg(test)]
fn cut_points(path: &VPath, params: Vec<f32>) -> Vec<f32> {
    let cuts = params
        .into_iter()
        .filter(|t| t.is_finite())
        .map(|t| (t, path_point(path, t)))
        .collect();
    clean_cuts(path, Affine::IDENTITY, cuts)
        .into_iter()
        .map(|(t, _)| t)
        .collect()
}

/// Cuts on `path` (placed by `affine`) sorted along it, without ones at the
/// ends of an open line or on top of each other. Each keeps its stage
/// point.
fn clean_cuts(path: &VPath, affine: Affine, mut cuts: Cuts) -> Cuts {
    let total = segment_count(path) as f32;
    if total <= 0.0 {
        return Vec::new();
    }
    cuts.retain(|(t, point)| t.is_finite() && point.x.is_finite() && point.y.is_finite());
    for (t, _) in &mut cuts {
        *t = if path.closed {
            let wrapped = t.rem_euclid(total);
            if wrapped >= total - 1.0e-6 {
                0.0
            } else {
                wrapped
            }
        } else {
            t.clamp(0.0, total)
        };
    }
    cuts.sort_by(|left, right| left.0.total_cmp(&right.0));
    let start = affine.apply(path.anchors[0].point);
    let finish = affine.apply(path.anchors[path.anchors.len() - 1].point);
    let mut kept: Cuts = Vec::new();
    for (t, point) in cuts {
        if !path.closed
            && (t <= MIN_PIECE
                || t >= total - MIN_PIECE
                || distance(point, start) <= SAME_CUT
                || distance(point, finish) <= SAME_CUT)
        {
            continue;
        }
        if kept.last().is_some_and(|(last_t, last)| {
            t - last_t <= 1.0e-5 || distance(point, *last) <= SAME_CUT
        }) {
            continue;
        }
        kept.push((t, point));
    }
    if path.closed && kept.len() > 1 && distance(kept[0].1, kept[kept.len() - 1].1) <= SAME_CUT {
        kept.pop();
    }
    kept
}

fn same(a: Vec2, b: Vec2) -> bool {
    distance(a, b) <= 1.0e-4
}

/// A cubic that does not go anywhere.
fn degenerate(c: &Cubic) -> bool {
    same(c[0], c[1]) && same(c[1], c[2]) && same(c[2], c[3])
}

/// Anchors running along `cubics`, straight pieces kept without handles.
fn anchors_of(cubics: &[Cubic]) -> Vec<Anchor> {
    let mut anchors: Vec<Anchor> = Vec::with_capacity(cubics.len() + 1);
    for (index, c) in cubics.iter().enumerate() {
        let out = (!same(c[1], c[0])).then_some(c[1]);
        let into = (!same(c[2], c[3])).then_some(c[2]);
        if index == 0 {
            anchors.push(Anchor {
                point: c[0],
                in_handle: None,
                out_handle: out,
            });
        } else if let Some(last) = anchors.last_mut() {
            last.out_handle = out;
        }
        anchors.push(Anchor {
            point: c[3],
            in_handle: into,
            out_handle: None,
        });
    }
    anchors
}

/// The cubics of `path` cut at `cuts` (sorted, inside the path), piece by
/// piece. A closed path's run before its first cut carries on after its
/// last, so its piece `k` runs from cut `k` to the next cut round.
fn split_cubics(path: &VPath, cuts: &[f32]) -> Vec<Vec<Cubic>> {
    let count = segment_count(path);
    let mut pieces: Vec<Vec<Cubic>> = vec![Vec::new()];
    for segment in 0..count {
        let whole = cubic(path, segment);
        // A straight segment is cut into straight pieces without handles.
        let straight = same(whole[1], whole[0]) && same(whole[2], whole[3]);
        let mut rest = whole;
        let mut done = 0.0;
        for t in cuts
            .iter()
            .filter(|t| (t.floor().max(0.0) as usize).min(count - 1) == segment)
            .map(|t| (t - segment as f32).clamp(0.0, 1.0))
        {
            let (left, right) = if straight {
                let cut = at(&whole, t);
                ([rest[0], rest[0], cut, cut], [cut, cut, whole[3], whole[3]])
            } else {
                split_cubic(
                    &rest,
                    ((t - done) / (1.0 - done).max(1.0e-6)).clamp(0.0, 1.0),
                )
            };
            if let Some(piece) = pieces.last_mut() {
                piece.push(left);
            }
            pieces.push(Vec::new());
            rest = right;
            done = t;
        }
        if let Some(piece) = pieces.last_mut() {
            piece.push(rest);
        }
    }
    if path.closed && pieces.len() > 1 {
        let head = pieces.remove(0);
        if let Some(last) = pieces.last_mut() {
            last.extend(head);
        }
    }
    pieces
}

/// Pieces as open paths, without steps that go nowhere or pieces too short
/// to see.
fn finish_pieces(pieces: Vec<Vec<Cubic>>, affine: Affine) -> Vec<VPath> {
    pieces
        .into_iter()
        .filter_map(|mut cubics| {
            cubics.retain(|c| !degenerate(c));
            let length: f32 = cubics
                .iter()
                .map(|c| distance(affine.apply(c[0]), affine.apply(c[3])))
                .sum();
            (!cubics.is_empty() && length >= MIN_LENGTH).then(|| VPath {
                anchors: anchors_of(&cubics),
                closed: false,
            })
        })
        .collect()
}

/// `path` cut at the parameters `cuts` (sorted, inside the path) into open
/// pieces. A closed path cut once opens into one line from the cut round
/// to it again.
pub(crate) fn split_path(path: &VPath, cuts: &[f32]) -> Vec<VPath> {
    if cuts.is_empty() || segment_count(path) == 0 {
        return vec![path.clone()];
    }
    finish_pieces(split_cubics(path, cuts), Affine::IDENTITY)
}

/// Move the start of a run of cubics to `point`, a straight start staying
/// straight.
fn snap_start(cubics: &mut [Cubic], point: Vec2) {
    if let Some(c) = cubics.first_mut() {
        if same(c[1], c[0]) {
            c[1] = point;
        }
        c[0] = point;
    }
}

fn snap_end(cubics: &mut [Cubic], point: Vec2) {
    if let Some(c) = cubics.last_mut() {
        if same(c[2], c[3]) {
            c[2] = point;
        }
        c[3] = point;
    }
}

/// `path` cut at `cuts` (sorted and cleaned, each with the point in the
/// path's own space where its pieces meet whatever cut it).
fn split_path_at(path: &VPath, cuts: &[(f32, Vec2)], affine: Affine) -> Vec<VPath> {
    if cuts.is_empty() || segment_count(path) == 0 {
        return vec![path.clone()];
    }
    let params: Vec<f32> = cuts.iter().map(|(t, _)| *t).collect();
    let mut pieces = split_cubics(path, &params);
    if path.closed {
        for (index, piece) in pieces.iter_mut().enumerate() {
            snap_start(piece, cuts[index % cuts.len()].1);
            snap_end(piece, cuts[(index + 1) % cuts.len()].1);
        }
    } else {
        for (index, (_, point)) in cuts.iter().enumerate() {
            if let Some(piece) = pieces.get_mut(index) {
                snap_end(piece, *point);
            }
            if let Some(piece) = pieces.get_mut(index + 1) {
                snap_start(piece, *point);
            }
        }
    }
    finish_pieces(pieces, affine)
}

/// How long `path` is at most, in stage pixels: its control polygon.
fn rough_length(path: &VPath, affine: Affine) -> f32 {
    (0..segment_count(path))
        .map(|segment| {
            let c = cubic(path, segment);
            let p = c.map(|point| affine.apply(point));
            distance(p[0], p[1]) + distance(p[1], p[2]) + distance(p[2], p[3])
        })
        .sum()
}

/// A line that can be cut: an open stroke, or an outline with no fill.
fn cuttable(vector: &q0s_format::v2::VectorAsset, path: &VPath) -> bool {
    vector.stroke.is_some() && (!path.closed || vector.fill.is_none()) && path.anchors.len() >= 2
}

/// Plain drawings on `layer_id` at `frame` whose lines can be cut: the
/// placement index, its asset and where it is placed.
fn drawings(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Vec<(usize, u16, Affine)> {
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return Vec::new();
    };
    crate::render::active_placements_at(layer, frame)
        .into_iter()
        .filter_map(|(placement_idx, transform)| {
            let placement = layer.placements.get(placement_idx)?;
            if placement.frame != frame || placement.tween.is_motion() {
                return None;
            }
            let Target::Asset(asset_id) = placement.target else {
                return None;
            };
            matches!(
                project.assets.iter().find(|asset| asset.id() == asset_id),
                Some(Asset::Vector(vector)) if vector.stroke.is_some()
            )
            .then(|| (placement_idx, asset_id, Affine::from_transform(transform)))
        })
        .collect()
}

fn vector(project: &ProjectV2, asset_id: u16) -> Option<&q0s_format::v2::VectorAsset> {
    match project.assets.iter().find(|asset| asset.id() == asset_id)? {
        Asset::Vector(vector) => Some(vector),
        _ => None,
    }
}

/// Cuts the line `new_path` of `asset_id` on `layer_id` and every line it
/// crosses or touches there where they meet. Returns whether anything was
/// cut. Assets shared with other keyframes are copied before they change.
///
/// Both lines are cut at one shared point, so their pieces meet exactly
/// and the paint bucket finds a closed corner there. Lines too short to
/// see are swept away rather than cut into ever smaller crumbs.
pub fn cut_at_crossings(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    asset_id: u16,
    new_path: usize,
) -> bool {
    let placed = drawings(project, q0rg_id, layer_id, frame);
    let Some(&(_, _, new_affine)) = placed.iter().find(|(_, id, _)| *id == asset_id) else {
        return false;
    };
    let Some(line) = vector(project, asset_id).and_then(|vector| {
        let path = vector.paths.get(new_path)?;
        (cuttable(vector, path) && rough_length(path, new_affine) >= MIN_LENGTH)
            .then(|| path.clone())
    }) else {
        return false;
    };
    let affine_of = |asset: u16| {
        placed
            .iter()
            .find(|(_, id, _)| *id == asset)
            .map(|(_, _, affine)| *affine)
    };
    // Cuts per (asset, path), each with the stage point it is shared at.
    let mut cuts: std::collections::BTreeMap<(u16, usize), Cuts> = Default::default();
    // Lines too short to see, to sweep away.
    let mut crumbs: std::collections::BTreeMap<u16, Vec<usize>> = Default::default();
    for &(_, other_asset, affine) in &placed {
        let Some(other) = vector(project, other_asset) else {
            continue;
        };
        for (path_idx, path) in other.paths.iter().enumerate() {
            if (other_asset == asset_id && path_idx == new_path) || !cuttable(other, path) {
                continue;
            }
            if !path.closed && rough_length(path, affine) < MIN_LENGTH {
                crumbs.entry(other_asset).or_default().push(path_idx);
                continue;
            }
            let (on_new, on_other) = crossings(&line, new_affine, path, affine);
            if !on_new.is_empty() {
                cuts.entry((asset_id, new_path)).or_default().extend(on_new);
            }
            if !on_other.is_empty() {
                cuts.entry((other_asset, path_idx))
                    .or_default()
                    .extend(on_other);
            }
        }
    }
    let mut by_asset: std::collections::BTreeMap<u16, Vec<(usize, Cuts)>> = Default::default();
    let mut pieces_made = 0usize;
    for ((asset, path_idx), found) in cuts {
        let (Some(path), Some(affine)) = (
            vector(project, asset).and_then(|vector| vector.paths.get(path_idx)),
            affine_of(asset),
        ) else {
            continue;
        };
        let Some(inverse) = affine.inverse() else {
            continue;
        };
        let kept: Cuts = clean_cuts(path, affine, found)
            .into_iter()
            .map(|(t, point)| (t, inverse.apply(point)))
            .collect();
        if !kept.is_empty() {
            pieces_made += kept.len() + 1;
            by_asset.entry(asset).or_default().push((path_idx, kept));
        }
    }
    if pieces_made > MAX_PIECES || (by_asset.is_empty() && crumbs.is_empty()) {
        return false;
    }
    let touched: std::collections::BTreeSet<u16> =
        by_asset.keys().chain(crumbs.keys()).copied().collect();
    let writable =
        crate::brush::prepare_writable_raw_assets(project, q0rg_id, layer_id, frame, &touched);
    let mut cut_any = false;
    for original in touched {
        let asset = *writable.get(&original).unwrap_or(&original);
        let affine = affine_of(original).unwrap_or(Affine::IDENTITY);
        let Some(Asset::Vector(vector)) = project.assets.iter_mut().find(|item| item.id() == asset)
        else {
            continue;
        };
        let mut replaced: Vec<Option<Vec<VPath>>> = vec![None; vector.paths.len()];
        for (path_idx, kept) in by_asset.get(&original).into_iter().flatten() {
            if let Some(path) = vector.paths.get(*path_idx) {
                replaced[*path_idx] = Some(split_path_at(path, kept, affine));
            }
        }
        for path_idx in crumbs.get(&original).into_iter().flatten() {
            if let Some(slot) = replaced.get_mut(*path_idx) {
                *slot = Some(Vec::new());
            }
        }
        // A cut path keeps its place with its first piece; the rest of its
        // pieces go on the end.
        let mut tail = Vec::new();
        let mut paths = Vec::with_capacity(vector.paths.len());
        for (path, replacement) in vector.paths.drain(..).zip(replaced) {
            match replacement {
                None => paths.push(path),
                Some(pieces) => {
                    let mut pieces = pieces.into_iter();
                    if let Some(first) = pieces.next() {
                        paths.push(first);
                    }
                    tail.extend(pieces);
                    cut_any = true;
                }
            }
        }
        paths.extend(tail);
        vector.paths = paths;
    }
    cut_any
}

/// One line on the frame: where it is, how it is placed and its points.
struct Line {
    reference: PathRef,
    /// A filled shape's outline, picked side by side.
    filled: bool,
    sides: usize,
    samples: Vec<Vec<(f32, Vec2)>>,
    bounds: (f32, f32, f32, f32),
}

fn lines_on(project: &ProjectV2, q0rg_id: u16, layer_id: u16, frame: u16) -> Vec<Line> {
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for (placement_idx, transform) in crate::render::active_placements_at(layer, frame) {
        let Some(placement) = layer.placements.get(placement_idx) else {
            continue;
        };
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let Some(vector) = vector(project, asset_id).filter(|vector| vector.stroke.is_some())
        else {
            continue;
        };
        let affine = Affine::from_transform(if placement.tween.is_motion() {
            transform
        } else {
            placement.transform
        });
        for (path_idx, path) in vector.paths.iter().enumerate() {
            if path.anchors.len() < 2 {
                continue;
            }
            let samples = samples(path, affine);
            let bounds = samples
                .iter()
                .flatten()
                .fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, (_, p)| {
                    (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y))
                });
            lines.push(Line {
                reference: PathRef {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                    path_idx,
                },
                filled: path.closed && vector.fill.is_some(),
                sides: segment_count(path),
                samples,
                bounds,
            });
        }
    }
    lines
}

fn touching(a: &Line, b: &Line) -> bool {
    if !overlap(a.bounds, b.bounds, JOINED) {
        return false;
    }
    for run_a in &a.samples {
        for pa in run_a.windows(2) {
            for run_b in &b.samples {
                for pb in run_b.windows(2) {
                    if segments_cross(pa[0].1, pa[1].1, pb[0].1, pb[1].1).is_some() {
                        return true;
                    }
                }
            }
        }
    }
    let near = |from: &Line, to: &Line| {
        from.samples.iter().flatten().any(|(_, point)| {
            nearest_on(&to.samples, *point).is_some_and(|(_, away)| away <= JOINED)
        })
    };
    near(a, b) || near(b, a)
}

/// Every line on the layer of `start` joined to it, directly or through
/// other lines, as a selection: whole lines, and every side of a filled
/// shape's outline.
pub fn connected_selection(project: &ProjectV2, frame: u16, start: PathRef) -> Option<Selection> {
    let lines = lines_on(project, start.q0rg_id, start.layer_id, frame);
    let first = lines.iter().position(|line| line.reference == start)?;
    let mut picked = vec![false; lines.len()];
    picked[first] = true;
    let mut queue = vec![first];
    while let Some(index) = queue.pop() {
        for other in 0..lines.len() {
            if !picked[other] && touching(&lines[index], &lines[other]) {
                picked[other] = true;
                queue.push(other);
            }
        }
    }
    let mut strokes = Vec::new();
    let mut stroke_segments = Vec::new();
    for (line, _) in lines.iter().zip(&picked).filter(|(_, picked)| **picked) {
        if line.filled {
            stroke_segments.extend((0..line.sides).map(|segment_idx| StrokeSegmentRef {
                path: line.reference,
                segment_idx,
            }));
        } else {
            strokes.push(line.reference);
        }
    }
    Some(if stroke_segments.is_empty() && strokes.len() == 1 {
        let only = strokes[0];
        Selection::Path {
            q0rg_id: only.q0rg_id,
            layer_id: only.layer_id,
            placement_idx: only.placement_idx,
            path_idx: only.path_idx,
        }
    } else if stroke_segments.is_empty() {
        Selection::Paths(strokes)
    } else {
        Selection::RawParts {
            fills: Vec::new(),
            strokes,
            stroke_segments,
            objects: Vec::new(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(points: &[(f32, f32)]) -> VPath {
        VPath {
            anchors: points
                .iter()
                .map(|&(x, y)| Anchor {
                    point: Vec2::new(x, y),
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
            closed: false,
        }
    }

    #[test]
    fn a_cross_cuts_both_lines_in_two() {
        let a = line(&[(0.0, 50.0), (100.0, 50.0)]);
        let b = line(&[(50.0, 0.0), (50.0, 100.0)]);
        let (on_a, on_b) = crossings(&a, Affine::IDENTITY, &b, Affine::IDENTITY);
        let on_a = cut_points(&a, on_a.into_iter().map(|(t, _)| t).collect());
        let on_b = cut_points(&b, on_b.into_iter().map(|(t, _)| t).collect());
        assert_eq!(on_a.len(), 1);
        assert_eq!(on_b.len(), 1);
        let pieces = split_path(&a, &on_a);
        assert_eq!(pieces.len(), 2);
        let middle = pieces[0].anchors.last().unwrap().point;
        assert!((middle.x - 50.0).abs() < 0.01 && (middle.y - 50.0).abs() < 0.01);
        assert_eq!(pieces[1].anchors[0].point, middle);
        assert!(pieces.iter().all(|piece| piece
            .anchors
            .iter()
            .all(|anchor| anchor.in_handle.is_none() && anchor.out_handle.is_none())));
    }

    #[test]
    fn a_line_ending_on_another_cuts_only_that_one() {
        let a = line(&[(0.0, 50.0), (100.0, 50.0)]);
        let t = line(&[(30.0, 0.0), (30.0, 50.0)]);
        let (on_a, on_t) = crossings(&a, Affine::IDENTITY, &t, Affine::IDENTITY);
        let params = |cuts: Cuts| cuts.into_iter().map(|(t, _)| t).collect();
        assert_eq!(cut_points(&a, params(on_a)).len(), 1);
        assert!(
            cut_points(&t, params(on_t)).is_empty(),
            "its own end is not a cut"
        );
    }

    #[test]
    fn cutting_a_curve_keeps_its_shape() {
        let curve = VPath {
            anchors: vec![
                Anchor {
                    point: Vec2::new(0.0, 0.0),
                    in_handle: None,
                    out_handle: Some(Vec2::new(0.0, 60.0)),
                },
                Anchor {
                    point: Vec2::new(100.0, 0.0),
                    in_handle: Some(Vec2::new(100.0, 60.0)),
                    out_handle: None,
                },
            ],
            closed: false,
        };
        let pieces = split_path(&curve, &[0.3, 0.7]);
        assert_eq!(pieces.len(), 3);
        let whole = cubic(&curve, 0);
        for (piece, (from, to)) in pieces.iter().zip([(0.0, 0.3), (0.3, 0.7), (0.7, 1.0)]) {
            let part = cubic(piece, 0);
            for step in 0..=4 {
                let k = step as f32 / 4.0;
                let expected = at(&whole, from + (to - from) * k);
                let got = at(&part, k);
                assert!(distance(expected, got) < 0.01, "{expected:?} {got:?}");
            }
        }
    }

    #[test]
    fn a_ring_cut_twice_opens_into_two_arcs() {
        let mut ring = line(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]);
        ring.closed = true;
        let pieces = split_path(&ring, &[0.5, 2.5]);
        assert_eq!(pieces.len(), 2);
        assert_eq!(
            pieces[0].anchors.first().unwrap().point,
            Vec2::new(50.0, 0.0)
        );
        assert_eq!(
            pieces[0].anchors.last().unwrap().point,
            Vec2::new(50.0, 100.0)
        );
        assert_eq!(
            pieces[1].anchors.first().unwrap().point,
            Vec2::new(50.0, 100.0)
        );
        assert_eq!(
            pieces[1].anchors.last().unwrap().point,
            Vec2::new(50.0, 0.0)
        );
    }
}
