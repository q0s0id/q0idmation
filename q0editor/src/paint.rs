//! A fill is a colour or a gradient. `PaintGradient` is what a person picks
//! (kind, stops, angle, size). Fitting it to a drawing's bounds gives the
//! stored `GradientFill`, so one gradient looks right on any shape, and the
//! tools can paint new shapes with it.

use q0s_format::transform::Affine;
use q0s_format::v2::{GradientFill, GradientKind, GradientStop, Rgba, Vec2, VectorAsset};

#[derive(Clone, Debug, PartialEq)]
pub struct PaintGradient {
    pub kind: GradientKind,
    /// Sorted, inside 0..=1, at least two.
    pub stops: Vec<GradientStop>,
    /// Direction in degrees: 0 runs left to right, 90 top to bottom.
    pub angle: f32,
    /// 1 spans the drawing; less packs the colours tighter.
    pub size: f32,
}

/// Local-space min and max of a drawing.
pub(crate) type Bounds = (Vec2, Vec2);

pub(crate) fn vector_bounds(vector: &VectorAsset) -> Option<Bounds> {
    let mut min = Vec2::new(f32::INFINITY, f32::INFINITY);
    let mut max = Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for anchor in vector.paths.iter().flat_map(|path| &path.anchors) {
        for point in std::iter::once(anchor.point)
            .chain(anchor.in_handle)
            .chain(anchor.out_handle)
        {
            min = Vec2::new(min.x.min(point.x), min.y.min(point.y));
            max = Vec2::new(max.x.max(point.x), max.y.max(point.y));
        }
    }
    (min.x.is_finite() && max.x.is_finite() && min.y.is_finite() && max.y.is_finite())
        .then_some((min, max))
}

pub(crate) fn stop(position: f32, color: Rgba) -> GradientStop {
    GradientStop { position, color }
}

const fn rgba(c: [u8; 4]) -> Rgba {
    Rgba {
        r: c[0],
        g: c[1],
        b: c[2],
        a: c[3],
    }
}

/// A named gradient: its stops as position and RGBA.
pub(crate) type Preset = (&'static str, &'static [(f32, [u8; 4])]);

/// Ready-made gradients in the paint box.
pub(crate) const PRESETS: &[Preset] = &[
    (
        "Sunset",
        &[
            (0.0, [255, 94, 77, 255]),
            (0.5, [255, 154, 68, 255]),
            (1.0, [255, 221, 107, 255]),
        ],
    ),
    (
        "Ocean",
        &[
            (0.0, [12, 52, 131, 255]),
            (0.55, [33, 150, 243, 255]),
            (1.0, [162, 236, 255, 255]),
        ],
    ),
    (
        "Fire",
        &[
            (0.0, [120, 6, 6, 255]),
            (0.45, [230, 57, 14, 255]),
            (0.8, [255, 170, 0, 255]),
            (1.0, [255, 247, 186, 255]),
        ],
    ),
    (
        "Mint",
        &[(0.0, [0, 128, 110, 255]), (1.0, [178, 255, 214, 255])],
    ),
    (
        "Candy",
        &[(0.0, [255, 72, 176, 255]), (1.0, [125, 92, 255, 255])],
    ),
    (
        "Night",
        &[
            (0.0, [8, 8, 24, 255]),
            (0.6, [48, 30, 92, 255]),
            (1.0, [214, 92, 140, 255]),
        ],
    ),
    (
        "Metal",
        &[
            (0.0, [90, 96, 104, 255]),
            (0.35, [232, 236, 240, 255]),
            (0.6, [120, 126, 134, 255]),
            (1.0, [210, 214, 220, 255]),
        ],
    ),
    (
        "Rainbow",
        &[
            (0.0, [255, 59, 48, 255]),
            (0.2, [255, 149, 0, 255]),
            (0.4, [255, 230, 0, 255]),
            (0.6, [52, 199, 89, 255]),
            (0.8, [0, 122, 255, 255]),
            (1.0, [175, 82, 222, 255]),
        ],
    ),
];

impl PaintGradient {
    /// What switching a colour to a gradient gives: the colour into a light
    /// tint of itself, so nothing jumps.
    pub(crate) fn starting_from(color: Rgba, kind: GradientKind) -> Self {
        let lift = |c: u8| c + ((255 - c) as f32 * 0.6) as u8;
        let light = Rgba {
            r: lift(color.r),
            g: lift(color.g),
            b: lift(color.b),
            a: color.a,
        };
        Self {
            kind,
            stops: vec![stop(0.0, color), stop(1.0, light)],
            angle: if kind == GradientKind::Linear {
                90.0
            } else {
                0.0
            },
            size: 1.0,
        }
    }

    pub(crate) fn preset(index: usize, kind: GradientKind, angle: f32) -> Self {
        let (_, stops) = PRESETS[index % PRESETS.len()];
        Self {
            kind,
            stops: stops.iter().map(|(p, c)| stop(*p, rgba(*c))).collect(),
            angle,
            size: 1.0,
        }
    }

    pub(crate) fn sample(&self, position: f32) -> Rgba {
        GradientFill {
            kind: self.kind,
            stops: self.stops.clone(),
            transform: Affine::IDENTITY,
        }
        .sample_position(position)
    }

    pub(crate) fn sort(&mut self) {
        for stop in &mut self.stops {
            stop.position = stop.position.clamp(0.0, 1.0);
        }
        self.stops.sort_by(|a, b| a.position.total_cmp(&b.position));
    }

    pub(crate) fn reverse(&mut self) {
        for stop in &mut self.stops {
            stop.position = 1.0 - stop.position;
        }
        self.stops.reverse();
    }

    /// The stored gradient for a drawing with these bounds.
    pub(crate) fn fit(&self, (min, max): Bounds) -> GradientFill {
        let center = Vec2::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5);
        let half = Vec2::new(
            ((max.x - min.x) * 0.5).max(1.0),
            ((max.y - min.y) * 0.5).max(1.0),
        );
        let (sin, cos) = self.angle.to_radians().sin_cos();
        let size = self.size.clamp(0.05, 20.0);
        // Normalized space runs -1..1 along x (linear) or out to radius 1
        // (radial); the columns are where its x and y axes land locally.
        let (x_len, y_len) = match self.kind {
            // the box's extent along the gradient's direction
            GradientKind::Linear => {
                let reach = half.x * cos.abs() + half.y * sin.abs();
                (reach * size, reach * size)
            }
            GradientKind::Radial => (half.x * size, half.y * size),
        };
        GradientFill {
            kind: self.kind,
            stops: self.stops.clone(),
            transform: Affine {
                a11: x_len * cos,
                a21: x_len * sin,
                a12: -y_len * sin,
                a22: y_len * cos,
                tx: center.x,
                ty: center.y,
            },
        }
    }

    /// The pick behind a stored gradient, so the box can show and edit it.
    pub(crate) fn from_fill(fill: &GradientFill, bounds: Option<Bounds>) -> Self {
        let t = fill.transform;
        let angle = t.a21.atan2(t.a11).to_degrees();
        let reach = t.a11.hypot(t.a21);
        let mut pick = Self {
            kind: fill.kind,
            stops: fill.stops.clone(),
            angle: if angle.abs() < 0.01 { 0.0 } else { angle },
            size: 1.0,
        };
        if let Some(bounds) = bounds {
            let unit = Self {
                size: 1.0,
                ..pick.clone()
            }
            .fit(bounds)
            .transform;
            let unit_reach = unit.a11.hypot(unit.a21);
            if unit_reach > f32::EPSILON {
                pick.size = (reach / unit_reach * 100.0).round() / 100.0;
            }
        }
        pick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Bounds {
        (Vec2::new(0.0, 0.0), Vec2::new(100.0, 50.0))
    }

    #[test]
    fn a_fitted_gradient_runs_edge_to_edge_in_its_direction() {
        let red = rgba([255, 0, 0, 255]);
        let mut pick = PaintGradient::starting_from(red, GradientKind::Linear);
        pick.angle = 0.0;
        let fill = pick.fit(square());
        assert_eq!(fill.position_at(Vec2::new(0.0, 25.0)), Some(0.0));
        assert_eq!(fill.position_at(Vec2::new(100.0, 25.0)), Some(1.0));
        pick.angle = 90.0;
        let fill = pick.fit(square());
        assert!(fill.position_at(Vec2::new(50.0, 0.0)).unwrap() < 0.01);
        assert!(fill.position_at(Vec2::new(50.0, 50.0)).unwrap() > 0.99);
    }

    #[test]
    fn reading_a_gradient_back_gives_the_same_pick() {
        let mut pick = PaintGradient::preset(2, GradientKind::Linear, 30.0);
        pick.size = 0.5;
        let back = PaintGradient::from_fill(&pick.fit(square()), Some(square()));
        assert!((back.angle - 30.0).abs() < 0.01);
        assert!((back.size - 0.5).abs() < 0.01);
        assert_eq!(back.stops, pick.stops);
        let radial = PaintGradient::preset(0, GradientKind::Radial, 0.0);
        let fill = radial.fit(square());
        assert_eq!(fill.position_at(Vec2::new(50.0, 25.0)), Some(0.0));
        assert_eq!(fill.position_at(Vec2::new(100.0, 25.0)), Some(1.0));
    }
}
