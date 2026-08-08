use geo::{ConvexHull, Coord, LineString, MultiPoint, MultiPolygon, Point, Polygon};
use q0s_format::v2::{Rgba, Vec2, VectorMaterial};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BrushMode {
    #[default]
    Classic,
    Advanced,
}

impl BrushMode {
    pub const ALL: [Self; 2] = [Self::Classic, Self::Advanced];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Advanced => "Advanced",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdvancedBrushSettings {
    pub color: Rgba,
    pub size: f32,
    /// Centre-line cleanup after the stabilizer, 0..=100.
    pub smoothing: u8,
    /// Real-time trailing stabilizer strength, 0..=100.
    pub stabilizer: u8,
    /// Minor-axis / major-axis ratio of the elliptical tip, 0.05..=1.
    pub roundness: f32,
    /// Fixed tip angle, or offset from the trajectory when auto-angle is enabled.
    pub angle_degrees: f32,
    pub auto_angle: bool,
    /// Fraction of stroke length used to ramp in/out from a point.
    pub taper_start: f32,
    pub taper_end: f32,
    pub pressure_size: bool,
    /// Size retained at zero pressure, 0.01..=1.
    pub pressure_min_size: f32,
    /// How strongly fast motion reduces size, 0..=1.
    pub velocity_size: f32,
    pub glow: bool,
    pub glow_radius: f32,
    pub glow_opacity: f32,
    pub scale_with_stage: bool,
}

pub fn builtin_presets() -> Vec<(&'static str, AdvancedBrushSettings)> {
    let base = AdvancedBrushSettings::default();
    let mut ink = base;
    ink.size = 14.0;
    ink.stabilizer = 22;
    ink.smoothing = 28;
    ink.taper_start = 0.06;
    ink.taper_end = 0.14;

    let mut glow = base;
    glow.size = 20.0;
    glow.stabilizer = 38;
    glow.smoothing = 45;
    glow.glow = true;
    glow.glow_radius = 16.0;
    glow.glow_opacity = 0.62;

    let mut calligraphy = base;
    calligraphy.size = 24.0;
    calligraphy.roundness = 0.28;
    calligraphy.angle_degrees = -32.0;
    calligraphy.pressure_min_size = 0.35;
    calligraphy.stabilizer = 18;

    let mut dynamic = base;
    dynamic.size = 22.0;
    dynamic.stabilizer = 42;
    dynamic.smoothing = 35;
    dynamic.velocity_size = 0.58;
    dynamic.taper_start = 0.08;
    dynamic.taper_end = 0.22;

    vec![
        ("Advanced Ink", ink),
        ("Soft Glow", glow),
        ("Calligraphy", calligraphy),
        ("Dynamic Taper", dynamic),
    ]
}

impl Default for AdvancedBrushSettings {
    fn default() -> Self {
        Self {
            color: Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            size: 18.0,
            smoothing: 35,
            stabilizer: 30,
            roundness: 1.0,
            angle_degrees: 0.0,
            auto_angle: false,
            taper_start: 0.0,
            taper_end: 0.0,
            pressure_size: true,
            pressure_min_size: 0.2,
            velocity_size: 0.0,
            glow: false,
            glow_radius: 12.0,
            glow_opacity: 0.55,
            scale_with_stage: true,
        }
    }
}

impl AdvancedBrushSettings {
    pub fn sanitized(mut self) -> Self {
        if !self.size.is_finite() {
            self.size = Self::default().size;
        }
        if !self.roundness.is_finite() {
            self.roundness = 1.0;
        }
        if !self.angle_degrees.is_finite() {
            self.angle_degrees = 0.0;
        }
        if !self.taper_start.is_finite() {
            self.taper_start = 0.0;
        }
        if !self.taper_end.is_finite() {
            self.taper_end = 0.0;
        }
        if !self.pressure_min_size.is_finite() {
            self.pressure_min_size = 0.2;
        }
        if !self.velocity_size.is_finite() {
            self.velocity_size = 0.0;
        }
        if !self.glow_radius.is_finite() {
            self.glow_radius = 12.0;
        }
        if !self.glow_opacity.is_finite() {
            self.glow_opacity = 0.55;
        }
        self.size = self.size.clamp(0.1, 1024.0);
        self.smoothing = self.smoothing.min(100);
        self.stabilizer = self.stabilizer.min(100);
        self.roundness = self.roundness.clamp(0.05, 1.0);
        self.angle_degrees = wrap_degrees(self.angle_degrees);
        self.taper_start = self.taper_start.clamp(0.0, 0.95);
        self.taper_end = self.taper_end.clamp(0.0, 0.95);
        self.pressure_min_size = self.pressure_min_size.clamp(0.01, 1.0);
        self.velocity_size = self.velocity_size.clamp(0.0, 1.0);
        self.glow_radius = self.glow_radius.clamp(0.25, 256.0);
        self.glow_opacity = self.glow_opacity.clamp(0.0, 1.0);
        self
    }

    pub fn material(self) -> Option<VectorMaterial> {
        if self.glow {
            Some(VectorMaterial::SoftHalo {
                radius: self.glow_radius,
                opacity: self.glow_opacity,
            })
        } else {
            None
        }
    }
}

fn wrap_degrees(value: f32) -> f32 {
    (value + 180.0).rem_euclid(360.0) - 180.0
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdvancedBrushSample {
    pub position: Vec2,
    pub pressure: Option<f32>,
    pub time_seconds: f64,
}

impl AdvancedBrushSample {
    pub fn mouse(position: Vec2, time_seconds: f64) -> Self {
        Self {
            position,
            pressure: None,
            time_seconds,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdvancedBrushStroke {
    pub samples: Vec<AdvancedBrushSample>,
    pub settings: AdvancedBrushSettings,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdvancedDab {
    pub center: Vec2,
    pub major_radius: f32,
    pub minor_radius: f32,
    pub angle_radians: f32,
    pub opacity: f32,
}

pub fn advanced_begin(
    settings: AdvancedBrushSettings,
    sample: AdvancedBrushSample,
) -> AdvancedBrushStroke {
    AdvancedBrushStroke {
        samples: vec![sample],
        settings: settings.sanitized(),
    }
}

pub fn advanced_add_sample(
    stroke: &mut AdvancedBrushStroke,
    settings: AdvancedBrushSettings,
    sample: AdvancedBrushSample,
) {
    if stroke
        .samples
        .last()
        .is_some_and(|last| distance(last.position, sample.position) <= 1.0e-5)
    {
        if let Some(last) = stroke.samples.last_mut() {
            *last = sample;
        }
    } else {
        stroke.samples.push(sample);
    }
    stroke.settings = settings.sanitized();
}

/// Dynamic nib placements shared by GPU live preview and the deterministic
/// vector commit path. No boolean geometry is built while the pointer moves.
pub fn advanced_dabs(stroke: &AdvancedBrushStroke) -> Vec<AdvancedDab> {
    build_dabs(&stroke.samples, stroke.settings)
}

pub fn advanced_finish(stroke: AdvancedBrushStroke) -> MultiPolygon<f64> {
    dabs_to_coverage(&build_dabs(&stroke.samples, stroke.settings))
}

fn build_dabs(
    samples: &[AdvancedBrushSample],
    settings: AdvancedBrushSettings,
) -> Vec<AdvancedDab> {
    if samples.is_empty() {
        return Vec::new();
    }
    let settings = settings.sanitized();
    let stabilized = stabilized_samples(samples, settings.stabilizer);
    let positions = smooth_positions(
        &stabilized
            .iter()
            .map(|sample| sample.position)
            .collect::<Vec<_>>(),
        settings.smoothing,
    );
    let mut lengths = vec![0.0f32; positions.len()];
    for index in 1..positions.len() {
        lengths[index] = lengths[index - 1] + distance(positions[index - 1], positions[index]);
    }
    let total_length = *lengths.last().unwrap_or(&0.0);

    positions
        .iter()
        .enumerate()
        .map(|(index, &position)| {
            let pressure = stabilized[index].pressure.unwrap_or(1.0).clamp(0.0, 1.0);
            let pressure_factor = if settings.pressure_size {
                settings.pressure_min_size + (1.0 - settings.pressure_min_size) * pressure
            } else {
                1.0
            };
            let speed = sample_speed(&stabilized, &positions, index);
            let reference_speed = (settings.size * 45.0).max(1.0);
            let normalized_speed = speed / reference_speed;
            let velocity_factor = 1.0 / (1.0 + settings.velocity_size * normalized_speed.max(0.0));
            let taper_factor = taper_factor(
                lengths[index],
                total_length,
                settings.taper_start,
                settings.taper_end,
            );
            let diameter = (settings.size * pressure_factor * velocity_factor * taper_factor)
                .max(settings.size * 0.0125)
                .max(0.05);
            let tangent_angle = trajectory_angle(&positions, index);
            let angle_radians = settings.angle_degrees.to_radians()
                + if settings.auto_angle {
                    tangent_angle
                } else {
                    0.0
                };
            AdvancedDab {
                center: position,
                major_radius: diameter * 0.5,
                minor_radius: diameter * 0.5 * settings.roundness,
                angle_radians,
                opacity: f32::from(settings.color.a) / 255.0,
            }
        })
        .collect()
}

fn stabilized_samples(samples: &[AdvancedBrushSample], stabilizer: u8) -> Vec<AdvancedBrushSample> {
    if samples.len() <= 1 || stabilizer == 0 {
        return samples.to_vec();
    }
    let strength = f32::from(stabilizer) / 100.0;
    let follow = 1.0 - strength * 0.88;
    let mut output = Vec::with_capacity(samples.len());
    output.push(samples[0]);
    let mut filtered = samples[0].position;
    for sample in &samples[1..] {
        filtered = Vec2::new(
            filtered.x + (sample.position.x - filtered.x) * follow,
            filtered.y + (sample.position.y - filtered.y) * follow,
        );
        output.push(AdvancedBrushSample {
            position: filtered,
            ..*sample
        });
    }
    output
}

fn smooth_positions(points: &[Vec2], smoothing: u8) -> Vec<Vec2> {
    if points.len() < 3 || smoothing == 0 {
        return points.to_vec();
    }
    let strength = f32::from(smoothing) / 100.0;
    let passes = 1 + usize::from(smoothing >= 40) + usize::from(smoothing >= 75);
    let mut current = points.to_vec();
    for _ in 0..passes {
        let mut next = current.clone();
        for index in 1..current.len() - 1 {
            let average = Vec2::new(
                (current[index - 1].x + current[index].x * 2.0 + current[index + 1].x) / 4.0,
                (current[index - 1].y + current[index].y * 2.0 + current[index + 1].y) / 4.0,
            );
            next[index] = Vec2::new(
                current[index].x + (average.x - current[index].x) * strength,
                current[index].y + (average.y - current[index].y) * strength,
            );
        }
        current = next;
    }
    current
}

fn sample_speed(samples: &[AdvancedBrushSample], positions: &[Vec2], index: usize) -> f32 {
    if samples.len() <= 1 {
        return 0.0;
    }
    let (a, b) = if index == 0 {
        (0, 1)
    } else {
        (index - 1, index)
    };
    let dt = (samples[b].time_seconds - samples[a].time_seconds)
        .abs()
        .max(1.0 / 1000.0) as f32;
    distance(positions[a], positions[b]) / dt
}

fn taper_factor(distance_along: f32, total: f32, start: f32, end: f32) -> f32 {
    if total <= 1.0e-5 {
        return 1.0;
    }
    let start_factor = if start > 0.0 {
        smoothstep((distance_along / (total * start)).clamp(0.0, 1.0))
    } else {
        1.0
    };
    let end_factor = if end > 0.0 {
        smoothstep(((total - distance_along) / (total * end)).clamp(0.0, 1.0))
    } else {
        1.0
    };
    start_factor.min(end_factor).max(0.0125)
}

fn smoothstep(value: f32) -> f32 {
    value * value * (3.0 - 2.0 * value)
}

fn trajectory_angle(points: &[Vec2], index: usize) -> f32 {
    if points.len() <= 1 {
        return 0.0;
    }
    let (a, b) = if index == 0 {
        (points[0], points[1])
    } else if index + 1 >= points.len() {
        (points[index - 1], points[index])
    } else {
        (points[index - 1], points[index + 1])
    };
    (b.y - a.y).atan2(b.x - a.x)
}

fn dabs_to_coverage(dabs: &[AdvancedDab]) -> MultiPolygon<f64> {
    let Some(first) = dabs.first().copied() else {
        return MultiPolygon(Vec::new());
    };
    if dabs.len() == 1 {
        return MultiPolygon(vec![ellipse_polygon(first, 20)]);
    }

    // Every bridge is the convex hull of two complete elliptical nib rings, so
    // it already contains both endpoint dabs. Build all sweep surfaces first
    // and union them in one balanced operation instead of repeatedly unioning
    // a growing polygon after every pointer sample. The old left-fold became
    // dramatically slower as a stroke grew even though no boolean work was
    // needed until pointer-up.
    let mut surfaces = Vec::with_capacity(dabs.len().saturating_sub(1));
    let mut previous_points = ellipse_points(first, 20);
    for &dab in &dabs[1..] {
        let points = ellipse_points(dab, 20);
        surfaces.push(
            MultiPoint::new(
                previous_points
                    .iter()
                    .chain(points.iter())
                    .copied()
                    .map(Point::from)
                    .collect::<Vec<_>>(),
            )
            .convex_hull(),
        );
        previous_points = points;
    }
    geo::unary_union(surfaces.iter())
}

fn ellipse_polygon(dab: AdvancedDab, segments: usize) -> Polygon<f64> {
    let mut points = ellipse_points(dab, segments);
    if let Some(first) = points.first().copied() {
        points.push(first);
    }
    Polygon::new(LineString::new(points), Vec::new())
}

fn ellipse_points(dab: AdvancedDab, segments: usize) -> Vec<Coord<f64>> {
    let cos_a = dab.angle_radians.cos();
    let sin_a = dab.angle_radians.sin();
    (0..segments)
        .map(|index| {
            let phase = std::f32::consts::TAU * index as f32 / segments as f32;
            let local_x = phase.cos() * dab.major_radius;
            let local_y = phase.sin() * dab.minor_radius;
            Coord {
                x: f64::from(dab.center.x + local_x * cos_a - local_y * sin_a),
                y: f64::from(dab.center.y + local_x * sin_a + local_y * cos_a),
            }
        })
        .collect()
}

fn distance(a: Vec2, b: Vec2) -> f32 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use geo::{Area, BoundingRect};

    use super::*;

    fn sample(x: f32, y: f32, pressure: f32, time: f64) -> AdvancedBrushSample {
        AdvancedBrushSample {
            position: Vec2::new(x, y),
            pressure: Some(pressure),
            time_seconds: time,
        }
    }

    #[test]
    fn advanced_defaults_are_plain_and_safe() {
        let settings = AdvancedBrushSettings::default();
        assert!(!settings.glow);
        assert_eq!(settings.roundness, 1.0);
        assert!(settings.pressure_size);
        assert!(settings.material().is_none());
    }

    #[test]
    fn pressure_and_taper_change_dab_size_without_changing_path() {
        let settings = AdvancedBrushSettings {
            stabilizer: 0,
            smoothing: 0,
            taper_start: 0.25,
            taper_end: 0.25,
            pressure_min_size: 0.1,
            ..AdvancedBrushSettings::default()
        };
        let stroke = AdvancedBrushStroke {
            samples: vec![
                sample(0.0, 0.0, 0.2, 0.0),
                sample(50.0, 0.0, 1.0, 0.1),
                sample(100.0, 0.0, 0.2, 0.2),
            ],
            settings,
        };
        let dabs = advanced_dabs(&stroke);
        assert!(dabs[1].major_radius > dabs[0].major_radius * 4.0);
        assert!(dabs[1].major_radius > dabs[2].major_radius * 4.0);
        assert_eq!(dabs[1].center, Vec2::new(50.0, 0.0));
    }

    #[test]
    fn stabilizer_reduces_high_frequency_cursor_wobble() {
        let samples = vec![
            sample(0.0, 0.0, 1.0, 0.0),
            sample(10.0, 8.0, 1.0, 0.01),
            sample(20.0, -8.0, 1.0, 0.02),
            sample(30.0, 8.0, 1.0, 0.03),
            sample(40.0, -8.0, 1.0, 0.04),
        ];
        let raw = stabilized_samples(&samples, 0);
        let stable = stabilized_samples(&samples, 90);
        let raw_excursion: f32 = raw.iter().map(|sample| sample.position.y.abs()).sum();
        let stable_excursion: f32 = stable.iter().map(|sample| sample.position.y.abs()).sum();
        assert!(stable_excursion < raw_excursion * 0.6);
    }

    #[test]
    fn velocity_dynamics_make_fast_segment_thinner() {
        let settings = AdvancedBrushSettings {
            stabilizer: 0,
            smoothing: 0,
            pressure_size: false,
            velocity_size: 1.0,
            ..AdvancedBrushSettings::default()
        };
        let slow = AdvancedBrushStroke {
            samples: vec![sample(0.0, 0.0, 1.0, 0.0), sample(20.0, 0.0, 1.0, 1.0)],
            settings,
        };
        let fast = AdvancedBrushStroke {
            samples: vec![sample(0.0, 0.0, 1.0, 0.0), sample(20.0, 0.0, 1.0, 0.01)],
            settings,
        };
        assert!(advanced_dabs(&slow)[1].major_radius > advanced_dabs(&fast)[1].major_radius);
    }

    #[test]
    fn roundness_and_angle_create_an_oriented_elliptical_tip() {
        let settings = AdvancedBrushSettings {
            roundness: 0.25,
            angle_degrees: 90.0,
            pressure_size: false,
            ..AdvancedBrushSettings::default()
        };
        let stroke = advanced_begin(settings, sample(0.0, 0.0, 1.0, 0.0));
        let coverage = advanced_finish(stroke);
        let bounds = coverage.bounding_rect().expect("ellipse bounds");
        assert!(bounds.height() > bounds.width() * 3.0);
    }

    #[test]
    fn glow_material_uses_advanced_radius_and_opacity() {
        let settings = AdvancedBrushSettings {
            glow: true,
            glow_radius: 23.0,
            glow_opacity: 0.37,
            ..AdvancedBrushSettings::default()
        };
        assert_eq!(
            settings.material(),
            Some(VectorMaterial::SoftHalo {
                radius: 23.0,
                opacity: 0.37,
            })
        );
    }

    #[test]
    fn builtin_library_exercises_distinct_advanced_dynamics() {
        let presets = builtin_presets();
        assert_eq!(presets.len(), 4);
        assert!(presets.iter().any(|(_, preset)| preset.glow));
        assert!(presets.iter().any(|(_, preset)| preset.roundness < 0.5));
        assert!(presets.iter().any(|(_, preset)| preset.velocity_size > 0.5));
        assert!(presets.iter().any(|(_, preset)| preset.taper_end > 0.1));
    }

    #[cfg(feature = "appearance-mask-eraser")]
    #[test]
    fn advanced_commit_stays_fill_only_vector_and_attaches_material() {
        let mut app = crate::app::EditorApp::default();
        let settings = AdvancedBrushSettings {
            glow: true,
            glow_radius: 15.0,
            glow_opacity: 0.48,
            stabilizer: 0,
            smoothing: 0,
            pressure_size: false,
            ..AdvancedBrushSettings::default()
        };
        let stroke = AdvancedBrushStroke {
            samples: vec![sample(20.0, 20.0, 1.0, 0.0), sample(80.0, 40.0, 1.0, 0.1)],
            settings,
        };
        let region = advanced_finish(stroke);
        let bridge = crate::brush::BrushSettings {
            color: settings.color,
            size: settings.size,
            smoothing: 0,
            nib: crate::brush::BrushNib::Circle,
            scale_with_stage: true,
            sync_with_eraser: true,
            ..crate::brush::BrushSettings::default()
        };
        crate::brush::commit_brush_region_with_material(
            &mut app,
            region,
            bridge,
            settings.material(),
        );
        let vector = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                q0s_format::v2::Asset::Vector(vector) => Some(vector),
                _ => None,
            })
            .expect("advanced vector asset");
        assert!(vector.fill.is_some());
        assert!(vector.stroke.is_none());
        assert!(!vector.paths.is_empty());
        assert_eq!(
            app.state.project.asset_appearances[&vector.asset_id].material,
            VectorMaterial::SoftHalo {
                radius: 15.0,
                opacity: 0.48,
            }
        );
    }

    #[test]
    fn dense_advanced_commit_keeps_one_continuous_surface_after_batched_union() {
        let settings = AdvancedBrushSettings {
            stabilizer: 0,
            smoothing: 0,
            pressure_size: false,
            ..AdvancedBrushSettings::default()
        };
        let samples = (0..240)
            .map(|index| {
                let x = index as f32 * 2.0;
                let y = (index as f32 * 0.09).sin() * 18.0;
                sample(x, y, 1.0, index as f64 / 240.0)
            })
            .collect();
        let coverage = advanced_finish(AdvancedBrushStroke { samples, settings });
        assert_eq!(coverage.0.len(), 1);
        assert!(coverage.unsigned_area() > 4_000.0);
    }

    #[test]
    fn dense_advanced_full_commit_keeps_raw_boundary_bounded() {
        let mut app = crate::app::EditorApp::default();
        let settings = AdvancedBrushSettings {
            stabilizer: 0,
            smoothing: 35,
            pressure_size: false,
            ..AdvancedBrushSettings::default()
        };
        let samples = (0..240)
            .map(|index| {
                let x = 20.0 + index as f32 * 2.0;
                let y = 120.0 + (index as f32 * 0.09).sin() * 18.0;
                sample(x, y, 1.0, index as f64 / 240.0)
            })
            .collect();
        let region = advanced_finish(AdvancedBrushStroke { samples, settings });
        let bridge = crate::brush::BrushSettings {
            color: settings.color,
            size: settings.size,
            smoothing: 0,
            nib: crate::brush::BrushNib::Circle,
            scale_with_stage: true,
            sync_with_eraser: true,
            ..crate::brush::BrushSettings::default()
        };
        crate::brush::commit_brush_region_with_material(&mut app, region, bridge, None);
        let vector = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                q0s_format::v2::Asset::Vector(vector) if vector.asset_id != 0 => Some(vector),
                _ => None,
            })
            .expect("dense advanced raw vector");
        let anchors: usize = vector.paths.iter().map(|path| path.anchors.len()).sum();
        assert!(
            anchors < 2_500,
            "dense advanced boundary exploded to {anchors} anchors"
        );
    }

    #[test]
    fn sparse_advanced_samples_commit_as_one_continuous_surface() {
        let settings = AdvancedBrushSettings {
            stabilizer: 0,
            smoothing: 0,
            pressure_size: false,
            ..AdvancedBrushSettings::default()
        };
        let stroke = AdvancedBrushStroke {
            samples: vec![sample(0.0, 0.0, 1.0, 0.0), sample(120.0, 0.0, 1.0, 0.1)],
            settings,
        };
        let coverage = advanced_finish(stroke);
        assert_eq!(coverage.0.len(), 1);
        assert!(coverage.unsigned_area() > f64::from(120.0_f32 * settings.size * 0.8));
    }
}
