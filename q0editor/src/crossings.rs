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
    if denominator.abs() < 1.0e-9 {
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

/// Where `a` and `b` (placed by their affines) cross or one ends on the
/// other: the parameters on `a` and on `b`.
fn crossings(a: &VPath, a_affine: Affine, b: &VPath, b_affine: Affine) -> (Vec<f32>, Vec<f32>) {
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
    for run_a in &a_samples {
        let box_a = box_of(run_a);
        for (run_b, box_b) in b_samples.iter().zip(&b_boxes) {
            if !overlap(box_a, *box_b, MEET) {
                continue;
            }
            for pa in run_a.windows(2) {
                for pb in run_b.windows(2) {
                    if let Some((u, v)) = segments_cross(pa[0].1, pa[1].1, pb[0].1, pb[1].1) {
                        on_a.push(pa[0].0 + (pa[1].0 - pa[0].0) * u);
                        on_b.push(pb[0].0 + (pb[1].0 - pb[0].0) * v);
                    }
                }
            }
        }
    }
    // A line that ends on another cuts that one there.
    for (_, end) in ends(a) {
        let end = a_affine.apply(end);
        if let Some((t, away)) = nearest_on(&b_samples, end) {
            if away <= MEET {
                on_b.push(t);
            }
        }
    }
    for (_, end) in ends(b) {
        let end = b_affine.apply(end);
        if let Some((t, away)) = nearest_on(&a_samples, end) {
            if away <= MEET {
                on_a.push(t);
            }
        }
    }
    (on_a, on_b)
}

/// `params` sorted, without ones at the ends of `path` or next to each
/// other.
fn cut_points(path: &VPath, mut params: Vec<f32>) -> Vec<f32> {
    let total = segment_count(path) as f32;
    params.retain(|t| t.is_finite());
    params.sort_by(f32::total_cmp);
    let mut kept: Vec<f32> = Vec::new();
    for t in params {
        if !path.closed && (t <= MIN_PIECE || t >= total - MIN_PIECE) {
            continue;
        }
        if kept.last().is_some_and(|last| t - last <= MIN_PIECE) {
            continue;
        }
        kept.push(t);
    }
    if path.closed && kept.len() > 1 {
        let first = kept[0];
        if total - kept[kept.len() - 1] + first <= MIN_PIECE {
            kept.pop();
        }
    }
    kept
}

fn same(a: Vec2, b: Vec2) -> bool {
    distance(a, b) <= 1.0e-4
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

/// `path` cut at the parameters `cuts` (sorted, inside the path) into open
/// pieces. A closed path cut once opens into one line from the cut round
/// to it again.
pub(crate) fn split_path(path: &VPath, cuts: &[f32]) -> Vec<VPath> {
    if cuts.is_empty() || segment_count(path) == 0 {
        return vec![path.clone()];
    }
    let mut pieces: Vec<Vec<Cubic>> = vec![Vec::new()];
    for segment in 0..segment_count(path) {
        let whole = cubic(path, segment);
        // A straight segment is cut into straight pieces without handles.
        let straight = same(whole[1], whole[0]) && same(whole[2], whole[3]);
        let mut rest = whole;
        let mut done = 0.0;
        for t in cuts
            .iter()
            .filter(|t| t.floor() as usize == segment)
            .map(|t| t - segment as f32)
        {
            let (left, right) = if straight {
                let cut = at(&whole, t);
                ([rest[0], rest[0], cut, cut], [cut, cut, whole[3], whole[3]])
            } else {
                split_cubic(&rest, ((t - done) / (1.0 - done)).clamp(0.0, 1.0))
            };
            pieces.last_mut().expect("a piece").push(left);
            pieces.push(Vec::new());
            rest = right;
            done = t;
        }
        pieces.last_mut().expect("a piece").push(rest);
    }
    if path.closed {
        // The run before the first cut carries on after the last one.
        let head = pieces.remove(0);
        pieces.last_mut().expect("cut at least once").extend(head);
    }
    pieces
        .into_iter()
        .filter(|cubics| !cubics.is_empty())
        .map(|cubics| VPath {
            anchors: anchors_of(&cubics),
            closed: false,
        })
        .collect()
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
        cuttable(vector, path).then(|| path.clone())
    }) else {
        return false;
    };
    // Parameters to cut at, per (asset, path).
    let mut cuts: std::collections::BTreeMap<(u16, usize), Vec<f32>> = Default::default();
    for &(_, other_asset, affine) in &placed {
        let Some(other) = vector(project, other_asset) else {
            continue;
        };
        for (path_idx, path) in other.paths.iter().enumerate() {
            if (other_asset == asset_id && path_idx == new_path) || !cuttable(other, path) {
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
    let mut cut_any = false;
    let mut by_asset: std::collections::BTreeMap<u16, Vec<(usize, Vec<f32>)>> = Default::default();
    for ((asset, path_idx), params) in cuts {
        let Some(path) = vector(project, asset).and_then(|vector| vector.paths.get(path_idx))
        else {
            continue;
        };
        let params = cut_points(path, params);
        if !params.is_empty() {
            by_asset.entry(asset).or_default().push((path_idx, params));
        }
    }
    if by_asset.is_empty() {
        return false;
    }
    let writable = crate::brush::prepare_writable_raw_assets(
        project,
        q0rg_id,
        layer_id,
        frame,
        &by_asset.keys().copied().collect(),
    );
    for (asset, paths) in by_asset {
        let asset = *writable.get(&asset).unwrap_or(&asset);
        let Some(Asset::Vector(vector)) = project.assets.iter_mut().find(|item| item.id() == asset)
        else {
            continue;
        };
        for (path_idx, params) in paths {
            let Some(path) = vector.paths.get(path_idx).cloned() else {
                continue;
            };
            let mut pieces = split_path(&path, &params).into_iter();
            let Some(first) = pieces.next() else {
                continue;
            };
            // The first piece keeps the path's place, so other paths keep
            // their indices; the rest go on the end.
            vector.paths[path_idx] = first;
            vector.paths.extend(pieces);
            cut_any = true;
        }
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
        let on_a = cut_points(&a, on_a);
        let on_b = cut_points(&b, on_b);
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
        assert_eq!(cut_points(&a, on_a).len(), 1);
        assert!(cut_points(&t, on_t).is_empty(), "its own end is not a cut");
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
