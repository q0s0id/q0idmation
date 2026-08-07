use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use geo::{
    Area, BooleanOps, BoundingRect, Buffer, Contains, ConvexHull, Coord, Line, LineString,
    MultiPoint, MultiPolygon, Point, Polygon, SimplifyVwPreserve,
};
use q0s_format::v2::{
    Anchor, Asset, Path as VPath, Placement, ProjectV2, Rgba, Target, Transform2D, Tween, Vec2,
    VectorAsset, VectorMaterial,
};

use crate::app::EditorApp;
use crate::render::flatten_path;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BrushNib {
    #[default]
    Circle,
    Square,
    Horizontal,
    Vertical,
    Slash,
    Backslash,
}

impl BrushNib {
    pub const ALL: [Self; 6] = [
        Self::Circle,
        Self::Square,
        Self::Horizontal,
        Self::Vertical,
        Self::Slash,
        Self::Backslash,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Circle => "Circle",
            Self::Square => "Square",
            Self::Horizontal => "Horizontal",
            Self::Vertical => "Vertical",
            Self::Slash => "Slash /",
            Self::Backslash => "Backslash \\",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushSettings {
    pub color: Rgba,
    pub size: f32,
    pub smoothing: u8,
    pub nib: BrushNib,
    /// Optional soft outer glow material. Classic brush paint is plain vector
    /// fill by default; glow must be explicitly enabled by the user.
    pub glow: bool,
    pub scale_with_stage: bool,
    pub sync_with_eraser: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            color: Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            size: 10.0,
            smoothing: 50,
            nib: BrushNib::Circle,
            glow: false,
            scale_with_stage: true,
            sync_with_eraser: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushSample {
    pub position: Vec2,
    pub pressure: Option<f32>,
}

impl BrushSample {
    pub fn mouse(position: Vec2) -> Self {
        Self {
            position,
            pressure: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BrushStroke {
    pub samples: Vec<BrushSample>,
    pub coverage: MultiPolygon<f64>,
    preview_paths: Vec<VPath>,
    nib: BrushNib,
    size: f32,
    smoothing: u8,
    pub dirty_preview: bool,
}

pub fn brush_begin(settings: BrushSettings, sample: BrushSample) -> BrushStroke {
    let coverage = sweep_nib(
        settings.nib,
        settings.size,
        sample.position,
        sample.position,
    );
    let preview_paths = coverage_to_linear_paths(&coverage);
    BrushStroke {
        samples: vec![sample],
        coverage,
        preview_paths,
        nib: settings.nib,
        size: settings.size,
        smoothing: settings.smoothing,
        dirty_preview: false,
    }
}

pub fn brush_add_sample(stroke: &mut BrushStroke, settings: BrushSettings, sample: BrushSample) {
    stroke.samples.push(sample);
    stroke.nib = settings.nib;
    stroke.size = settings.size;
    stroke.smoothing = settings.smoothing;
    stroke.dirty_preview = true;
}

/// Materialize the vector coverage from all raw samples in one operation.
/// Circular live preview stays on the cheap round-stroke tessellator. Polygon
/// nibs derive the same fixed-angle silhouette without mutating this cached
/// commit coverage, so one gesture still has one authoritative sweep.
pub fn brush_flush_pending(stroke: &mut BrushStroke) {
    if !stroke.dirty_preview {
        return;
    }
    let trajectory = build_sweep_trajectory(&stroke.samples, stroke.smoothing, stroke.size);
    stroke.coverage = sweep_trajectory_nib(stroke.nib, stroke.size, &trajectory);
    stroke.preview_paths = coverage_to_linear_paths(&stroke.coverage);
    stroke.dirty_preview = false;
}

pub fn brush_preview_geometry(stroke: &BrushStroke) -> &[VPath] {
    &stroke.preview_paths
}

pub fn brush_preview_size(stroke: &BrushStroke) -> f32 {
    stroke.size
}

pub fn brush_preview_nib(stroke: &BrushStroke) -> BrushNib {
    stroke.nib
}

/// Materialize the current gesture silhouette for non-circular live previews.
/// Circle stays on the cheaper renderer path; fixed polygon nibs use this exact
/// sweep so preview and commit cannot disagree about corners or orientation.
pub fn brush_preview_paths_for_render(stroke: &BrushStroke) -> Vec<VPath> {
    if !stroke.dirty_preview {
        return stroke.preview_paths.clone();
    }
    let trajectory = build_sweep_trajectory(&stroke.samples, stroke.smoothing, stroke.size);
    coverage_to_linear_paths(&sweep_trajectory_nib(stroke.nib, stroke.size, &trajectory))
}

/// The live preview and committed coverage share this exact centre trajectory.
/// It keeps every available pointer sample, but gently attenuates only the
/// high-frequency, shallow turns that otherwise imprint a scalloped edge when
/// drawing zoomed out. Endpoints and deliberate sharp corners stay fixed.
pub fn brush_preview_trajectory(stroke: &BrushStroke) -> Vec<Vec2> {
    build_sweep_trajectory(&stroke.samples, stroke.smoothing, stroke.size)
}

pub fn brush_finish(mut stroke: BrushStroke, settings: BrushSettings) -> MultiPolygon<f64> {
    let is_single_dab = stroke.samples.len() == 1;
    stroke.nib = settings.nib;
    stroke.size = settings.size;
    stroke.smoothing = settings.smoothing;
    stroke.dirty_preview = true;
    brush_flush_pending(&mut stroke);
    if is_single_dab {
        // Smoothing is a boundary cleanup for a gesture. A one-sample gesture
        // is already the exact static nib and must keep its literal footprint.
        stroke.coverage
    } else {
        let mut finished = smooth_contours(&stroke.coverage, settings.smoothing, settings.size);
        // A circular nib has no semantic corners, so restoring its two exact
        // endpoint discs protects the classic round cap from simplification.
        // Polygon nibs are different: unioning their literal endpoint imprint
        // back in after smoothing resurrects the very sharp corners smoothing
        // just removed. Their caps therefore stay part of the smoothed boundary.
        if settings.nib == BrushNib::Circle {
            if let Some(first) = stroke.samples.first().map(|sample| sample.position) {
                finished = finished.union(&sweep_nib(settings.nib, settings.size, first, first));
            }
            if let Some(last) = stroke.samples.last().map(|sample| sample.position) {
                finished = finished.union(&sweep_nib(settings.nib, settings.size, last, last));
            }
        }
        finished
    }
}

fn build_sweep_trajectory(samples: &[BrushSample], smoothing: u8, brush_size: f32) -> Vec<Vec2> {
    let mut points = Vec::with_capacity(samples.len());
    for sample in samples {
        if points
            .last()
            .is_none_or(|previous| vec2_distance(*previous, sample.position) > 1.0e-5)
        {
            points.push(sample.position);
        }
    }
    if points.len() < 3 || smoothing == 0 {
        return points;
    }

    let original = points.clone();
    let strength = f32::from(smoothing.min(100)) / 100.0;
    let passes = 1 + usize::from(smoothing >= 34) + usize::from(smoothing >= 67);
    let blend = 0.75 * strength.sqrt();
    let radius = brush_size.max(0.1) * 0.5;
    let max_deviation = radius * (0.06 + strength * 0.30);

    for _ in 0..passes {
        let before = points.clone();
        for index in 1..(before.len() - 1) {
            let previous = before[index - 1];
            let current = before[index];
            let next = before[index + 1];
            let incoming = vec2_sub(current, previous);
            let outgoing = vec2_sub(next, current);
            let incoming_len = vec2_length(incoming);
            let outgoing_len = vec2_length(outgoing);
            if incoming_len <= 1.0e-5 || outgoing_len <= 1.0e-5 {
                continue;
            }

            let alignment = vec2_dot(
                vec2_scale(incoming, incoming_len.recip()),
                vec2_scale(outgoing, outgoing_len.recip()),
            )
            .clamp(-1.0, 1.0);
            // Only shallow turns are sampling wobble. A roughly 75-degree or
            // sharper change is deliberate geometry and must remain untouched.
            let corner_guard = ((alignment - 0.25) / 0.75).clamp(0.0, 1.0);
            if corner_guard <= 0.0 {
                continue;
            }

            let local_average = vec2_scale(
                vec2_add(vec2_add(previous, vec2_scale(current, 2.0)), next),
                0.25,
            );
            let candidate = vec2_add(
                current,
                vec2_scale(vec2_sub(local_average, current), blend * corner_guard),
            );
            let displacement = vec2_sub(candidate, original[index]);
            let displacement_len = vec2_length(displacement);
            points[index] = if displacement_len > max_deviation {
                vec2_add(
                    original[index],
                    vec2_scale(displacement, max_deviation / displacement_len),
                )
            } else {
                candidate
            };
        }
        points[0] = original[0];
        let last = points.len() - 1;
        points[last] = original[last];
    }

    points
}

fn sweep_trajectory_nib(nib: BrushNib, size: f32, trajectory: &[Vec2]) -> MultiPolygon<f64> {
    let Some(first) = trajectory.first().copied() else {
        return MultiPolygon(Vec::new());
    };
    if trajectory.len() == 1 {
        return sweep_nib(nib, size, first, first);
    }

    if nib == BrushNib::Circle {
        let radius = f64::from(size.max(0.1)) * 0.5;
        let mut coords: Vec<Coord<f64>> = trajectory
            .iter()
            .map(|point| Coord {
                x: f64::from(point.x),
                y: f64::from(point.y),
            })
            .collect();
        coords.dedup();
        if coords.len() == 1 {
            return sweep_nib(nib, size, first, first);
        }
        if (coords[0].x, coords[0].y) > (coords[coords.len() - 1].x, coords[coords.len() - 1].y) {
            coords.reverse();
        }
        return LineString::new(coords).buffer(radius);
    }

    let mut coverage = sweep_nib(nib, size, first, first);
    for segment in trajectory.windows(2) {
        coverage = coverage.union(&sweep_nib(nib, size, segment[0], segment[1]));
    }
    coverage
}

pub fn sweep_nib(nib: BrushNib, size: f32, start: Vec2, end: Vec2) -> MultiPolygon<f64> {
    if nib == BrushNib::Circle {
        let radius = f64::from(size.max(0.1)) * 0.5;
        if vec2_distance(start, end) <= 1.0e-6 {
            return circular_nib_imprint(start, radius);
        }
        // Canonical endpoint order keeps a circular nib bit-for-bit
        // independent of gesture direction, not merely visually equal.
        let (start, end) = if (start.x, start.y) <= (end.x, end.y) {
            (start, end)
        } else {
            (end, start)
        };
        return Line::new(
            Coord {
                x: f64::from(start.x),
                y: f64::from(start.y),
            },
            Coord {
                x: f64::from(end.x),
                y: f64::from(end.y),
            },
        )
        .buffer(radius);
    }

    let mut points = Vec::with_capacity(8);
    for center in [start, end] {
        points.extend(
            nib_outline(nib, size, center)
                .into_iter()
                .map(|point| Point::new(f64::from(point.x), f64::from(point.y))),
        );
    }
    if vec2_distance(start, end) <= 1.0e-6 {
        return polygon_from_nib_outline(nib_outline(nib, size, start));
    }
    MultiPolygon(vec![MultiPoint(points).convex_hull()])
}

/// Exact static footprint for cursor rendering, clicks and segment sweeps.
/// The shape never rotates with pointer direction; slash nibs keep their
/// chosen angle for the entire gesture, matching a classic fixed pen.
pub fn nib_outline(nib: BrushNib, size: f32, center: Vec2) -> Vec<Vec2> {
    let size = size.max(0.1);
    if nib == BrushNib::Circle {
        const SEGMENTS: usize = 64;
        let radius = size * 0.5;
        return (0..SEGMENTS)
            .map(|index| {
                let angle = std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
                Vec2::new(
                    center.x + radius * angle.cos(),
                    center.y + radius * angle.sin(),
                )
            })
            .collect();
    }

    let (width, height, angle) = match nib {
        BrushNib::Circle => unreachable!(),
        BrushNib::Square => (size, size, 0.0),
        BrushNib::Horizontal => (size, size * 0.34, 0.0),
        BrushNib::Vertical => (size * 0.34, size, 0.0),
        BrushNib::Slash => (size, size * 0.34, -std::f32::consts::FRAC_PI_4),
        BrushNib::Backslash => (size, size * 0.34, std::f32::consts::FRAC_PI_4),
    };
    let half_w = width * 0.5;
    let half_h = height * 0.5;
    let (sin, cos) = angle.sin_cos();
    [
        Vec2::new(-half_w, -half_h),
        Vec2::new(half_w, -half_h),
        Vec2::new(half_w, half_h),
        Vec2::new(-half_w, half_h),
    ]
    .into_iter()
    .map(|point| {
        Vec2::new(
            center.x + point.x * cos - point.y * sin,
            center.y + point.x * sin + point.y * cos,
        )
    })
    .collect()
}

fn polygon_from_nib_outline(points: Vec<Vec2>) -> MultiPolygon<f64> {
    let mut coords: Vec<Coord<f64>> = points
        .into_iter()
        .map(|point| Coord {
            x: f64::from(point.x),
            y: f64::from(point.y),
        })
        .collect();
    if let Some(first) = coords.first().copied() {
        coords.push(first);
    }
    MultiPolygon(vec![Polygon::new(LineString::new(coords), Vec::new())])
}

fn circular_nib_imprint(center: Vec2, radius: f64) -> MultiPolygon<f64> {
    const SEGMENTS: usize = 64;
    let mut coords = Vec::with_capacity(SEGMENTS + 1);
    for index in 0..SEGMENTS {
        let angle = std::f64::consts::TAU * index as f64 / SEGMENTS as f64;
        coords.push(Coord {
            x: f64::from(center.x) + radius * angle.cos(),
            y: f64::from(center.y) + radius * angle.sin(),
        });
    }
    coords.push(coords[0]);
    MultiPolygon(vec![Polygon::new(LineString::new(coords), Vec::new())])
}
pub fn smooth_contours(
    coverage: &MultiPolygon<f64>,
    smoothing: u8,
    brush_size: f32,
) -> MultiPolygon<f64> {
    if smoothing == 0 || coverage.0.is_empty() {
        return coverage.clone();
    }

    let linear_strength = f64::from(smoothing.min(100)) / 100.0;
    // Keep the lower half restrained, but make 80..100 meaningfully stronger.
    // This only post-processes the finished boundary; the nib sweep itself is
    // still built from the exact preview trajectory above.
    let strength = linear_strength.powf(1.8);
    let radius = f64::from(brush_size.max(0.1)) * 0.5;
    let target_tolerance = radius * (linear_strength * 0.01 + strength * 0.22);
    let target_passes = contour_smoothing_passes(smoothing);

    // Aggressive cleanup is attempted first. If a particular contour would
    // violate topology/area/bounds, progressively back off instead of dropping
    // all the way to the unsmoothed boundary.
    for backoff in [1.0_f64, 0.78, 0.58, 0.38] {
        let effective_strength = strength * backoff;
        let passes = ((target_passes as f64) * backoff).ceil() as usize;
        let softened = smooth_coverage_boundaries(
            coverage,
            passes,
            0.06 + effective_strength * 0.18,
            radius * (0.01 + effective_strength * 0.16),
        );
        let epsilon = (target_tolerance * backoff).powi(2).max(1.0e-8);
        let candidate = softened.simplify_vw_preserve(epsilon);
        if simplification_is_safe(coverage, &candidate, strength, radius) {
            return candidate;
        }
    }

    coverage.clone()
}

fn contour_smoothing_passes(smoothing: u8) -> usize {
    match smoothing.min(100) {
        0..=20 => 0,
        21..=50 => 1,
        51..=75 => 2,
        76..=90 => 3,
        _ => 4,
    }
}

fn smooth_coverage_boundaries(
    coverage: &MultiPolygon<f64>,
    passes: usize,
    blend: f64,
    max_deviation: f64,
) -> MultiPolygon<f64> {
    if passes == 0 {
        return coverage.clone();
    }
    MultiPolygon(
        coverage
            .0
            .iter()
            .map(|polygon| {
                Polygon::new(
                    smooth_ring_boundary(polygon.exterior(), passes, blend, max_deviation),
                    polygon
                        .interiors()
                        .iter()
                        .map(|ring| smooth_ring_boundary(ring, passes, blend, max_deviation))
                        .collect(),
                )
            })
            .collect(),
    )
}

fn smooth_ring_boundary(
    ring: &LineString<f64>,
    passes: usize,
    blend: f64,
    max_deviation: f64,
) -> LineString<f64> {
    let mut points = ring.0.clone();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    if points.len() < 4 {
        return ring.clone();
    }

    let original = points.clone();
    let count = points.len();
    for _ in 0..passes {
        let before = points.clone();
        for index in 0..count {
            let previous = before[(index + count - 1) % count];
            let current = before[index];
            let next = before[(index + 1) % count];
            let incoming_x = current.x - previous.x;
            let incoming_y = current.y - previous.y;
            let outgoing_x = next.x - current.x;
            let outgoing_y = next.y - current.y;
            let incoming_len = incoming_x.hypot(incoming_y);
            let outgoing_len = outgoing_x.hypot(outgoing_y);
            if incoming_len <= 1.0e-8 || outgoing_len <= 1.0e-8 {
                continue;
            }

            let alignment = ((incoming_x * outgoing_x + incoming_y * outgoing_y)
                / (incoming_len * outgoing_len))
                .clamp(-1.0, 1.0);
            // Preserve deliberate corners. Only shallow boundary wobble gets a
            // Laplacian-style relaxation before topology-safe simplification.
            let corner_guard = ((alignment - 0.25) / 0.75).clamp(0.0, 1.0);
            if corner_guard <= 0.0 {
                continue;
            }

            let average_x = (previous.x + current.x * 2.0 + next.x) * 0.25;
            let average_y = (previous.y + current.y * 2.0 + next.y) * 0.25;
            let mut candidate = Coord {
                x: current.x + (average_x - current.x) * blend * corner_guard,
                y: current.y + (average_y - current.y) * blend * corner_guard,
            };
            let dx = candidate.x - original[index].x;
            let dy = candidate.y - original[index].y;
            let deviation = dx.hypot(dy);
            if deviation > max_deviation {
                candidate.x = original[index].x + dx * max_deviation / deviation;
                candidate.y = original[index].y + dy * max_deviation / deviation;
            }
            points[index] = candidate;
        }
    }

    points.push(points[0]);
    LineString::new(points)
}

fn simplification_is_safe(
    original: &MultiPolygon<f64>,
    candidate: &MultiPolygon<f64>,
    strength: f64,
    brush_radius: f64,
) -> bool {
    if original.0.len() != candidate.0.len() || candidate.0.is_empty() {
        return false;
    }

    for (before, after) in original.0.iter().zip(&candidate.0) {
        if before.interiors().len() != after.interiors().len()
            || after.exterior().0.len() < 4
            || after.interiors().iter().any(|ring| ring.0.len() < 4)
            || !ring_is_simple(after.exterior())
            || after.interiors().iter().any(|ring| !ring_is_simple(ring))
            || ring_introduces_long_chord(before.exterior(), after.exterior(), brush_radius)
            || signed_ring_area(before.exterior()).signum()
                != signed_ring_area(after.exterior()).signum()
        {
            return false;
        }
        if before
            .interiors()
            .iter()
            .zip(after.interiors())
            .any(|(a, b)| {
                signed_ring_area(a).signum() != signed_ring_area(b).signum()
                    || ring_introduces_long_chord(a, b, brush_radius)
            })
        {
            return false;
        }
    }

    let original_area = original.unsigned_area();
    let candidate_area = candidate.unsigned_area();
    if original_area <= 1.0e-8 || candidate_area <= 1.0e-8 {
        return false;
    }
    let allowed_area_delta = original_area * (0.015 + strength * 0.12);
    if (original_area - candidate_area).abs() > allowed_area_delta {
        return false;
    }

    let Some(before_bbox) = original.bounding_rect() else {
        return false;
    };
    let Some(after_bbox) = candidate.bounding_rect() else {
        return false;
    };
    let expansion_tolerance = 1.0e-6;
    let inset_tolerance = brush_radius * (0.02 + strength * 0.28) + expansion_tolerance;
    after_bbox.min().x >= before_bbox.min().x - expansion_tolerance
        && after_bbox.min().y >= before_bbox.min().y - expansion_tolerance
        && after_bbox.max().x <= before_bbox.max().x + expansion_tolerance
        && after_bbox.max().y <= before_bbox.max().y + expansion_tolerance
        && after_bbox.min().x <= before_bbox.min().x + inset_tolerance
        && after_bbox.min().y <= before_bbox.min().y + inset_tolerance
        && after_bbox.max().x >= before_bbox.max().x - inset_tolerance
        && after_bbox.max().y >= before_bbox.max().y - inset_tolerance
}

fn ring_max_edge_length(ring: &LineString<f64>) -> f64 {
    ring.0
        .windows(2)
        .map(|pair| (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y))
        .fold(0.0, f64::max)
}

fn ring_introduces_long_chord(
    original: &LineString<f64>,
    candidate: &LineString<f64>,
    brush_radius: f64,
) -> bool {
    let original_max = ring_max_edge_length(original);
    let candidate_max = ring_max_edge_length(candidate);
    candidate_max > original_max * 2.5 + brush_radius * 0.35
}

fn ring_is_simple(ring: &LineString<f64>) -> bool {
    let mut points: Vec<Vec2> = ring
        .0
        .iter()
        .map(|coord| Vec2::new(coord.x as f32, coord.y as f32))
        .collect();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points.len() >= 3 && !polyline_self_intersects(&points)
}

fn signed_ring_area(ring: &LineString<f64>) -> f64 {
    ring.0
        .windows(2)
        .map(|pair| pair[0].x * pair[1].y - pair[1].x * pair[0].y)
        .sum::<f64>()
        * 0.5
}

pub fn coverage_to_paths(coverage: &MultiPolygon<f64>) -> Vec<VPath> {
    let mut paths = Vec::new();
    for polygon in &coverage.0 {
        if let Some(path) = ring_to_path(polygon.exterior()) {
            paths.push(path);
        }
        for hole in polygon.interiors() {
            if let Some(path) = ring_to_path(hole) {
                paths.push(path);
            }
        }
    }
    paths
}

fn coverage_to_linear_paths(coverage: &MultiPolygon<f64>) -> Vec<VPath> {
    let mut paths = Vec::new();
    for polygon in &coverage.0 {
        if let Some(path) = linear_ring_to_path(polygon.exterior()) {
            paths.push(path);
        }
        for hole in polygon.interiors() {
            if let Some(path) = linear_ring_to_path(hole) {
                paths.push(path);
            }
        }
    }
    paths
}

fn linear_ring_to_path(ring: &LineString<f64>) -> Option<VPath> {
    let mut points: Vec<Vec2> = ring
        .0
        .iter()
        .map(|coord| Vec2::new(coord.x as f32, coord.y as f32))
        .collect();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points.dedup_by(|a, b| vec2_distance(*a, *b) <= 1.0e-5);
    (points.len() >= 3).then(|| linear_ring_path(&points))
}

fn ring_to_path(ring: &LineString<f64>) -> Option<VPath> {
    let mut points: Vec<Vec2> = ring
        .0
        .iter()
        .map(|coord| Vec2::new(coord.x as f32, coord.y as f32))
        .collect();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points.dedup_by(|a, b| vec2_distance(*a, *b) <= 1.0e-5);
    if points.len() < 3 {
        return None;
    }

    let linear = linear_ring_path(&points);
    let curved = curved_ring_path(&points);
    if curved_path_is_safe(ring, &curved) {
        Some(curved)
    } else {
        Some(linear)
    }
}

fn linear_ring_path(points: &[Vec2]) -> VPath {
    VPath {
        anchors: points
            .iter()
            .copied()
            .map(|point| Anchor {
                point,
                in_handle: None,
                out_handle: None,
            })
            .collect(),
        closed: true,
    }
}

/// Build a restrained closed cubic spline from the polygon boundary. Handles
/// are only introduced where neighbouring edges already describe a smooth
/// turn. Deliberately sharp corners stay linear instead of being rounded away.
fn curved_ring_path(points: &[Vec2]) -> VPath {
    let count = points.len();
    let mut anchors = Vec::with_capacity(count);
    for index in 0..count {
        let previous = points[(index + count - 1) % count];
        let point = points[index];
        let next = points[(index + 1) % count];
        let incoming = vec2_sub(point, previous);
        let outgoing = vec2_sub(next, point);
        let incoming_len = vec2_length(incoming);
        let outgoing_len = vec2_length(outgoing);

        let (in_handle, out_handle) = if incoming_len <= 1.0e-5 || outgoing_len <= 1.0e-5 {
            (None, None)
        } else {
            let incoming_unit = vec2_scale(incoming, incoming_len.recip());
            let outgoing_unit = vec2_scale(outgoing, outgoing_len.recip());
            let alignment = vec2_dot(incoming_unit, outgoing_unit).clamp(-1.0, 1.0);
            let tangent = vec2_add(incoming_unit, outgoing_unit);
            let tangent_len = vec2_length(tangent);

            // A turn sharper than roughly 75 degrees is intentional geometry,
            // not polygonal sampling noise. Leave it with zero handles.
            if alignment <= 0.25 || tangent_len <= 1.0e-5 {
                (None, None)
            } else {
                let smoothness = ((alignment - 0.25) / 0.75).clamp(0.0, 1.0);
                let handle_len = incoming_len.min(outgoing_len) * (0.18 + smoothness * 0.15);
                let tangent = vec2_scale(tangent, tangent_len.recip());
                (
                    Some(vec2_sub(point, vec2_scale(tangent, handle_len))),
                    Some(vec2_add(point, vec2_scale(tangent, handle_len))),
                )
            }
        };

        anchors.push(Anchor {
            point,
            in_handle,
            out_handle,
        });
    }
    VPath {
        anchors,
        closed: true,
    }
}

fn curved_path_is_safe(original: &LineString<f64>, candidate: &VPath) -> bool {
    if candidate
        .anchors
        .iter()
        .flat_map(|anchor| {
            [Some(anchor.point), anchor.in_handle, anchor.out_handle]
                .into_iter()
                .flatten()
        })
        .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        return false;
    }

    let mut flattened = flatten_path(candidate);
    if flattened.len() > 1 && flattened.first() == flattened.last() {
        flattened.pop();
    }
    if flattened.len() < 3 || polyline_self_intersects(&flattened) {
        return false;
    }

    let original_area = signed_ring_area(original);
    let candidate_area = signed_points_area(&flattened);
    if original_area.abs() <= 1.0e-8
        || candidate_area.abs() <= 1.0e-8
        || original_area.signum() != candidate_area.signum()
        || (candidate_area.abs() - original_area.abs()).abs() > original_area.abs() * 0.05
    {
        return false;
    }

    let original_points: Vec<Vec2> = original
        .0
        .iter()
        .map(|coord| Vec2::new(coord.x as f32, coord.y as f32))
        .collect();
    let Some((original_min, original_max)) = points_bounds(&original_points) else {
        return false;
    };
    let Some((candidate_min, candidate_max)) = points_bounds(&flattened) else {
        return false;
    };
    let mean_segment = mean_closed_segment_length(&original_points).max(1.0e-4);
    let bbox_tolerance = mean_segment * 0.18 + 1.0e-4;
    if candidate_min.x < original_min.x - bbox_tolerance
        || candidate_min.y < original_min.y - bbox_tolerance
        || candidate_max.x > original_max.x + bbox_tolerance
        || candidate_max.y > original_max.y + bbox_tolerance
    {
        return false;
    }

    let deviation_tolerance = mean_segment * 0.42 + 1.0e-4;
    curve_stays_close_to_source_edges(candidate, deviation_tolerance)
}

fn points_bounds(points: &[Vec2]) -> Option<(Vec2, Vec2)> {
    let first = *points.first()?;
    let mut min = first;
    let mut max = first;
    for point in &points[1..] {
        min.x = min.x.min(point.x);
        min.y = min.y.min(point.y);
        max.x = max.x.max(point.x);
        max.y = max.y.max(point.y);
    }
    Some((min, max))
}

fn signed_points_area(points: &[Vec2]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    (0..points.len())
        .map(|index| {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            f64::from(a.x) * f64::from(b.y) - f64::from(b.x) * f64::from(a.y)
        })
        .sum::<f64>()
        * 0.5
}

fn mean_closed_segment_length(points: &[Vec2]) -> f32 {
    if points.len() < 2 {
        return 0.0;
    }
    (0..points.len())
        .map(|index| vec2_distance(points[index], points[(index + 1) % points.len()]))
        .sum::<f32>()
        / points.len() as f32
}

fn curve_stays_close_to_source_edges(candidate: &VPath, tolerance: f32) -> bool {
    let count = candidate.anchors.len();
    if count < 3 {
        return false;
    }

    for index in 0..count {
        let current = &candidate.anchors[index];
        let next = &candidate.anchors[(index + 1) % count];
        let p0 = current.point;
        let p1 = current.out_handle.unwrap_or(p0);
        let p3 = next.point;
        let p2 = next.in_handle.unwrap_or(p3);
        let curved = current.out_handle.is_some() || next.in_handle.is_some();
        let samples = if curved { 16 } else { 1 };

        for step in 1..=samples {
            let t = step as f32 / samples as f32;
            let point = cubic_point(p0, p1, p2, p3, t);
            if point_segment_distance(point, p0, p3) > tolerance {
                return false;
            }
        }
    }
    true
}

fn cubic_point(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f32) -> Vec2 {
    let one_minus_t = 1.0 - t;
    let a = one_minus_t * one_minus_t * one_minus_t;
    let b = 3.0 * one_minus_t * one_minus_t * t;
    let c = 3.0 * one_minus_t * t * t;
    let d = t * t * t;
    Vec2::new(
        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    )
}

fn point_segment_distance(point: Vec2, start: Vec2, end: Vec2) -> f32 {
    let segment = vec2_sub(end, start);
    let length_squared = vec2_dot(segment, segment);
    if length_squared <= 1.0e-12 {
        return vec2_distance(point, start);
    }
    let t = (vec2_dot(vec2_sub(point, start), segment) / length_squared).clamp(0.0, 1.0);
    vec2_distance(point, vec2_add(start, vec2_scale(segment, t)))
}

fn polyline_self_intersects(points: &[Vec2]) -> bool {
    let count = points.len();
    if count < 4 {
        return false;
    }
    let Some((min, max)) = points_bounds(points) else {
        return false;
    };
    let max_extent = (max.x - min.x).max(max.y - min.y).max(1.0e-3);
    let cell_size = (max_extent / (count as f32).sqrt()).max(1.0e-3);
    let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    let mut checked_pairs: HashSet<(usize, usize)> = HashSet::new();

    for first in 0..count {
        let first_next = (first + 1) % count;
        let a = points[first];
        let b = points[first_next];
        let min_cell_x = ((a.x.min(b.x) - min.x) / cell_size).floor() as i32;
        let max_cell_x = ((a.x.max(b.x) - min.x) / cell_size).floor() as i32;
        let min_cell_y = ((a.y.min(b.y) - min.y) / cell_size).floor() as i32;
        let max_cell_y = ((a.y.max(b.y) - min.y) / cell_size).floor() as i32;

        for cell_x in min_cell_x..=max_cell_x {
            for cell_y in min_cell_y..=max_cell_y {
                let bucket = cells.entry((cell_x, cell_y)).or_default();
                for &second in bucket.iter() {
                    let pair = if first < second {
                        (first, second)
                    } else {
                        (second, first)
                    };
                    if !checked_pairs.insert(pair) {
                        continue;
                    }
                    let second_next = (second + 1) % count;
                    if first == second
                        || first_next == second
                        || second_next == first
                        || (first == 0 && second_next == 0)
                    {
                        continue;
                    }
                    if segments_intersect(a, b, points[second], points[second_next]) {
                        return true;
                    }
                }
                bucket.push(first);
            }
        }
    }
    false
}

fn segments_intersect(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    const EPSILON: f32 = 1.0e-5;
    let ab_c = cross(vec2_sub(b, a), vec2_sub(c, a));
    let ab_d = cross(vec2_sub(b, a), vec2_sub(d, a));
    let cd_a = cross(vec2_sub(d, c), vec2_sub(a, c));
    let cd_b = cross(vec2_sub(d, c), vec2_sub(b, c));

    if ((ab_c > EPSILON && ab_d < -EPSILON) || (ab_c < -EPSILON && ab_d > EPSILON))
        && ((cd_a > EPSILON && cd_b < -EPSILON) || (cd_a < -EPSILON && cd_b > EPSILON))
    {
        return true;
    }
    (ab_c.abs() <= EPSILON && point_on_segment(c, a, b))
        || (ab_d.abs() <= EPSILON && point_on_segment(d, a, b))
        || (cd_a.abs() <= EPSILON && point_on_segment(a, c, d))
        || (cd_b.abs() <= EPSILON && point_on_segment(b, c, d))
}

fn point_on_segment(point: Vec2, start: Vec2, end: Vec2) -> bool {
    const EPSILON: f32 = 1.0e-5;
    point.x >= start.x.min(end.x) - EPSILON
        && point.x <= start.x.max(end.x) + EPSILON
        && point.y >= start.y.min(end.y) - EPSILON
        && point.y <= start.y.max(end.y) + EPSILON
}

fn cross(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

fn vec2_add(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new(a.x + b.x, a.y + b.y)
}

fn vec2_sub(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new(a.x - b.x, a.y - b.y)
}

fn vec2_scale(value: Vec2, scale: f32) -> Vec2 {
    Vec2::new(value.x * scale, value.y * scale)
}

fn vec2_dot(a: Vec2, b: Vec2) -> f32 {
    a.x * b.x + a.y * b.y
}

fn vec2_length(value: Vec2) -> f32 {
    vec2_dot(value, value).sqrt()
}

fn vec2_distance(a: Vec2, b: Vec2) -> f32 {
    vec2_length(vec2_sub(a, b))
}

#[derive(Debug, Clone)]
struct RawFillCandidate {
    asset_id: u16,
    color: Rgba,
    source_surfaces: Vec<MultiPolygon<f64>>,
    geometry: MultiPolygon<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RawFillStyle {
    color: Rgba,
    material: Option<VectorMaterial>,
    fragmented: bool,
}

fn raw_fill_style(project: &ProjectV2, candidate: &RawFillCandidate) -> RawFillStyle {
    let appearance = project.asset_appearances.get(&candidate.asset_id);
    RawFillStyle {
        color: candidate.color,
        material: appearance.map(|appearance| appearance.material),
        fragmented: appearance.is_some_and(|appearance| {
            !appearance.material_source.is_empty() || !appearance.clip_mask.is_empty()
        }),
    }
}

fn requested_brush_material(settings: BrushSettings) -> Option<VectorMaterial> {
    #[cfg(feature = "appearance-mask-eraser")]
    {
        crate::appearance::brush_material(settings)
    }
    #[cfg(not(feature = "appearance-mask-eraser"))]
    {
        let _ = settings;
        None
    }
}

pub fn commit_brush_region(
    app: &mut EditorApp,
    region: MultiPolygon<f64>,
    settings: BrushSettings,
) {
    if region.0.is_empty() {
        return;
    }

    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    let candidates = collect_raw_fill_candidates(&app.state.project, q0rg_id, layer_id, frame);
    #[cfg(feature = "appearance-mask-eraser")]
    let freshly_painted = region.clone();
    let requested_material = requested_brush_material(settings);
    let requested_style = RawFillStyle {
        color: settings.color,
        material: requested_material,
        fragmented: false,
    };

    // A raw drawing is one planar paint surface per visual style. Plain vector
    // paint and glowing paint of the same colour must stay distinct; otherwise
    // toggling Glow would silently restyle older artwork.
    let mut painted = region;
    let mut same_style_assets = BTreeSet::new();
    for candidate in &candidates {
        if raw_fill_style(&app.state.project, candidate) == requested_style
            && same_style_assets.insert(candidate.asset_id)
        {
            painted = painted.union(&candidate.geometry);
        }
    }
    let painted_paths = coverage_to_paths(&painted);
    if painted_paths.is_empty() {
        return;
    }
    #[cfg(feature = "appearance-mask-eraser")]
    let merged_appearance = requested_material.map(|material| {
        crate::appearance::merged_brush_appearance(
            &app.state.project,
            &same_style_assets,
            &freshly_painted,
            material,
        )
    });

    let mut different_updates = Vec::new();
    let mut seen_different = BTreeSet::new();
    for candidate in &candidates {
        if raw_fill_style(&app.state.project, candidate) != requested_style
            && seen_different.insert(candidate.asset_id)
        {
            different_updates.push((candidate.asset_id, candidate.geometry.difference(&painted)));
        }
    }

    app.history.snapshot(&app.state.project);
    if crate::tools::materialize_layer_keyframe_for_edit(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
    )
    .is_none()
    {
        return;
    }

    let writable_different = prepare_writable_raw_assets(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
        &seen_different,
    );
    let mut remove_asset_ids = same_style_assets.clone();
    for (original_asset_id, geometry) in &different_updates {
        let asset_id = *writable_different
            .get(original_asset_id)
            .unwrap_or(original_asset_id);
        let paths = coverage_to_paths(geometry);
        if paths.is_empty() {
            remove_asset_ids.insert(asset_id);
        } else if let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == asset_id)
        {
            vector.paths = paths;
            vector.stroke = None;
        }
    }

    remove_current_layer_asset_placements(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
        &remove_asset_ids,
    );
    remove_unreferenced_assets(&mut app.state.project, &remove_asset_ids);

    let painted_asset_id = append_raw_fill(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
        settings.color,
        painted_paths,
    );
    #[cfg(not(feature = "appearance-mask-eraser"))]
    let _ = painted_asset_id;
    #[cfg(feature = "appearance-mask-eraser")]
    {
        if let Some(appearance) = merged_appearance {
            app.state
                .project
                .asset_appearances
                .insert(painted_asset_id, appearance);
        }
        app.textures.invalidate();
    }

    app.session.selection = crate::state::Selection::None;
    app.session.status = format!("Brush merge drawing updated ({} regions)", painted.0.len());
    app.state.dirty = true;
}

/// Rebuild same-colour fill-only raw graphics after a transform gesture.
/// Drawing already performs this planar union on commit; moving/scaling raw
/// contours must end with the same topology or semi-transparent overlaps render
/// darker and remain separately selectable. Disconnected regions are left
/// alone, while touching or overlapping components are consolidated.
pub(crate) fn merge_touching_raw_fills_after_edit(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> bool {
    let candidates = collect_raw_fill_candidates(project, q0rg_id, layer_id, frame);
    let mut groups: Vec<(RawFillStyle, Vec<RawFillCandidate>)> = Vec::new();
    for candidate in candidates {
        let style = raw_fill_style(project, &candidate);
        if let Some((_, items)) = groups
            .iter_mut()
            .find(|(candidate_style, _)| *candidate_style == style)
        {
            if !items.iter().any(|item| item.asset_id == candidate.asset_id) {
                items.push(candidate);
            }
        } else {
            groups.push((style, vec![candidate]));
        }
    }

    let mut updates = Vec::new();
    for (style, items) in groups {
        // A post-material fragment is a slice of one already-resolved filter
        // field. Geometry-merging those slices would make the filter evaluate
        // again from the merged contours and resurrect the split-edge glow.
        if style.fragmented {
            continue;
        }
        let color = style.color;
        let source_surfaces: Vec<MultiPolygon<f64>> = items
            .iter()
            .flat_map(|item| item.source_surfaces.iter().cloned())
            .collect();
        // Strict containment is not a boundary merge. Automatically unioning a
        // small moved fill fully inside a larger one makes the moved object
        // disappear and destroys its independent editability.
        if !raw_surfaces_require_boundary_merge(&source_surfaces) {
            continue;
        }

        let mut merged = MultiPolygon(Vec::new());
        let mut asset_ids = BTreeSet::new();
        for item in &items {
            asset_ids.insert(item.asset_id);
            merged = merged.union(&item.geometry);
        }
        let paths = coverage_to_paths(&merged);
        if paths.is_empty()
            || !merged_paths_preserve_every_source(color, &paths, &merged, &source_surfaces)
        {
            continue;
        }
        updates.push((style, asset_ids, paths));
    }
    if updates.is_empty() {
        return false;
    }

    for (style, asset_ids, paths) in updates {
        #[cfg(feature = "appearance-mask-eraser")]
        let merged_appearance = style.material.map(|material| {
            crate::appearance::merged_brush_appearance(
                project,
                &asset_ids,
                &MultiPolygon(Vec::new()),
                material,
            )
        });
        remove_current_layer_asset_placements(project, q0rg_id, layer_id, frame, &asset_ids);
        remove_unreferenced_assets(project, &asset_ids);
        let new_asset_id = append_raw_fill(project, q0rg_id, layer_id, frame, style.color, paths);
        #[cfg(not(feature = "appearance-mask-eraser"))]
        let _ = new_asset_id;
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = merged_appearance {
            project.asset_appearances.insert(new_asset_id, appearance);
        }
    }
    true
}

fn raw_surfaces_require_boundary_merge(surfaces: &[MultiPolygon<f64>]) -> bool {
    for left in 0..surfaces.len() {
        for right in left + 1..surfaces.len() {
            let a = &surfaces[left];
            let b = &surfaces[right];
            let a_area = a.unsigned_area();
            let b_area = b.unsigned_area();
            if a_area <= 1.0e-8 || b_area <= 1.0e-8 {
                continue;
            }
            let intersection_area = a.intersection(b).unsigned_area();
            let min_area = a_area.min(b_area);
            let tolerance = (min_area * 1.0e-7).max(1.0e-8);
            // Partial overlap means both visible boundaries participate in the
            // resulting planar surface. Full containment/identity does not.
            if intersection_area > tolerance && intersection_area < min_area - tolerance {
                return true;
            }
            if intersection_area <= tolerance {
                let union = a.union(b);
                if union.0.len() < a.0.len() + b.0.len() {
                    return true;
                }
            }
        }
    }
    false
}

fn merged_paths_preserve_every_source(
    color: Rgba,
    paths: &[VPath],
    expected: &MultiPolygon<f64>,
    sources: &[MultiPolygon<f64>],
) -> bool {
    let reconstructed = vector_fill_geometry(&VectorAsset {
        asset_id: 0,
        paths: paths.to_vec(),
        fill: Some(color),
        stroke: None,
    });
    if reconstructed.0.is_empty() {
        return false;
    }
    let tolerance = (expected.unsigned_area() * 0.002).max(0.02);
    let mismatch = expected
        .difference(&reconstructed)
        .union(&reconstructed.difference(expected))
        .unsigned_area();
    if mismatch > tolerance {
        return false;
    }
    sources
        .iter()
        .all(|source| source.difference(&reconstructed).unsigned_area() <= tolerance)
}

/// Subtract one already-unioned eraser gesture from every raw fill on the
/// current layer/frame. Returns whether any geometry actually changed.
pub fn erase_brush_region(app: &mut EditorApp, region: MultiPolygon<f64>) -> bool {
    erase_brush_region_impl(app, region, true)
}

pub(crate) fn erase_brush_region_without_snapshot(
    app: &mut EditorApp,
    region: MultiPolygon<f64>,
) -> bool {
    erase_brush_region_impl(app, region, false)
}

fn erase_brush_region_impl(
    app: &mut EditorApp,
    region: MultiPolygon<f64>,
    snapshot_history: bool,
) -> bool {
    if region.0.is_empty() {
        return false;
    }

    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    let candidates = collect_raw_fill_candidates(&app.state.project, q0rg_id, layer_id, frame);

    let mut updates = Vec::new();
    let mut seen_assets = BTreeSet::new();
    for candidate in &candidates {
        #[cfg(feature = "appearance-mask-eraser")]
        if app
            .state
            .project
            .asset_appearances
            .contains_key(&candidate.asset_id)
        {
            // Appearance-enabled fills are erased only by the post-material
            // mask. The classic geometry eraser must never cut their source.
            continue;
        }
        if !seen_assets.insert(candidate.asset_id) {
            continue;
        }
        let overlap = candidate.geometry.intersection(&region);
        if overlap.unsigned_area() > 1.0e-8 {
            updates.push((candidate.asset_id, candidate.geometry.difference(&region)));
        }
    }
    if updates.is_empty() {
        return false;
    }

    if snapshot_history {
        app.history.snapshot(&app.state.project);
    }
    if crate::tools::materialize_layer_keyframe_for_edit(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
    )
    .is_none()
    {
        return false;
    }
    let writable_assets = prepare_writable_raw_assets(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
        &seen_assets,
    );
    let mut remove_asset_ids = BTreeSet::new();
    for (original_asset_id, geometry) in &updates {
        let asset_id = *writable_assets
            .get(original_asset_id)
            .unwrap_or(original_asset_id);
        let paths = coverage_to_paths(geometry);
        if paths.is_empty() {
            remove_asset_ids.insert(asset_id);
        } else if let Some(Asset::Vector(vector)) = app
            .state
            .project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == asset_id)
        {
            vector.paths = paths;
            vector.stroke = None;
        }
    }

    remove_current_layer_asset_placements(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        frame,
        &remove_asset_ids,
    );
    remove_unreferenced_assets(&mut app.state.project, &remove_asset_ids);

    app.session.selection = crate::state::Selection::None;
    app.session.status = "Raw fill erased".to_string();
    app.state.dirty = true;
    true
}

fn collect_raw_fill_candidates(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> Vec<RawFillCandidate> {
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
        .filter_map(|(placement_idx, _)| {
            let placement = layer.placements.get(placement_idx)?;
            if placement.transform != Transform2D::IDENTITY
                || !matches!(placement.tween, Tween::None)
            {
                return None;
            }
            let Target::Asset(asset_id) = placement.target else {
                return None;
            };
            let Some(Asset::Vector(vector)) =
                project.assets.iter().find(|asset| asset.id() == asset_id)
            else {
                return None;
            };
            let color = vector.fill?;
            if vector.stroke.is_some() {
                return None;
            }
            let geometry = vector_fill_geometry(vector);
            let source_surfaces = vector
                .paths
                .iter()
                .filter(|path| path.closed)
                .filter_map(|path| {
                    let single = VectorAsset {
                        asset_id: 0,
                        paths: vec![path.clone()],
                        fill: Some(color),
                        stroke: None,
                    };
                    let surface = vector_fill_geometry(&single);
                    (!surface.0.is_empty()).then_some(surface)
                })
                .collect();
            (!geometry.0.is_empty()).then_some(RawFillCandidate {
                asset_id,
                color,
                source_surfaces,
                geometry,
            })
        })
        .collect()
}

pub(crate) fn prepare_writable_raw_assets(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    asset_ids: &BTreeSet<u16>,
) -> BTreeMap<u16, u16> {
    let mut result = BTreeMap::new();
    for asset_id in asset_ids {
        if !asset_has_external_references(project, *asset_id, q0rg_id, layer_id, frame) {
            result.insert(*asset_id, *asset_id);
            continue;
        }

        let Some(Asset::Vector(mut clone)) = project
            .assets
            .iter()
            .find(|asset| asset.id() == *asset_id)
            .cloned()
        else {
            result.insert(*asset_id, *asset_id);
            continue;
        };
        let new_asset_id = next_asset_id(project);
        clone.asset_id = new_asset_id;
        project.assets.push(Asset::Vector(clone));
        #[cfg(feature = "appearance-mask-eraser")]
        crate::appearance::clone_asset_appearance(project, *asset_id, new_asset_id);

        if let Some(layer) = project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == layer_id)
            })
        {
            for placement in &mut layer.placements {
                if placement.frame == frame
                    && placement.transform == Transform2D::IDENTITY
                    && matches!(placement.tween, Tween::None)
                    && matches!(placement.target, Target::Asset(id) if id == *asset_id)
                {
                    placement.target = Target::Asset(new_asset_id);
                }
            }
        }
        result.insert(*asset_id, new_asset_id);
    }
    result
}

fn asset_has_external_references(
    project: &ProjectV2,
    asset_id: u16,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> bool {
    project.q0rgs.iter().any(|q0rg| {
        q0rg.layers.iter().any(|layer| {
            layer.placements.iter().any(|placement| {
                if !matches!(placement.target, Target::Asset(id) if id == asset_id) {
                    return false;
                }
                !(q0rg.q0rg_id == q0rg_id
                    && layer.layer_id == layer_id
                    && placement.frame == frame
                    && placement.transform == Transform2D::IDENTITY
                    && matches!(placement.tween, Tween::None))
            })
        })
    })
}

fn remove_current_layer_asset_placements(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    asset_ids: &BTreeSet<u16>,
) {
    if asset_ids.is_empty() {
        return;
    }
    if let Some(layer) = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)
        })
    {
        layer.placements.retain(|placement| {
            placement.frame != frame
                || placement.transform != Transform2D::IDENTITY
                || !matches!(placement.tween, Tween::None)
                || !matches!(placement.target, Target::Asset(id) if asset_ids.contains(&id))
        });
    }
}

fn append_raw_fill(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    color: Rgba,
    paths: Vec<VPath>,
) -> u16 {
    let asset_id = next_asset_id(project);
    project.assets.push(Asset::Vector(VectorAsset {
        asset_id,
        paths,
        fill: Some(color),
        stroke: None,
    }));
    if let Some(layer) = project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)
        })
    {
        layer.placements.push(Placement {
            frame,
            target: Target::Asset(asset_id),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        });
    }
    asset_id
}
pub(crate) fn vector_fill_geometry(vector: &VectorAsset) -> MultiPolygon<f64> {
    let mut rings: Vec<(&VPath, Polygon<f64>, f64)> = vector
        .paths
        .iter()
        .filter(|path| path.closed)
        .filter_map(|path| {
            let polygon = path_to_polygon(path)?;
            let area = signed_ring_area(polygon.exterior());
            (area.abs() > 1.0e-8).then_some((path, polygon, area))
        })
        .collect();
    rings.sort_by(|left, right| right.2.abs().total_cmp(&left.2.abs()));

    let mut surface = MultiPolygon(Vec::new());
    let mut inside_windings: Vec<i32> = Vec::with_capacity(rings.len());
    for index in 0..rings.len() {
        let (_, polygon, area) = &rings[index];
        let sample = polygon
            .exterior()
            .0
            .first()
            .map(|coord| Point::new(coord.x, coord.y));
        let parent = sample.and_then(|point| {
            (0..index)
                .rev()
                .find(|candidate| rings[*candidate].1.contains(&point))
        });
        let outside_winding = parent.map(|parent| inside_windings[parent]).unwrap_or(0);
        let inside_winding = outside_winding + if *area > 0.0 { 1 } else { -1 };

        // Evaluate NonZero fill at each nested boundary. A positive island
        // inside a negative hole crosses 0 -> 1 and must be added back; doing
        // all unions before all differences erased that island entirely.
        if outside_winding == 0 && inside_winding != 0 {
            surface = surface.union(polygon);
        } else if outside_winding != 0 && inside_winding == 0 {
            surface = surface.difference(polygon);
        }
        inside_windings.push(inside_winding);
    }
    surface
}

fn path_to_polygon(path: &VPath) -> Option<Polygon<f64>> {
    if path.anchors.len() < 3 {
        return None;
    }

    // Brush Bézier handles are a render approximation of the canonical raw
    // boundary, not a second source of merge-drawing geometry. Re-flattening
    // every curved segment here multiplied the point count by 16 on every
    // successive brush commit: N -> 16N -> 256N -> ... and caused the
    // exponential freezes reported after the second and third strokes.
    let mut coords: Vec<Coord<f64>> = path
        .anchors
        .iter()
        .map(|anchor| Coord {
            x: anchor.point.x as f64,
            y: anchor.point.y as f64,
        })
        .collect();
    coords.dedup();
    if coords.len() < 3 {
        return None;
    }
    if coords.first() != coords.last() {
        coords.push(coords[0]);
    }
    Some(Polygon::new(LineString::new(coords), Vec::new()))
}

fn next_asset_id(project: &ProjectV2) -> u16 {
    project
        .assets
        .iter()
        .map(Asset::id)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn remove_unreferenced_assets(project: &mut ProjectV2, candidates: &BTreeSet<u16>) {
    if candidates.is_empty() {
        return;
    }
    let referenced: BTreeSet<u16> = project
        .q0rgs
        .iter()
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
        .filter_map(|placement| match placement.target {
            Target::Asset(asset_id) => Some(asset_id),
            Target::Q0rg(_) => None,
        })
        .collect();
    let removed: Vec<u16> = candidates
        .iter()
        .copied()
        .filter(|asset_id| !referenced.contains(asset_id))
        .collect();
    project
        .assets
        .retain(|asset| !removed.contains(&asset.id()));
    for asset_id in removed {
        project.asset_names.remove(&asset_id);
        project.asset_appearances.remove(&asset_id);
    }
}

#[cfg(test)]
mod tests {
    use geo::{Contains, Point};

    use super::*;

    fn settings(size: f32, smoothing: u8) -> BrushSettings {
        BrushSettings {
            size,
            smoothing,
            ..BrushSettings::default()
        }
    }

    fn square_path(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> VPath {
        VPath {
            anchors: [
                Vec2::new(min_x, min_y),
                Vec2::new(max_x, min_y),
                Vec2::new(max_x, max_y),
                Vec2::new(min_x, max_y),
            ]
            .into_iter()
            .map(|point| Anchor {
                point,
                in_handle: None,
                out_handle: None,
            })
            .collect(),
            closed: true,
        }
    }

    fn raw_fill_asset(asset_id: u16, color: Rgba, path: VPath) -> Asset {
        Asset::Vector(VectorAsset {
            asset_id,
            paths: vec![path],
            fill: Some(color),
            stroke: None,
        })
    }

    fn stroke(points: &[Vec2], settings: BrushSettings) -> BrushStroke {
        let mut result = brush_begin(settings, BrushSample::mouse(points[0]));
        for point in &points[1..] {
            brush_add_sample(&mut result, settings, BrushSample::mouse(*point));
        }
        brush_flush_pending(&mut result);
        result
    }

    fn anchor_count(paths: &[VPath]) -> usize {
        paths.iter().map(|path| path.anchors.len()).sum()
    }

    fn max_ring_edge_length(ring: &LineString<f64>) -> f64 {
        ring.0
            .windows(2)
            .map(|pair| (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y))
            .fold(0.0, f64::max)
    }

    fn flattened_bounds(paths: &[VPath]) -> (Vec2, Vec2) {
        let points: Vec<Vec2> = paths.iter().flat_map(flatten_path).collect();
        points_bounds(&points).expect("flattened brush bounds")
    }

    fn assert_paths_are_finite_and_simple(paths: &[VPath]) {
        for path in paths {
            let mut flattened = flatten_path(path);
            if flattened.len() > 1 && flattened.first() == flattened.last() {
                flattened.pop();
            }
            assert!(flattened
                .iter()
                .all(|point| point.x.is_finite() && point.y.is_finite()));
            assert!(!polyline_self_intersects(&flattened));
        }
    }

    fn trajectory_wobble(points: &[Vec2]) -> f32 {
        points
            .windows(3)
            .map(|window| {
                let midpoint = vec2_scale(vec2_add(window[0], window[2]), 0.5);
                vec2_distance(window[1], midpoint)
            })
            .sum()
    }

    #[test]
    fn smoothing_zero_keeps_every_raw_sweep_sample() {
        let settings = settings(10.0, 0);
        let points = [
            Vec2::new(0.0, 0.0),
            Vec2::new(8.0, 1.0),
            Vec2::new(16.0, -1.0),
            Vec2::new(24.0, 0.0),
        ];
        let stroke = stroke(&points, settings);
        assert_eq!(brush_preview_trajectory(&stroke), points);
    }

    #[test]
    fn smoothing_reduces_shallow_sample_wobble_without_moving_endpoints() {
        let settings = settings(12.0, 75);
        let points = [
            Vec2::new(0.0, 0.0),
            Vec2::new(12.0, 2.0),
            Vec2::new(24.0, -2.0),
            Vec2::new(36.0, 2.0),
            Vec2::new(48.0, -2.0),
            Vec2::new(60.0, 0.0),
        ];
        let stroke = stroke(&points, settings);
        let trajectory = brush_preview_trajectory(&stroke);

        assert_eq!(trajectory.first(), points.first());
        assert_eq!(trajectory.last(), points.last());
        assert!(trajectory_wobble(&trajectory) < trajectory_wobble(&points) * 0.65);

        let radius = settings.size * 0.5;
        let max_deviation = radius * (0.06 + 0.75 * 0.30) + 1.0e-4;
        for (raw, smooth) in points.iter().zip(&trajectory) {
            assert!(vec2_distance(*raw, *smooth) <= max_deviation);
        }
    }

    #[test]
    fn sharp_sweep_corners_are_not_rounded_into_a_shortcut() {
        let settings = settings(10.0, 100);
        let points = [
            Vec2::new(0.0, 0.0),
            Vec2::new(30.0, 0.0),
            Vec2::new(30.0, 30.0),
        ];
        let stroke = stroke(&points, settings);
        assert_eq!(brush_preview_trajectory(&stroke), points);
    }

    #[test]
    fn preview_trajectory_is_the_committed_sweep_source() {
        let settings = settings(14.0, 70);
        let points = [
            Vec2::new(0.0, 0.0),
            Vec2::new(18.0, 2.0),
            Vec2::new(36.0, -2.0),
            Vec2::new(54.0, 0.0),
        ];
        let mut stroke = brush_begin(settings, BrushSample::mouse(points[0]));
        for point in &points[1..] {
            brush_add_sample(&mut stroke, settings, BrushSample::mouse(*point));
        }
        let trajectory = brush_preview_trajectory(&stroke);
        let expected = sweep_trajectory_nib(settings.nib, settings.size, &trajectory);
        brush_flush_pending(&mut stroke);
        let mismatch = expected
            .difference(&stroke.coverage)
            .union(&stroke.coverage.difference(&expected));
        assert!(mismatch.unsigned_area() < 1.0e-6);
    }

    #[test]
    fn distant_pointer_samples_are_joined_by_one_continuous_sweep() {
        let settings = settings(10.0, 0);
        let stroke = stroke(&[Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)], settings);
        assert!(stroke.coverage.contains(&Point::new(50.0, 0.0)));
    }

    #[test]
    fn movement_shorter_than_two_and_a_half_units_is_not_lost() {
        let settings = settings(10.0, 0);
        let stroke = stroke(&[Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)], settings);
        assert!(stroke.coverage.contains(&Point::new(5.75, 0.0)));
        assert_eq!(stroke.samples.len(), 2);
    }

    #[test]
    fn fast_and_slow_sampling_of_same_straight_trajectory_match() {
        let settings = settings(12.0, 0);
        let fast = stroke(&[Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)], settings);
        let slow = stroke(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(20.0, 0.0),
                Vec2::new(40.0, 0.0),
                Vec2::new(60.0, 0.0),
                Vec2::new(80.0, 0.0),
                Vec2::new(100.0, 0.0),
            ],
            settings,
        );
        let mismatch = fast
            .coverage
            .difference(&slow.coverage)
            .union(&slow.coverage.difference(&fast.coverage));
        assert!(
            mismatch.unsigned_area() < 1.0e-5,
            "sampling mismatch area: {}",
            mismatch.unsigned_area()
        );
    }

    #[test]
    fn single_click_creates_a_round_nib_imprint() {
        let settings = settings(10.0, 0);
        let center = Vec2::new(25.0, 30.0);
        let stroke = stroke(&[center], settings);
        let bbox = stroke.coverage.bounding_rect().expect("dab bbox");
        assert!(stroke.coverage.contains(&Point::new(25.0, 30.0)));
        assert!((bbox.width() - 10.0).abs() < 0.05);
        assert!((bbox.height() - 10.0).abs() < 0.05);

        let ring = &stroke.coverage.0[0].exterior().0;
        assert_eq!(ring.len(), 65, "circle must keep 64 boundary segments");
        for coord in &ring[..ring.len() - 1] {
            let radius = ((coord.x - f64::from(center.x)).powi(2)
                + (coord.y - f64::from(center.y)).powi(2))
            .sqrt();
            assert!(
                (radius - 5.0).abs() < 1.0e-8,
                "non-circular dab radius {radius}"
            );
        }
    }

    #[test]
    fn single_click_stays_round_after_high_smoothing_finish() {
        let settings = settings(40.0, 100);
        let center = Vec2::new(25.0, 30.0);
        let finished = brush_finish(stroke(&[center], settings), settings);
        let ring = &finished.0[0].exterior().0;
        assert_eq!(
            ring.len(),
            65,
            "single dab must keep the exact circular nib"
        );
        for coord in &ring[..ring.len() - 1] {
            let radius = ((coord.x - f64::from(center.x)).powi(2)
                + (coord.y - f64::from(center.y)).powi(2))
            .sqrt();
            assert!(
                (radius - 20.0).abs() < 1.0e-8,
                "dab radius drifted to {radius}"
            );
        }
        let paths = coverage_to_paths(&finished);
        assert_eq!(paths.len(), 1);
        let flattened = flatten_path(&paths[0]);
        assert!(flattened.len() >= 64);
        for point in flattened {
            let radius = vec2_distance(point, center);
            assert!((radius - 20.0).abs() < 0.20, "rendered dab radius {radius}");
        }
    }

    #[test]
    fn circular_nib_is_independent_of_motion_direction() {
        let settings = settings(14.0, 0);
        let forward = stroke(&[Vec2::new(5.0, 8.0), Vec2::new(70.0, 30.0)], settings);
        let reverse = stroke(&[Vec2::new(70.0, 30.0), Vec2::new(5.0, 8.0)], settings);
        let mismatch = forward
            .coverage
            .difference(&reverse.coverage)
            .union(&reverse.coverage.difference(&forward.coverage));
        assert!(
            mismatch.unsigned_area() < 1.0e-5,
            "sampling mismatch area: {}",
            mismatch.unsigned_area()
        );
    }

    #[test]
    fn every_classic_nib_creates_a_real_static_imprint() {
        let center = Vec2::new(25.0, 30.0);
        for nib in BrushNib::ALL {
            let mut nib_settings = settings(20.0, 0);
            nib_settings.nib = nib;
            let stroke = stroke(&[center], nib_settings);
            let bbox = stroke.coverage.bounding_rect().expect("nib dab bbox");
            assert!(
                stroke.coverage.contains(&Point::new(25.0, 30.0)),
                "{nib:?} dab lost its center"
            );
            assert!(
                stroke.coverage.unsigned_area() > 1.0,
                "{nib:?} dab is empty"
            );
            assert!(bbox.width().is_finite() && bbox.height().is_finite());
        }
    }

    #[test]
    fn flat_nibs_keep_their_declared_orientation() {
        let center = Vec2::new(0.0, 0.0);
        let horizontal = sweep_nib(BrushNib::Horizontal, 30.0, center, center)
            .bounding_rect()
            .expect("horizontal bbox");
        let vertical = sweep_nib(BrushNib::Vertical, 30.0, center, center)
            .bounding_rect()
            .expect("vertical bbox");
        assert!(horizontal.width() > horizontal.height() * 2.5);
        assert!(vertical.height() > vertical.width() * 2.5);

        let slash_covariance: f32 = nib_outline(BrushNib::Slash, 30.0, center)
            .iter()
            .map(|point| point.x * point.y)
            .sum();
        let backslash_covariance: f32 = nib_outline(BrushNib::Backslash, 30.0, center)
            .iter()
            .map(|point| point.x * point.y)
            .sum();
        assert!(slash_covariance < 0.0, "slash angle flipped");
        assert!(backslash_covariance > 0.0, "backslash angle flipped");
    }

    #[test]
    fn every_static_nib_sweeps_sparse_samples_without_gaps_or_direction_drift() {
        let start = Vec2::new(5.0, 8.0);
        let end = Vec2::new(105.0, 48.0);
        for nib in BrushNib::ALL {
            let forward = sweep_nib(nib, 18.0, start, end);
            let reverse = sweep_nib(nib, 18.0, end, start);
            assert!(
                forward.contains(&Point::new(55.0, 28.0)),
                "{nib:?} left a sparse-sample gap"
            );
            let mismatch = forward
                .difference(&reverse)
                .union(&reverse.difference(&forward))
                .unsigned_area();
            assert!(mismatch < 1.0e-5, "{nib:?} direction mismatch: {mismatch}");
        }
    }

    #[test]
    fn polygon_nib_preview_and_commit_share_the_same_sweep() {
        let mut nib_settings = settings(24.0, 0);
        nib_settings.nib = BrushNib::Slash;
        let mut gesture = brush_begin(nib_settings, BrushSample::mouse(Vec2::new(10.0, 10.0)));
        brush_add_sample(
            &mut gesture,
            nib_settings,
            BrushSample::mouse(Vec2::new(90.0, 45.0)),
        );
        let preview_paths = brush_preview_paths_for_render(&gesture);
        let preview = vector_fill_geometry(&VectorAsset {
            asset_id: 0,
            paths: preview_paths,
            fill: Some(nib_settings.color),
            stroke: None,
        });
        brush_flush_pending(&mut gesture);
        let mismatch = preview
            .difference(&gesture.coverage)
            .union(&gesture.coverage.difference(&preview))
            .unsigned_area();
        assert!(mismatch < 0.05, "preview/commit mismatch: {mismatch}");
    }
    #[test]
    fn smoothing_polygon_nibs_does_not_restore_sharp_endpoint_imprints() {
        let points = [
            Vec2::new(20.0, 40.0),
            Vec2::new(70.0, 72.0),
            Vec2::new(130.0, 28.0),
            Vec2::new(190.0, 58.0),
        ];
        for nib in [
            BrushNib::Square,
            BrushNib::Horizontal,
            BrushNib::Vertical,
            BrushNib::Slash,
            BrushNib::Backslash,
        ] {
            let mut high = settings(30.0, 100);
            high.nib = nib;
            let raw = stroke(&points, high);
            let expected = smooth_contours(&raw.coverage, high.smoothing, high.size);
            let finished = brush_finish(raw, high);
            let mismatch = expected
                .difference(&finished)
                .union(&finished.difference(&expected))
                .unsigned_area();
            assert!(
                mismatch < 1.0e-6,
                "{nib:?} restored a sharp endpoint imprint: mismatch {mismatch}"
            );
            let paths = coverage_to_paths(&finished);
            assert_paths_are_finite_and_simple(&paths);
        }
    }
    #[test]
    fn dense_polygon_nib_preview_does_not_reintroduce_pathological_latency() {
        let mut nib_settings = settings(18.0, 35);
        nib_settings.nib = BrushNib::Square;
        let points: Vec<Vec2> = (0..=240)
            .map(|index| {
                let x = index as f32 * 1.5;
                Vec2::new(x, (index as f32 * 0.11).sin() * 24.0)
            })
            .collect();
        let mut gesture = brush_begin(nib_settings, BrushSample::mouse(points[0]));
        for point in &points[1..] {
            brush_add_sample(&mut gesture, nib_settings, BrushSample::mouse(*point));
        }

        let started = std::time::Instant::now();
        let preview = brush_preview_paths_for_render(&gesture);
        assert!(!preview.is_empty());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "dense square-nib preview took {:?}",
            started.elapsed()
        );
    }
    #[test]
    fn self_crossing_stroke_is_one_filled_component() {
        let settings = settings(12.0, 0);
        let stroke = stroke(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(60.0, 60.0),
                Vec2::new(0.0, 60.0),
                Vec2::new(60.0, 0.0),
            ],
            settings,
        );
        assert_eq!(stroke.coverage.0.len(), 1);
    }

    #[test]
    fn sharp_zigzag_never_grows_bezier_spikes() {
        let settings = settings(8.0, 100);
        let stroke = stroke(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(20.0, 40.0),
                Vec2::new(40.0, 0.0),
                Vec2::new(60.0, 40.0),
                Vec2::new(80.0, 0.0),
            ],
            settings,
        );
        let finished = brush_finish(stroke, settings);
        let bbox = finished.bounding_rect().expect("zigzag bbox");
        assert!(bbox.min().x >= -4.01 && bbox.max().x <= 84.01);
        assert!(bbox.min().y >= -4.01 && bbox.max().y <= 44.01);
        let paths = coverage_to_paths(&finished);
        let (min, max) = flattened_bounds(&paths);
        assert!(min.x >= -4.2 && max.x <= 84.2);
        assert!(min.y >= -4.2 && max.y <= 44.2);
        assert_paths_are_finite_and_simple(&paths);
    }

    #[test]
    fn star_gesture_never_grows_outside_nib_bounds() {
        let settings = settings(8.0, 100);
        let points = [
            Vec2::new(50.0, 0.0),
            Vec2::new(61.0, 35.0),
            Vec2::new(98.0, 35.0),
            Vec2::new(68.0, 57.0),
            Vec2::new(79.0, 92.0),
            Vec2::new(50.0, 70.0),
            Vec2::new(21.0, 92.0),
            Vec2::new(32.0, 57.0),
            Vec2::new(2.0, 35.0),
            Vec2::new(39.0, 35.0),
            Vec2::new(50.0, 0.0),
        ];
        let finished = brush_finish(stroke(&points, settings), settings);
        let bbox = finished.bounding_rect().expect("star bbox");
        assert!(bbox.min().x >= -2.01 && bbox.max().x <= 102.01);
        assert!(bbox.min().y >= -4.01 && bbox.max().y <= 96.01);
        let paths = coverage_to_paths(&finished);
        let (min, max) = flattened_bounds(&paths);
        assert!(min.x >= -2.2 && max.x <= 102.2);
        assert!(min.y >= -4.2 && max.y <= 96.2);
        assert_paths_are_finite_and_simple(&paths);
    }

    #[test]
    fn round_brush_boundary_uses_safe_bezier_curves() {
        let settings = settings(40.0, 50);
        let finished = brush_finish(stroke(&[Vec2::new(25.0, 30.0)], settings), settings);
        let paths = coverage_to_paths(&finished);
        assert_eq!(paths.len(), 1);
        assert!(paths[0]
            .anchors
            .iter()
            .any(|anchor| anchor.in_handle.is_some() && anchor.out_handle.is_some()));
        assert_paths_are_finite_and_simple(&paths);

        let (min, max) = flattened_bounds(&paths);
        assert!((min.x - 5.0).abs() < 0.25);
        assert!((max.x - 45.0).abs() < 0.25);
        assert!((min.y - 10.0).abs() < 0.25);
        assert!((max.y - 50.0).abs() < 0.25);
    }

    #[test]
    fn preview_and_zero_smoothing_commit_have_the_same_silhouette() {
        let settings = settings(9.0, 0);
        let stroke = stroke(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(30.0, 10.0),
                Vec2::new(45.0, 35.0),
            ],
            settings,
        );
        let preview = brush_preview_geometry(&stroke).to_vec();
        let preview_bounds = flattened_bounds(&preview);
        let committed = coverage_to_paths(&brush_finish(stroke, settings));
        let committed_bounds = flattened_bounds(&committed);
        assert!((preview_bounds.0.x - committed_bounds.0.x).abs() < 0.15);
        assert!((preview_bounds.0.y - committed_bounds.0.y).abs() < 0.15);
        assert!((preview_bounds.1.x - committed_bounds.1.x).abs() < 0.15);
        assert!((preview_bounds.1.y - committed_bounds.1.y).abs() < 0.15);
    }

    #[test]
    fn repeated_brush_commits_do_not_explode_curve_validation_work() {
        let mut app = EditorApp::default();
        let paint = settings(18.0, 50);
        let started = std::time::Instant::now();

        for offset in [0.0_f32, 28.0, 56.0] {
            let points: Vec<Vec2> = (0..=180)
                .map(|index| {
                    let x = index as f32 * 2.0;
                    let y = offset + (index as f32 * 0.13).sin() * 14.0;
                    Vec2::new(x, y)
                })
                .collect();
            let gesture = stroke(&points, paint);
            commit_brush_region(&mut app, brush_finish(gesture, paint), paint);
        }

        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "three medium commits took {:?}",
            started.elapsed()
        );
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 1);
    }

    #[test]
    fn repeated_overlapping_brush_commits_keep_geometry_bounded() {
        let mut app = EditorApp::default();
        let paint = settings(18.0, 50);
        let mut previous_anchor_count = 0usize;
        let started = std::time::Instant::now();

        for phase in [0.0_f32, 0.8, 1.6] {
            let points: Vec<Vec2> = (0..=360)
                .map(|index| {
                    let x = index as f32;
                    let y = (index as f32 * 0.17 + phase).sin() * 30.0
                        + (index as f32 * 0.047 + phase).cos() * 12.0;
                    Vec2::new(x, y)
                })
                .collect();
            let gesture = stroke(&points, paint);
            commit_brush_region(&mut app, brush_finish(gesture, paint), paint);
            let anchor_count: usize = app
                .state
                .project
                .assets
                .iter()
                .filter_map(|asset| match asset {
                    Asset::Vector(vector) if vector.fill == Some(paint.color) => Some(vector),
                    _ => None,
                })
                .flat_map(|vector| &vector.paths)
                .map(|path| path.anchors.len())
                .sum();
            assert!(
                previous_anchor_count == 0
                    || anchor_count < previous_anchor_count.saturating_mul(4),
                "anchor count exploded from {previous_anchor_count} to {anchor_count}"
            );
            previous_anchor_count = anchor_count;
        }

        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "three overlapping commits took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn raw_pointer_events_do_not_build_boolean_geometry_while_drawing() {
        let settings = settings(12.0, 0);
        let mut stroke = brush_begin(settings, BrushSample::mouse(Vec2::new(0.0, 0.0)));
        for x in 1..=240 {
            brush_add_sample(
                &mut stroke,
                settings,
                BrushSample::mouse(Vec2::new(x as f32, (x % 3) as f32)),
            );
        }

        assert!(stroke.dirty_preview);
        let before = stroke.coverage.bounding_rect().expect("initial dab bounds");
        assert!(before.max().x < 7.0, "add_sample must remain geometry-free");

        brush_flush_pending(&mut stroke);
        assert!(!stroke.dirty_preview);
        assert!(stroke.coverage.contains(&Point::new(120.0, 0.0)));
        assert!(!brush_preview_geometry(&stroke).is_empty());
        assert_eq!(stroke.coverage.0.len(), 1);
    }

    #[test]
    fn smoothing_hundred_visibly_reduces_boundary_points_vs_zero_and_fifty() {
        let raw_settings = settings(14.0, 0);
        let points: Vec<Vec2> = (0..=72)
            .map(|index| {
                let x = index as f32 * 2.0;
                let y = (index as f32 * 0.62).sin() * 1.8 + (index as f32 * 1.37).sin() * 0.65;
                Vec2::new(x, y)
            })
            .collect();
        let raw = stroke(&points, raw_settings).coverage;
        let fifty = smooth_contours(&raw, 50, raw_settings.size);
        let hundred = smooth_contours(&raw, 100, raw_settings.size);
        let zero_count = anchor_count(&coverage_to_paths(&raw));
        let fifty_count = anchor_count(&coverage_to_paths(&fifty));
        let hundred_count = anchor_count(&coverage_to_paths(&hundred));

        assert!(
            hundred_count < zero_count,
            "smoothing 0: {zero_count}, smoothing 100: {hundred_count}"
        );
        assert!(
            hundred_count < fifty_count,
            "smoothing 50: {fifty_count}, smoothing 100: {hundred_count}"
        );
    }

    #[test]
    fn smoothing_hundred_reduces_local_wobble_on_slow_stroke() {
        let raw_settings = settings(12.0, 0);
        let points: Vec<Vec2> = (0..=80)
            .map(|index| {
                let x = index as f32 * 1.5;
                let y = if index == 0 || index == 80 {
                    0.0
                } else {
                    (index as f32 * 0.83).sin() * 1.1
                };
                Vec2::new(x, y)
            })
            .collect();
        let raw = stroke(&points, raw_settings).coverage;
        let smooth = smooth_contours(&raw, 100, raw_settings.size);
        let ideal = sweep_nib(
            BrushNib::Circle,
            raw_settings.size,
            points[0],
            points[points.len() - 1],
        );
        let clip = MultiPolygon(vec![Polygon::new(
            LineString::new(vec![
                Coord { x: 12.0, y: -12.0 },
                Coord { x: 108.0, y: -12.0 },
                Coord { x: 108.0, y: 12.0 },
                Coord { x: 12.0, y: 12.0 },
                Coord { x: 12.0, y: -12.0 },
            ]),
            Vec::new(),
        )]);
        let raw = raw.intersection(&clip);
        let smooth = smooth.intersection(&clip);
        let ideal = ideal.intersection(&clip);
        let raw_error = raw
            .difference(&ideal)
            .union(&ideal.difference(&raw))
            .unsigned_area();
        let smooth_error = smooth
            .difference(&ideal)
            .union(&ideal.difference(&smooth))
            .unsigned_area();

        assert!(
            smooth_error < raw_error * 0.82,
            "raw wobble area: {raw_error}, smoothing 100 wobble area: {smooth_error}"
        );
    }

    #[test]
    fn smoothing_hundred_does_not_straighten_curves_into_long_chords() {
        let raw_settings = settings(30.0, 0);
        let points: Vec<Vec2> = (0..=180)
            .map(|index| {
                let t = index as f32 / 180.0;
                Vec2::new(
                    20.0 + t * 320.0,
                    120.0
                        + (t * std::f32::consts::TAU * 1.75).sin() * 68.0
                        + (t * std::f32::consts::TAU * 5.0).sin() * 4.0,
                )
            })
            .collect();
        let raw = stroke(&points, raw_settings).coverage;
        let smooth = smooth_contours(&raw, 100, raw_settings.size);
        let raw_max = max_ring_edge_length(raw.0[0].exterior());
        let smooth_max = max_ring_edge_length(smooth.0[0].exterior());
        let limit = raw_max * 2.5 + f64::from(raw_settings.size) * 0.18;

        assert!(
            smooth_max <= limit,
            "smoothing created a straighten chord: raw max {raw_max}, smoothed max {smooth_max}, limit {limit}"
        );
    }

    #[test]
    fn smoothing_hundred_preserves_general_bbox_within_brush_tolerance() {
        let raw_settings = settings(18.0, 0);
        let points = [
            Vec2::new(0.0, 0.0),
            Vec2::new(18.0, 2.0),
            Vec2::new(36.0, -2.0),
            Vec2::new(54.0, 2.5),
            Vec2::new(72.0, -1.5),
            Vec2::new(90.0, 0.0),
        ];
        let raw = stroke(&points, raw_settings).coverage;
        let smooth = smooth_contours(&raw, 100, raw_settings.size);
        let before = raw.bounding_rect().expect("raw bbox");
        let after = smooth.bounding_rect().expect("smoothed bbox");
        let tolerance = f64::from(raw_settings.size) * 0.15 + 1.0e-6;

        assert!((after.min().x - before.min().x).abs() <= tolerance);
        assert!((after.min().y - before.min().y).abs() <= tolerance);
        assert!((after.max().x - before.max().x).abs() <= tolerance);
        assert!((after.max().y - before.max().y).abs() <= tolerance);
    }

    #[test]
    fn smoothing_hundred_keeps_long_stroke_caps_round() {
        let high = settings(30.0, 100);
        let start = Vec2::new(20.0, 40.0);
        let end = Vec2::new(180.0, 40.0);
        let finished = brush_finish(stroke(&[start, end], high), high);
        let radius = high.size * 0.5;

        for degrees in [90.0_f32, 120.0, 150.0, 180.0, 210.0, 240.0, 270.0] {
            let angle = degrees.to_radians();
            let point = Vec2::new(
                start.x + angle.cos() * radius * 0.92,
                start.y + angle.sin() * radius * 0.92,
            );
            assert!(
                finished.contains(&Point::new(f64::from(point.x), f64::from(point.y))),
                "start cap became polygonal at {degrees} degrees"
            );
        }
        for degrees in [-90.0_f32, -60.0, -30.0, 0.0, 30.0, 60.0, 90.0] {
            let angle = degrees.to_radians();
            let point = Vec2::new(
                end.x + angle.cos() * radius * 0.92,
                end.y + angle.sin() * radius * 0.92,
            );
            assert!(
                finished.contains(&Point::new(f64::from(point.x), f64::from(point.y))),
                "end cap became polygonal at {degrees} degrees"
            );
        }
    }

    #[test]
    fn smoothing_hundred_does_not_shortcut_acute_bends() {
        let high = settings(10.0, 100);
        let points = [
            Vec2::new(0.0, 0.0),
            Vec2::new(40.0, 0.0),
            Vec2::new(43.0, 28.0),
        ];
        let finished = brush_finish(stroke(&points, high), high);

        for point in points {
            assert!(
                finished.contains(&Point::new(f64::from(point.x), f64::from(point.y))),
                "high smoothing cut through the gesture at {point:?}"
            );
        }
        assert!(finished.contains(&Point::new(42.5, -2.5)));
    }

    #[test]
    fn stage_geometry_has_no_zoom_input() {
        let settings = settings(10.0, 50);
        let first = stroke(&[Vec2::new(10.0, 10.0), Vec2::new(80.0, 45.0)], settings);
        let second = stroke(&[Vec2::new(10.0, 10.0), Vec2::new(80.0, 45.0)], settings);
        assert_eq!(
            coverage_to_paths(&brush_finish(first, settings)),
            coverage_to_paths(&brush_finish(second, settings))
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn classic_brush_is_plain_vector_by_default_and_glow_is_opt_in() {
        let mut plain_app = EditorApp::default();
        let plain = settings(16.0, 0);
        assert!(!plain.glow);
        let plain_stroke = stroke(&[Vec2::new(10.0, 10.0), Vec2::new(40.0, 10.0)], plain);
        commit_brush_region(&mut plain_app, brush_finish(plain_stroke, plain), plain);
        assert!(plain_app.state.project.asset_appearances.is_empty());

        let mut glow_app = EditorApp::default();
        let glow = BrushSettings {
            glow: true,
            ..settings(16.0, 0)
        };
        let glow_stroke = stroke(&[Vec2::new(10.0, 10.0), Vec2::new(40.0, 10.0)], glow);
        commit_brush_region(&mut glow_app, brush_finish(glow_stroke, glow), glow);
        assert_eq!(glow_app.state.project.asset_appearances.len(), 1);
        let appearance = glow_app
            .state
            .project
            .asset_appearances
            .values()
            .next()
            .unwrap();
        assert!(matches!(
            appearance.material,
            VectorMaterial::SoftHalo { .. }
        ));
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn plain_and_glowing_paint_of_same_colour_remain_distinct_styles() {
        let mut app = EditorApp::default();
        let plain = settings(12.0, 0);
        let glow = BrushSettings {
            glow: true,
            ..plain
        };
        let left = stroke(&[Vec2::new(10.0, 20.0), Vec2::new(30.0, 20.0)], plain);
        commit_brush_region(&mut app, brush_finish(left, plain), plain);
        let right = stroke(&[Vec2::new(50.0, 20.0), Vec2::new(70.0, 20.0)], glow);
        commit_brush_region(&mut app, brush_finish(right, glow), glow);

        assert_eq!(app.state.project.assets.len(), 2);
        assert_eq!(app.state.project.asset_appearances.len(), 1);
    }

    #[cfg(not(feature = "appearance-mask-eraser"))]
    #[test]
    fn disabling_appearance_engine_restores_plain_merge_drawing_even_if_glow_is_set() {
        let mut app = EditorApp::default();
        let plain = settings(12.0, 0);
        let glow = BrushSettings {
            glow: true,
            ..plain
        };
        let left = stroke(&[Vec2::new(10.0, 20.0), Vec2::new(30.0, 20.0)], plain);
        commit_brush_region(&mut app, brush_finish(left, plain), plain);
        let right = stroke(&[Vec2::new(25.0, 20.0), Vec2::new(45.0, 20.0)], glow);
        commit_brush_region(&mut app, brush_finish(right, glow), glow);

        assert_eq!(app.state.project.assets.len(), 1);
        assert!(app.state.project.asset_appearances.is_empty());
    }

    #[test]
    fn committed_brush_asset_is_fill_only() {
        let mut app = EditorApp::default();
        let settings = settings(10.0, 0);
        let stroke = stroke(&[Vec2::new(10.0, 10.0), Vec2::new(50.0, 10.0)], settings);
        commit_brush_region(&mut app, brush_finish(stroke, settings), settings);
        let Asset::Vector(vector) = &app.state.project.assets[0] else {
            panic!("brush must commit a vector asset");
        };
        assert_eq!(vector.fill, Some(settings.color));
        assert_eq!(vector.stroke, None);
        assert!(vector.paths.iter().all(|path| path.closed));
    }

    #[test]
    fn contained_raw_fill_is_not_destroyed_by_post_drag_merge() {
        let mut app = EditorApp::default();
        let color = Rgba {
            r: 20,
            g: 40,
            b: 80,
            a: 128,
        };
        app.state.project.assets = vec![
            raw_fill_asset(1, color, square_path(0.0, 0.0, 100.0, 100.0)),
            raw_fill_asset(2, color, square_path(30.0, 30.0, 70.0, 70.0)),
        ];
        app.state.project.q0rgs[0].layers[0].placements = vec![
            Placement {
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            },
            Placement {
                frame: 0,
                target: Target::Asset(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            },
        ];

        assert!(!merge_touching_raw_fills_after_edit(
            &mut app.state.project,
            1,
            1,
            0
        ));
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 2);
        assert_eq!(app.state.project.assets.len(), 2);
        let inner = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == 2)
            .unwrap();
        let Asset::Vector(inner) = inner else {
            unreachable!()
        };
        assert!(vector_fill_geometry(inner).contains(&Point::new(50.0, 50.0)));
    }

    #[test]
    fn moved_overlapping_translucent_raw_fills_become_one_surface() {
        let mut app = EditorApp::default();
        let color = Rgba {
            r: 20,
            g: 40,
            b: 80,
            a: 96,
        };
        app.state.project.assets = vec![
            raw_fill_asset(1, color, square_path(0.0, 0.0, 10.0, 10.0)),
            raw_fill_asset(2, color, square_path(5.0, 0.0, 15.0, 10.0)),
        ];
        app.state.project.q0rgs[0].layers[0].placements = vec![
            Placement {
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            },
            Placement {
                frame: 0,
                target: Target::Asset(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            },
        ];

        assert!(merge_touching_raw_fills_after_edit(
            &mut app.state.project,
            1,
            1,
            0
        ));
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 1);
        assert_eq!(app.state.project.assets.len(), 1);
        let Asset::Vector(vector) = &app.state.project.assets[0] else {
            panic!("merged raw fill must remain vector");
        };
        assert_eq!(vector.fill, Some(color));
        assert_eq!(vector.paths.len(), 1);
        assert!((vector_fill_geometry(vector).unsigned_area() - 150.0).abs() < 0.1);
    }

    #[test]
    fn moving_one_contour_into_its_raw_asset_neighbour_unions_them() {
        let mut app = EditorApp::default();
        let color = Rgba {
            r: 120,
            g: 80,
            b: 30,
            a: 128,
        };
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![
                square_path(0.0, 0.0, 10.0, 10.0),
                square_path(5.0, 0.0, 15.0, 10.0),
            ],
            fill: Some(color),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];

        assert!(merge_touching_raw_fills_after_edit(
            &mut app.state.project,
            1,
            1,
            0
        ));
        let Asset::Vector(vector) = &app.state.project.assets[0] else {
            panic!("raw fill must remain vector");
        };
        assert_eq!(vector.paths.len(), 1);
        assert!((vector_fill_geometry(vector).unsigned_area() - 150.0).abs() < 0.1);
    }

    #[test]
    fn disconnected_same_colour_raw_fills_stay_independent_after_move() {
        let mut app = EditorApp::default();
        let color = Rgba {
            r: 20,
            g: 40,
            b: 80,
            a: 96,
        };
        app.state.project.assets = vec![
            raw_fill_asset(1, color, square_path(0.0, 0.0, 10.0, 10.0)),
            raw_fill_asset(2, color, square_path(20.0, 0.0, 30.0, 10.0)),
        ];
        app.state.project.q0rgs[0].layers[0].placements = vec![
            Placement {
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            },
            Placement {
                frame: 0,
                target: Target::Asset(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            },
        ];

        assert!(!merge_touching_raw_fills_after_edit(
            &mut app.state.project,
            1,
            1,
            0
        ));
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 2);
        assert_eq!(app.state.project.assets.len(), 2);
    }

    #[test]
    fn disconnected_same_colour_brush_strokes_survive_later_commits() {
        let mut app = EditorApp::default();
        let paint = settings(18.0, 50);
        let centers = [
            Vec2::new(30.0, 30.0),
            Vec2::new(90.0, 30.0),
            Vec2::new(150.0, 30.0),
        ];

        for (index, center) in centers.iter().copied().enumerate() {
            let gesture = stroke(
                &[
                    Vec2::new(center.x - 12.0, center.y),
                    Vec2::new(center.x + 12.0, center.y),
                ],
                paint,
            );
            commit_brush_region(&mut app, brush_finish(gesture, paint), paint);

            let vector = app
                .state
                .project
                .assets
                .iter()
                .find_map(|asset| match asset {
                    Asset::Vector(vector) if vector.fill == Some(paint.color) => Some(vector),
                    _ => None,
                })
                .expect("merged brush vector");
            let geometry = vector_fill_geometry(vector);
            for expected in &centers[..=index] {
                assert!(
                    geometry.contains(&Point::new(expected.x as f64, expected.y as f64)),
                    "stroke at {expected:?} vanished after commit {}",
                    index + 1
                );
            }
        }
    }

    #[test]
    fn same_colour_strokes_inside_closed_raw_ring_survive_and_remain_islands() {
        let mut app = EditorApp::default();
        let paint = settings(12.0, 50);
        let mut inner = square_path(20.0, 20.0, 180.0, 180.0);
        inner.anchors.reverse();
        app.state.project.assets = vec![Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![square_path(0.0, 0.0, 200.0, 200.0), inner],
            fill: Some(paint.color),
            stroke: None,
        })];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];

        let centers = [Vec2::new(60.0, 100.0), Vec2::new(140.0, 100.0)];
        for (index, center) in centers.iter().copied().enumerate() {
            let gesture = stroke(
                &[
                    Vec2::new(center.x - 15.0, center.y),
                    Vec2::new(center.x + 15.0, center.y),
                ],
                paint,
            );
            commit_brush_region(&mut app, brush_finish(gesture, paint), paint);
            let vector = app
                .state
                .project
                .assets
                .iter()
                .find_map(|asset| match asset {
                    Asset::Vector(vector) if vector.fill == Some(paint.color) => Some(vector),
                    _ => None,
                })
                .expect("ring plus inner strokes");
            let geometry = vector_fill_geometry(vector);
            assert!(
                !geometry.contains(&Point::new(100.0, 60.0)),
                "untouched part of the ring hole must remain empty"
            );
            for expected in &centers[..=index] {
                assert!(
                    geometry.contains(&Point::new(expected.x as f64, expected.y as f64)),
                    "inner stroke at {expected:?} vanished after commit {}",
                    index + 1
                );
            }
        }
    }

    #[test]
    fn intersecting_gestures_are_geometrically_unioned() {
        let mut app = EditorApp::default();
        let settings = settings(12.0, 0);
        let horizontal = stroke(&[Vec2::new(0.0, 20.0), Vec2::new(60.0, 20.0)], settings);
        let vertical = stroke(&[Vec2::new(30.0, 0.0), Vec2::new(30.0, 40.0)], settings);
        commit_brush_region(&mut app, brush_finish(horizontal, settings), settings);
        commit_brush_region(&mut app, brush_finish(vertical, settings), settings);

        assert_eq!(app.state.project.assets.len(), 1);
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 1);
        let Asset::Vector(vector) = &app.state.project.assets[0] else {
            panic!("brush must commit a vector asset");
        };
        assert_eq!(vector_fill_geometry(vector).0.len(), 1);
    }

    #[test]
    fn different_colour_brush_cuts_old_raw_fill() {
        let mut app = EditorApp::default();
        let mut red = settings(20.0, 0);
        red.color = Rgba {
            r: 220,
            g: 20,
            b: 20,
            a: 255,
        };
        let mut blue = settings(12.0, 0);
        blue.color = Rgba {
            r: 20,
            g: 40,
            b: 220,
            a: 255,
        };

        let horizontal = stroke(&[Vec2::new(0.0, 20.0), Vec2::new(60.0, 20.0)], red);
        let vertical = stroke(&[Vec2::new(30.0, -20.0), Vec2::new(30.0, 60.0)], blue);
        commit_brush_region(&mut app, brush_finish(horizontal, red), red);
        commit_brush_region(&mut app, brush_finish(vertical, blue), blue);

        let red_vector = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(red.color) => Some(vector),
                _ => None,
            })
            .expect("red raw fill");
        let blue_vector = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(blue.color) => Some(vector),
                _ => None,
            })
            .expect("blue raw fill");

        let red_geometry = vector_fill_geometry(red_vector);
        let blue_geometry = vector_fill_geometry(blue_vector);
        assert!(!red_geometry.contains(&Point::new(30.0, 20.0)));
        assert!(blue_geometry.contains(&Point::new(30.0, 20.0)));
        assert_eq!(
            red_geometry.0.len(),
            2,
            "old colour must be split, not covered"
        );
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 2);
    }

    #[test]
    fn area_eraser_subtracts_and_splits_raw_fill() {
        let mut app = EditorApp::default();
        let paint = settings(20.0, 0);
        let eraser = settings(12.0, 0);
        let horizontal = stroke(&[Vec2::new(0.0, 20.0), Vec2::new(60.0, 20.0)], paint);
        let cut = stroke(&[Vec2::new(30.0, -20.0), Vec2::new(30.0, 60.0)], eraser);
        commit_brush_region(&mut app, brush_finish(horizontal, paint), paint);
        // This test exercises the legacy raw-geometry eraser specifically.
        // Appearance-enabled brush assets are owned by the mask eraser instead.
        app.state.project.asset_appearances.clear();

        assert!(erase_brush_region(&mut app, brush_finish(cut, eraser)));
        let Asset::Vector(vector) = &app.state.project.assets[0] else {
            panic!("remaining raw fill");
        };
        let geometry = vector_fill_geometry(vector);
        assert!(!geometry.contains(&Point::new(30.0, 20.0)));
        assert_eq!(geometry.0.len(), 2);
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 1);
    }

    #[test]
    fn area_eraser_removes_empty_raw_asset_and_placement() {
        let mut app = EditorApp::default();
        let paint = settings(10.0, 0);
        let dab = stroke(&[Vec2::new(20.0, 20.0)], paint);
        commit_brush_region(&mut app, brush_finish(dab, paint), paint);
        // This test exercises complete deletion in the legacy raw-geometry path.
        app.state.project.asset_appearances.clear();

        let mut large_eraser = settings(40.0, 0);
        large_eraser.color = paint.color;
        let cut = stroke(&[Vec2::new(20.0, 20.0)], large_eraser);
        assert!(erase_brush_region(
            &mut app,
            brush_finish(cut, large_eraser)
        ));
        assert!(app.state.project.assets.is_empty());
        assert!(app.state.project.q0rgs[0].layers[0].placements.is_empty());
    }

    #[test]
    fn painting_one_frame_does_not_mutate_shared_asset_on_another_frame() {
        let mut app = EditorApp::default();
        let mut red = settings(20.0, 0);
        red.color = Rgba {
            r: 220,
            g: 20,
            b: 20,
            a: 255,
        };
        let mut blue = settings(12.0, 0);
        blue.color = Rgba {
            r: 20,
            g: 40,
            b: 220,
            a: 255,
        };
        let horizontal = stroke(&[Vec2::new(0.0, 20.0), Vec2::new(60.0, 20.0)], red);
        commit_brush_region(&mut app, brush_finish(horizontal, red), red);

        let shared_asset_id = match app.state.project.q0rgs[0].layers[0].placements[0].target {
            Target::Asset(asset_id) => asset_id,
            Target::Q0rg(_) => panic!("raw vector asset"),
        };
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                frame: 1,
                target: Target::Asset(shared_asset_id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            });

        let vertical = stroke(&[Vec2::new(30.0, -20.0), Vec2::new(30.0, 60.0)], blue);
        commit_brush_region(&mut app, brush_finish(vertical, blue), blue);

        let frame_one_asset = app.state.project.q0rgs[0].layers[0]
            .placements
            .iter()
            .find(|placement| placement.frame == 1)
            .and_then(|placement| match placement.target {
                Target::Asset(asset_id) => Some(asset_id),
                Target::Q0rg(_) => None,
            })
            .expect("frame one raw asset");
        assert_eq!(frame_one_asset, shared_asset_id);
        let Asset::Vector(frame_one_vector) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == frame_one_asset)
            .expect("frame one vector")
        else {
            panic!("vector asset");
        };
        assert!(vector_fill_geometry(frame_one_vector).contains(&Point::new(30.0, 20.0)));

        let frame_zero_red = app.state.project.q0rgs[0].layers[0]
            .placements
            .iter()
            .filter(|placement| placement.frame == 0)
            .filter_map(|placement| match placement.target {
                Target::Asset(asset_id) => app
                    .state
                    .project
                    .assets
                    .iter()
                    .find(|asset| asset.id() == asset_id),
                Target::Q0rg(_) => None,
            })
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(red.color) => Some(vector),
                _ => None,
            })
            .expect("frame zero red fill");
        assert!(!vector_fill_geometry(frame_zero_red).contains(&Point::new(30.0, 20.0)));
    }

    #[test]
    fn boolean_reconstruction_uses_boundary_anchors_not_bezier_samples() {
        let path = VPath {
            anchors: vec![
                Anchor {
                    point: Vec2::new(0.0, 0.0),
                    in_handle: Some(Vec2::new(-2.0, 0.0)),
                    out_handle: Some(Vec2::new(2.0, 0.0)),
                },
                Anchor {
                    point: Vec2::new(20.0, 0.0),
                    in_handle: Some(Vec2::new(18.0, -2.0)),
                    out_handle: Some(Vec2::new(22.0, 2.0)),
                },
                Anchor {
                    point: Vec2::new(20.0, 20.0),
                    in_handle: Some(Vec2::new(22.0, 18.0)),
                    out_handle: Some(Vec2::new(18.0, 22.0)),
                },
                Anchor {
                    point: Vec2::new(0.0, 20.0),
                    in_handle: Some(Vec2::new(2.0, 22.0)),
                    out_handle: Some(Vec2::new(-2.0, 18.0)),
                },
            ],
            closed: true,
        };

        assert!(flatten_path(&path).len() > path.anchors.len() * 8);
        let polygon = path_to_polygon(&path).expect("canonical raw polygon");
        assert_eq!(
            polygon.exterior().0.len(),
            path.anchors.len() + 1,
            "merge drawing must not multiply Bézier samples on every commit"
        );
    }

    #[test]
    fn repeated_same_colour_commits_do_not_multiply_boundary_points() {
        let mut app = EditorApp::default();
        let paint = settings(18.0, 50);
        let mut counts = Vec::new();

        for offset in 0..6 {
            let y = 20.0 + offset as f32 * 7.0;
            let gesture = stroke(
                &[
                    Vec2::new(10.0, y),
                    Vec2::new(45.0, y + 12.0),
                    Vec2::new(80.0, y),
                ],
                paint,
            );
            commit_brush_region(&mut app, brush_finish(gesture, paint), paint);
            let vector = app
                .state
                .project
                .assets
                .iter()
                .find_map(|asset| match asset {
                    Asset::Vector(vector) if vector.fill == Some(paint.color) => Some(vector),
                    _ => None,
                })
                .expect("merged brush vector");
            counts.push(anchor_count(&vector.paths));

            let canonical_points: usize = vector
                .paths
                .iter()
                .filter_map(path_to_polygon)
                .map(|polygon| polygon.exterior().0.len().saturating_sub(1))
                .sum();
            assert_eq!(canonical_points, anchor_count(&vector.paths));
        }

        for pair in counts.windows(2) {
            assert!(
                pair[1] < pair[0].saturating_mul(4).max(256),
                "boundary point count exploded across commits: {counts:?}"
            );
        }
        assert!(
            counts.last().copied().unwrap_or_default() < 2_000,
            "six ordinary gestures created an unreasonable boundary: {counts:?}"
        );
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 1);
    }

    #[test]
    fn three_dense_strokes_commit_without_exponential_freeze() {
        use std::time::{Duration, Instant};

        let mut app = EditorApp::default();
        let paint = settings(14.0, 50);
        let started = Instant::now();

        for stroke_index in 0..3 {
            let points: Vec<Vec2> = (0..=240)
                .map(|sample| {
                    let x = sample as f32 * 1.5;
                    let wave = ((sample as f32) * 0.08).sin() * 28.0;
                    Vec2::new(20.0 + x, 100.0 + wave + stroke_index as f32 * 9.0)
                })
                .collect();
            let gesture = stroke(&points, paint);
            commit_brush_region(&mut app, brush_finish(gesture, paint), paint);
        }

        assert!(
            started.elapsed() < Duration::from_secs(20),
            "three dense commits took {:?}",
            started.elapsed()
        );
        let vector = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(paint.color) => Some(vector),
                _ => None,
            })
            .expect("merged dense brush vector");
        assert!(anchor_count(&vector.paths) < 20_000);
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 1);
    }

    #[test]
    fn merge_drawing_keeps_transformed_instance_of_shared_asset() {
        let mut app = EditorApp::default();
        let paint = settings(12.0, 0);
        let first = stroke(&[Vec2::new(0.0, 20.0), Vec2::new(40.0, 20.0)], paint);
        commit_brush_region(&mut app, brush_finish(first, paint), paint);

        let original_asset_id = app.state.project.assets[0].id();
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                frame: 0,
                target: Target::Asset(original_asset_id),
                transform: Transform2D {
                    tx: 100.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
            });

        let second = stroke(&[Vec2::new(20.0, 0.0), Vec2::new(20.0, 40.0)], paint);
        commit_brush_region(&mut app, brush_finish(second, paint), paint);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(layer.placements.len(), 2);
        assert!(layer.placements.iter().any(|placement| {
            placement.target == Target::Asset(original_asset_id) && placement.transform.tx == 100.0
        }));
        assert!(layer.placements.iter().any(|placement| {
            placement.transform == Transform2D::IDENTITY
                && placement.target != Target::Asset(original_asset_id)
        }));
        assert!(app
            .state
            .project
            .assets
            .iter()
            .any(|asset| asset.id() == original_asset_id));
    }
}
