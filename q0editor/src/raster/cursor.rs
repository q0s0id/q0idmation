//! The raster tools' pointers: an animated glyph from `crate::cursors`
//! (brush, eraser, fingertip) on the hotspot, and around it a ring as big
//! as the nib really is on screen, shaped like it (oval, turned), in the
//! colour it paints. While a stabilized stroke lags behind, a rope runs
//! from the pointer to the nib.

use egui::{Color32, Context, Painter, Pos2, Shape, Stroke};

use super::engine::{Blend, BrushSettings};
use super::RasterTool;
use crate::cursors::CursorKind;

/// Rings smaller than this many screen points are left out: the glyph's
/// tip says where a fine nib is.
const SMALLEST_RING: f32 = 3.0;

#[allow(clippy::too_many_arguments)]
pub(crate) fn nib(
    ctx: &Context,
    painter: &Painter,
    nib: Pos2,
    pointer: Pos2,
    radius: f32,
    settings: &BrushSettings,
    tool: RasterTool,
    paintable: bool,
    color: Option<Color32>,
) {
    if !paintable {
        crate::cursors::request(ctx, CursorKind::Locked);
        return;
    }
    let kind = match tool {
        RasterTool::Eraser => CursorKind::RasterEraser,
        RasterTool::Smudge => CursorKind::Smudge,
        _ => CursorKind::RasterBrush,
    };
    crate::cursors::request(ctx, kind);
    if nib.distance(pointer) > 1.0 {
        // The stabilizer's rope.
        painter.line_segment(
            [pointer, nib],
            Stroke::new(1.0_f32, Color32::from_white_alpha(140)),
        );
        painter.circle_filled(nib, 2.0, Color32::from_black_alpha(160));
    }
    if radius < SMALLEST_RING {
        return;
    }
    let ink = match settings.blend {
        Blend::Erase => Color32::from_rgb(230, 80, 80),
        _ => color.unwrap_or(Color32::WHITE),
    };
    let angle = settings.angle.to_radians();
    let points: Vec<Pos2> = (0..48)
        .map(|i| {
            let t = i as f32 / 48.0 * std::f32::consts::TAU;
            let (x, y) = (t.cos() * radius, t.sin() * radius * settings.roundness);
            let (sin, cos) = angle.sin_cos();
            nib + egui::vec2(x * cos - y * sin, x * sin + y * cos)
        })
        .collect();
    // Dark under light: shows on any colour.
    painter.add(Shape::closed_line(
        points.clone(),
        Stroke::new(2.5_f32, Color32::from_black_alpha(110)),
    ));
    painter.add(Shape::closed_line(
        points.clone(),
        Stroke::new(1.0_f32, Color32::from_white_alpha(220)),
    ));
    // The paint colour as a thin inner band.
    if radius > 8.0 {
        let inner: Vec<Pos2> = points
            .iter()
            .map(|p| nib + (*p - nib) * ((radius - 2.0) / radius))
            .collect();
        painter.add(Shape::closed_line(
            inner,
            Stroke::new(1.0_f32, ink.gamma_multiply(0.85)),
        ));
    }
    // A soft nib shows its fall-off as a second ring.
    if settings.hardness < 0.6 && radius > 10.0 {
        let core = (settings.hardness * 0.8 + 0.2).clamp(0.2, 0.9);
        let inner: Vec<Pos2> = points.iter().map(|p| nib + (*p - nib) * core).collect();
        painter.add(Shape::closed_line(
            inner,
            Stroke::new(1.0_f32, Color32::from_white_alpha(90)),
        ));
    }
}

/// The fill bucket's pointer, with a drop of the colour it pours beside it.
pub(crate) fn bucket(ctx: &Context, painter: &Painter, pointer: Pos2, fill: Color32) {
    crate::cursors::request(ctx, CursorKind::Bucket);
    let at = pointer + egui::vec2(-9.0, 10.0);
    painter.circle_filled(at, 4.0, fill);
    painter.circle_stroke(
        at,
        4.0,
        Stroke::new(1.0_f32, Color32::from_black_alpha(170)),
    );
}
