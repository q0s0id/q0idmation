//! Shape tweens: morph one keyframe's vector artwork into the next keyframe's.
//!
//! A placement whose tween is [`Tween::Shape`] keeps its own transform for the
//! whole span. What changes is its geometry and paint: on every frame strictly
//! between the two keyframes the renderers ask [`layer_morph`] for replacement
//! vectors and draw those instead of the stored asset. The stored project is
//! never modified, so both keyframes stay ordinary editable drawings.
//!
//! Correspondence is geometric rather than authored: shapes, and the outlines
//! inside them, are paired by position and size. A pair of outlines is walked
//! by arc length from the start point that moves it least, with a point
//! wherever either outline has one, so no corner or curve of either drawing
//! is lost.
use std::collections::HashMap;

use crate::geom::CapShape;
use crate::transform::Affine;
use crate::v2::{Anchor, Layer, Path, Rgba, Stroke, Target, Tween, Vec2, VectorAsset};

const SAMPLES_PER_CURVE: usize = 16;
/// Resolution used only to choose matching start points and directions.
const ALIGN_SAMPLES: usize = 128;
/// Keeps holes with holes and open lines with open lines while any such
/// pairing is still available.
const MISMATCH_PENALTY: f32 = 1.0e9;
const SWAP_PASSES: usize = 8;

/// The shape-tween span a frame lies strictly inside.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeSpan {
    pub from: u16,
    pub to: u16,
    /// Eased progress in `0..=1`.
    pub t: f32,
}

/// Replacement vectors for the shape-tweened placements of one layer frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerMorph {
    shapes: HashMap<usize, Vec<VectorAsset>>,
}

impl LayerMorph {
    /// The vectors to draw instead of the placement at `index`, expressed in
    /// that placement's own local space. Usually one; several when the next
    /// keyframe holds more shapes than this one and a shape has to split.
    pub fn for_placement(&self, index: usize) -> Option<&[VectorAsset]> {
        self.shapes.get(&index).map(Vec::as_slice)
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    pub fn placements(&self) -> impl Iterator<Item = (usize, &[VectorAsset])> {
        self.shapes
            .iter()
            .map(|(index, shapes)| (*index, shapes.as_slice()))
    }
}

/// True when `frame` needs morphed artwork on this layer. Cheap enough for
/// render loops: it only looks at the held keyframe's placements.
pub fn span_at(layer: &Layer, frame: u16) -> Option<ShapeSpan> {
    if layer
        .timeline_frame_count
        .is_some_and(|frame_count| frame >= frame_count)
    {
        return None;
    }
    let keyframe = layer
        .explicit_keyframes
        .iter()
        .copied()
        .chain(layer.placements.iter().map(|placement| placement.frame))
        .filter(|candidate| *candidate <= frame)
        .max()?;
    if frame == keyframe {
        return None;
    }
    layer
        .placements
        .iter()
        .filter(|placement| placement.frame == keyframe)
        .find_map(|placement| match placement.tween {
            Tween::Shape { to_frame, easing } if to_frame > frame => {
                let raw = f32::from(frame - keyframe) / f32::from(to_frame - keyframe);
                Some(ShapeSpan {
                    from: keyframe,
                    to: to_frame,
                    t: easing.sample(raw.clamp(0.0, 1.0)).clamp(0.0, 1.0),
                })
            }
            _ => None,
        })
}

/// Morphed artwork for `frame`, or `None` when the frame is not inside a shape
/// tween. `vector` resolves an asset id to its vector asset.
pub fn layer_morph<'a>(
    layer: &Layer,
    frame: u16,
    vector: impl Fn(u16) -> Option<&'a VectorAsset>,
) -> Option<LayerMorph> {
    let span = span_at(layer, frame)?;
    let mut sources = Vec::new();
    let mut destinations = Vec::new();
    for (index, placement) in layer.placements.iter().enumerate() {
        let Target::Asset(asset_id) = placement.target else {
            continue;
        };
        let affine = Affine::from_transform(placement.transform);
        if placement.frame == span.from {
            if matches!(placement.tween, Tween::Shape { to_frame, .. } if to_frame == span.to) {
                if let Some(asset) = vector(asset_id) {
                    sources.push((index, affine, Shape::new(asset, affine)));
                }
            }
        } else if placement.frame == span.to {
            if let Some(asset) = vector(asset_id) {
                destinations.push(Shape::new(asset, affine));
            }
        }
    }
    if sources.is_empty() {
        return None;
    }

    let source_shapes = sources
        .iter()
        .map(|(_, _, shape)| shape)
        .collect::<Vec<_>>();
    let targets = pair_shapes(&source_shapes, &destinations);
    let mut shapes = HashMap::with_capacity(sources.len());
    for ((index, affine, source), targets) in sources.iter().zip(targets) {
        let to_local = affine.inverse().unwrap_or(Affine::IDENTITY);
        let local_scale = affine.uniform_scale().max(1e-6);
        let morphed = if targets.is_empty() {
            // Nothing to become: collapse into the shape's own centre and fade.
            vec![morph(
                source,
                &Shape::vanished(source.centroid),
                span.t,
                to_local,
                local_scale,
            )]
        } else {
            targets
                .into_iter()
                .map(|target| morph(source, &destinations[target], span.t, to_local, local_scale))
                .collect()
        };
        shapes.insert(*index, morphed);
    }
    Some(LayerMorph { shapes })
}

/// [`layer_morph`] for callers without their own asset index.
pub fn project_layer_morph(
    project: &crate::v2::ProjectV2,
    layer: &Layer,
    frame: u16,
) -> Option<LayerMorph> {
    // Checked first so ordinary layers never pay for an asset scan.
    span_at(layer, frame)?;
    layer_morph(layer, frame, |asset_id| {
        project.assets.iter().find_map(|asset| match asset {
            crate::v2::Asset::Vector(vector) if vector.asset_id == asset_id => Some(vector),
            _ => None,
        })
    })
}

#[derive(Debug, Clone)]
struct Contour {
    points: Vec<Vec2>,
    /// Arc-length position of every point, normalised to `0..1`. A closed
    /// contour's last point is implicitly followed by its first at `1`.
    at: Vec<f32>,
    closed: bool,
    /// Signed area; zero for open contours.
    area: f32,
    centre: Vec2,
    extent: f32,
    /// Winds against the shape's main outline, i.e. cuts a hole in it.
    hole: bool,
}

impl Contour {
    fn new(points: Vec<Vec2>, closed: bool) -> Self {
        let area = if closed { signed_area(&points) } else { 0.0 };
        let (centre, extent) = bounds_centre_and_extent(&points);
        let at = arc_positions(&points, closed);
        Self {
            points,
            at,
            closed,
            area,
            centre,
            extent,
            hole: false,
        }
    }

    fn reversed(&self) -> Self {
        let mut points = self.points.clone();
        points.reverse();
        Self {
            hole: self.hole,
            ..Self::new(points, self.closed)
        }
    }

    /// The point at arc-length position `u`. Closed contours wrap.
    fn eval(&self, u: f32) -> Vec2 {
        let count = self.points.len();
        if count == 1 {
            return self.points[0];
        }
        let u = if self.closed {
            u.rem_euclid(1.0)
        } else {
            u.clamp(0.0, 1.0)
        };
        // First vertex strictly after `u`; the segment before it holds `u`.
        let next = self.at.partition_point(|at| *at <= u);
        let from = next.saturating_sub(1).min(count - 1);
        let (to, end) = if next < count {
            (next, self.at[next])
        } else if self.closed {
            (0, 1.0)
        } else {
            return self.points[count - 1];
        };
        let start = self.at[from];
        let span = end - start;
        let along = if span > f32::EPSILON {
            ((u - start) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        lerp_point(self.points[from], self.points[to], along)
    }
}

/// A vector asset flattened into layer space.
#[derive(Debug, Clone)]
struct Shape {
    asset_id: u16,
    contours: Vec<Contour>,
    fill: Option<Rgba>,
    stroke: Option<Stroke>,
    centroid: Vec2,
    extent: f32,
}

impl Shape {
    fn new(asset: &VectorAsset, affine: Affine) -> Self {
        let scale = affine.uniform_scale();
        let mut contours = asset
            .paths
            .iter()
            .filter_map(|path| {
                let mut points = crate::geom::flatten_path(path, SAMPLES_PER_CURVE)
                    .into_iter()
                    .map(|point| affine.apply(point))
                    .collect::<Vec<_>>();
                if path.closed && points.len() > 1 && points.first() == points.last() {
                    points.pop();
                }
                // Repeated points carry no shape and would stall the walk
                // along the outline.
                points.dedup();
                (!points.is_empty()).then(|| Contour::new(points, path.closed))
            })
            .collect::<Vec<_>>();
        // Largest outline first: it is the shape's main outline, and ties in
        // the pairing below then favour the bigger contours.
        contours.sort_by(|left, right| {
            contour_weight(right)
                .partial_cmp(&contour_weight(left))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let main_winding = contours
            .iter()
            .find(|contour| contour.closed)
            .map_or(0.0, |contour| contour.area);
        for contour in &mut contours {
            contour.hole = contour.closed && contour.area * main_winding < 0.0;
        }
        let all_points = contours
            .iter()
            .flat_map(|contour| contour.points.iter().copied())
            .collect::<Vec<_>>();
        let (centroid, extent) = bounds_centre_and_extent(&all_points);
        Self {
            asset_id: asset.asset_id,
            contours,
            fill: asset.fill,
            stroke: asset.stroke.map(|stroke| Stroke {
                width: stroke.width * scale,
                ..stroke
            }),
            centroid,
            extent,
        }
    }

    fn vanished(at: Vec2) -> Self {
        Self {
            asset_id: 0,
            contours: Vec::new(),
            fill: None,
            stroke: None,
            centroid: at,
            extent: 0.0,
        }
    }
}

fn contour_weight(contour: &Contour) -> f32 {
    if contour.closed {
        contour.area.abs()
    } else {
        // Open strokes have no area; rank them by length below any outline of
        // comparable size.
        polyline_length(&contour.points, false) * 1e-3
    }
}

fn signed_area(points: &[Vec2]) -> f32 {
    let mut twice = 0.0;
    for (index, point) in points.iter().enumerate() {
        let next = points[(index + 1) % points.len()];
        twice += point.x * next.y - next.x * point.y;
    }
    twice * 0.5
}

fn bounds_centre_and_extent(points: &[Vec2]) -> (Vec2, f32) {
    let mut min = Vec2::new(f32::INFINITY, f32::INFINITY);
    let mut max = Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for point in points {
        min.x = min.x.min(point.x);
        min.y = min.y.min(point.y);
        max.x = max.x.max(point.x);
        max.y = max.y.max(point.y);
    }
    if !min.x.is_finite() {
        return (Vec2::new(0.0, 0.0), 0.0);
    }
    (
        Vec2::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5),
        (max.x - min.x).hypot(max.y - min.y),
    )
}

fn distance(left: Vec2, right: Vec2) -> f32 {
    (left.x - right.x).hypot(left.y - right.y)
}

struct Pairing {
    /// Every `(cost, left, right)` combination, cheapest first.
    candidates: Vec<(f32, usize, usize)>,
    /// The right-hand partner of each left-hand item, if it got one.
    partner: Vec<Option<usize>>,
    right_used: Vec<bool>,
}

/// One-to-one matching: cheapest pair first, then improved by trading
/// partners. Ties keep authoring order, so identical drawings pair index to
/// index.
fn cheapest_pairs(
    left_count: usize,
    right_count: usize,
    cost: impl Fn(usize, usize) -> f32,
) -> Pairing {
    let mut candidates = Vec::with_capacity(left_count * right_count);
    for left in 0..left_count {
        for right in 0..right_count {
            candidates.push((cost(left, right), left, right));
        }
    }
    candidates.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });
    let mut partner = vec![None; left_count];
    let mut right_used = vec![false; right_count];
    for &(_, left, right) in &candidates {
        if partner[left].is_none() && !right_used[right] {
            partner[left] = Some(right);
            right_used[right] = true;
        }
    }
    // Cheapest-first can strand two outlines with each other's partner (a
    // large one grabbing the small one it happens to resemble most). Trading
    // partners wherever that lowers the combined cost undoes those.
    for _ in 0..SWAP_PASSES {
        let mut improved = false;
        for first in 0..left_count {
            for second in first + 1..left_count {
                let (Some(a), Some(b)) = (partner[first], partner[second]) else {
                    continue;
                };
                let kept = cost(first, a) + cost(second, b);
                let traded = cost(first, b) + cost(second, a);
                if traded + 1e-4 < kept {
                    partner[first] = Some(b);
                    partner[second] = Some(a);
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    Pairing {
        candidates,
        partner,
        right_used,
    }
}

/// For every source shape, the destination shapes it turns into.
///
/// Shapes are matched closest and most similar first. When the two keyframes
/// hold different numbers of shapes nothing is dropped: a spare source merges
/// into its nearest destination, and a spare destination splits off its
/// nearest source.
fn pair_shapes(sources: &[&Shape], destinations: &[Shape]) -> Vec<Vec<usize>> {
    let mut targets = vec![Vec::new(); sources.len()];
    if destinations.is_empty() {
        return targets;
    }
    let Pairing {
        candidates,
        partner,
        right_used: mut destination_used,
    } = cheapest_pairs(sources.len(), destinations.len(), |source, destination| {
        distance(sources[source].centroid, destinations[destination].centroid)
            + (sources[source].extent - destinations[destination].extent).abs()
    });
    let mut source_used = vec![false; sources.len()];
    for (source, destination) in partner.iter().enumerate() {
        if let Some(destination) = destination {
            targets[source].push(*destination);
            source_used[source] = true;
        }
    }
    for &(_, source, destination) in &candidates {
        if !source_used[source] {
            source_used[source] = true;
            targets[source].push(destination);
        } else if !destination_used[destination] {
            destination_used[destination] = true;
            targets[source].push(destination);
        }
    }
    targets
}

fn morph(
    source: &Shape,
    destination: &Shape,
    t: f32,
    to_local: Affine,
    local_scale: f32,
) -> VectorAsset {
    // Outlines are matched by where they are and how big they are, never by
    // their order: a drawing of many strokes keeps every stroke in place. A
    // hole only pairs with a hole, and an open line only with an open line,
    // unless nothing else is left.
    let Pairing {
        partner,
        right_used: destination_used,
        ..
    } = cheapest_pairs(
        source.contours.len(),
        destination.contours.len(),
        |from, to| {
            let from = &source.contours[from];
            let to = &destination.contours[to];
            let mismatch = if from.closed != to.closed || from.hole != to.hole {
                MISMATCH_PENALTY
            } else {
                0.0
            };
            distance(from.centre, to.centre) + (from.extent - to.extent).abs() + mismatch
        },
    );
    // Paired outlines take the source's winding. If that turned the
    // destination's main outline around, outlines that only exist in the
    // destination must turn with it, or its holes would fill in until the
    // last frame.
    let turned = match (source.contours.first(), destination.contours.first()) {
        (Some(from), Some(to)) => from.area * to.area < 0.0,
        _ => false,
    };

    let mut paths = Vec::with_capacity(source.contours.len() + destination.contours.len());
    let mut push = |points: Vec<Vec2>, closed: bool| {
        paths.push(Path {
            anchors: points
                .into_iter()
                .map(|point| Anchor {
                    point: to_local.apply(point),
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
            closed,
        });
    };
    for (from, partner) in source.contours.iter().zip(&partner) {
        match partner {
            Some(to) => {
                let (points, closed) = morph_contour(from, &destination.contours[*to], t);
                push(points, closed);
            }
            // An outline with nothing to become shrinks away where it is.
            None => push(
                from.points
                    .iter()
                    .map(|point| lerp_point(*point, from.centre, t))
                    .collect(),
                from.closed,
            ),
        }
    }
    for (to, used) in destination.contours.iter().zip(&destination_used) {
        if *used {
            continue;
        }
        // A new outline grows where it will end up.
        let mut points = to
            .points
            .iter()
            .map(|point| lerp_point(to.centre, *point, t))
            .collect::<Vec<_>>();
        if turned && to.closed {
            points.reverse();
        }
        push(points, to.closed);
    }
    VectorAsset {
        asset_id: source.asset_id,
        paths,
        fill: lerp_paint(source.fill, destination.fill, t),
        stroke: lerp_stroke(source.stroke, destination.stroke, t, local_scale),
    }
}

/// Blend two outlines without losing a single corner of either.
///
/// Both outlines are walked by arc length. The result has a point wherever
/// *either* outline has one, so at `t = 0` it is exactly the first drawing and
/// at `t = 1` exactly the second, however detailed they are. Only the choice
/// of matching start point and direction is made on a coarse resampling.
fn morph_contour(from: &Contour, to: &Contour, t: f32) -> (Vec<Vec2>, bool) {
    if from.closed != to.closed {
        // Closed on one side only: keep that side's topology for its half of
        // the tween and fall back to an even resampling.
        let samples = (from.points.len().max(to.points.len())).max(ALIGN_SAMPLES);
        let from_points = resample(&from.points, from.closed, samples);
        let mut to_points = resample(&to.points, to.closed, samples);
        align(&from_points, &mut to_points, false);
        let closed = if t < 0.5 { from.closed } else { to.closed };
        return (
            from_points
                .iter()
                .zip(&to_points)
                .map(|(from, to)| lerp_point(*from, *to, t))
                .collect(),
            closed,
        );
    }

    let closed = from.closed;
    let reverse = if closed {
        from.area * to.area < 0.0
    } else {
        // Run an open line whichever way moves it least.
        let from_points = resample(&from.points, false, ALIGN_SAMPLES);
        let mut to_points = resample(&to.points, false, ALIGN_SAMPLES);
        let forward = travel(&from_points, &to_points, 0);
        to_points.reverse();
        travel(&from_points, &to_points, 0) < forward
    };
    let to = if reverse {
        std::borrow::Cow::Owned(to.reversed())
    } else {
        std::borrow::Cow::Borrowed(to)
    };

    // How far along `to` the point matching `from`'s start lies.
    let shift = if closed {
        let from_points = resample(&from.points, true, ALIGN_SAMPLES);
        let to_points = resample(&to.points, true, ALIGN_SAMPLES);
        let mut best = (f32::INFINITY, 0);
        for offset in 0..ALIGN_SAMPLES {
            let cost = travel(&from_points, &to_points, offset);
            if cost < best.0 {
                best = (cost, offset);
            }
        }
        best.1 as f32 / ALIGN_SAMPLES as f32
    } else {
        0.0
    };

    let mut stops = from.at.clone();
    stops.extend(to.at.iter().map(|at| {
        if closed {
            (at - shift).rem_euclid(1.0)
        } else {
            *at
        }
    }));
    stops.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    stops.dedup_by(|next, previous| (*next - *previous).abs() < 1e-6);
    if closed && stops.len() > 1 && (stops[stops.len() - 1] - 1.0).abs() < 1e-6 {
        // Position 1 is position 0 again.
        stops.pop();
    }
    (
        stops
            .into_iter()
            .map(|stop| lerp_point(from.eval(stop), to.eval(stop + shift), t))
            .collect(),
        closed,
    )
}

fn polyline_length(points: &[Vec2], closed: bool) -> f32 {
    let mut length = points
        .windows(2)
        .map(|pair| distance(pair[0], pair[1]))
        .sum::<f32>();
    if closed && points.len() > 1 {
        length += distance(points[points.len() - 1], points[0]);
    }
    length
}

/// Normalised arc-length position of every point.
fn arc_positions(points: &[Vec2], closed: bool) -> Vec<f32> {
    let total = polyline_length(points, closed);
    if total <= f32::EPSILON {
        return vec![0.0; points.len()];
    }
    let mut walked = 0.0;
    let mut positions = Vec::with_capacity(points.len());
    for (index, point) in points.iter().enumerate() {
        if index > 0 {
            walked += distance(points[index - 1], *point);
        }
        positions.push((walked / total).min(1.0));
    }
    positions
}

/// `count` points spaced evenly by arc length. Closed contours do not repeat
/// their first point; open ones include both ends.
fn resample(points: &[Vec2], closed: bool, count: usize) -> Vec<Vec2> {
    let total = polyline_length(points, closed);
    if points.len() < 2 || total <= f32::EPSILON {
        return vec![points.first().copied().unwrap_or(Vec2::new(0.0, 0.0)); count];
    }
    let segments = if closed {
        points.len()
    } else {
        points.len() - 1
    };
    let step = if closed {
        total / count as f32
    } else {
        total / (count.max(2) - 1) as f32
    };
    let mut out = Vec::with_capacity(count);
    let mut segment = 0;
    let mut walked = 0.0;
    for sample in 0..count {
        let wanted = step * sample as f32;
        loop {
            let from = points[segment % points.len()];
            let to = points[(segment + 1) % points.len()];
            let length = distance(from, to);
            if wanted <= walked + length || segment + 1 >= segments {
                let along = if length > f32::EPSILON {
                    ((wanted - walked) / length).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                out.push(lerp_point(from, to, along));
                break;
            }
            walked += length;
            segment += 1;
        }
    }
    out
}

fn centre(points: &[Vec2]) -> Vec2 {
    let mut sum = Vec2::new(0.0, 0.0);
    for point in points {
        sum.x += point.x;
        sum.y += point.y;
    }
    let count = points.len().max(1) as f32;
    Vec2::new(sum.x / count, sum.y / count)
}

/// How far the points of `from` travel to reach `to` rotated by `offset`,
/// measured relative to each outline's own centre so that a shape which only
/// moves keeps its start point.
fn travel(from: &[Vec2], to: &[Vec2], offset: usize) -> f32 {
    let from_centre = centre(from);
    let to_centre = centre(to);
    let mut sum = 0.0;
    for (index, point) in from.iter().enumerate() {
        let other = to[(index + offset) % to.len()];
        let dx = (point.x - from_centre.x) - (other.x - to_centre.x);
        let dy = (point.y - from_centre.y) - (other.y - to_centre.y);
        sum += dx * dx + dy * dy;
    }
    sum
}

/// Reorder evenly resampled `to` so that each point travels as little as
/// possible: same winding, best start point, best direction.
fn align(from: &[Vec2], to: &mut [Vec2], closed: bool) {
    if !closed {
        let forward = travel(from, to, 0);
        to.reverse();
        if forward <= travel(from, to, 0) {
            to.reverse();
        }
        return;
    }
    if signed_area(from) * signed_area(to) < 0.0 {
        to.reverse();
    }
    let mut best = (f32::INFINITY, 0);
    for offset in 0..to.len() {
        let cost = travel(from, to, offset);
        if cost < best.0 {
            best = (cost, offset);
        }
    }
    to.rotate_left(best.1);
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

fn lerp_point(from: Vec2, to: Vec2, t: f32) -> Vec2 {
    Vec2::new(lerp(from.x, to.x, t), lerp(from.y, to.y, t))
}

fn lerp_channel(from: u8, to: u8, t: f32) -> u8 {
    lerp(f32::from(from), f32::from(to), t)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn lerp_rgba(from: Rgba, to: Rgba, t: f32) -> Rgba {
    Rgba {
        r: lerp_channel(from.r, to.r, t),
        g: lerp_channel(from.g, to.g, t),
        b: lerp_channel(from.b, to.b, t),
        a: lerp_channel(from.a, to.a, t),
    }
}

fn transparent(color: Rgba) -> Rgba {
    Rgba { a: 0, ..color }
}

/// A missing paint fades in or out in the other side's colour.
fn lerp_paint(from: Option<Rgba>, to: Option<Rgba>, t: f32) -> Option<Rgba> {
    match (from, to) {
        (Some(from), Some(to)) => Some(lerp_rgba(from, to, t)),
        (Some(from), None) => Some(lerp_rgba(from, transparent(from), t)),
        (None, Some(to)) => Some(lerp_rgba(transparent(to), to, t)),
        (None, None) => None,
    }
}

fn lerp_stroke(
    from: Option<Stroke>,
    to: Option<Stroke>,
    t: f32,
    local_scale: f32,
) -> Option<Stroke> {
    let (from, to) = match (from, to) {
        (Some(from), Some(to)) => (from, to),
        (Some(from), None) => (
            from,
            Stroke {
                color: transparent(from.color),
                ..from
            },
        ),
        (None, Some(to)) => (
            Stroke {
                color: transparent(to.color),
                ..to
            },
            to,
        ),
        (None, None) => return None,
    };
    let (cap, join): (CapShape, _) = if t < 0.5 {
        (from.cap, from.join)
    } else {
        (to.cap, to.join)
    };
    Some(Stroke {
        color: lerp_rgba(from.color, to.color, t),
        width: lerp(from.width, to.width, t) / local_scale,
        cap,
        join,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::{Easing, Placement, Transform2D};

    fn polygon(points: &[(f32, f32)]) -> Path {
        Path {
            anchors: points
                .iter()
                .map(|(x, y)| Anchor {
                    point: Vec2::new(*x, *y),
                    in_handle: None,
                    out_handle: None,
                })
                .collect(),
            closed: true,
        }
    }

    fn square(min: f32, max: f32) -> Path {
        polygon(&[(min, min), (max, min), (max, max), (min, max)])
    }

    fn filled(asset_id: u16, paths: Vec<Path>, color: (u8, u8, u8)) -> VectorAsset {
        VectorAsset {
            asset_id,
            paths,
            fill: Some(Rgba {
                r: color.0,
                g: color.1,
                b: color.2,
                a: 255,
            }),
            stroke: None,
        }
    }

    fn placement(frame: u16, asset_id: u16, tween: Tween) -> Placement {
        Placement {
            pivot: None,
            instance_id: 0,
            frame,
            target: Target::Asset(asset_id),
            transform: Transform2D::IDENTITY,
            tween,
            fx: Default::default(),
        }
    }

    fn shape_to(to_frame: u16) -> Tween {
        Tween::Shape {
            to_frame,
            easing: Easing::Linear,
        }
    }

    fn layer(placements: Vec<Placement>) -> Layer {
        Layer {
            layer_id: 1,
            timeline_frame_count: None,
            name: "shapes".into(),
            explicit_keyframes: Vec::new(),
            placements,
        }
    }

    fn bounds(asset: &VectorAsset) -> (f32, f32, f32, f32) {
        let points = asset
            .paths
            .iter()
            .flat_map(|path| path.anchors.iter().map(|anchor| anchor.point));
        let mut out = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for point in points {
            out = (
                out.0.min(point.x),
                out.1.min(point.y),
                out.2.max(point.x),
                out.3.max(point.y),
            );
        }
        out
    }

    fn area(asset: &VectorAsset) -> f32 {
        let points = asset.paths[0]
            .anchors
            .iter()
            .map(|anchor| anchor.point)
            .collect::<Vec<_>>();
        signed_area(&points).abs()
    }

    fn assert_close(left: f32, right: f32, tolerance: f32) {
        assert!(
            (left - right).abs() <= tolerance,
            "{left} is not within {tolerance} of {right}"
        );
    }

    fn morph_at(layer: &Layer, assets: &[VectorAsset], frame: u16) -> Option<LayerMorph> {
        layer_morph(layer, frame, |id| {
            assets.iter().find(|asset| asset.asset_id == id)
        })
    }

    #[test]
    fn only_frames_strictly_inside_a_shape_tween_are_morphed() {
        let layer = layer(vec![
            placement(2, 1, shape_to(10)),
            placement(10, 2, Tween::None),
        ]);
        assert_eq!(span_at(&layer, 1), None, "before the first key");
        assert_eq!(span_at(&layer, 2), None, "the key itself is the drawing");
        assert_eq!(
            span_at(&layer, 6),
            Some(ShapeSpan {
                from: 2,
                to: 10,
                t: 0.5
            })
        );
        assert_eq!(span_at(&layer, 10), None);
        assert_eq!(span_at(&layer, 14), None);

        let held = self::layer(vec![placement(2, 1, Tween::None)]);
        assert_eq!(span_at(&held, 6), None, "ordinary holds are untouched");
    }

    #[test]
    fn a_square_grows_and_changes_colour_evenly() {
        let assets = [
            filled(1, vec![square(0.0, 10.0)], (200, 0, 0)),
            filled(2, vec![square(-10.0, 30.0)], (0, 0, 100)),
        ];
        let layer = layer(vec![
            placement(0, 1, shape_to(4)),
            placement(4, 2, Tween::None),
        ]);
        let morph = morph_at(&layer, &assets, 2).expect("mid tween");
        let shapes = morph.for_placement(0).expect("source placement");
        assert_eq!(shapes.len(), 1);
        let (min_x, min_y, max_x, max_y) = bounds(&shapes[0]);
        assert_close(min_x, -5.0, 0.01);
        assert_close(min_y, -5.0, 0.01);
        assert_close(max_x, 20.0, 0.01);
        assert_close(max_y, 20.0, 0.01);
        assert_close(area(&shapes[0]), 625.0, 1.0);
        assert_eq!(
            shapes[0].fill,
            Some(Rgba {
                r: 100,
                g: 0,
                b: 50,
                a: 255
            })
        );
        assert_eq!(shapes[0].asset_id, 1);
        assert!(
            morph.for_placement(1).is_none(),
            "the next key is not drawn"
        );
    }

    #[test]
    fn winding_and_start_point_do_not_twist_the_shape() {
        // The same square, drawn the other way round from a different corner.
        let assets = [
            filled(1, vec![square(0.0, 10.0)], (0, 0, 0)),
            filled(
                2,
                vec![polygon(&[
                    (10.0, 10.0),
                    (10.0, 0.0),
                    (0.0, 0.0),
                    (0.0, 10.0),
                ])],
                (0, 0, 0),
            ),
        ];
        let layer = layer(vec![
            placement(0, 1, shape_to(4)),
            placement(4, 2, Tween::None),
        ]);
        for frame in 1..4 {
            let morph = morph_at(&layer, &assets, frame).unwrap();
            let shape = &morph.for_placement(0).unwrap()[0];
            assert_close(area(shape), 100.0, 0.5);
            let (min_x, min_y, max_x, max_y) = bounds(shape);
            assert_close(min_x, 0.0, 0.01);
            assert_close(min_y, 0.0, 0.01);
            assert_close(max_x, 10.0, 0.01);
            assert_close(max_y, 10.0, 0.01);
        }
    }

    #[test]
    fn a_moving_shape_is_paired_by_shape_not_by_position() {
        let assets = [
            filled(1, vec![square(0.0, 10.0)], (0, 0, 0)),
            filled(2, vec![square(100.0, 110.0)], (0, 0, 0)),
        ];
        let layer = layer(vec![
            placement(0, 1, shape_to(2)),
            placement(2, 2, Tween::None),
        ]);
        let morph = morph_at(&layer, &assets, 1).unwrap();
        let shape = &morph.for_placement(0).unwrap()[0];
        assert_close(area(shape), 100.0, 0.5);
        assert_close(bounds(shape).0, 50.0, 0.01);
    }

    #[test]
    fn output_is_in_the_source_placements_local_space() {
        let assets = [
            filled(1, vec![square(0.0, 10.0)], (0, 0, 0)),
            filled(2, vec![square(0.0, 10.0)], (0, 0, 0)),
        ];
        let mut from = placement(0, 1, shape_to(2));
        from.transform.tx = 40.0;
        from.transform.sx = 2.0;
        from.transform.sy = 2.0;
        let mut to = placement(2, 2, Tween::None);
        to.transform.tx = 60.0;
        let layer = layer(vec![from, to]);
        let morph = morph_at(&layer, &assets, 1).unwrap();
        let shape = &morph.for_placement(0).unwrap()[0];
        // Layer space: 40..60 becomes 60..70, so the midpoint is 50..65. Seen
        // through the source placement (x * 2 + 40) that is 5..12.5.
        let (min_x, _, max_x, _) = bounds(shape);
        assert_close(min_x, 5.0, 0.01);
        assert_close(max_x, 12.5, 0.01);
    }

    #[test]
    fn holes_pair_with_holes() {
        let ring = |outer: f32, inner: f32| vec![square(-outer, outer), square(-inner, inner)];
        let assets = [
            filled(1, ring(10.0, 2.0), (0, 0, 0)),
            filled(2, ring(20.0, 8.0), (0, 0, 0)),
        ];
        let layer = layer(vec![
            placement(0, 1, shape_to(2)),
            placement(2, 2, Tween::None),
        ]);
        let morph = morph_at(&layer, &assets, 1).unwrap();
        let shape = &morph.for_placement(0).unwrap()[0];
        assert_eq!(shape.paths.len(), 2);
        let widths = shape
            .paths
            .iter()
            .map(|path| {
                let xs = path.anchors.iter().map(|anchor| anchor.point.x);
                xs.clone().fold(f32::NEG_INFINITY, f32::max) - xs.fold(f32::INFINITY, f32::min)
            })
            .collect::<Vec<_>>();
        assert_close(widths[0], 30.0, 0.01);
        assert_close(widths[1], 10.0, 0.01);
    }

    #[test]
    fn a_hole_that_appears_mid_tween_is_a_hole_from_the_start() {
        // The ring is drawn the other way round from the solid square, so its
        // outer edge has to be turned to match; its hole must turn with it.
        let assets = [
            filled(1, vec![square(-10.0, 10.0)], (0, 0, 0)),
            filled(
                2,
                vec![
                    polygon(&[(-20.0, -20.0), (-20.0, 20.0), (20.0, 20.0), (20.0, -20.0)]),
                    square(-8.0, 8.0),
                ],
                (0, 0, 0),
            ),
        ];
        let layer = layer(vec![
            placement(0, 1, shape_to(4)),
            placement(4, 2, Tween::None),
        ]);
        for frame in 1..4 {
            let morph = morph_at(&layer, &assets, frame).unwrap();
            let shape = &morph.for_placement(0).unwrap()[0];
            assert_eq!(shape.paths.len(), 2);
            let windings = shape
                .paths
                .iter()
                .map(|path| {
                    let points = path
                        .anchors
                        .iter()
                        .map(|anchor| anchor.point)
                        .collect::<Vec<_>>();
                    signed_area(&points)
                })
                .collect::<Vec<_>>();
            assert!(
                windings[0] * windings[1] < 0.0,
                "frame {frame}: the hole must wind against its outline, got {windings:?}"
            );
        }
    }

    fn path_points(path: &Path) -> Vec<Vec2> {
        path.anchors.iter().map(|anchor| anchor.point).collect()
    }

    /// A gear with `teeth` sharp teeth: plenty of corners to lose.
    fn gear(teeth: usize, inner: f32, outer: f32) -> Path {
        let mut points = Vec::new();
        for tooth in 0..teeth * 2 {
            let radius = if tooth % 2 == 0 { outer } else { inner };
            let angle = std::f32::consts::TAU * tooth as f32 / (teeth * 2) as f32;
            points.push((radius * angle.cos(), radius * angle.sin()));
        }
        polygon(&points)
    }

    #[test]
    fn detailed_drawings_keep_every_corner() {
        // 600 corners on each side; the old 192-point resampling rounded these
        // off on every in-between frame.
        let assets = [
            filled(1, vec![gear(300, 40.0, 50.0)], (0, 0, 0)),
            filled(2, vec![gear(300, 40.0, 50.0)], (0, 0, 0)),
        ];
        let layer = layer(vec![
            placement(0, 1, shape_to(10)),
            placement(10, 2, Tween::None),
        ]);
        let wanted = path_points(&assets[0].paths[0]);
        for frame in [1, 5, 9] {
            let morph = morph_at(&layer, &assets, frame).unwrap();
            let shape = &morph.for_placement(0).unwrap()[0];
            let points = path_points(&shape.paths[0]);
            assert!(points.len() >= wanted.len());
            for corner in &wanted {
                let nearest = points
                    .iter()
                    .map(|point| distance(*point, *corner))
                    .fold(f32::INFINITY, f32::min);
                assert!(nearest < 0.02, "frame {frame} lost a corner by {nearest}");
            }
            assert_close(area(shape), signed_area(&wanted).abs(), 0.5);
        }
    }

    #[test]
    fn the_last_in_between_is_almost_the_next_drawing() {
        // Different drawings with different point counts: one frame before the
        // end, every corner of the destination is already nearly in place.
        let assets = [
            filled(1, vec![square(-30.0, 30.0)], (0, 0, 0)),
            filled(2, vec![gear(40, 40.0, 50.0)], (0, 0, 0)),
        ];
        let layer = layer(vec![
            placement(0, 1, shape_to(100)),
            placement(100, 2, Tween::None),
        ]);
        let morph = morph_at(&layer, &assets, 99).unwrap();
        let points = path_points(&morph.for_placement(0).unwrap()[0].paths[0]);
        for corner in path_points(&assets[1].paths[0]) {
            let nearest = points
                .iter()
                .map(|point| distance(*point, corner))
                .fold(f32::INFINITY, f32::min);
            assert!(nearest < 0.5, "a destination corner is {nearest} away");
        }
    }

    #[test]
    fn strokes_inside_a_drawing_pair_by_position_not_by_order() {
        // The same two dots, listed in the opposite order on the next key and
        // with the far one a touch bigger so a size ranking would swap them.
        let dot =
            |x: f32, size: f32| polygon(&[(x, 0.0), (x + size, 0.0), (x + size, size), (x, size)]);
        let assets = [
            filled(1, vec![dot(0.0, 10.0), dot(100.0, 9.0)], (0, 0, 0)),
            filled(2, vec![dot(100.0, 11.0), dot(0.0, 10.0)], (0, 0, 0)),
        ];
        let layer = layer(vec![
            placement(0, 1, shape_to(2)),
            placement(2, 2, Tween::None),
        ]);
        let morph = morph_at(&layer, &assets, 1).unwrap();
        let shape = &morph.for_placement(0).unwrap()[0];
        assert_eq!(shape.paths.len(), 2);
        for path in &shape.paths {
            let xs = path_points(path);
            let min = xs.iter().map(|point| point.x).fold(f32::INFINITY, f32::min);
            assert!(
                min.abs() < 0.01 || (min - 100.0).abs() < 0.01,
                "a dot wandered to x = {min}"
            );
        }
    }

    #[test]
    fn unequal_shape_counts_split_and_merge_without_dropping_artwork() {
        let assets = [
            filled(1, vec![square(0.0, 10.0)], (0, 0, 0)),
            filled(2, vec![square(0.0, 10.0)], (0, 0, 0)),
            filled(3, vec![square(50.0, 60.0)], (0, 0, 0)),
        ];
        let split = layer(vec![
            placement(0, 1, shape_to(2)),
            placement(2, 2, Tween::None),
            placement(2, 3, Tween::None),
        ]);
        let morph = morph_at(&split, &assets, 1).unwrap();
        assert_eq!(morph.for_placement(0).unwrap().len(), 2);

        let merge = layer(vec![
            placement(0, 2, shape_to(2)),
            placement(0, 3, shape_to(2)),
            placement(2, 1, Tween::None),
        ]);
        let morph = morph_at(&merge, &assets, 1).unwrap();
        assert_eq!(morph.for_placement(0).unwrap().len(), 1);
        assert_eq!(morph.for_placement(1).unwrap().len(), 1);
        let far = &morph.for_placement(1).unwrap()[0];
        assert_close(bounds(far).0, 25.0, 0.01);
    }

    #[test]
    fn a_shape_with_nothing_to_become_shrinks_and_fades() {
        let assets = [filled(1, vec![square(0.0, 10.0)], (10, 20, 30))];
        let mut layer = layer(vec![placement(0, 1, shape_to(4))]);
        layer.explicit_keyframes.push(4);
        let morph = morph_at(&layer, &assets, 3).unwrap();
        let shape = &morph.for_placement(0).unwrap()[0];
        assert_close(area(shape), 100.0 / 16.0, 0.2);
        assert_eq!(shape.fill.unwrap().a, 64);
        assert_eq!(shape.fill.unwrap().r, 10);
    }

    #[test]
    fn strokes_interpolate_width_and_fade_in_when_new() {
        let line = |y: f32| Path {
            anchors: vec![
                Anchor {
                    point: Vec2::new(0.0, y),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(10.0, y),
                    in_handle: None,
                    out_handle: None,
                },
            ],
            closed: false,
        };
        let stroke = |width: f32| {
            Some(Stroke {
                color: Rgba {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                width,
                cap: CapShape::Round,
                join: crate::geom::JoinShape::Bevel,
            })
        };
        let assets = [
            VectorAsset {
                asset_id: 1,
                paths: vec![line(0.0)],
                fill: None,
                stroke: stroke(2.0),
            },
            VectorAsset {
                asset_id: 2,
                // Drawn right to left: the tween must not flip the line.
                paths: vec![Path {
                    anchors: line(20.0).anchors.into_iter().rev().collect(),
                    closed: false,
                }],
                fill: None,
                stroke: stroke(6.0),
            },
        ];
        let layer = layer(vec![
            placement(0, 1, shape_to(2)),
            placement(2, 2, Tween::None),
        ]);
        let morph = morph_at(&layer, &assets, 1).unwrap();
        let shape = &morph.for_placement(0).unwrap()[0];
        assert_close(shape.stroke.unwrap().width, 4.0, 0.001);
        assert!(!shape.paths[0].closed);
        let first = shape.paths[0].anchors.first().unwrap().point;
        let last = shape.paths[0].anchors.last().unwrap().point;
        assert_close(first.x, 0.0, 0.01);
        assert_close(last.x, 10.0, 0.01);
        assert_close(first.y, 10.0, 0.01);
        assert_eq!(shape.fill, None);
    }

    #[test]
    fn easing_shapes_the_progress() {
        let assets = [
            filled(1, vec![square(0.0, 10.0)], (0, 0, 0)),
            filled(2, vec![square(0.0, 30.0)], (0, 0, 0)),
        ];
        let layer = layer(vec![
            placement(
                0,
                1,
                Tween::Shape {
                    to_frame: 10,
                    easing: Easing::Hold,
                },
            ),
            placement(10, 2, Tween::None),
        ]);
        let morph = morph_at(&layer, &assets, 9).unwrap();
        assert_close(bounds(&morph.for_placement(0).unwrap()[0]).2, 10.0, 0.01);
    }
}
