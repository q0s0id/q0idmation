//! Raster drawings on the stage: their meshes, and what the raster tools
//! draw over the stage (the nib ring, the selection).

use egui::epaint::Mesh;
use egui::{Color32, Context, Painter, Response};
use q0s_format::pixels::RasterAsset;
use q0s_format::transform::Affine;

use crate::app::EditorApp;
use crate::render::{StageView, TextureCache};

use super::RasterTool;

/// The meshes that draw `raster` placed by `transform` (its space to the
/// stage's), as the render pass asks for any asset.
pub(crate) fn meshes(
    textures: &mut TextureCache,
    ctx: &Context,
    raster: &RasterAsset,
    transform: Affine,
    view: &StageView,
    tint: Color32,
) -> Vec<Mesh> {
    let scale = transform.uniform_scale() * view.effective_scale();
    textures.raster.meshes(
        ctx,
        raster,
        |point| view.stage_to_screen(transform.apply(point)),
        scale,
        tint,
        Some(ctx.screen_rect().expand(8.0)),
    )
}

/// Draws over the stage while a raster tool is picked.
pub(crate) fn draw_overlay(
    app: &mut EditorApp,
    response: &Response,
    painter: &Painter,
    view: &StageView,
) {
    let Some(tool) = app.session.raster.tool else {
        return;
    };
    super::select::draw(app, painter, view);
    let ctx = painter.ctx().clone();
    let Some(pointer) = response.hover_pos() else {
        return;
    };
    match tool {
        RasterTool::Brush | RasterTool::Eraser | RasterTool::Smudge => {
            if ctx.input(|input| input.modifiers.alt) && !app.session.raster.painting() {
                return;
            }
            let Some(family) = tool.family() else {
                return;
            };
            let (_, settings) = app.settings.raster.preset(family);
            let paintable = super::check_layer(app).is_ok();
            // The nib's real size on screen (sizes are in stage pixels), so
            // what it covers is plain.
            let radius = settings.size * 0.5 * view.effective_scale();
            let at = app
                .session
                .raster
                .stroke
                .as_ref()
                .and_then(|active| {
                    let (x, y) = active.stroke.nib()?;
                    let stage = active
                        .canvas
                        .to_local
                        .inverse()?
                        .apply(q0s_format::v2::Vec2::new(x, y));
                    Some(view.stage_to_screen(stage))
                })
                .unwrap_or(pointer);
            super::cursor::nib(
                &ctx,
                painter,
                at,
                pointer,
                radius,
                &settings,
                tool,
                paintable,
                super::paint_color(app).map(|c| Color32::from_rgb(c.r, c.g, c.b)),
            );
        }
        RasterTool::Fill => {
            let fill = super::fill_color(app);
            super::cursor::bucket(
                &ctx,
                painter,
                pointer,
                Color32::from_rgb(fill.r, fill.g, fill.b),
            );
        }
        RasterTool::Picker => {
            // The raster paint under the pointer, read straight from the
            // drawings: a few lookups, never a render.
            let stage = view.screen_to_stage(pointer);
            let sample =
                super::sample_raster(app, stage).map(|[r, g, b, _]| Color32::from_rgb(r, g, b));
            crate::cursors::request_with_sample(
                &ctx,
                crate::cursors::CursorKind::Eyedropper,
                sample,
            );
        }
        RasterTool::Marquee => {
            let modifiers = ctx.input(|input| input.modifiers);
            crate::cursors::request(
                &ctx,
                super::select::pointer_kind(app, view, pointer, modifiers, false),
            );
        }
        RasterTool::Lasso => {
            let modifiers = ctx.input(|input| input.modifiers);
            crate::cursors::request(
                &ctx,
                super::select::pointer_kind(app, view, pointer, modifiers, true),
            );
        }
    }
}
