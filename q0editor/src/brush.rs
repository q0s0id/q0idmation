use std::cell::{Cell, OnceCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use geo::algorithm::buffer::{BufferStyle, LineCap, LineJoin};
use geo::{
    Area, BooleanOps, BoundingRect, Buffer, Contains, ConvexHull, Coord, Intersects, Line,
    LineString, MultiPoint, MultiPolygon, Point, Polygon, SimplifyVw, SimplifyVwPreserve,
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
    /// Use tablet/touch pressure to modulate the nib size.
    pub pressure_size: bool,
    /// Use pointer velocity to make fast motion thinner.
    pub velocity_size: bool,
    /// Shared response strength for pressure and velocity, 0..=100.
    pub dynamics_sensitivity: u8,
    /// Smallest dynamic diameter as a fraction of the nominal size, 0.01..=1.
    pub dynamics_min_size: f32,
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
            pressure_size: false,
            velocity_size: false,
            dynamics_sensitivity: 50,
            dynamics_min_size: 0.2,
            scale_with_stage: true,
            sync_with_eraser: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushSample {
    pub position: Vec2,
    pub pressure: Option<f32>,
    pub time_seconds: f64,
}

impl BrushSample {
    pub fn mouse(position: Vec2) -> Self {
        Self {
            position,
            pressure: None,
            time_seconds: 0.0,
        }
    }

    pub fn pointer(position: Vec2, pressure: Option<f32>, time_seconds: f64) -> Self {
        Self {
            position,
            pressure,
            time_seconds,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BrushStroke {
    pub samples: Vec<BrushSample>,
    // Stage-space nib size when each input sample was recorded. In screen-size
    // mode zoom changes future dabs, never the widths of earlier artwork.
    nominal_sizes: Vec<f32>,
    prerender_geometry: Vec<Vec<Vec2>>,
    prerender_dirty_from: Option<usize>,
    #[cfg(test)]
    pub(crate) prerender_segment_builds: usize,
    pub coverage: MultiPolygon<f64>,
    preview_paths: Vec<VPath>,
    nib: BrushNib,
    size: f32,
    smoothing: u8,
    pressure_size: bool,
    velocity_size: bool,
    dynamics_sensitivity: u8,
    dynamics_min_size: f32,
    last_pressure: Option<f32>,
    pub dirty_preview: bool,
}

pub fn brush_begin(settings: BrushSettings, sample: BrushSample) -> BrushStroke {
    let initial_size = dynamic_sample_sizes(&[sample], settings)[0];
    let coverage = sweep_nib(settings.nib, initial_size, sample.position, sample.position);
    let preview_paths = coverage_to_linear_paths(&coverage);
    BrushStroke {
        samples: vec![sample],
        nominal_sizes: vec![settings.size],
        prerender_geometry: Vec::new(),
        prerender_dirty_from: Some(0),
        #[cfg(test)]
        prerender_segment_builds: 0,
        coverage,
        preview_paths,
        nib: settings.nib,
        size: settings.size,
        smoothing: settings.smoothing,
        pressure_size: settings.pressure_size,
        velocity_size: settings.velocity_size,
        dynamics_sensitivity: settings.dynamics_sensitivity.min(100),
        dynamics_min_size: sanitized_dynamic_min_size(settings.dynamics_min_size),
        last_pressure: sample.pressure,
        dirty_preview: false,
    }
}

pub fn brush_add_sample(
    stroke: &mut BrushStroke,
    settings: BrushSettings,
    mut sample: BrushSample,
) {
    let mut changed_from = (stroke.nib != settings.nib
        || stroke.pressure_size != settings.pressure_size
        || stroke.velocity_size != settings.velocity_size
        || stroke.dynamics_sensitivity != settings.dynamics_sensitivity.min(100)
        || stroke.dynamics_min_size != sanitized_dynamic_min_size(settings.dynamics_min_size))
    .then_some(0);
    // Windows pen input is not guaranteed to attach pressure to every motion
    // event. Once a gesture has produced real pen pressure, a pressureless
    // motion sample is missing data, not a mouse sample at 100% pressure.
    // Carry the last real value across those gaps. If pressure first appears a
    // few events into the gesture, backfill the short pressureless prefix too,
    // otherwise the committed vector gets a full-size circular start blob.
    if let Some(pressure) = sample.pressure {
        if stroke.last_pressure.is_none() {
            changed_from = Some(0);
            for previous in &mut stroke.samples {
                if previous.pressure.is_none() {
                    previous.pressure = Some(pressure);
                }
            }
        }
        stroke.last_pressure = Some(pressure);
    } else if let Some(pressure) = stroke.last_pressure {
        sample.pressure = Some(pressure);
    }

    if stroke
        .samples
        .last()
        .is_some_and(|last| vec2_distance(last.position, sample.position) <= 1.0e-5)
    {
        if let Some(last) = stroke.samples.last_mut() {
            if sample.pressure.is_some() && sample.pressure != last.pressure {
                last.pressure = sample.pressure;
                changed_from = Some(
                    changed_from
                        .unwrap_or(usize::MAX)
                        .min(stroke.samples.len().saturating_sub(2)),
                );
            }
        }
    } else {
        changed_from = Some(
            changed_from
                .unwrap_or(usize::MAX)
                .min(stroke.samples.len().saturating_sub(1)),
        );
        stroke.samples.push(sample);
        stroke.nominal_sizes.push(settings.size);
    }
    stroke.nib = settings.nib;
    stroke.size = settings.size;
    stroke.smoothing = settings.smoothing;
    stroke.pressure_size = settings.pressure_size;
    stroke.velocity_size = settings.velocity_size;
    stroke.dynamics_sensitivity = settings.dynamics_sensitivity.min(100);
    stroke.dynamics_min_size = sanitized_dynamic_min_size(settings.dynamics_min_size);
    if let Some(first) = changed_from {
        stroke.prerender_dirty_from = Some(stroke.prerender_dirty_from.unwrap_or(first).min(first));
    }
    stroke.dirty_preview = true;
}

pub(crate) fn brush_prerender_dirty_from(stroke: &BrushStroke) -> Option<usize> {
    stroke.prerender_dirty_from
}

fn prerender_nib(nib: BrushNib, center: Vec2, size: f32) -> Vec<Vec2> {
    if nib != BrushNib::Circle {
        return nib_outline(nib, size.max(0.1), center);
    }
    (0..24)
        .map(|index| {
            let angle = std::f32::consts::TAU * index as f32 / 24.0;
            Vec2::new(
                center.x + angle.cos() * size.max(0.1) * 0.5,
                center.y + angle.sin() * size.max(0.1) * 0.5,
            )
        })
        .collect()
}

/// Cache raw nib sweeps in document coordinates. Zoom only projects these
/// vertices; it never repeats convex-hull construction for the stroke history.
/// The cache includes all samples, including artwork outside the stage paper.
pub(crate) fn brush_prerender_geometry(stroke: &mut BrushStroke) -> &[Vec<Vec2>] {
    if let Some(first) = stroke.prerender_dirty_from.take() {
        if stroke.samples.len() == 1 {
            let (center, size) = brush_prerender_dab_at(stroke, 0).unwrap();
            stroke.prerender_geometry = vec![prerender_nib(stroke.nib, center, size)];
            #[cfg(test)]
            {
                stroke.prerender_segment_builds += 1;
            }
        } else {
            stroke.prerender_geometry.truncate(first);
            for index in stroke.prerender_geometry.len()..stroke.samples.len() - 1 {
                let (start, start_size) = brush_prerender_dab_at(stroke, index).unwrap();
                let (end, end_size) = brush_prerender_dab_at(stroke, index + 1).unwrap();
                let hull = MultiPoint::new(
                    prerender_nib(stroke.nib, start, start_size)
                        .into_iter()
                        .chain(prerender_nib(stroke.nib, end, end_size))
                        .map(|point| Point::new(f64::from(point.x), f64::from(point.y)))
                        .collect(),
                )
                .convex_hull();
                let mut contour: Vec<_> = hull
                    .exterior()
                    .0
                    .iter()
                    .map(|point| Vec2::new(point.x as f32, point.y as f32))
                    .collect();
                if contour.first() == contour.last() {
                    contour.pop();
                }
                stroke.prerender_geometry.push(contour);
                #[cfg(test)]
                {
                    stroke.prerender_segment_builds += 1;
                }
            }
        }
    }
    &stroke.prerender_geometry
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
    let sizes = stroke_sample_sizes(stroke);
    stroke.coverage = if stroke_has_variable_sizes(stroke) {
        sweep_variable_trajectory_nib(stroke.nib, &trajectory, &sizes)
    } else {
        sweep_trajectory_nib(stroke.nib, stroke.nominal_sizes[0], &trajectory)
    };
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

pub fn brush_size_dynamics_enabled(stroke: &BrushStroke) -> bool {
    stroke.pressure_size || stroke.velocity_size
}

fn stroke_has_variable_sizes(stroke: &BrushStroke) -> bool {
    brush_size_dynamics_enabled(stroke)
        || stroke
            .nominal_sizes
            .windows(2)
            .any(|pair| pair[0] != pair[1])
}

pub fn brush_preview_dabs(stroke: &BrushStroke) -> Vec<(Vec2, f32)> {
    let trajectory = build_sweep_trajectory(&stroke.samples, stroke.smoothing, stroke.size);
    let sizes = stroke_sample_sizes(stroke);
    trajectory.into_iter().zip(sizes).collect()
}

/// Raw, temporary classic-brush preview samples. Unlike the committed vector
/// sweep this deliberately skips trajectory/boundary smoothing and boolean
/// reconstruction: pointer-down paints a transient pre-render, pointer-up is
/// where the authoritative vector fill is produced.
pub fn brush_prerender_dabs(stroke: &BrushStroke) -> Vec<(Vec2, f32)> {
    let sizes = stroke_sample_sizes(stroke);
    stroke
        .samples
        .iter()
        .map(|sample| sample.position)
        .zip(sizes)
        .collect()
}

/// One raw pre-render dab without scanning the complete gesture. Used by the
/// incremental texture preview so adding a new pointer sample stays O(1).
pub fn brush_prerender_dab_at(stroke: &BrushStroke, index: usize) -> Option<(Vec2, f32)> {
    let sample = stroke.samples.get(index)?;
    let settings = BrushSettings {
        size: *stroke.nominal_sizes.get(index)?,
        pressure_size: stroke.pressure_size,
        velocity_size: stroke.velocity_size,
        dynamics_sensitivity: stroke.dynamics_sensitivity,
        dynamics_min_size: stroke.dynamics_min_size,
        ..BrushSettings::default()
    };
    Some((
        sample.position,
        dynamic_sample_size(&stroke.samples, index, settings),
    ))
}

/// Current dynamic nib diameter in stage units. This intentionally evaluates
/// only the latest raw sample, so cursor feedback stays O(1) even on a very
/// long gesture and cannot reintroduce progressive preview lag.
pub fn brush_current_size(stroke: &BrushStroke) -> f32 {
    stroke
        .samples
        .len()
        .checked_sub(1)
        .and_then(|index| brush_prerender_dab_at(stroke, index))
        .map(|(_, size)| size)
        .unwrap_or_else(|| stroke.size.max(0.1))
}

/// Materialize the current gesture silhouette for non-circular live previews.
/// Circle stays on the cheaper renderer path; fixed polygon nibs use this exact
/// sweep so preview and commit cannot disagree about corners or orientation.
pub fn brush_preview_paths_for_render(stroke: &BrushStroke) -> Vec<VPath> {
    if !stroke.dirty_preview {
        return stroke.preview_paths.clone();
    }
    let trajectory = build_sweep_trajectory(&stroke.samples, stroke.smoothing, stroke.size);
    let coverage = if stroke_has_variable_sizes(stroke) {
        let sizes = stroke_sample_sizes(stroke);
        sweep_variable_trajectory_nib(stroke.nib, &trajectory, &sizes)
    } else {
        sweep_trajectory_nib(stroke.nib, stroke.nominal_sizes[0], &trajectory)
    };
    coverage_to_linear_paths(&coverage)
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
    // Viewport changes on the release frame must not change either the
    // authored widths or the gesture's smoothing scale.
    stroke.size = stroke.nominal_sizes[0];
    stroke.smoothing = settings.smoothing;
    stroke.pressure_size = settings.pressure_size;
    stroke.velocity_size = settings.velocity_size;
    stroke.dynamics_sensitivity = settings.dynamics_sensitivity.min(100);
    stroke.dynamics_min_size = sanitized_dynamic_min_size(settings.dynamics_min_size);
    stroke.dirty_preview = true;
    brush_flush_pending(&mut stroke);
    if is_single_dab {
        // Smoothing is a boundary cleanup for a gesture. A one-sample gesture
        // is already the exact static nib and must keep its literal footprint.
        stroke.coverage
    } else {
        let mut finished = smooth_contours(&stroke.coverage, settings.smoothing, stroke.size);
        // A circular nib has no semantic corners, so restoring its two exact
        // endpoint discs protects the classic round cap from simplification.
        // Polygon nibs are different: unioning their literal endpoint imprint
        // back in after smoothing resurrects the very sharp corners smoothing
        // just removed. Their caps therefore stay part of the smoothed boundary.
        if settings.nib == BrushNib::Circle && !brush_size_dynamics_enabled(&stroke) {
            let sizes = stroke_sample_sizes(&stroke);
            if let Some((first, size)) = stroke.samples.first().zip(sizes.first()) {
                finished = finished.union(&sweep_nib(
                    settings.nib,
                    *size,
                    first.position,
                    first.position,
                ));
            }
            if let Some((last, size)) = stroke.samples.last().zip(sizes.last()) {
                finished = finished.union(&sweep_nib(
                    settings.nib,
                    *size,
                    last.position,
                    last.position,
                ));
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

fn sanitized_dynamic_min_size(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.01, 1.0)
    } else {
        BrushSettings::default().dynamics_min_size
    }
}

fn dynamic_sample_size(samples: &[BrushSample], index: usize, settings: BrushSettings) -> f32 {
    let min_ratio = sanitized_dynamic_min_size(settings.dynamics_min_size);
    let sensitivity = f32::from(settings.dynamics_sensitivity.min(100)) / 100.0;
    let pressure_gamma = 2.0_f32.powf((sensitivity - 0.5) * 2.0);
    let velocity_reference = settings.size.max(0.1) * (80.0 - sensitivity * 60.0);
    let sample = samples.get(index);
    let mut factor = 1.0_f32;
    if settings.pressure_size {
        let pressure = sample
            .and_then(|sample| sample.pressure)
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let shaped = pressure.powf(pressure_gamma);
        factor *= min_ratio + (1.0 - min_ratio) * shaped;
    }
    if settings.velocity_size && sample.is_some() {
        let speed = brush_sample_speed(samples, index);
        let normalized = (speed / velocity_reference.max(1.0)).max(0.0);
        let speed_factor = min_ratio + (1.0 - min_ratio) / (1.0 + normalized);
        factor *= speed_factor;
    }
    settings.size.max(0.1) * factor.clamp(min_ratio, 1.0)
}

fn dynamic_sample_sizes(samples: &[BrushSample], settings: BrushSettings) -> Vec<f32> {
    (0..samples.len())
        .map(|index| dynamic_sample_size(samples, index, settings))
        .collect()
}

fn brush_segment_speed(samples: &[BrushSample], a: usize, b: usize) -> f32 {
    let dt = (samples[b].time_seconds - samples[a].time_seconds)
        .abs()
        .max(1.0 / 1000.0) as f32;
    vec2_distance(samples[a].position, samples[b].position) / dt
}

fn brush_sample_speed(samples: &[BrushSample], index: usize) -> f32 {
    if samples.len() <= 1 {
        return 0.0;
    }
    if index == 0 {
        return brush_segment_speed(samples, 0, 1);
    }
    let current = brush_segment_speed(samples, index - 1, index);
    if index < 2 {
        return current;
    }
    // One tiny terminal move is common immediately before pen-up. Treat it as
    // event jitter instead of letting velocity dynamics jump back to a full-size
    // circular dab. A genuinely slow gesture still becomes thick after two
    // consecutive slow segments.
    current.max(brush_segment_speed(samples, index - 2, index - 1))
}

fn stroke_sample_sizes(stroke: &BrushStroke) -> Vec<f32> {
    (0..stroke.samples.len())
        .filter_map(|index| brush_prerender_dab_at(stroke, index).map(|(_, size)| size))
        .collect()
}

fn sweep_variable_trajectory_nib(
    nib: BrushNib,
    trajectory: &[Vec2],
    sizes: &[f32],
) -> MultiPolygon<f64> {
    let Some(first) = trajectory.first().copied() else {
        return MultiPolygon(Vec::new());
    };
    let first_size = sizes.first().copied().unwrap_or(0.1).max(0.1);
    if trajectory.len() == 1 {
        return sweep_nib(nib, first_size, first, first);
    }

    // A left-fold union repeatedly booleaned a growing contour against every
    // new segment, so long pressure/velocity strokes became progressively more
    // expensive at pointer-up. Build independent bridge surfaces first and let
    // geo merge the whole set with its balanced unary-union path instead.
    let mut surfaces: Vec<Polygon<f64>> = Vec::with_capacity(trajectory.len() - 1);
    for index in 1..trajectory.len() {
        let start = trajectory[index - 1];
        let end = trajectory[index];
        let start_size = sizes.get(index - 1).copied().unwrap_or(first_size).max(0.1);
        let end_size = sizes.get(index).copied().unwrap_or(start_size).max(0.1);
        if (start_size - end_size).abs() <= 1.0e-4 {
            surfaces.extend(sweep_nib(nib, start_size, start, end).0);
        } else {
            surfaces.push(variable_nib_segment(nib, start, start_size, end, end_size));
        }
    }
    geo::unary_union(surfaces.iter())
}

fn variable_nib_segment(
    nib: BrushNib,
    start: Vec2,
    start_size: f32,
    end: Vec2,
    end_size: f32,
) -> Polygon<f64> {
    let points = nib_outline(nib, start_size, start)
        .into_iter()
        .chain(nib_outline(nib, end_size, end))
        .map(|point| Point::new(f64::from(point.x), f64::from(point.y)))
        .collect::<Vec<_>>();
    MultiPoint(points).convex_hull()
}

const ROUND_NIB_SEGMENTS: usize = 64;

fn circular_sweep_buffer(
    line: &LineString<f64>,
    radius: f64,
    path_length: f32,
    size: f32,
) -> MultiPolygon<f64> {
    // geo's default round buffer is intentionally fairly coarse. That is fine
    // for long strokes, where the cap occupies a tiny part of the silhouette,
    // but a dot-like gesture is almost entirely cap and visibly turns polygonal.
    // Match the 64-segment static circular nib only for strokes shorter than one
    // brush diameter, keeping the long-stroke fast path unchanged.
    if path_length <= size.max(0.1) {
        let max_angle = std::f64::consts::TAU / ROUND_NIB_SEGMENTS as f64;
        line.buffer_with_style(
            BufferStyle::new(radius)
                .line_cap(LineCap::Round(max_angle))
                .line_join(LineJoin::Round(max_angle)),
        )
    } else {
        line.buffer(radius)
    }
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
        let mut path_length = 0.0_f32;
        let mut coords: Vec<Coord<f64>> = Vec::with_capacity(trajectory.len());
        for point in trajectory {
            if let Some(previous) = coords.last() {
                path_length +=
                    vec2_distance(Vec2::new(previous.x as f32, previous.y as f32), *point);
            }
            let coord = Coord {
                x: f64::from(point.x),
                y: f64::from(point.y),
            };
            if coords.last().copied() != Some(coord) {
                coords.push(coord);
            }
        }
        if coords.len() == 1 {
            return sweep_nib(nib, size, first, first);
        }
        if (coords[0].x, coords[0].y) > (coords[coords.len() - 1].x, coords[coords.len() - 1].y) {
            coords.reverse();
        }
        let line = LineString::new(coords);
        return circular_sweep_buffer(&line, radius, path_length, size);
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
        let line = LineString::new(vec![
            Coord {
                x: f64::from(start.x),
                y: f64::from(start.y),
            },
            Coord {
                x: f64::from(end.x),
                y: f64::from(end.y),
            },
        ]);
        return circular_sweep_buffer(&line, radius, vec2_distance(start, end), size);
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
    let mut coords = Vec::with_capacity(ROUND_NIB_SEGMENTS + 1);
    for index in 0..ROUND_NIB_SEGMENTS {
        let angle = std::f64::consts::TAU * index as f64 / ROUND_NIB_SEGMENTS as f64;
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
        // The preserving simplifier checks intermediate edits one by one.
        // Most brush rings simplify safely in one heap pass; validate that
        // completed result, with the original preserving path as fallback.
        let mut fast = softened.simplify_vw(epsilon);
        restore_long_simplified_spans(coverage, &softened, &mut fast, radius);
        if simplification_is_safe(coverage, &fast, strength, radius)
            && simplified_ring_relationships_are_valid(&fast)
        {
            return fast;
        }
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

fn simplified_ring_relationships_are_valid(candidate: &MultiPolygon<f64>) -> bool {
    let ring_count: usize = candidate
        .0
        .iter()
        .map(|polygon| 1 + polygon.interiors().len())
        .sum();
    if ring_count < 2 {
        return true;
    }
    let mut segments = Vec::new();
    let mut ring_index = 0;
    for polygon in &candidate.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let segment_count = ring.0.len().saturating_sub(1);
            for (segment_index, pair) in ring.0.windows(2).enumerate() {
                if pair[0] != pair[1] {
                    segments.push(FillBoundarySegment {
                        ring_index,
                        segment_index,
                        segment_count,
                        line: Line::new(pair[0], pair[1]),
                    });
                }
            }
            ring_index += 1;
        }
    }
    if fill_boundaries_intersect(segments, false) {
        return false;
    }
    for (index, polygon) in candidate.0.iter().enumerate() {
        let exterior = Polygon::new(polygon.exterior().clone(), Vec::new());
        for (hole_index, hole) in polygon.interiors().iter().enumerate() {
            let point = Point(hole.0[0]);
            if !exterior.contains(&point) {
                return false;
            }
            for other in &polygon.interiors()[..hole_index] {
                if Polygon::new(other.clone(), Vec::new()).contains(&point)
                    || Polygon::new(hole.clone(), Vec::new()).contains(&Point(other.0[0]))
                {
                    return false;
                }
            }
        }
        let point = Point(polygon.exterior().0[0]);
        if candidate
            .0
            .iter()
            .enumerate()
            .any(|(other, polygon)| other != index && polygon.contains(&point))
        {
            return false;
        }
    }
    true
}

/// Put source vertices back along overlong simplification chords. This keeps
/// the existing bend guard without retrying expensive topology-preserving
/// simplification four times only to discard each complete result.
fn restore_long_simplified_spans(
    original: &MultiPolygon<f64>,
    softened: &MultiPolygon<f64>,
    candidate: &mut MultiPolygon<f64>,
    radius: f64,
) {
    let restore = |original: &LineString<f64>,
                   source: &LineString<f64>,
                   target: &mut LineString<f64>| {
        let limit = ring_max_edge_length(original) * 2.5 + radius * 0.35;
        if ring_max_edge_length(target) <= limit {
            return;
        }
        let mut retained = Vec::with_capacity(target.0.len());
        let mut search = 0;
        for point in &target.0 {
            let Some(offset) = source.0[search..].iter().position(|source| source == point) else {
                return;
            };
            search += offset;
            retained.push(search);
            search += 1;
        }
        let mut restored = Vec::with_capacity(source.0.len());
        restored.push(target.0[0]);
        for pair in retained.windows(2) {
            let mut start = pair[0];
            if (source.0[start].x - source.0[pair[1]].x)
                .hypot(source.0[start].y - source.0[pair[1]].y)
                > limit
            {
                for index in pair[0] + 1..=pair[1] {
                    if (source.0[start].x - source.0[index].x)
                        .hypot(source.0[start].y - source.0[index].y)
                        > limit
                    {
                        if index - 1 <= start {
                            return;
                        }
                        restored.push(source.0[index - 1]);
                        start = index - 1;
                    }
                }
            }
            restored.push(source.0[pair[1]]);
        }
        target.0 = restored;
    };
    for ((before, source), target) in original.0.iter().zip(&softened.0).zip(&mut candidate.0) {
        target.exterior_mut(|ring| restore(before.exterior(), source.exterior(), ring));
        target.interiors_mut(|rings| {
            for ((before, source), target) in
                before.interiors().iter().zip(source.interiors()).zip(rings)
            {
                restore(before, source, target);
            }
        });
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

/// Remove numerical crumbs, while retaining the smallest authored nibs.
/// Stage area is not screen area: the former 0.01 threshold discarded real
/// 0.1-unit brush marks that are several pixels wide when zoomed in.
pub(crate) const SPECK_AREA: f64 = 0.0001;

/// Whether `ring` encloses too little to be seen.
pub(crate) fn is_speck(ring: &LineString<f64>) -> bool {
    signed_ring_area(ring).abs() <= SPECK_AREA
}

pub fn coverage_to_paths(coverage: &MultiPolygon<f64>) -> Vec<VPath> {
    let mut paths = Vec::new();
    for polygon in &coverage.0 {
        // A speck's holes are smaller still.
        if is_speck(polygon.exterior()) {
            continue;
        }
        if let Some(path) = ring_to_path(polygon.exterior()) {
            paths.push(path);
        }
        for hole in polygon.interiors().iter().filter(|hole| !is_speck(hole)) {
            if let Some(path) = ring_to_path(hole) {
                paths.push(path);
            }
        }
    }
    paths
}

/// `coverage` as raw fill paths that keep its separate pieces apart.
/// Smoothed curves can bulge across a hairline gap, such as a knife's cut or
/// a thin eraser stroke, and fuse the pieces on either side back into one;
/// then the exact straight edges are kept instead.
pub(crate) fn coverage_to_separate_paths(coverage: &MultiPolygon<f64>) -> Vec<VPath> {
    let paths = coverage_to_paths(coverage);
    let pieces = coverage
        .0
        .iter()
        .filter(|polygon| !is_speck(polygon.exterior()))
        .count();
    if pieces < 2 {
        return paths;
    }
    let fitted = VectorAsset {
        asset_id: 0,
        paths,
        fill: Some(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }),
        stroke: None,
    };
    if vector_fill_topology(&fitted).surface.0.len() >= pieces {
        fitted.paths
    } else {
        coverage_to_linear_paths(coverage)
    }
}

pub(crate) fn coverage_to_linear_paths(coverage: &MultiPolygon<f64>) -> Vec<VPath> {
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

    let fit_tolerance = canonical_curve_fit_tolerance(&points);
    let baseline_points = collapse_subprecision_ring_points(&points, fit_tolerance);
    if baseline_points.len() < 3 {
        return None;
    }
    let hairpin_points = collapse_micro_hairpins(&baseline_points, fit_tolerance);
    let aggressive_points = collapse_micro_backsteps(&hairpin_points, fit_tolerance);
    let variants = [&aggressive_points, &hairpin_points, &baseline_points];
    for (index, fit_points) in variants.iter().copied().enumerate() {
        if variants[..index].contains(&fit_points) {
            continue;
        }
        if fit_points.len() < 3 {
            continue;
        }
        for tolerance_scale in [1.0_f32, 0.5, 0.25, 0.125] {
            let tolerance = fit_tolerance * tolerance_scale;
            if let Some(fitted) = fit_closed_ring_path(fit_points, tolerance) {
                if fitted.anchors.len() < fit_points.len()
                    && fitted_ring_path_is_safe(ring, &fitted, tolerance)
                {
                    return Some(fitted);
                }
            }
        }
    }
    let linear = linear_ring_path(&baseline_points);
    let curved = curved_ring_path(&baseline_points);
    if curved_path_is_safe(ring, &curved) {
        Some(curved)
    } else {
        Some(linear)
    }
}

#[derive(Debug, Clone, Copy)]
struct FittedCubic {
    p0: Vec2,
    p1: Vec2,
    p2: Vec2,
    p3: Vec2,
}

fn canonical_curve_fit_tolerance(points: &[Vec2]) -> f32 {
    let Some((min, max)) = points_bounds(points) else {
        return 0.0025;
    };
    let diagonal = (max.x - min.x).hypot(max.y - min.y);
    (diagonal * 1.0e-5).clamp(0.0025, 0.05)
}

fn collapse_subprecision_ring_points(points: &[Vec2], tolerance: f32) -> Vec<Vec2> {
    if points.len() < 4 {
        return points.to_vec();
    }
    // Representation canonicalization only: discard consecutive samples that
    // are below useful f32/sub-pixel precision. No artistic smoothing here.
    let minimum_spacing = (tolerance * 0.1).max(1.0e-5);
    let mut compact = Vec::with_capacity(points.len());
    for &point in points {
        if compact
            .last()
            .is_none_or(|&previous| vec2_distance(previous, point) > minimum_spacing)
        {
            compact.push(point);
        }
    }
    while compact.len() > 3
        && vec2_distance(compact[0], *compact.last().unwrap()) <= minimum_spacing
    {
        compact.pop();
    }
    compact
}

fn collapse_micro_hairpins(points: &[Vec2], tolerance: f32) -> Vec<Vec2> {
    if points.len() < 4 {
        return points.to_vec();
    }
    let minimum_spacing = (tolerance * 0.1).max(1.0e-5);
    let return_radius = (tolerance * 0.15).max(minimum_spacing);
    let excursion_limit = tolerance * 0.5;
    let mut compact = points.to_vec();
    for _ in 0..4 {
        if compact.len() <= 3 {
            break;
        }
        let count = compact.len();
        let mut remove = vec![false; count];
        for index in 0..count {
            let previous = compact[(index + count - 1) % count];
            let current = compact[index];
            let next = compact[(index + 1) % count];
            if vec2_distance(previous, next) <= return_radius
                && vec2_distance(previous, current).max(vec2_distance(current, next))
                    <= excursion_limit
            {
                remove[index] = true;
            }
        }
        let remove_count = remove.iter().filter(|&&value| value).count();
        if remove_count == 0 || count - remove_count < 3 {
            break;
        }
        compact = compact
            .into_iter()
            .enumerate()
            .filter_map(|(index, point)| (!remove[index]).then_some(point))
            .collect();
        compact = collapse_subprecision_ring_points(&compact, tolerance);
    }
    compact
}

fn collapse_micro_backsteps(points: &[Vec2], tolerance: f32) -> Vec<Vec2> {
    if points.len() < 4 {
        return points.to_vec();
    }
    let mut compact = points.to_vec();
    for _ in 0..4 {
        let count = compact.len();
        if count <= 3 {
            break;
        }
        let mut remove = vec![false; count];
        for index in 0..count {
            let previous = compact[(index + count - 1) % count];
            let current = compact[index];
            let next_index = (index + 1) % count;
            let next = compact[next_index];
            let after = compact[(index + 2) % count];
            let incoming = vec2_sub(current, previous);
            let backstep = vec2_sub(next, current);
            let outgoing = vec2_sub(after, next);
            let incoming_len = vec2_length(incoming);
            let backstep_len = vec2_length(backstep);
            let outgoing_len = vec2_length(outgoing);
            if backstep_len <= tolerance * 0.75
                && incoming_len > backstep_len * 4.0
                && outgoing_len > backstep_len * 4.0
            {
                let incoming_unit = vec2_scale(incoming, incoming_len.recip());
                let backstep_unit = vec2_scale(backstep, backstep_len.max(1.0e-12).recip());
                let outgoing_unit = vec2_scale(outgoing, outgoing_len.recip());
                if vec2_dot(incoming_unit, outgoing_unit) > 0.8
                    && vec2_dot(incoming_unit, backstep_unit) < -0.5
                    && vec2_dot(outgoing_unit, backstep_unit) < -0.5
                {
                    remove[next_index] = true;
                }
            }
        }
        let remove_count = remove.iter().filter(|&&value| value).count();
        if remove_count == 0 || count - remove_count < 3 {
            break;
        }
        compact = compact
            .into_iter()
            .enumerate()
            .filter_map(|(index, point)| (!remove[index]).then_some(point))
            .collect();
        compact = collapse_subprecision_ring_points(&compact, tolerance);
    }
    compact
}

fn normalized_or(value: Vec2, fallback: Vec2) -> Vec2 {
    let length = vec2_length(value);
    if length > 1.0e-8 {
        vec2_scale(value, length.recip())
    } else {
        let fallback_length = vec2_length(fallback);
        if fallback_length > 1.0e-8 {
            vec2_scale(fallback, fallback_length.recip())
        } else {
            Vec2::new(1.0, 0.0)
        }
    }
}

fn bezier_basis(t: f32) -> [f32; 4] {
    let mt = 1.0 - t;
    [mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t]
}

fn cubic_derivative(curve: FittedCubic, t: f32) -> Vec2 {
    let mt = 1.0 - t;
    vec2_add(
        vec2_add(
            vec2_scale(vec2_sub(curve.p1, curve.p0), 3.0 * mt * mt),
            vec2_scale(vec2_sub(curve.p2, curve.p1), 6.0 * mt * t),
        ),
        vec2_scale(vec2_sub(curve.p3, curve.p2), 3.0 * t * t),
    )
}

fn cubic_second_derivative(curve: FittedCubic, t: f32) -> Vec2 {
    let a = vec2_add(vec2_sub(curve.p2, vec2_scale(curve.p1, 2.0)), curve.p0);
    let b = vec2_add(vec2_sub(curve.p3, vec2_scale(curve.p2, 2.0)), curve.p1);
    vec2_add(vec2_scale(a, 6.0 * (1.0 - t)), vec2_scale(b, 6.0 * t))
}

fn chord_parameters(points: &[Vec2]) -> Vec<f32> {
    let mut values = Vec::with_capacity(points.len());
    values.push(0.0);
    for pair in points.windows(2) {
        let next = values.last().copied().unwrap_or(0.0) + vec2_distance(pair[0], pair[1]);
        values.push(next);
    }
    let total = values.last().copied().unwrap_or(0.0);
    if total <= 1.0e-8 {
        let denom = (points.len().saturating_sub(1)).max(1) as f32;
        for (index, value) in values.iter_mut().enumerate() {
            *value = index as f32 / denom;
        }
    } else {
        for value in &mut values {
            *value /= total;
        }
    }
    values
}

fn generate_fitted_cubic(
    points: &[Vec2],
    parameters: &[f32],
    start_tangent: Vec2,
    end_tangent: Vec2,
) -> FittedCubic {
    let p0 = points[0];
    let p3 = *points.last().unwrap_or(&p0);
    let mut c00 = 0.0_f32;
    let mut c01 = 0.0_f32;
    let mut c11 = 0.0_f32;
    let mut x0 = 0.0_f32;
    let mut x1 = 0.0_f32;

    for (&point, &t) in points.iter().zip(parameters) {
        let [b0, b1, b2, b3] = bezier_basis(t);
        let a1 = vec2_scale(start_tangent, b1);
        let a2 = vec2_scale(end_tangent, b2);
        let base = vec2_add(vec2_scale(p0, b0 + b1), vec2_scale(p3, b2 + b3));
        let residual = vec2_sub(point, base);
        c00 += vec2_dot(a1, a1);
        c01 += vec2_dot(a1, a2);
        c11 += vec2_dot(a2, a2);
        x0 += vec2_dot(a1, residual);
        x1 += vec2_dot(a2, residual);
    }

    let determinant = c00 * c11 - c01 * c01;
    let chord = vec2_distance(p0, p3);
    let fallback = chord / 3.0;
    let (mut alpha_start, mut alpha_end) = if determinant.abs() > 1.0e-10 {
        (
            (x0 * c11 - x1 * c01) / determinant,
            (c00 * x1 - c01 * x0) / determinant,
        )
    } else {
        (fallback, fallback)
    };
    let minimum_handle = chord * 1.0e-4;
    if !alpha_start.is_finite()
        || !alpha_end.is_finite()
        || alpha_start < minimum_handle
        || alpha_end < minimum_handle
    {
        alpha_start = fallback;
        alpha_end = fallback;
    }
    // Least-squares fitting can technically minimize sampled error with a
    // control handle far beyond the segment chord. Between samples that can
    // create a tiny Bezier loop even though the source polyline is simple. A
    // cubic never needs a handle longer than the endpoint chord here: tighter
    // bends are recursively split into smaller fitted runs instead.
    let max_handle = chord.max(minimum_handle);
    alpha_start = alpha_start.min(max_handle);
    alpha_end = alpha_end.min(max_handle);

    FittedCubic {
        p0,
        p1: vec2_add(p0, vec2_scale(start_tangent, alpha_start)),
        p2: vec2_add(p3, vec2_scale(end_tangent, alpha_end)),
        p3,
    }
}

fn fitted_cubic_has_sample_loop(curve: FittedCubic) -> bool {
    const SAMPLES: usize = 16;
    let mut points = Vec::with_capacity(SAMPLES + 1);
    for index in 0..=SAMPLES {
        let t = index as f32 / SAMPLES as f32;
        points.push(cubic_point(curve.p0, curve.p1, curve.p2, curve.p3, t));
    }
    for i in 0..SAMPLES {
        let a = points[i];
        let b = points[i + 1];
        for j in (i + 2)..SAMPLES {
            let c = points[j];
            let d = points[j + 1];
            if segments_intersect(a, b, c, d) {
                return true;
            }
        }
    }
    false
}

fn fitted_cubic_max_error(points: &[Vec2], parameters: &[f32], curve: FittedCubic) -> (f32, usize) {
    let mut max_error = 0.0_f32;
    let mut split = points.len() / 2;
    for index in 1..points.len().saturating_sub(1) {
        let point = cubic_point(curve.p0, curve.p1, curve.p2, curve.p3, parameters[index]);
        let error = vec2_distance(point, points[index]);
        if error > max_error {
            max_error = error;
            split = index;
        }
    }
    (max_error, split)
}

fn reparameterize_cubic(points: &[Vec2], parameters: &mut [f32], curve: FittedCubic) {
    for (point, parameter) in points
        .iter()
        .zip(parameters.iter_mut())
        .skip(1)
        .take(points.len().saturating_sub(2))
    {
        let q = cubic_point(curve.p0, curve.p1, curve.p2, curve.p3, *parameter);
        let q1 = cubic_derivative(curve, *parameter);
        let q2 = cubic_second_derivative(curve, *parameter);
        let delta = vec2_sub(q, *point);
        let denominator = vec2_dot(q1, q1) + vec2_dot(delta, q2);
        if denominator.abs() <= 1.0e-10 {
            continue;
        }
        let next = *parameter - vec2_dot(delta, q1) / denominator;
        if next.is_finite() {
            *parameter = next.clamp(0.0, 1.0);
        }
    }
    for index in 1..parameters.len() {
        parameters[index] = parameters[index].max(parameters[index - 1]);
    }
}

fn fit_cubic_run(
    points: &[Vec2],
    start_tangent: Vec2,
    end_tangent: Vec2,
    tolerance: f32,
    depth: u8,
    output: &mut Vec<FittedCubic>,
) {
    if points.len() < 2 {
        return;
    }
    if points.len() == 2 {
        let chord = vec2_sub(points[1], points[0]);
        output.push(FittedCubic {
            p0: points[0],
            p1: vec2_add(points[0], vec2_scale(chord, 1.0 / 3.0)),
            p2: vec2_add(points[0], vec2_scale(chord, 2.0 / 3.0)),
            p3: points[1],
        });
        return;
    }

    let mut parameters = chord_parameters(points);
    let mut curve = generate_fitted_cubic(points, &parameters, start_tangent, end_tangent);
    let mut error = fitted_cubic_max_error(points, &parameters, curve);
    if error.0 <= tolerance && !fitted_cubic_has_sample_loop(curve) {
        output.push(curve);
        return;
    }

    if error.0 <= tolerance * 4.0 {
        for _ in 0..4 {
            reparameterize_cubic(points, &mut parameters, curve);
            curve = generate_fitted_cubic(points, &parameters, start_tangent, end_tangent);
            error = fitted_cubic_max_error(points, &parameters, curve);
            if error.0 <= tolerance && !fitted_cubic_has_sample_loop(curve) {
                output.push(curve);
                return;
            }
        }
    }

    let split = if fitted_cubic_has_sample_loop(curve) {
        points.len() / 2
    } else {
        error.1
    }
    .clamp(1, points.len() - 2);
    if depth >= 20 {
        let left_tangent = normalized_or(vec2_sub(points[split], points[split - 1]), start_tangent);
        let right_tangent = normalized_or(vec2_sub(points[split], points[split + 1]), end_tangent);
        fit_cubic_run(
            &points[..=split],
            start_tangent,
            left_tangent,
            tolerance,
            depth + 1,
            output,
        );
        fit_cubic_run(
            &points[split..],
            vec2_scale(right_tangent, -1.0),
            end_tangent,
            tolerance,
            depth + 1,
            output,
        );
        return;
    }

    let center_tangent = normalized_or(
        vec2_sub(points[split - 1], points[split + 1]),
        vec2_scale(start_tangent, -1.0),
    );
    fit_cubic_run(
        &points[..=split],
        start_tangent,
        center_tangent,
        tolerance,
        depth + 1,
        output,
    );
    fit_cubic_run(
        &points[split..],
        vec2_scale(center_tangent, -1.0),
        end_tangent,
        tolerance,
        depth + 1,
        output,
    );
}

fn is_authored_corner(points: &[Vec2], index: usize) -> bool {
    let count = points.len();
    let previous = points[(index + count - 1) % count];
    let current = points[index];
    let next = points[(index + 1) % count];
    let incoming = vec2_sub(current, previous);
    let outgoing = vec2_sub(next, current);
    let incoming_len = vec2_length(incoming);
    let outgoing_len = vec2_length(outgoing);
    if incoming_len <= 1.0e-6 || outgoing_len <= 1.0e-6 {
        return false;
    }
    let alignment = vec2_dot(
        vec2_scale(incoming, incoming_len.recip()),
        vec2_scale(outgoing, outgoing_len.recip()),
    );
    alignment < 0.5
}

fn fitted_segments_to_closed_path(segments: &[FittedCubic]) -> Option<VPath> {
    if segments.len() < 3 {
        return None;
    }
    let mut anchors: Vec<Anchor> = segments
        .iter()
        .map(|segment| Anchor {
            point: segment.p0,
            in_handle: None,
            out_handle: Some(segment.p1),
        })
        .collect();
    for (index, segment) in segments.iter().enumerate() {
        let next = (index + 1) % anchors.len();
        anchors[next].in_handle = Some(segment.p2);
    }
    Some(VPath {
        anchors,
        closed: true,
    })
}

fn fit_closed_ring_path(points: &[Vec2], tolerance: f32) -> Option<VPath> {
    if points.len() < 4 {
        return None;
    }
    let count = points.len();
    let mut breaks = vec![0_usize, count / 4, count / 2, count * 3 / 4];
    breaks.extend((0..count).filter(|&index| is_authored_corner(points, index)));
    breaks.sort_unstable();
    breaks.dedup();
    if breaks.len() < 3 {
        return None;
    }

    let mut segments = Vec::new();
    for break_index in 0..breaks.len() {
        let start = breaks[break_index];
        let end = if break_index + 1 < breaks.len() {
            breaks[break_index + 1]
        } else {
            breaks[0] + count
        };
        let mut run = Vec::with_capacity(end - start + 1);
        for index in start..=end {
            run.push(points[index % count]);
        }
        if run.len() < 2 {
            continue;
        }
        let start_tangent = normalized_or(vec2_sub(run[1], run[0]), Vec2::new(1.0, 0.0));
        let end_tangent = normalized_or(
            vec2_sub(run[run.len() - 2], run[run.len() - 1]),
            vec2_scale(start_tangent, -1.0),
        );
        fit_cubic_run(
            &run,
            start_tangent,
            end_tangent,
            tolerance,
            0,
            &mut segments,
        );
    }
    fitted_segments_to_closed_path(&segments)
}

fn fitted_ring_path_is_safe(original: &LineString<f64>, candidate: &VPath, tolerance: f32) -> bool {
    if candidate.anchors.len() < 3
        || candidate
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
    let bbox_tolerance = tolerance * 2.0 + 1.0e-4;
    if candidate_min.x < original_min.x - bbox_tolerance
        || candidate_min.y < original_min.y - bbox_tolerance
        || candidate_max.x > original_max.x + bbox_tolerance
        || candidate_max.y > original_max.y + bbox_tolerance
        || candidate_min.x > original_min.x + bbox_tolerance
        || candidate_min.y > original_min.y + bbox_tolerance
        || candidate_max.x < original_max.x - bbox_tolerance
        || candidate_max.y < original_max.y - bbox_tolerance
    {
        return false;
    }
    let perimeter = original
        .0
        .windows(2)
        .map(|pair| (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y))
        .sum::<f64>();
    let allowed_area_delta = perimeter * f64::from(tolerance) * 2.5 + 1.0e-6;
    (candidate_area.abs() - original_area.abs()).abs() <= allowed_area_delta
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
    const BROADPHASE_EPSILON: f32 = 1.0e-5;

    #[derive(Clone, Copy)]
    struct SegmentBounds {
        index: usize,
        a: Vec2,
        b: Vec2,
        min_primary: f32,
        max_primary: f32,
        min_secondary: f32,
        max_secondary: f32,
    }

    let count = points.len();
    if count < 4 {
        return false;
    }
    let Some((min, max)) = points_bounds(points) else {
        return false;
    };
    // Sweep along the longer overall axis. The previous uniform grid used one
    // global cell size and became effectively quadratic for a huge dense ring:
    // hundreds of thousands of tiny Bezier samples piled into the same cells.
    // This broad phase never changes the exact intersection predicate below;
    // it only discards segment pairs whose expanded AABBs cannot intersect.
    let sweep_x = max.x - min.x >= max.y - min.y;
    let mut segments = Vec::with_capacity(count);
    for index in 0..count {
        let a = points[index];
        let b = points[(index + 1) % count];
        let (a_primary, a_secondary, b_primary, b_secondary) = if sweep_x {
            (a.x, a.y, b.x, b.y)
        } else {
            (a.y, a.x, b.y, b.x)
        };
        segments.push(SegmentBounds {
            index,
            a,
            b,
            min_primary: a_primary.min(b_primary),
            max_primary: a_primary.max(b_primary),
            min_secondary: a_secondary.min(b_secondary),
            max_secondary: a_secondary.max(b_secondary),
        });
    }
    segments.sort_unstable_by(|left, right| {
        left.min_primary
            .total_cmp(&right.min_primary)
            .then_with(|| left.max_primary.total_cmp(&right.max_primary))
            .then_with(|| left.index.cmp(&right.index))
    });

    let mut active: Vec<SegmentBounds> = Vec::new();
    for current in segments {
        active
            .retain(|candidate| candidate.max_primary + BROADPHASE_EPSILON >= current.min_primary);
        for candidate in &active {
            let distance = current.index.abs_diff(candidate.index);
            let adjacent = distance <= 1 || distance + 1 == count;
            if adjacent
                || candidate.max_secondary + BROADPHASE_EPSILON < current.min_secondary
                || current.max_secondary + BROADPHASE_EPSILON < candidate.min_secondary
            {
                continue;
            }
            if segments_intersect(current.a, current.b, candidate.a, candidate.b) {
                return true;
            }
        }
        active.push(current);
    }
    false
}

fn segments_intersect(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    // A fixed epsilon on a cross product has area units. On short fitted
    // segments it could treat separated edges as collinear intersections and
    // reject every fitting/backoff attempt. f64 robust orientation keeps
    // actual crossings, touching endpoints and collinear overlaps exact.
    let coord = |point: Vec2| Coord {
        x: f64::from(point.x),
        y: f64::from(point.y),
    };
    geo::line_intersection::line_intersection(
        Line::new(coord(a), coord(b)),
        Line::new(coord(c), coord(d)),
    )
    .is_some()
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
    stroke: Option<q0s_format::v2::Stroke>,
    geometry: MultiPolygon<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RawFillStyle {
    color: Rgba,
    stroke: Option<q0s_format::v2::Stroke>,
    material: Option<VectorMaterial>,
    fragmented: bool,
}

fn raw_fill_style(project: &ProjectV2, candidate: &RawFillCandidate) -> RawFillStyle {
    let appearance = project.asset_appearances.get(&candidate.asset_id);
    RawFillStyle {
        color: candidate.color,
        stroke: candidate.stroke,
        material: appearance
            .map(|appearance| appearance.material)
            .filter(|m| *m != VectorMaterial::Solid),
        fragmented: appearance.is_some_and(|appearance| {
            appearance.material != VectorMaterial::Solid && !appearance.clip_mask.is_empty()
        }),
    }
}

fn raw_regions_intersect(left: &MultiPolygon<f64>, right: &MultiPolygon<f64>) -> bool {
    let (Some(left_bounds), Some(right_bounds)) = (left.bounding_rect(), right.bounding_rect())
    else {
        return false;
    };
    if left_bounds.max().x < right_bounds.min().x
        || right_bounds.max().x < left_bounds.min().x
        || left_bounds.max().y < right_bounds.min().y
        || right_bounds.max().y < left_bounds.min().y
    {
        return false;
    }
    left.intersects(right)
}

pub fn commit_brush_region(
    app: &mut EditorApp,
    region: MultiPolygon<f64>,
    settings: BrushSettings,
) {
    commit_brush_region_with_material(app, region, settings, None);
}

pub fn commit_brush_region_with_material(
    app: &mut EditorApp,
    region: MultiPolygon<f64>,
    settings: BrushSettings,
    requested_material: Option<VectorMaterial>,
) {
    #[cfg(not(feature = "appearance-mask-eraser"))]
    let requested_material = {
        let _ = requested_material;
        None
    };
    if region.0.is_empty() {
        return;
    }
    // Publish one authoritative new contour, then use that same contour for
    // cutting and merging. Independently fitting either side creates cracks.
    let fresh_paths = coverage_to_paths(&region);
    let (q0rg_id, layer_id, frame) = (
        app.session.current_q0rg_id,
        app.session.current_layer_id,
        app.session.current_frame,
    );
    // Shapes drawn before fills and outlines were kept apart are split
    // first, so the new paint can join and cover their fills too.
    let snapshot = if mixed_raw_drawings(&app.state.project, q0rg_id, layer_id, frame).is_empty() {
        true
    } else {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
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
        split_mixed_raw_drawings(&mut app.state.project, q0rg_id, layer_id, frame);
        app.textures.invalidate();
        false
    };
    if paint_raw_fill(
        app,
        fresh_paths,
        settings.color,
        requested_material,
        snapshot,
    ) {
        app.session.selection = crate::state::Selection::None;
    }
}

/// Lay new fill paint, outlined by `fresh_paths`, onto the current layer's
/// drawing the way merge drawing does: it joins the paint of its own colour
/// that it touches into one fill and covers other colours under it. Returns
/// whether anything was painted. `snapshot` takes the undo step itself; a
/// caller that already took one passes false.
pub(crate) fn paint_raw_fill(
    app: &mut EditorApp,
    fresh_paths: Vec<VPath>,
    color: Rgba,
    requested_material: Option<VectorMaterial>,
    snapshot: bool,
) -> bool {
    #[cfg(not(feature = "appearance-mask-eraser"))]
    let requested_material = {
        let _ = requested_material;
        None
    };
    let settings = BrushSettings {
        color,
        ..BrushSettings::default()
    };
    if fresh_paths.is_empty() {
        return false;
    }
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    let candidates =
        collect_raw_fill_candidates(&app.state.project, q0rg_id, layer_id, frame, false);
    let requested_style = RawFillStyle {
        color: settings.color,
        stroke: None,
        material: requested_material,
        fragmented: false,
    };

    // A raw drawing is one planar paint surface per visual style. Plain vector
    // paint and glowing paint of the same colour must stay distinct; otherwise
    // toggling Glow would silently restyle older artwork.
    let fresh_geometry = vector_fill_geometry(&VectorAsset {
        asset_id: 0,
        paths: fresh_paths.clone(),
        fill: Some(settings.color),
        stroke: None,
    });
    let mut painted = fresh_geometry.clone();
    let mut same_style_assets = BTreeSet::new();
    // Merge drawing is local topology, not a reason to rewrite every same-colour
    // fill in the frame. Keep disconnected raw components as independent assets;
    // only the connected component reached by this fresh stroke is absorbed.
    //
    // Do not progressively union a growing Advanced Brush contour while looking
    // for that component. A dense contour makes each subsequent boolean pass more
    // expensive than the previous one and was the source of pointer-up freezes.
    // Connectivity does not require a union: every point in A union B belongs to
    // A or B. Walk touching source surfaces first, then run one balanced union.
    let mut connected_indices = Vec::new();
    let mut frontier = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if raw_fill_style(&app.state.project, candidate) == requested_style
            && !same_style_assets.contains(&candidate.asset_id)
            && raw_regions_intersect(&candidate.geometry, &painted)
        {
            same_style_assets.insert(candidate.asset_id);
            connected_indices.push(index);
            frontier.push(index);
        }
    }
    let mut frontier_cursor = 0;
    while frontier_cursor < frontier.len() {
        let source_index = frontier[frontier_cursor];
        frontier_cursor += 1;
        for (index, candidate) in candidates.iter().enumerate() {
            if raw_fill_style(&app.state.project, candidate) == requested_style
                && !same_style_assets.contains(&candidate.asset_id)
                && raw_regions_intersect(&candidate.geometry, &candidates[source_index].geometry)
            {
                same_style_assets.insert(candidate.asset_id);
                connected_indices.push(index);
                frontier.push(index);
            }
        }
    }
    if !connected_indices.is_empty() {
        let mut surfaces = painted.0;
        for index in connected_indices {
            surfaces.extend(candidates[index].geometry.0.iter().cloned());
        }
        painted = geo::unary_union(surfaces.iter());
    }
    let sources = crate::brush_geometry::BoundarySources::new(
        app.state
            .project
            .assets
            .iter()
            .filter_map(|asset| match asset {
                Asset::Vector(vector) if same_style_assets.contains(&vector.asset_id) => {
                    Some(vector.paths.iter())
                }
                _ => None,
            })
            .flatten()
            .chain(&fresh_paths),
    );
    let painted_paths = sources.paths(&painted);
    if painted_paths.is_empty() {
        return false;
    }
    #[cfg(feature = "appearance-mask-eraser")]
    let merged_appearance = requested_material.map(|material| {
        crate::appearance::merged_brush_appearance(
            &app.state.project,
            &same_style_assets,
            &fresh_geometry,
            material,
        )
    });

    let mut different_updates = Vec::new();
    let mut seen_different = BTreeSet::new();
    for candidate in &candidates {
        if raw_fill_style(&app.state.project, candidate) != requested_style
            && raw_regions_intersect(&candidate.geometry, &fresh_geometry)
            && seen_different.insert(candidate.asset_id)
        {
            // Only the new stroke overwrites other colours. Re-cutting them
            // against the complete same-colour union perturbs distant seams.
            different_updates.push((
                candidate.asset_id,
                candidate.geometry.difference(&fresh_geometry),
            ));
        }
    }

    if snapshot {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
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
        let Some(Asset::Vector(source)) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
        else {
            continue;
        };
        // Match this fill's own segments first. Neighbours can trace the same
        // curve backwards with different flattening samples; mixing their
        // provenance would leave a chord at an otherwise untouched join.
        let paths =
            crate::brush_geometry::BoundarySources::new(source.paths.iter().chain(&fresh_paths))
                .paths(geometry);
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

    // Replacing a connected fill must not lift its entire old contour above
    // neighbouring colours. The fresh footprint has already cut those fills.
    let untouched_tail = app.state.project.q0rgs.iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
        .and_then(|layer| {
            layer.placements.iter().rposition(|placement| placement.frame == frame
                && placement.transform == Transform2D::IDENTITY && !placement.tween.is_motion()
                && matches!(placement.target, Target::Asset(id) if same_style_assets.contains(&id)))
                .map(|index| layer.placements[index + 1..].iter().filter(|placement| {
                    !(placement.frame == frame && placement.transform == Transform2D::IDENTITY
                        && !placement.tween.is_motion()
                        && matches!(placement.target, Target::Asset(id) if remove_asset_ids.contains(&id)))
                }).count())
        }).unwrap_or(0);
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
    if untouched_tail > 0 {
        if let Some(layer) = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == layer_id)
            })
        {
            if let Some(placement) = layer.placements.pop() {
                let insertion = layer.placements.len().saturating_sub(untouched_tail);
                layer.placements.insert(insertion, placement);
            }
        }
    }
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
        let mut changed_asset_ids = remove_asset_ids;
        for (original_asset_id, _) in &different_updates {
            changed_asset_ids.insert(
                *writable_different
                    .get(original_asset_id)
                    .unwrap_or(original_asset_id),
            );
        }
        changed_asset_ids.insert(painted_asset_id);
        app.textures.invalidate_assets(changed_asset_ids);
    }

    app.session.status = format!("Brush merge drawing updated ({} regions)", painted.0.len());
    app.state.mark_dirty();
    true
}

/// Rebuild same-colour fill-only raw graphics after a transform gesture.
/// Drawing already performs this planar union on commit; moving/scaling raw
/// contours must end with the same topology or semi-transparent overlaps render
/// darker and remain separately selectable. Disconnected regions are left
/// alone, while touching or overlapping components are consolidated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawFillMerge {
    pub q0rg_id: u16,
    pub layer_id: u16,
    pub old_asset_ids: BTreeSet<u16>,
    pub new_asset_id: u16,
}

#[cfg(test)]
pub(crate) fn merge_touching_raw_fills_after_edit(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> bool {
    !merge_touching_raw_fills_after_edit_impl(project, q0rg_id, layer_id, frame, None).is_empty()
}

#[cfg(test)]
pub(crate) fn merge_touching_raw_fills_after_edit_focused(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    edited_asset_ids: &BTreeSet<u16>,
) -> bool {
    !merge_touching_raw_fills_after_edit_focused_with_remap(
        project,
        q0rg_id,
        layer_id,
        frame,
        edited_asset_ids,
    )
    .is_empty()
}

pub(crate) fn merge_touching_raw_fills_after_edit_focused_with_remap(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    edited_asset_ids: &BTreeSet<u16>,
) -> Vec<RawFillMerge> {
    if edited_asset_ids.is_empty() {
        return Vec::new();
    }
    merge_touching_raw_fills_after_edit_impl(
        project,
        q0rg_id,
        layer_id,
        frame,
        Some(edited_asset_ids),
    )
}

fn merge_touching_raw_fills_after_edit_impl(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    edited_asset_ids: Option<&BTreeSet<u16>>,
) -> Vec<RawFillMerge> {
    let candidates = collect_raw_fill_candidates(project, q0rg_id, layer_id, frame, true);
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
        // Clipped halo fragments retain their resolved material field. Solid
        // fragments are ordinary visible paint and can participate in a union.
        if style.fragmented {
            continue;
        }
        let mut visited = vec![false; items.len()];
        for seed in 0..items.len() {
            if visited[seed] {
                continue;
            }
            let mut connected = vec![seed];
            visited[seed] = true;
            let mut cursor = 0;
            while cursor < connected.len() {
                let current = connected[cursor];
                for other in 0..items.len() {
                    if !visited[other]
                        && raw_regions_intersect(&items[current].geometry, &items[other].geometry)
                    {
                        visited[other] = true;
                        connected.push(other);
                    }
                }
                cursor += 1;
            }
            // Older projects may contain a "merged" asset that merely stores
            // overlapping source contours. Canonicalize it on its next edit too.
            let legacy_overlap = connected.len() == 1
                && project
                    .assets
                    .iter()
                    .find_map(|asset| match asset {
                        Asset::Vector(vector) if vector.asset_id == items[seed].asset_id => Some(
                            vector.paths.len()
                                > items[seed]
                                    .geometry
                                    .0
                                    .iter()
                                    .map(|p| 1 + p.interiors().len())
                                    .sum::<usize>(),
                        ),
                        _ => None,
                    })
                    .unwrap_or(false);
            if (connected.len() < 2 && !legacy_overlap)
                || edited_asset_ids.is_some_and(|edited| {
                    !connected
                        .iter()
                        .any(|i| edited.contains(&items[*i].asset_id))
                })
            {
                continue;
            }
            let mut merged = MultiPolygon(Vec::new());
            let mut asset_ids = BTreeSet::new();
            for index in connected {
                asset_ids.insert(items[index].asset_id);
                merged = merged.union(&items[index].geometry);
            }
            // Persist only the union boundary. Concatenating source paths left
            // interior edges selectable/stroked and could cancel opposite windings.
            let paths = coverage_to_separate_paths(&merged);
            if !paths.is_empty() {
                updates.push((style, asset_ids, paths));
            }
        }
    }
    if updates.is_empty() {
        return Vec::new();
    }
    let mut merges = Vec::with_capacity(updates.len());
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
        // Keep the union at the frontmost source's depth. Appending it above
        // unrelated artwork would make a bucket fill cover its own outline.
        let untouched_tail = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| {
                layer.placements.iter().rposition(|placement| {
                    placement.frame == frame
                        && placement.transform == Transform2D::IDENTITY
                        && !placement.tween.is_motion()
                        && matches!(placement.target, Target::Asset(id) if asset_ids.contains(&id))
                }).map(|index| layer.placements.len() - index - 1)
            })
            .unwrap_or(0);
        remove_current_layer_asset_placements(project, q0rg_id, layer_id, frame, &asset_ids);
        // Allocate before deleting old assets so a new shape never inherits an
        // old asset id and its cached body/halo/selection geometry.
        let new_asset_id = append_raw_fill(project, q0rg_id, layer_id, frame, style.color, paths);
        if untouched_tail > 0 {
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
                if let Some(placement) = layer.placements.pop() {
                    let insertion = layer.placements.len().saturating_sub(untouched_tail);
                    layer.placements.insert(insertion, placement);
                }
            }
        }
        if let Some(Asset::Vector(vector)) =
            project.assets.iter_mut().find(|a| a.id() == new_asset_id)
        {
            vector.stroke = style.stroke;
        }
        #[cfg(feature = "appearance-mask-eraser")]
        if let Some(appearance) = merged_appearance {
            project.asset_appearances.insert(new_asset_id, appearance);
        }
        remove_unreferenced_assets(project, &asset_ids);
        merges.push(RawFillMerge {
            q0rg_id,
            layer_id,
            old_asset_ids: asset_ids,
            new_asset_id,
        });
    }
    merges
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
    let candidates =
        collect_raw_fill_candidates(&app.state.project, q0rg_id, layer_id, frame, false);

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
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
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
    let changed_asset_ids: BTreeSet<u16> = updates
        .iter()
        .map(|(original_asset_id, _)| {
            *writable_assets
                .get(original_asset_id)
                .unwrap_or(original_asset_id)
        })
        .collect();
    let mut remove_asset_ids = BTreeSet::new();
    for (original_asset_id, geometry) in &updates {
        let asset_id = *writable_assets
            .get(original_asset_id)
            .unwrap_or(original_asset_id);
        let paths = coverage_to_separate_paths(geometry);
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
    app.textures.invalidate_assets(changed_asset_ids);

    app.session.selection = crate::state::Selection::None;
    app.session.status = "Raw fill erased".to_string();
    app.state.mark_dirty();
    true
}

fn collect_raw_fill_candidates(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
    include_stroked_fills: bool,
) -> Vec<RawFillCandidate> {
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return Vec::new();
    };

    let assets_by_id: HashMap<u16, &Asset> = project
        .assets
        .iter()
        .map(|asset| (asset.id(), asset))
        .collect();
    crate::render::active_placements_at(layer, frame)
        .into_iter()
        .filter_map(|(placement_idx, _)| {
            let placement = layer.placements.get(placement_idx)?;
            if placement.transform != Transform2D::IDENTITY
                || placement.tween.is_motion()
                || !placement.fx.is_identity()
            {
                return None;
            }
            let Target::Asset(asset_id) = placement.target else {
                return None;
            };
            let Some(Asset::Vector(vector)) = assets_by_id.get(&asset_id).copied() else {
                return None;
            };
            if project
                .asset_names
                .get(&asset_id)
                .is_some_and(|name| crate::text_tool::decode_source(name).is_some())
            {
                return None;
            }
            let color = vector.fill?;
            if (!include_stroked_fills && vector.stroke.is_some())
                || vector.paths.iter().any(|p| !p.closed)
            {
                return None;
            }
            #[cfg(feature = "appearance-mask-eraser")]
            let geometry = if project
                .asset_appearances
                .get(&asset_id)
                .is_some_and(|a| a.material == VectorMaterial::Solid)
            {
                crate::appearance::visible_source_surface_for_vector(
                    vector,
                    project.asset_appearances.get(&asset_id),
                )
            } else {
                vector_fill_geometry(vector)
            };
            #[cfg(not(feature = "appearance-mask-eraser"))]
            let geometry = vector_fill_geometry(vector);
            (!geometry.0.is_empty()).then_some(RawFillCandidate {
                asset_id,
                color,
                stroke: vector.stroke,
                geometry,
            })
        })
        .collect()
}

/// Raw drawings on the frame stored the old way, fill and outline in one
/// asset: closed, plain, and on a layer without shape tweens (whose
/// keyframes are matched drawing by drawing, so they keep their shape).
pub(crate) fn mixed_raw_drawings(
    project: &ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> BTreeSet<u16> {
    let Some(layer) = project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
    else {
        return BTreeSet::new();
    };
    if layer
        .placements
        .iter()
        .any(|placement| placement.tween.is_shape())
    {
        return BTreeSet::new();
    }
    crate::render::active_placements_at(layer, frame)
        .into_iter()
        .filter_map(|(index, _)| {
            let placement = layer.placements.get(index)?;
            if placement.transform != Transform2D::IDENTITY
                || placement.tween.is_motion()
                || !placement.fx.is_identity()
            {
                return None;
            }
            let Target::Asset(asset_id) = placement.target else {
                return None;
            };
            let Some(Asset::Vector(vector)) = project.assets.iter().find(|a| a.id() == asset_id)
            else {
                return None;
            };
            let plain = project.asset_appearances.get(&asset_id).is_none_or(|look| {
                look.material == VectorMaterial::Solid
                    && look.erase_mask.is_empty()
                    && look.clip_mask.is_empty()
            });
            let text = project
                .asset_names
                .get(&asset_id)
                .is_some_and(|name| crate::text_tool::decode_source(name).is_some());
            (vector.fill.is_some()
                && vector.stroke.is_some()
                && !vector.paths.is_empty()
                && vector.paths.iter().all(|path| path.closed)
                && plain
                && !text)
                .then_some(asset_id)
        })
        .collect()
}

/// Merge drawing keeps fills and lines apart: a fill joins and covers
/// other paint, a line is cut by other lines. Split every drawing on the
/// frame that holds both (see [`mixed_raw_drawings`]) into its fill, which
/// keeps the asset, and its outline right above it. Returns whether any
/// was split. The caller has taken the undo step and made the keyframe.
pub(crate) fn split_mixed_raw_drawings(
    project: &mut ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    frame: u16,
) -> bool {
    let mixed = mixed_raw_drawings(project, q0rg_id, layer_id, frame);
    if mixed.is_empty() {
        return false;
    }
    let writable = prepare_writable_raw_assets(project, q0rg_id, layer_id, frame, &mixed);
    for original in mixed {
        let asset_id = *writable.get(&original).unwrap_or(&original);
        let Some(Asset::Vector(vector)) = project
            .assets
            .iter_mut()
            .find(|asset| asset.id() == asset_id)
        else {
            continue;
        };
        let Some(stroke) = vector.stroke.take() else {
            continue;
        };
        let paths = vector.paths.clone();
        let outline_id = next_asset_id(project);
        project.assets.push(Asset::Vector(VectorAsset {
            asset_id: outline_id,
            paths,
            fill: None,
            stroke: Some(stroke),
        }));
        let Some(layer) = project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == layer_id)
            })
        else {
            continue;
        };
        let Some(index) = layer.placements.iter().position(|placement| {
            placement.frame == frame && placement.target == Target::Asset(asset_id)
        }) else {
            continue;
        };
        let mut outline = layer.placements[index].clone();
        outline.target = Target::Asset(outline_id);
        layer.placements.insert(index + 1, outline);
    }
    true
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
                    && !placement.tween.is_motion()
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
                    && !placement.tween.is_motion())
            })
        })
    })
}

pub(crate) fn remove_current_layer_asset_placements(
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
                || placement.tween.is_motion()
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
        layer.push_placement(Placement {
            pivot: None,
            instance_id: 0,
            frame,
            target: Target::Asset(asset_id),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        });
    }
    asset_id
}
#[derive(Clone, Debug)]
struct FillBoundarySegment {
    ring_index: usize,
    segment_index: usize,
    segment_count: usize,
    line: Line<f64>,
}

/// A bounding-box sweep avoids scheduling an intersection event for every
/// adjacent endpoint in dense, already planar brush contours. Only the broad
/// phase is approximate work reduction; the f64 intersection predicate is exact.
fn fill_boundaries_intersect(segments: Vec<FillBoundarySegment>, check_self: bool) -> bool {
    struct BoundedSegment {
        source: FillBoundarySegment,
        bounds: [f64; 4],
    }
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut lengths = [0.0; 2];
    for segment in &segments {
        let line = segment.line;
        if ![line.start.x, line.start.y, line.end.x, line.end.y]
            .into_iter()
            .all(f64::is_finite)
        {
            return true;
        }
        bounds[0] = bounds[0].min(line.start.x.min(line.end.x));
        bounds[1] = bounds[1].min(line.start.y.min(line.end.y));
        bounds[2] = bounds[2].max(line.start.x.max(line.end.x));
        bounds[3] = bounds[3].max(line.start.y.max(line.end.y));
        lengths[0] += (line.end.x - line.start.x).abs();
        lengths[1] += (line.end.y - line.start.y).abs();
    }
    // Choose the axis with fewer expected active intervals, including narrow
    // comb-like drawings whose long edges all overlap along the other axis.
    let sweep_x = lengths[0] / (bounds[2] - bounds[0]).max(1.0e-10)
        <= lengths[1] / (bounds[3] - bounds[1]).max(1.0e-10);
    let mut bounded: Vec<_> = segments
        .into_iter()
        .map(|source| {
            let line = source.line;
            let (a, b, c, d) = if sweep_x {
                (line.start.x, line.end.x, line.start.y, line.end.y)
            } else {
                (line.start.y, line.end.y, line.start.x, line.end.x)
            };
            BoundedSegment {
                source,
                bounds: [a.min(b), a.max(b), c.min(d), c.max(d)],
            }
        })
        .collect();
    bounded.sort_unstable_by(|a, b| a.bounds[0].total_cmp(&b.bounds[0]));
    let mut active: Vec<&BoundedSegment> = Vec::new();
    for current in &bounded {
        active.retain(|previous| previous.bounds[1] >= current.bounds[0]);
        for previous in &active {
            if previous.bounds[3] < current.bounds[2] || current.bounds[3] < previous.bounds[2] {
                continue;
            }
            let left = &previous.source;
            let right = &current.source;
            if left.ring_index == right.ring_index {
                let distance = left.segment_index.abs_diff(right.segment_index);
                if !check_self || distance <= 1 || distance + 1 == left.segment_count {
                    continue;
                }
            }
            if geo::line_intersection::line_intersection(left.line, right.line).is_some() {
                return true;
            }
        }
        active.push(current);
    }
    false
}

#[derive(Debug, Clone)]
pub(crate) struct VectorFillTopology {
    pub surface: MultiPolygon<f64>,
    /// Present only when the input consists of simple, mutually non-crossing
    /// rings. In that common/canonical case every output polygon is assembled
    /// directly from source rings and we know exactly which raw paths own its
    /// visible exterior + holes without geometric proximity scans.
    pub component_path_indices: Option<Vec<Vec<usize>>>,
}

fn canonical_vector_fill_topology(vector: &VectorAsset) -> Option<VectorFillTopology> {
    struct Ring {
        path_index: usize,
        polygon: Polygon<f64>,
        area: f64,
        bounds: (f64, f64, f64, f64),
        containment_queries: Cell<usize>,
        containment_index: OnceCell<geo::indexed::IntervalTreeMultiPolygon<f64>>,
    }

    let mut rings = Vec::new();
    let mut segments = Vec::new();
    for (path_index, path) in vector
        .paths
        .iter()
        .enumerate()
        .filter(|(_, path)| path.closed)
    {
        let polygon = path_to_polygon(path)?;
        let area = signed_ring_area(polygon.exterior());
        if area.abs() <= SPECK_AREA {
            continue;
        }
        let bounds = polygon.bounding_rect()?;
        let coords = &polygon.exterior().0;
        let segment_count = coords.len().saturating_sub(1);
        if segment_count < 3 {
            continue;
        }
        let ring_index = rings.len();
        segments.extend(
            coords
                .windows(2)
                .enumerate()
                .filter_map(|(segment_index, pair)| {
                    let line = Line::new(pair[0], pair[1]);
                    (line.start != line.end).then_some(FillBoundarySegment {
                        ring_index,
                        segment_index,
                        segment_count,
                        line,
                    })
                }),
        );
        rings.push(Ring {
            path_index,
            polygon,
            area,
            containment_queries: Cell::new(0),
            containment_index: OnceCell::new(),
            bounds: (
                bounds.min().x,
                bounds.min().y,
                bounds.max().x,
                bounds.max().y,
            ),
        });
    }
    if rings.is_empty() {
        return Some(VectorFillTopology {
            surface: MultiPolygon(Vec::new()),
            component_path_indices: Some(Vec::new()),
        });
    }

    // Canonical raw-fill assets have simple rings that may nest but never cross.
    // Prove that with a broad-phase bounds sweep. If an imported/legacy
    // vector really contains crossing or self-intersecting rings, keep the exact
    // old boolean path below instead of guessing topology.
    if fill_boundaries_intersect(segments, true) {
        return None;
    }

    rings.sort_by(|left, right| right.area.abs().total_cmp(&left.area.abs()));
    let mut inside_windings = Vec::with_capacity(rings.len());
    let mut owners: Vec<Option<usize>> = Vec::with_capacity(rings.len());
    let mut exteriors: Vec<LineString<f64>> = Vec::new();
    let mut holes: Vec<Vec<LineString<f64>>> = Vec::new();
    let mut component_path_indices: Vec<Vec<usize>> = Vec::new();

    for index in 0..rings.len() {
        let ring = &rings[index];
        let sample_coord = ring.polygon.exterior().0.first().copied()?;
        let sample = Point::new(sample_coord.x, sample_coord.y);
        let parent = (0..index).rev().find(|candidate| {
            let candidate = &rings[*candidate];
            candidate.bounds.0 <= sample.x()
                && candidate.bounds.1 <= sample.y()
                && candidate.bounds.2 >= sample.x()
                && candidate.bounds.3 >= sample.y()
                && {
                    let queries = candidate.containment_queries.get() + 1;
                    candidate.containment_queries.set(queries);
                    // Many holes used to rescan the complete dense exterior
                    // once per ring. Build geo's interval index only when that
                    // same boundary is actually queried repeatedly.
                    if queries < 4 || candidate.polygon.exterior().0.len() < 128 {
                        candidate.polygon.contains(&sample)
                    } else {
                        candidate
                            .containment_index
                            .get_or_init(|| {
                                geo::indexed::IntervalTreeMultiPolygon::new(&MultiPolygon(vec![
                                    candidate.polygon.clone(),
                                ]))
                            })
                            .contains(&sample)
                    }
                }
        });
        let outside_winding = parent.map(|parent| inside_windings[parent]).unwrap_or(0);
        let inside_winding = outside_winding + if ring.area > 0.0 { 1 } else { -1 };
        let parent_owner = parent.and_then(|parent| owners[parent]);

        let owner = if outside_winding == 0 && inside_winding != 0 {
            let owner = exteriors.len();
            exteriors.push(ring.polygon.exterior().clone());
            holes.push(Vec::new());
            component_path_indices.push(vec![ring.path_index]);
            Some(owner)
        } else if outside_winding != 0 && inside_winding == 0 {
            if let Some(owner) = parent_owner {
                holes[owner].push(ring.polygon.exterior().clone());
                component_path_indices[owner].push(ring.path_index);
            }
            parent_owner
        } else {
            parent_owner
        };
        owners.push(owner);
        inside_windings.push(inside_winding);
    }

    let surface = MultiPolygon(
        exteriors
            .into_iter()
            .zip(holes)
            .map(|(exterior, interiors)| Polygon::new(exterior, interiors))
            .collect(),
    );
    Some(VectorFillTopology {
        surface,
        component_path_indices: Some(component_path_indices),
    })
}

fn vector_fill_geometry_boolean(vector: &VectorAsset) -> MultiPolygon<f64> {
    let mut rings: Vec<(Polygon<f64>, f64)> = vector
        .paths
        .iter()
        .filter(|path| path.closed)
        .filter_map(|path| {
            let polygon = path_to_polygon(path)?;
            let area = signed_ring_area(polygon.exterior());
            (area.abs() > SPECK_AREA).then_some((polygon, area))
        })
        .collect();
    rings.sort_by(|left, right| right.1.abs().total_cmp(&left.1.abs()));

    let mut surface = MultiPolygon(Vec::new());
    let mut inside_windings: Vec<i32> = Vec::with_capacity(rings.len());
    for index in 0..rings.len() {
        let (polygon, area) = &rings[index];
        let sample = polygon
            .exterior()
            .0
            .first()
            .map(|coord| Point::new(coord.x, coord.y));
        let parent = sample.and_then(|point| {
            (0..index)
                .rev()
                .find(|candidate| rings[*candidate].0.contains(&point))
        });
        let outside_winding = parent.map(|parent| inside_windings[parent]).unwrap_or(0);
        let inside_winding = outside_winding + if *area > 0.0 { 1 } else { -1 };

        if outside_winding == 0 && inside_winding != 0 {
            surface = surface.union(polygon);
        } else if outside_winding != 0 && inside_winding == 0 {
            surface = surface.difference(polygon);
        }
        inside_windings.push(inside_winding);
    }
    surface
}

pub(crate) fn vector_fill_topology(vector: &VectorAsset) -> VectorFillTopology {
    canonical_vector_fill_topology(vector).unwrap_or_else(|| VectorFillTopology {
        surface: vector_fill_geometry_boolean(vector),
        component_path_indices: None,
    })
}

pub(crate) fn vector_fill_geometry(vector: &VectorAsset) -> MultiPolygon<f64> {
    vector_fill_topology(vector).surface
}

fn path_to_polygon(path: &VPath) -> Option<Polygon<f64>> {
    if path.anchors.len() < 3 {
        return None;
    }

    // Adaptive subdivision follows authored curves without multiplying every
    // already-short brush segment by a fixed sample count on each union.
    let mut points = vec![path.anchors[0].point];
    for i in 0..path.anchors.len() {
        let a = &path.anchors[i];
        let b = &path.anchors[(i + 1) % path.anchors.len()];
        crate::brush_geometry::flatten_segment(a, b, |_, to, _, _| points.push(to));
    }
    let mut coords: Vec<Coord<f64>> = points
        .into_iter()
        .map(|p| Coord {
            x: p.x as f64,
            y: p.y as f64,
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

pub(crate) fn remove_unreferenced_assets(project: &mut ProjectV2, candidates: &BTreeSet<u16>) {
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
    crate::tools::remove_assets_and_metadata(project, removed);
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

    fn render_current_frame_once(app: &mut EditorApp) {
        let ctx = egui::Context::default();
        let stage_rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(
                app.state.project.meta.stage_width as f32,
                app.state.project.meta.stage_height as f32,
            ),
        );
        let view = crate::render::StageView {
            origin: stage_rect.min,
            scale: 1.0,
            stage_rect,
            edit_transform: q0s_format::transform::Affine::IDENTITY,
        };
        let q0rg_id = app.session.current_q0rg_id;
        let frame = app.session.current_frame;
        let project = &app.state.project;
        let textures = &mut app.textures;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(stage_rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("brush-eraser-cache-regression"),
                ));
                crate::render::render_stage(
                    &painter, project, q0rg_id, frame, &view, textures, ctx,
                );
            },
        );
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
    fn dense_smooth_boundary_is_stored_as_beziers_not_thousands_of_authored_points() {
        let center = Vec2::new(300.0, 180.0);
        let rx = 240.0_f32;
        let ry = 90.0_f32;
        let samples = 4096_usize;
        let mut coords = Vec::with_capacity(samples + 1);
        for index in 0..samples {
            let angle = std::f32::consts::TAU * index as f32 / samples as f32;
            coords.push(Coord {
                x: f64::from(center.x + rx * angle.cos()),
                y: f64::from(center.y + ry * angle.sin()),
            });
        }
        coords.push(coords[0]);
        let ring = LineString::new(coords);

        let path = ring_to_path(&ring).expect("dense ellipse path");
        assert!(
            path.anchors.len() <= 64,
            "a smooth 4096-sample ellipse survived as {} authored anchors instead of cubic Beziers",
            path.anchors.len()
        );
        assert!(path
            .anchors
            .iter()
            .any(|anchor| anchor.in_handle.is_some() || anchor.out_handle.is_some()));

        let flattened = flatten_path(&path);
        let max_radial_error = flattened
            .iter()
            .map(|point| {
                let nx = (point.x - center.x) / rx;
                let ny = (point.y - center.y) / ry;
                (nx * nx + ny * ny - 1.0).abs()
            })
            .fold(0.0_f32, f32::max);
        assert!(
            max_radial_error < 0.002,
            "curve fitting visibly drifted from the smooth source: normalized error {max_radial_error}"
        );
        assert_paths_are_finite_and_simple(std::slice::from_ref(&path));
    }

    #[test]
    fn subprecision_boundary_samples_are_eaten_before_bezier_fitting() {
        let origin = Vec2::new(550.0, 126.0);
        let mut points = Vec::new();
        for edge in 0..4 {
            let (start, end) = match edge {
                0 => (Vec2::new(0.0, 0.0), Vec2::new(80.0, 0.0)),
                1 => (Vec2::new(80.0, 0.0), Vec2::new(80.0, 40.0)),
                2 => (Vec2::new(80.0, 40.0), Vec2::new(0.0, 40.0)),
                _ => (Vec2::new(0.0, 40.0), Vec2::new(0.0, 0.0)),
            };
            for index in 0..1024 {
                let t = index as f32 / 1024.0;
                let base = Vec2::new(
                    origin.x + start.x + (end.x - start.x) * t,
                    origin.y + start.y + (end.y - start.y) * t,
                );
                points.push(base);
                // A second authored sample below useful f32/visual precision.
                points.push(Vec2::new(base.x + 0.000_06, base.y + 0.000_02));
            }
        }
        let mut coords: Vec<Coord<f64>> = points
            .iter()
            .map(|point| Coord {
                x: f64::from(point.x),
                y: f64::from(point.y),
            })
            .collect();
        coords.push(coords[0]);
        let ring = LineString::new(coords);
        let path = ring_to_path(&ring).expect("compact quantized rectangle");

        assert!(
            path.anchors.len() <= 16,
            "subprecision samples survived as {} authored anchors",
            path.anchors.len()
        );
        let flattened = flatten_path(&path);
        let (min, max) = points_bounds(&flattened).expect("compact bounds");
        assert!((min.x - origin.x).abs() < 0.01);
        assert!((min.y - origin.y).abs() < 0.01);
        assert!((max.x - (origin.x + 80.0)).abs() < 0.01);
        assert!((max.y - (origin.y + 40.0)).abs() < 0.01);
        assert_paths_are_finite_and_simple(std::slice::from_ref(&path));
    }

    #[test]
    fn curve_compaction_preserves_dense_hole_topology_and_winding() {
        fn dense_circle(
            center: Vec2,
            radius: f32,
            samples: usize,
            reverse: bool,
        ) -> LineString<f64> {
            let mut coords = Vec::with_capacity(samples + 1);
            for index in 0..samples {
                let logical = if reverse { samples - 1 - index } else { index };
                let angle = std::f32::consts::TAU * logical as f32 / samples as f32;
                coords.push(Coord {
                    x: f64::from(center.x + radius * angle.cos()),
                    y: f64::from(center.y + radius * angle.sin()),
                });
            }
            coords.push(coords[0]);
            LineString::new(coords)
        }

        let exterior = dense_circle(Vec2::new(200.0, 160.0), 120.0, 4096, false);
        let hole = dense_circle(Vec2::new(200.0, 160.0), 45.0, 2048, true);
        let coverage = MultiPolygon(vec![Polygon::new(exterior, vec![hole])]);
        let paths = coverage_to_paths(&coverage);
        assert_eq!(paths.len(), 2, "curve compaction changed hole count");
        assert!(paths.iter().all(|path| path.anchors.len() <= 64));
        let outer_area = signed_points_area(&flatten_path(&paths[0]));
        let hole_area = signed_points_area(&flatten_path(&paths[1]));
        assert!(
            outer_area.signum() != hole_area.signum(),
            "hole winding was lost"
        );
        assert_paths_are_finite_and_simple(&paths);
    }

    #[test]
    fn canonical_curve_compaction_is_independent_of_brush_smoothing_slider() {
        let low = settings(18.0, 0);
        let high = settings(18.0, 100);
        let points: Vec<Vec2> = (0..=320)
            .map(|index| {
                let x = index as f32 * 1.25;
                Vec2::new(x, 80.0 + (x * 0.018).sin() * 24.0)
            })
            .collect();

        let zero_region = brush_finish(stroke(&points, low), low);
        let high_region = brush_finish(stroke(&points, high), high);
        let zero_linear_anchors = anchor_count(&coverage_to_linear_paths(&zero_region));
        let zero_paths = coverage_to_paths(&zero_region);
        let high_paths = coverage_to_paths(&high_region);
        let zero_anchors = anchor_count(&zero_paths);
        let high_anchors = anchor_count(&high_paths);
        assert!(
            zero_anchors * 4 < zero_linear_anchors,
            "smoothing=0 canonicalization barely compacted the same silhouette: linear={zero_linear_anchors}, cubic={zero_anchors}"
        );
        assert!(high_anchors <= zero_anchors);
        assert_paths_are_finite_and_simple(&zero_paths);
        assert_paths_are_finite_and_simple(&high_paths);
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
    fn intermittent_missing_pen_pressure_never_becomes_full_size_dabs() {
        let mut dynamic = settings(40.0, 0);
        dynamic.pressure_size = true;
        dynamic.dynamics_sensitivity = 50;
        dynamic.dynamics_min_size = 0.1;
        let mut gesture = brush_begin(
            dynamic,
            BrushSample::pointer(Vec2::new(0.0, 0.0), None, 0.0),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(10.0, 0.0), Some(0.2), 0.01),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(20.0, 0.0), None, 0.02),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(30.0, 0.0), Some(0.2), 0.03),
        );

        assert!(gesture
            .samples
            .iter()
            .all(|sample| sample.pressure == Some(0.2)));
        let dabs = brush_preview_dabs(&gesture);
        let min = dabs
            .iter()
            .map(|(_, size)| *size)
            .fold(f32::INFINITY, f32::min);
        let max = dabs
            .iter()
            .map(|(_, size)| *size)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (max - min).abs() < 1.0e-4,
            "pressure gaps created size beads: {dabs:?}"
        );
        assert!(
            max < dynamic.size * 0.6,
            "pressure gap became a full nib: {max}"
        );
    }

    #[test]
    fn velocity_terminal_micro_move_does_not_create_a_stop_dot() {
        let mut dynamic = settings(40.0, 0);
        dynamic.velocity_size = true;
        dynamic.dynamics_sensitivity = 50;
        dynamic.dynamics_min_size = 0.1;
        let mut gesture = brush_begin(
            dynamic,
            BrushSample::pointer(Vec2::new(0.0, 0.0), None, 0.0),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(100.0, 0.0), None, 0.05),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(101.0, 0.0), None, 0.10),
        );
        let dabs = brush_preview_dabs(&gesture);
        assert!(
            dabs[2].1 <= dabs[1].1 * 1.05,
            "terminal micro-move inflated velocity cap: {dabs:?}"
        );
    }

    #[test]
    fn dynamic_finish_does_not_reunion_literal_endpoint_discs() {
        let mut dynamic = settings(40.0, 100);
        dynamic.pressure_size = true;
        dynamic.dynamics_sensitivity = 50;
        dynamic.dynamics_min_size = 0.1;
        let mut gesture = brush_begin(
            dynamic,
            BrushSample::pointer(Vec2::new(0.0, 0.0), Some(0.2), 0.0),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(60.0, 0.0), Some(0.2), 0.05),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(120.0, 0.0), Some(0.2), 0.10),
        );
        let last_size = brush_preview_dabs(&gesture).last().unwrap().1;
        let finished = brush_finish(gesture, dynamic);
        let bounds = finished.bounding_rect().expect("dynamic brush bounds");
        assert!(
            bounds.max().x <= 120.0 + f64::from(last_size) * 0.60,
            "finish restored an oversized literal endpoint disc: {:?}, size={last_size}",
            bounds
        );
    }

    #[test]
    fn pressureless_release_sample_does_not_inflate_the_endpoint() {
        let mut dynamic = settings(20.0, 0);
        dynamic.pressure_size = true;
        dynamic.velocity_size = true;
        dynamic.dynamics_sensitivity = 50;
        dynamic.dynamics_min_size = 0.2;
        let mut gesture = brush_begin(
            dynamic,
            BrushSample::pointer(Vec2::new(0.0, 0.0), Some(0.4), 0.0),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(20.0, 0.0), Some(0.35), 0.05),
        );
        let before = brush_preview_dabs(&gesture);
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(20.0, 0.0), None, 0.30),
        );
        let after = brush_preview_dabs(&gesture);

        assert_eq!(gesture.samples.len(), 2);
        assert_eq!(gesture.samples[1].pressure, Some(0.35));
        assert!((gesture.samples[1].time_seconds - 0.05).abs() < f64::EPSILON);
        assert!((before[1].1 - after[1].1).abs() < 1.0e-5);
    }

    #[test]
    fn pressure_dynamics_respects_minimum_size() {
        let mut dynamic = settings(20.0, 0);
        dynamic.pressure_size = true;
        dynamic.dynamics_sensitivity = 50;
        dynamic.dynamics_min_size = 0.25;
        let mut gesture = brush_begin(
            dynamic,
            BrushSample::pointer(Vec2::new(0.0, 0.0), Some(0.0), 0.0),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(20.0, 0.0), Some(1.0), 0.1),
        );
        let dabs = brush_preview_dabs(&gesture);
        assert!((dabs[0].1 - 5.0).abs() < 1.0e-4);
        assert!((dabs[1].1 - 20.0).abs() < 1.0e-4);
    }

    #[test]
    fn pressure_sensitivity_changes_the_response_curve() {
        let sample = BrushSample::pointer(Vec2::new(0.0, 0.0), Some(0.5), 0.0);
        let mut gentle = settings(20.0, 0);
        gentle.pressure_size = true;
        gentle.dynamics_min_size = 0.2;
        gentle.dynamics_sensitivity = 0;
        let mut sensitive = gentle;
        sensitive.dynamics_sensitivity = 100;
        let gentle_size = brush_preview_dabs(&brush_begin(gentle, sample))[0].1;
        let sensitive_size = brush_preview_dabs(&brush_begin(sensitive, sample))[0].1;
        assert!(sensitive_size < gentle_size * 0.7);
    }

    #[test]
    fn pressure_mode_keeps_plain_mouse_at_nominal_size() {
        let mut dynamic = settings(20.0, 0);
        dynamic.pressure_size = true;
        dynamic.dynamics_min_size = 0.1;
        let gesture = brush_begin(dynamic, BrushSample::mouse(Vec2::new(10.0, 10.0)));
        assert!((brush_preview_dabs(&gesture)[0].1 - 20.0).abs() < 1.0e-4);
    }

    #[test]
    fn velocity_dynamics_makes_fast_motion_thinner_without_crossing_minimum() {
        let mut dynamic = settings(20.0, 0);
        dynamic.velocity_size = true;
        dynamic.dynamics_sensitivity = 50;
        dynamic.dynamics_min_size = 0.2;
        let mut gesture = brush_begin(
            dynamic,
            BrushSample::pointer(Vec2::new(0.0, 0.0), None, 0.0),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(10.0, 0.0), None, 1.0),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(100.0, 0.0), None, 1.01),
        );
        let dabs = brush_preview_dabs(&gesture);
        assert!(dabs[2].1 < dabs[1].1 * 0.5, "{dabs:?}");
        assert!(dabs[2].1 >= 4.0 - 1.0e-4, "{dabs:?}");
    }

    #[test]
    fn variable_size_sparse_samples_still_form_one_continuous_sweep() {
        let mut dynamic = settings(24.0, 0);
        dynamic.pressure_size = true;
        dynamic.dynamics_min_size = 0.2;
        let mut gesture = brush_begin(
            dynamic,
            BrushSample::pointer(Vec2::new(0.0, 0.0), Some(0.2), 0.0),
        );
        brush_add_sample(
            &mut gesture,
            dynamic,
            BrushSample::pointer(Vec2::new(100.0, 0.0), Some(1.0), 0.1),
        );
        let finished = brush_finish(gesture, dynamic);
        assert_eq!(finished.0.len(), 1);
        assert!(finished.contains(&Point::new(50.0, 0.0)));
    }

    #[test]
    fn long_variable_width_commit_uses_batched_union_without_progressive_freeze() {
        let mut dynamic = settings(18.0, 0);
        dynamic.pressure_size = true;
        dynamic.dynamics_min_size = 0.15;
        let mut gesture = brush_begin(
            dynamic,
            BrushSample::pointer(Vec2::new(0.0, 0.0), Some(0.5), 0.0),
        );
        for index in 1..=1_200 {
            let pressure = 0.15 + (index as f32 * 0.031).sin().abs() * 0.85;
            brush_add_sample(
                &mut gesture,
                dynamic,
                BrushSample::pointer(
                    Vec2::new(index as f32 * 0.75, (index as f32 * 0.045).sin() * 28.0),
                    Some(pressure),
                    index as f64 / 240.0,
                ),
            );
        }

        let started = std::time::Instant::now();
        brush_flush_pending(&mut gesture);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "1200-sample variable-width commit took {:?}",
            started.elapsed()
        );
        assert!(!gesture.coverage.0.is_empty());
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
    fn dot_like_circular_stroke_keeps_a_smooth_capsule_instead_of_coarse_caps() {
        let settings = settings(40.0, 0);
        let start = Vec2::new(25.0, 30.0);
        let end = Vec2::new(25.6, 30.2);
        let actual = sweep_nib(BrushNib::Circle, settings.size, start, end);

        let ideal_points = nib_outline(BrushNib::Circle, settings.size, start)
            .into_iter()
            .chain(nib_outline(BrushNib::Circle, settings.size, end))
            .map(|point| Point::new(f64::from(point.x), f64::from(point.y)))
            .collect::<Vec<_>>();
        let ideal = MultiPolygon(vec![MultiPoint(ideal_points).convex_hull()]);
        let mismatch = actual
            .difference(&ideal)
            .union(&ideal.difference(&actual))
            .unsigned_area();
        let mismatch_ratio = mismatch / ideal.unsigned_area().max(1.0e-9);
        assert!(
            mismatch_ratio < 0.003,
            "tiny circular stroke has visibly coarse caps: mismatch ratio {mismatch_ratio}"
        );

        let finished = brush_finish(stroke(&[start, end], settings), settings);
        let finished_mismatch = finished
            .difference(&ideal)
            .union(&ideal.difference(&finished))
            .unsigned_area();
        assert!(
            finished_mismatch / ideal.unsigned_area().max(1.0e-9) < 0.004,
            "finished dot-like stroke gained angular shoulders"
        );
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
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 3);
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
    fn classic_brush_runtime_cannot_create_a_material() {
        let mut app = EditorApp::default();
        let classic = settings(16.0, 0);
        let classic_stroke = stroke(&[Vec2::new(10.0, 10.0), Vec2::new(40.0, 10.0)], classic);
        commit_brush_region(&mut app, brush_finish(classic_stroke, classic), classic);
        assert!(
            app.state.project.asset_appearances.is_empty(),
            "classic runtime has no material setting and must stay pure vector"
        );
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn explicit_advanced_material_stays_distinct_from_plain_paint() {
        let mut app = EditorApp::default();
        let plain = settings(12.0, 0);
        let left = stroke(&[Vec2::new(10.0, 20.0), Vec2::new(30.0, 20.0)], plain);
        commit_brush_region(&mut app, brush_finish(left, plain), plain);

        let right = stroke(&[Vec2::new(50.0, 20.0), Vec2::new(70.0, 20.0)], plain);
        commit_brush_region_with_material(
            &mut app,
            brush_finish(right, plain),
            plain,
            Some(VectorMaterial::SoftHalo {
                radius: 12.0,
                opacity: 0.55,
            }),
        );

        assert_eq!(app.state.project.assets.len(), 2);
        assert_eq!(app.state.project.asset_appearances.len(), 1);
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn disconnected_advanced_glow_strokes_keep_independent_material_fields() {
        let mut app = EditorApp::default();
        let brush = settings(12.0, 0);
        let material = VectorMaterial::SoftHalo {
            radius: 12.0,
            opacity: 0.55,
        };

        for x in [20.0_f32, 140.0, 260.0] {
            let gesture = stroke(&[Vec2::new(x, 40.0), Vec2::new(x + 40.0, 40.0)], brush);
            commit_brush_region_with_material(
                &mut app,
                brush_finish(gesture, brush),
                brush,
                Some(material),
            );
        }

        assert_eq!(app.state.project.assets.len(), 3);
        assert_eq!(app.state.project.asset_appearances.len(), 3);
        assert!(app
            .state
            .project
            .asset_appearances
            .values()
            .all(|appearance| {
                appearance.material_source.is_empty()
                    && appearance.clip_mask.is_empty()
                    && appearance.erase_mask.is_empty()
            }));

        // Touch the first stroke. It should merge with that one field only; the
        // two distant cached fields must remain independent.
        let touching = stroke(&[Vec2::new(45.0, 40.0), Vec2::new(90.0, 40.0)], brush);
        commit_brush_region_with_material(
            &mut app,
            brush_finish(touching, brush),
            brush,
            Some(material),
        );
        assert_eq!(
            app.state.project.assets.len(),
            3,
            "touching Advanced paint merges only its connected material field",
        );
        assert_eq!(app.state.project.asset_appearances.len(), 3);
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn twenty_distant_advanced_glow_strokes_do_not_grow_one_monolithic_source() {
        let mut app = EditorApp::default();
        let brush = settings(8.0, 0);
        let material = VectorMaterial::SoftHalo {
            radius: 10.0,
            opacity: 0.55,
        };
        for index in 0..20 {
            let x = 12.0 + (index % 5) as f32 * 110.0;
            let y = 20.0 + (index / 5) as f32 * 90.0;
            let gesture = stroke(&[Vec2::new(x, y), Vec2::new(x + 35.0, y + 8.0)], brush);
            commit_brush_region_with_material(
                &mut app,
                brush_finish(gesture, brush),
                brush,
                Some(material),
            );
        }
        assert_eq!(app.state.project.asset_appearances.len(), 20);
        assert_eq!(app.state.project.assets.len(), 20);
        let largest_body = app
            .state
            .project
            .assets
            .iter()
            .filter_map(|asset| match asset {
                Asset::Vector(vector) => Some(
                    vector
                        .paths
                        .iter()
                        .map(|path| path.anchors.len())
                        .sum::<usize>(),
                ),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        assert!(
            largest_body < 300,
            "a distant stroke accidentally accumulated the geometry of its neighbours: {largest_body} anchors",
        );
    }

    #[cfg(not(feature = "appearance-mask-eraser"))]
    #[test]
    fn disabling_appearance_engine_ignores_advanced_material_and_keeps_plain_merge_drawing() {
        let mut app = EditorApp::default();
        let plain = settings(12.0, 0);
        let left = stroke(&[Vec2::new(10.0, 20.0), Vec2::new(30.0, 20.0)], plain);
        commit_brush_region(&mut app, brush_finish(left, plain), plain);
        let right = stroke(&[Vec2::new(25.0, 20.0), Vec2::new(45.0, 20.0)], plain);
        commit_brush_region_with_material(
            &mut app,
            brush_finish(right, plain),
            plain,
            Some(VectorMaterial::SoftHalo {
                radius: 12.0,
                opacity: 0.55,
            }),
        );

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
    fn contained_raw_fill_merges_into_one_surface() {
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
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
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
            unreachable!()
        };
        assert_eq!(vector.paths.len(), 1);
        assert!((vector_fill_geometry(vector).unsigned_area() - 10_000.0).abs() < 0.1);
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
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
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
    fn post_drag_merge_preserves_bezier_boundaries_instead_of_polygonizing() {
        let color = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        let circle = |center_x: f32| {
            let r = 10.0;
            let k = r * 0.552_284_8;
            VPath {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(center_x, -r),
                        in_handle: Some(Vec2::new(center_x - k, -r)),
                        out_handle: Some(Vec2::new(center_x + k, -r)),
                    },
                    Anchor {
                        point: Vec2::new(center_x + r, 0.0),
                        in_handle: Some(Vec2::new(center_x + r, -k)),
                        out_handle: Some(Vec2::new(center_x + r, k)),
                    },
                    Anchor {
                        point: Vec2::new(center_x, r),
                        in_handle: Some(Vec2::new(center_x + k, r)),
                        out_handle: Some(Vec2::new(center_x - k, r)),
                    },
                    Anchor {
                        point: Vec2::new(center_x - r, 0.0),
                        in_handle: Some(Vec2::new(center_x - r, k)),
                        out_handle: Some(Vec2::new(center_x - r, -k)),
                    },
                ],
                closed: true,
            }
        };
        let left = circle(0.0);
        let right = circle(12.0);
        let mut app = EditorApp::default();
        app.state.project.assets = vec![
            raw_fill_asset(1, color, left.clone()),
            raw_fill_asset(2, color, right.clone()),
        ];
        app.state.project.q0rgs[0].layers[0].placements = vec![
            Placement {
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
        ];

        let edited = BTreeSet::from([1]);
        assert!(merge_touching_raw_fills_after_edit_focused(
            &mut app.state.project,
            1,
            1,
            0,
            &edited,
        ));
        assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 1);
        assert_eq!(app.state.project.assets.len(), 1);
        let Asset::Vector(vector) = &app.state.project.assets[0] else {
            panic!("merged raw fill must remain vector");
        };
        assert_eq!(
            vector.paths.len(),
            1,
            "the internal contours must be removed"
        );
        assert!(vector
            .paths
            .iter()
            .flat_map(|p| &p.anchors)
            .any(|a| a.in_handle.is_some() || a.out_handle.is_some()));
        let shape = |path| {
            vector_fill_geometry(&VectorAsset {
                asset_id: 0,
                paths: vec![path],
                fill: Some(color),
                stroke: None,
            })
        };
        let expected = shape(left).union(&shape(right));
        let actual = vector_fill_geometry(vector);
        let difference = expected.difference(&actual).unsigned_area()
            + actual.difference(&expected).unsigned_area();
        assert!(
            difference < expected.unsigned_area() * 0.01,
            "union changed the visible curved boundary: {difference}"
        );
    }

    #[test]
    fn legacy_concatenated_merge_is_normalized_on_edit() {
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
            pivot: None,
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
        let normalized = app.state.project.clone();
        assert!(!merge_touching_raw_fills_after_edit(
            &mut app.state.project,
            1,
            1,
            0
        ));
        assert_eq!(
            app.state.project, normalized,
            "an already-canonical boundary must stay untouched"
        );
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
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
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
    fn post_edit_merge_does_not_rebuild_an_unchanged_rounded_brush_asset() {
        let mut app = EditorApp::default();
        let paint = settings(24.0, 65);
        let gesture = stroke(
            &[
                Vec2::new(20.0, 40.0),
                Vec2::new(55.0, 28.0),
                Vec2::new(95.0, 52.0),
                Vec2::new(140.0, 36.0),
            ],
            paint,
        );
        commit_brush_region(&mut app, brush_finish(gesture, paint), paint);
        let before = app.state.project.assets[0].clone();

        assert!(!merge_touching_raw_fills_after_edit(
            &mut app.state.project,
            1,
            1,
            0,
        ));
        assert_eq!(app.state.project.assets[0], before);
    }

    #[test]
    fn focused_post_edit_merge_never_rebuilds_unrelated_touching_round_graphics() {
        let color = Rgba {
            r: 60,
            g: 90,
            b: 120,
            a: 255,
        };
        let r = 18.0;
        let k = r * 0.552_284_8;
        let center = Vec2::new(120.0, 30.0);
        let rounded = VPath {
            anchors: vec![
                Anchor {
                    point: Vec2::new(center.x, center.y - r),
                    in_handle: Some(Vec2::new(center.x - k, center.y - r)),
                    out_handle: Some(Vec2::new(center.x + k, center.y - r)),
                },
                Anchor {
                    point: Vec2::new(center.x + r, center.y),
                    in_handle: Some(Vec2::new(center.x + r, center.y - k)),
                    out_handle: Some(Vec2::new(center.x + r, center.y + k)),
                },
                Anchor {
                    point: Vec2::new(center.x, center.y + r),
                    in_handle: Some(Vec2::new(center.x + k, center.y + r)),
                    out_handle: Some(Vec2::new(center.x - k, center.y + r)),
                },
                Anchor {
                    point: Vec2::new(center.x - r, center.y),
                    in_handle: Some(Vec2::new(center.x - r, center.y + k)),
                    out_handle: Some(Vec2::new(center.x - r, center.y - k)),
                },
            ],
            closed: true,
        };
        let mut app = EditorApp::default();
        app.state.project.assets = vec![
            raw_fill_asset(1, color, square_path(0.0, 0.0, 20.0, 20.0)),
            raw_fill_asset(2, color, square_path(20.0, 0.0, 40.0, 20.0)),
            raw_fill_asset(3, color, rounded),
        ];
        app.state.project.q0rgs[0].layers[0].placements = (1..=3)
            .map(|asset_id| Placement {
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(asset_id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            })
            .collect();

        let mut old_global_behavior = app.state.project.clone();
        assert!(merge_touching_raw_fills_after_edit(
            &mut old_global_behavior,
            1,
            1,
            0,
        ));

        let before = app.state.project.clone();
        let edited = BTreeSet::from([3]);
        assert!(!merge_touching_raw_fills_after_edit_focused(
            &mut app.state.project,
            1,
            1,
            0,
            &edited,
        ));
        assert_eq!(app.state.project, before);
    }

    #[test]
    fn brush_and_edit_candidates_share_the_same_geometry() {
        let mut app = EditorApp::default();
        let paint = settings(16.0, 40);
        let gesture = stroke(
            &[
                Vec2::new(10.0, 20.0),
                Vec2::new(50.0, 24.0),
                Vec2::new(90.0, 20.0),
            ],
            paint,
        );
        commit_brush_region(&mut app, brush_finish(gesture, paint), paint);

        let cheap = collect_raw_fill_candidates(
            &app.state.project,
            app.session.current_q0rg_id,
            app.session.current_layer_id,
            app.session.current_frame,
            false,
        );
        assert!(!cheap.is_empty());
        let edit = collect_raw_fill_candidates(
            &app.state.project,
            app.session.current_q0rg_id,
            app.session.current_layer_id,
            app.session.current_frame,
            true,
        );
        assert_eq!(cheap.len(), edit.len());
        assert_eq!(
            cheap[0].geometry, edit[0].geometry,
            "click/move and painting must share the same boundary"
        );
    }

    #[test]
    fn brush_cut_preserves_a_small_curved_cap_for_further_edits() {
        let mut app = EditorApp::default();
        let green = Rgba {
            r: 0,
            g: 220,
            b: 95,
            a: 255,
        };
        let mut arch = square_path(0.0, 80.0, 100.0, 160.0);
        arch.anchors[0].out_handle = Some(Vec2::new(0.0, 0.0));
        arch.anchors[1].in_handle = Some(Vec2::new(100.0, 0.0));
        app.state.project.assets = vec![raw_fill_asset(1, green, arch)];
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            pivot: None,
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
        }];
        let cut = vector_fill_geometry(&VectorAsset {
            asset_id: 0,
            paths: vec![square_path(-10.0, 30.0, 110.0, 170.0)],
            fill: Some(green),
            stroke: None,
        });
        commit_brush_region(&mut app, cut, settings(10.0, 0));
        let cap = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(green) => Some(vector),
                _ => None,
            })
            .expect("curved cap must survive the cut");
        assert!(vector_fill_geometry(cap).contains(&Point::new(50.0, 25.0)));
        assert!(cap.paths.iter().all(|path| path.anchors.len() >= 3));
    }

    #[test]
    fn local_brush_edit_keeps_detailed_neighbour_boundaries_pixel_identical() {
        let mut app = EditorApp::default();
        app.state.project.meta.stage_width = 160;
        app.state.project.meta.stage_height = 160;
        let green = Rgba {
            r: 0,
            g: 220,
            b: 95,
            a: 255,
        };
        let cyan = Rgba {
            r: 0,
            g: 190,
            b: 165,
            a: 255,
        };
        let mut left = square_path(10.0, 10.0, 80.0, 150.0);
        let mut boundary = Vec::new();
        for i in 0..=28 {
            let y = 10.0 + i as f32 * 5.0;
            let x = 80.0 + (i as f32 * 1.7).sin() * 8.0;
            boundary.push(Anchor {
                point: Vec2::new(x, y),
                in_handle: Some(Vec2::new(x - 3.5, y - 1.7)),
                out_handle: Some(Vec2::new(x + 3.5, y + 1.7)),
            });
        }
        left.anchors.splice(1..3, boundary.iter().cloned());
        let mut right = square_path(80.0, 10.0, 150.0, 150.0);
        right.anchors = vec![right.anchors[1].clone(), right.anchors[2].clone()];
        right
            .anchors
            .extend(boundary.iter().rev().map(|anchor| Anchor {
                point: anchor.point,
                in_handle: anchor.out_handle,
                out_handle: anchor.in_handle,
            }));
        app.state.project.assets = vec![
            raw_fill_asset(1, green, left),
            raw_fill_asset(2, cyan, right),
        ];
        app.state.project.q0rgs[0].layers[0].placements = (1..=2)
            .map(|id| Placement {
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            })
            .collect();
        let render = |project: &ProjectV2| {
            q0s_format::raster::rasterize_q0rg_frame_scaled(
                project,
                1,
                0,
                960,
                960,
                1,
                [40, 40, 40, 255],
            )
        };
        let original = app.state.project.clone();
        let output =
            std::env::var_os("Q0EDITOR_BRUSH_REGRESSION_OUTPUT").map(std::path::PathBuf::from);
        if let Some(dir) = &output {
            std::fs::create_dir_all(dir).unwrap();
            crate::file_io::save_project(&dir.join("before.q1s"), &original).unwrap();
        }
        let black = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        for color in [black, green] {
            app.state.project = original.clone();
            let mut paint = settings(10.0, 0);
            paint.color = color;
            // Cross a straight edge, paint an interior hole, then cross the
            // detailed shared curve. Repeat to catch accumulating drift.
            for (x, y) in [(8.0, 60.0), (25.0, 95.0), (70.0, 115.0), (8.0, 60.0)] {
                let before = render(&app.state.project);
                let gesture = stroke(&[Vec2::new(x, y), Vec2::new(x + 18.0, y + 10.0)], paint);
                commit_brush_region(&mut app, brush_finish(gesture, paint), paint);
                let after = render(&app.state.project);
                let holes = before
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(after.as_chunks::<4>().0.iter())
                    .filter(|(a, b)| **a != [40, 40, 40, 255] && **b == [40, 40, 40, 255])
                    .count();
                assert_eq!(
                    holes, 0,
                    "painting must not expose background through the existing fill"
                );
                if let Some(dir) = &output {
                    let prefix = format!("{}-{x}-{y}", color.g);
                    for (name, pixels) in [("before", &before), ("after", &after)] {
                        image::save_buffer(
                            dir.join(format!("{prefix}-{name}.png")),
                            pixels,
                            960,
                            960,
                            image::ColorType::Rgba8,
                        )
                        .unwrap();
                    }
                    crate::file_io::save_project(
                        &dir.join(format!("{prefix}.q1s")),
                        &app.state.project,
                    )
                    .unwrap();
                }
                let changed = before
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(after.as_chunks::<4>().0.iter())
                    .enumerate()
                    .filter(|(index, (a, b))| {
                        let px = (index % 960) as f32 / 6.0;
                        let py = (index / 960) as f32 / 6.0;
                        let outside =
                            px < x - 7.0 || px > x + 25.0 || py < y - 7.0 || py > y + 17.0;
                        outside && a != b
                    })
                    .count();
                assert_eq!(
                    changed, 0,
                    "a local stroke at ({x}, {y}) changed distant pixels, colour {color:?}; first differences: {:?}",
                    before.as_chunks::<4>().0.iter().zip(after.as_chunks::<4>().0.iter()).enumerate()
                        .filter(|(i, (a, b))| a != b && (((i / 960) as f32 / 6.0) < y - 7.0 || ((i / 960) as f32 / 6.0) > y + 17.0))
                        .map(|(i, (a, b))| ((i % 960, i / 960), a, b)).take(15).collect::<Vec<_>>()
                );
                if color == black {
                    let center =
                        (((y + 5.0) * 6.0) as usize * 960 + ((x + 9.0) * 6.0) as usize) * 4;
                    assert_eq!(
                        &after[center..center + 4],
                        &[0, 0, 0, 255],
                        "the new stroke must still paint"
                    );
                }
            }
        }
    }

    #[test]
    fn distant_different_colour_commit_does_not_rebuild_old_vector() {
        let mut app = EditorApp::default();
        let mut red = settings(18.0, 50);
        red.color = Rgba {
            r: 220,
            g: 30,
            b: 30,
            a: 255,
        };
        let mut blue = red;
        blue.color = Rgba {
            r: 30,
            g: 60,
            b: 220,
            a: 255,
        };

        let first = stroke(&[Vec2::new(10.0, 20.0), Vec2::new(90.0, 28.0)], red);
        commit_brush_region(&mut app, brush_finish(first, red), red);
        let before = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(red.color) => Some(vector.clone()),
                _ => None,
            })
            .expect("red vector");

        let distant = stroke(
            &[Vec2::new(2_000.0, 2_000.0), Vec2::new(2_080.0, 2_008.0)],
            blue,
        );
        commit_brush_region(&mut app, brush_finish(distant, blue), blue);
        let after = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.asset_id == before.asset_id => Some(vector),
                _ => None,
            })
            .expect("distant paint must keep old red asset");
        assert_eq!(
            after, &before,
            "distant paint rebuilt unrelated vector geometry"
        );
    }

    #[test]
    fn distant_same_colour_commit_keeps_old_raw_asset_untouched() {
        let mut app = EditorApp::default();
        let paint = settings(18.0, 50);
        let first = stroke(&[Vec2::new(10.0, 20.0), Vec2::new(90.0, 28.0)], paint);
        commit_brush_region(&mut app, brush_finish(first, paint), paint);
        let before = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.fill == Some(paint.color) => Some(vector.clone()),
                _ => None,
            })
            .expect("first raw vector");

        let distant = stroke(
            &[Vec2::new(2_000.0, 2_000.0), Vec2::new(2_080.0, 2_008.0)],
            paint,
        );
        commit_brush_region(&mut app, brush_finish(distant, paint), paint);

        let after = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Vector(vector) if vector.asset_id == before.asset_id => Some(vector),
                _ => None,
            })
            .expect("distant same-colour paint must preserve the old raw asset");
        assert_eq!(after, &before);
        assert_eq!(
            app.state
                .project
                .assets
                .iter()
                .filter(|asset| matches!(asset, Asset::Vector(_)))
                .count(),
            2,
            "disconnected same-colour components must not collapse into one growing asset",
        );
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

            let vectors: Vec<&VectorAsset> = app
                .state
                .project
                .assets
                .iter()
                .filter_map(|asset| match asset {
                    Asset::Vector(vector) if vector.fill == Some(paint.color) => Some(vector),
                    _ => None,
                })
                .collect();
            for expected in &centers[..=index] {
                assert!(
                    vectors.iter().any(|vector| {
                        vector_fill_geometry(vector)
                            .contains(&Point::new(expected.x as f64, expected.y as f64))
                    }),
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
            pivot: None,
            instance_id: 0,
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
            fx: Default::default(),
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
            let vectors: Vec<&VectorAsset> = app
                .state
                .project
                .assets
                .iter()
                .filter_map(|asset| match asset {
                    Asset::Vector(vector) if vector.fill == Some(paint.color) => Some(vector),
                    _ => None,
                })
                .collect();
            assert!(
                !vectors.iter().any(|vector| {
                    vector_fill_geometry(vector).contains(&Point::new(100.0, 60.0))
                }),
                "untouched part of the ring hole must remain empty"
            );
            for expected in &centers[..=index] {
                assert!(
                    vectors.iter().any(|vector| {
                        vector_fill_geometry(vector)
                            .contains(&Point::new(expected.x as f64, expected.y as f64))
                    }),
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
    fn area_eraser_invalidates_cached_vector_render_geometry_immediately() {
        let mut app = EditorApp::default();
        let paint = settings(20.0, 0);
        let eraser = settings(12.0, 0);
        let horizontal = stroke(&[Vec2::new(0.0, 20.0), Vec2::new(60.0, 20.0)], paint);
        let cut = stroke(&[Vec2::new(30.0, -20.0), Vec2::new(30.0, 60.0)], eraser);
        commit_brush_region(&mut app, brush_finish(horizontal, paint), paint);
        app.state.project.asset_appearances.clear();

        render_current_frame_once(&mut app);
        assert_eq!(app.textures.vector_render_geometry_build_count(), 1);

        assert!(erase_brush_region(&mut app, brush_finish(cut, eraser)));
        render_current_frame_once(&mut app);
        assert_eq!(
            app.textures.vector_render_geometry_build_count(),
            2,
            "eraser geometry changed in the project but the stage reused the stale pre-erase tessellation"
        );
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
                pivot: None,
                instance_id: 0,
                frame: 1,
                target: Target::Asset(shared_asset_id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
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
    fn boolean_reconstruction_follows_curves_with_bounded_subdivision() {
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
        assert!(
            polygon.exterior().0.len() > path.anchors.len() + 1,
            "curved edges cannot be replaced with anchor chords"
        );
        assert!(
            polygon.exterior().0.len() <= flatten_path(&path).len(),
            "adaptive curve sampling exceeded fixed sampling"
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
    fn brush_release_short_disjoint_edges_are_not_false_intersections() {
        assert!(!segments_intersect(
            Vec2::new(0.0, 0.0),
            Vec2::new(0.01, 0.005),
            Vec2::new(0.005, 0.003),
            Vec2::new(0.02, 0.01)
        ));
        assert!(segments_intersect(
            Vec2::new(0.0, 0.0),
            Vec2::new(0.01, 0.01),
            Vec2::new(0.0, 0.01),
            Vec2::new(0.01, 0.0)
        ));
        assert!(segments_intersect(
            Vec2::new(0.0, 0.0),
            Vec2::new(0.01, 0.0),
            Vec2::new(0.005, 0.0),
            Vec2::new(0.02, 0.0)
        ));
        assert!(segments_intersect(
            Vec2::new(0.0, 0.0),
            Vec2::new(0.01, 0.0),
            Vec2::new(0.01, 0.0),
            Vec2::new(0.02, 0.01)
        ));
    }

    #[test]
    fn brush_release_topology_guard_rejects_changed_hole_relationships() {
        let ring = |x0, y0, x1, y1| {
            path_to_polygon(&square_path(x0, y0, x1, y1))
                .unwrap()
                .exterior()
                .clone()
        };
        assert!(simplified_ring_relationships_are_valid(&MultiPolygon(
            vec![Polygon::new(
                ring(0.0, 0.0, 20.0, 20.0),
                vec![ring(3.0, 3.0, 7.0, 7.0)]
            )]
        )));
        assert!(!simplified_ring_relationships_are_valid(&MultiPolygon(
            vec![Polygon::new(
                ring(0.0, 0.0, 20.0, 20.0),
                vec![ring(30.0, 30.0, 32.0, 32.0)]
            )]
        )));
        assert!(!simplified_ring_relationships_are_valid(&MultiPolygon(
            vec![Polygon::new(
                ring(0.0, 0.0, 20.0, 20.0),
                vec![ring(18.0, 3.0, 22.0, 7.0)]
            )]
        )));
        assert!(!simplified_ring_relationships_are_valid(&MultiPolygon(
            vec![Polygon::new(
                ring(0.0, 0.0, 20.0, 20.0),
                vec![ring(2.0, 2.0, 12.0, 12.0), ring(4.0, 4.0, 6.0, 6.0)]
            )]
        )));
        assert!(!simplified_ring_relationships_are_valid(&MultiPolygon(
            vec![
                Polygon::new(ring(0.0, 0.0, 20.0, 20.0), Vec::new()),
                Polygon::new(ring(2.0, 2.0, 4.0, 4.0), Vec::new())
            ]
        )));
        assert!(simplified_ring_relationships_are_valid(&MultiPolygon(
            vec![
                Polygon::new(ring(0.0, 0.0, 20.0, 20.0), vec![ring(2.0, 2.0, 12.0, 12.0)]),
                Polygon::new(ring(4.0, 4.0, 6.0, 6.0), Vec::new())
            ]
        )));
    }

    #[test]
    fn merge_drawing_dense_structure_phase_timings() {
        for same_colour in [true, false] {
            for dab in [true, false] {
                let mut app = EditorApp::default();
                let paint = settings(14.0, 50);
                let mut paths = vec![VPath {
                    anchors: (0..8192)
                        .map(|index| {
                            let angle = std::f32::consts::TAU * index as f32 / 8192.0;
                            let radius = 260.0 + (angle * 37.0).sin() * 5.0;
                            Anchor {
                                point: Vec2::new(
                                    300.0 + angle.cos() * radius,
                                    300.0 + angle.sin() * radius,
                                ),
                                in_handle: None,
                                out_handle: None,
                            }
                        })
                        .collect(),
                    closed: true,
                }];
                for row in -6..=6 {
                    for column in -6..=6 {
                        let dx = column as f32 * 30.0;
                        let dy = row as f32 * 30.0;
                        if dx.hypot(dy) > 205.0 {
                            continue;
                        }
                        paths.push(VPath {
                            anchors: (0..32)
                                .map(|index| {
                                    let angle = -std::f32::consts::TAU * index as f32 / 32.0;
                                    Anchor {
                                        point: Vec2::new(
                                            300.0 + dx + angle.cos() * 6.0,
                                            300.0 + dy + angle.sin() * 6.0,
                                        ),
                                        in_handle: None,
                                        out_handle: None,
                                    }
                                })
                                .collect(),
                            closed: true,
                        });
                    }
                }
                let vector = VectorAsset {
                    asset_id: 1,
                    paths,
                    fill: Some(if same_colour {
                        paint.color
                    } else {
                        Rgba {
                            r: 40,
                            g: 80,
                            b: 200,
                            a: 255,
                        }
                    }),
                    stroke: None,
                };
                let input_anchors = anchor_count(&vector.paths);
                let hole_count = vector.paths.len() - 1;
                app.state.project.assets = vec![Asset::Vector(vector)];
                app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
                    pivot: None,
                    instance_id: 0,
                    frame: 0,
                    target: Target::Asset(1),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                    fx: Default::default(),
                }];
                render_current_frame_once(&mut app);
                let points = if dab {
                    vec![Vec2::new(560.0, 300.0)]
                } else {
                    vec![Vec2::new(550.0, 300.0), Vec2::new(580.0, 306.0)]
                };
                let region = brush_finish(stroke(&points, paint), paint);
                let start = std::time::Instant::now();
                let candidates = collect_raw_fill_candidates(
                    &app.state.project,
                    app.session.current_q0rg_id,
                    app.session.current_layer_id,
                    0,
                    false,
                );
                let collect = start.elapsed();
                assert_eq!(candidates.len(), 1);
                let fresh_paths = coverage_to_paths(&region);
                let fresh_geometry = vector_fill_geometry(&VectorAsset {
                    asset_id: 0,
                    paths: fresh_paths.clone(),
                    fill: Some(paint.color),
                    stroke: None,
                });
                let start = std::time::Instant::now();
                let edited = if same_colour {
                    candidates[0].geometry.union(&fresh_geometry)
                } else {
                    candidates[0].geometry.difference(&fresh_geometry)
                };
                let boolean = start.elapsed();
                let Asset::Vector(source) = &app.state.project.assets[0] else {
                    unreachable!()
                };
                let start = std::time::Instant::now();
                let sources = crate::brush_geometry::BoundarySources::new(
                    source.paths.iter().chain(&fresh_paths),
                );
                let source_index = start.elapsed();
                let start = std::time::Instant::now();
                let _ = sources.paths(&edited);
                let reconstruct = start.elapsed();
                let start = std::time::Instant::now();
                commit_brush_region(&mut app, region, paint);
                let commit = start.elapsed();
                let start = std::time::Instant::now();
                render_current_frame_once(&mut app);
                let render = start.elapsed();
                eprintln!("dense merge same_colour={same_colour} dab={dab} anchors={input_anchors}: collect={collect:?} boolean={boolean:?} source_index={source_index:?} reconstruct={reconstruct:?} commit={commit:?} render={render:?}");
                let expected = points.last().unwrap();
                assert!(app.state.project.assets.iter().any(|asset| matches!(asset, Asset::Vector(vector) if vector.fill == Some(paint.color) && vector_fill_geometry(vector).contains(&Point::new(f64::from(expected.x), f64::from(expected.y))))));
                let surfaces: Vec<_> = app
                    .state
                    .project
                    .assets
                    .iter()
                    .filter_map(|asset| match asset {
                        Asset::Vector(vector) => Some(vector_fill_geometry(vector)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    surfaces
                        .iter()
                        .flat_map(|surface| &surface.0)
                        .map(|polygon| polygon.interiors().len())
                        .sum::<usize>(),
                    hole_count
                );
                assert!(surfaces
                    .iter()
                    .all(|surface| !surface.contains(&Point::new(300.0, 300.0))));
            }
        }
    }

    #[test]
    fn merge_drawing_boundary_sweep_matches_exact_reference() {
        let mut seed = 17_u64;
        for case in 0..64 {
            let mut segments = Vec::new();
            for ring_index in 0..3 {
                let count = 3 + case % 9;
                let points: Vec<_> = (0..count)
                    .map(|_| {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                        let x = ((seed >> 32) % 21) as f64
                            + ring_index as f64 * if case % 2 == 0 { 30.0 } else { 5.0 };
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                        Coord {
                            x,
                            y: ((seed >> 32) % 21) as f64,
                        }
                    })
                    .collect();
                for segment_index in 0..count {
                    let line =
                        Line::new(points[segment_index], points[(segment_index + 1) % count]);
                    if line.start != line.end {
                        segments.push(FillBoundarySegment {
                            ring_index,
                            segment_index,
                            segment_count: count,
                            line,
                        });
                    }
                }
            }
            for check_self in [true, false] {
                let expected = segments.iter().enumerate().any(|(index, left)| {
                    segments[..index].iter().any(|right| {
                        if left.ring_index == right.ring_index {
                            let distance = left.segment_index.abs_diff(right.segment_index);
                            if !check_self || distance <= 1 || distance + 1 == left.segment_count {
                                return false;
                            }
                        }
                        geo::line_intersection::line_intersection(left.line, right.line).is_some()
                    })
                });
                assert_eq!(
                    fill_boundaries_intersect(segments.clone(), check_self),
                    expected,
                    "case={case} check_self={check_self}"
                );
            }
        }
    }

    #[test]
    fn brush_release_dense_scene_phase_timings() {
        for dynamic in [false, true] {
            let mut app = EditorApp::default();
            app.state.project.assets.clear();
            app.state.project.q0rgs[0].layers[0].placements.clear();
            let mut paint = settings(14.0, 50);
            paint.pressure_size = dynamic;
            for index in 0..9 {
                let cx = 100.0 + index as f32 * 140.0;
                let cy = 150.0;
                let path = VPath {
                    anchors: (0..1024)
                        .map(|vertex| {
                            let angle = std::f32::consts::TAU * vertex as f32 / 1024.0;
                            Anchor {
                                point: Vec2::new(cx + angle.cos() * 60.0, cy + angle.sin() * 60.0),
                                in_handle: None,
                                out_handle: None,
                            }
                        })
                        .collect(),
                    closed: true,
                };
                let id = index + 1;
                app.state.project.assets.push(raw_fill_asset(
                    id,
                    Rgba {
                        r: 70,
                        g: 90,
                        b: 140,
                        a: 255,
                    },
                    path,
                ));
                app.state.project.q0rgs[0].layers[0]
                    .placements
                    .push(Placement {
                        pivot: None,
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(id),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
                    });
            }
            let mut gesture = brush_begin(
                paint,
                BrushSample::pointer(Vec2::new(70.0, 140.0), Some(0.6), 0.0),
            );
            for index in 1..=480 {
                brush_add_sample(
                    &mut gesture,
                    paint,
                    BrushSample::pointer(
                        Vec2::new(
                            70.0 + index as f32 * 0.5,
                            140.0 + (index as f32 * 0.07).sin() * 30.0,
                        ),
                        Some(0.6 + (index as f32 * 0.03).sin() * 0.2),
                        index as f64 / 240.0,
                    ),
                );
            }
            let started = std::time::Instant::now();
            let trajectory = build_sweep_trajectory(&gesture.samples, paint.smoothing, paint.size);
            let sizes = stroke_sample_sizes(&gesture);
            let coverage = if dynamic {
                sweep_variable_trajectory_nib(paint.nib, &trajectory, &sizes)
            } else {
                sweep_trajectory_nib(paint.nib, paint.size, &trajectory)
            };
            let sweep = started.elapsed();
            let started = std::time::Instant::now();
            let smoothed = smooth_contours(&coverage, paint.smoothing, paint.size);
            let smooth = started.elapsed();
            let started = std::time::Instant::now();
            let paths = coverage_to_paths(&smoothed);
            let fit = started.elapsed();
            let started = std::time::Instant::now();
            let finished = brush_finish(gesture, paint);
            let finish = started.elapsed();
            let started = std::time::Instant::now();
            commit_brush_region(&mut app, finished, paint);
            eprintln!("brush release pressure={dynamic}: sweep={sweep:?}, smooth={smooth:?}, fit={fit:?} ({} anchors), complete finish={finish:?}, commit={:?}", anchor_count(&paths), started.elapsed());
            assert!(app.state.project.assets.iter().any(
                |asset| matches!(asset, Asset::Vector(vector) if vector.fill == Some(paint.color))
            ));
        }
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
                pivot: None,
                instance_id: 0,
                frame: 0,
                target: Target::Asset(original_asset_id),
                transform: Transform2D {
                    tx: 100.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
                fx: Default::default(),
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

    #[test]
    fn brush_zoom_screen_sized_samples_keep_their_individual_widths() {
        for pressure_size in [false, true] {
            let mut settings = BrushSettings {
                size: 12.0,
                smoothing: 0,
                pressure_size,
                scale_with_stage: false,
                ..Default::default()
            };
            let mut stroke = brush_begin(
                settings,
                BrushSample::pointer(Vec2::new(50.0, 50.0), Some(1.0), 0.0),
            );
            settings.size /= 8.0;
            brush_add_sample(
                &mut stroke,
                settings,
                BrushSample::pointer(Vec2::new(100.0, 50.0), Some(1.0), 0.1),
            );
            assert_eq!(
                brush_prerender_dabs(&stroke),
                vec![(Vec2::new(50.0, 50.0), 12.0), (Vec2::new(100.0, 50.0), 1.5)]
            );
            assert_eq!(brush_current_size(&stroke), 1.5);
            // Another zoom before pointer-up must not resize either sample.
            settings.size /= 4.0;
            let finished = brush_finish(stroke, settings);
            assert!(
                finished.contains(&Point::new(50.0, 54.0)),
                "zoom shrank the earlier wide part"
            );
            assert!(
                finished.contains(&Point::new(100.0, 50.5)),
                "release shrank the later thin part"
            );
            assert!(
                !finished.contains(&Point::new(100.0, 52.0)),
                "new samples kept the old oversized nib"
            );
        }
    }

    #[test]
    fn brush_zoom_small_authored_nibs_survive_commit_and_later_edits() {
        for nib in BrushNib::ALL {
            let mut app = EditorApp::default();
            app.session.current_q0rg_id = app.state.project.meta.entry_q0rg_id;
            app.session.current_layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
            let settings = BrushSettings {
                nib,
                size: 0.1,
                smoothing: 0,
                ..Default::default()
            };
            let at = Vec2::new(170.0, 318.0);
            commit_brush_region(
                &mut app,
                brush_finish(brush_begin(settings, BrushSample::mouse(at)), settings),
                settings,
            );
            let first = app
                .state
                .project
                .assets
                .iter()
                .find_map(|asset| match asset {
                    Asset::Vector(vector) => Some(vector.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!("visible zoomed {nib:?} dab was discarded on pointer-up")
                });
            assert!(
                vector_fill_geometry(&first).contains(&Point::new(170.0, 318.0)),
                "small {nib:?} fill disappeared from editable geometry"
            );
            let other = Vec2::new(180.0, 318.0);
            commit_brush_region(
                &mut app,
                brush_finish(brush_begin(settings, BrushSample::mouse(other)), settings),
                settings,
            );
            assert!(app.state.project.assets.iter().any(|asset| matches!(asset, Asset::Vector(vector) if vector.asset_id == first.asset_id && vector.paths == first.paths)),
                "a subsequent stroke removed the original {nib:?} dab");
        }
    }

    /// Crumbs like the ones bucket fills used to leave along nearly shared
    /// edges: each a few thousandths of a pixel wide.
    fn crumb(x: f32, y: f32) -> VPath {
        VPath {
            anchors: [
                Vec2::new(x, y),
                Vec2::new(x + 0.001, y + 0.03),
                Vec2::new(x + 0.0005, y + 0.05),
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

    #[test]
    fn boolean_crumbs_are_left_out_of_fill_geometry() {
        let mut paths = vec![square_path(0.0, 0.0, 20.0, 20.0)];
        // Apart from the square, where each would count as a piece of its own.
        paths.extend((0..200).map(|step| crumb(25.0, step as f32 * 0.1)));
        let vector = VectorAsset {
            asset_id: 1,
            paths,
            fill: Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            stroke: None,
        };

        let surface = vector_fill_geometry(&vector);
        assert_eq!(surface.0.len(), 1, "the square alone, without 200 crumbs");
        assert!((surface.unsigned_area() - 400.0).abs() < 0.01);
    }

    #[test]
    fn boolean_results_drop_their_crumbs_when_they_become_paths() {
        let polygon = |path: &VPath| path_to_polygon(path).expect("polygon");
        let square = polygon(&square_path(0.0, 0.0, 20.0, 20.0));
        let with_crumbs = MultiPolygon(vec![
            square.clone(),
            polygon(&crumb(30.0, 5.0)),
            polygon(&crumb(31.0, 5.0)),
        ]);
        assert_eq!(coverage_to_paths(&with_crumbs).len(), 1);
        assert_eq!(
            crate::tools::geo_multi_polygon_to_linear_paths(&with_crumbs).len(),
            1
        );
        // A real hole survives; a crumb of a hole does not.
        let holed = MultiPolygon(vec![Polygon::new(
            square.exterior().clone(),
            vec![
                polygon(&square_path(5.0, 5.0, 10.0, 10.0))
                    .exterior()
                    .clone(),
                polygon(&crumb(15.0, 15.0)).exterior().clone(),
            ],
        )]);
        assert_eq!(coverage_to_paths(&holed).len(), 2);
        assert_eq!(
            crate::tools::geo_multi_polygon_to_linear_paths(&holed).len(),
            2
        );
    }
}
