//! The raster tools' page in the Properties panel.
//!
//! Brushes show as a library of tiles, each with a stroke painted by the
//! real engine in the current colour, so what you pick is what you get.
//! Below the library sit the chosen brush's own settings; changing one
//! keeps the change on that brush, and Reset takes it back.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use egui::{vec2, Color32, ColorImage, Id, Rect, Sense, TextureHandle, TextureOptions, Ui};
use q0s_format::pixels::{PixelRect, RasterAsset};

use super::engine::{self, Blend, BrushSettings, Grain, Tip};
use super::presets::{self, Family, Preset};
use super::RasterTool;
use crate::app::EditorApp;

const TILE: egui::Vec2 = vec2(92.0, 46.0);

/// The Tool page while a raster tool is picked.
pub fn tool_options(app: &mut EditorApp, ui: &mut Ui) {
    let Some(tool) = app.session.raster.tool else {
        return;
    };
    crate::ui::section_header(ui, tool.label());
    layer_line(app, ui);
    match tool {
        RasterTool::Brush | RasterTool::Eraser | RasterTool::Smudge => {
            if let Some(family) = tool.family() {
                brush_page(app, ui, family);
            }
        }
        RasterTool::Fill => fill_page(app, ui),
        RasterTool::Picker => {
            let mut all = app.settings.raster.pick_all_layers;
            if crate::ui::switch(ui, &mut all, "Pick from every layer")
        .on_hover_text("Raster paint is read from the drawings; vector art and the paper off the screen.")
        .changed()
    {
                app.settings.raster.pick_all_layers = all;
            }
            ui.label(
                egui::RichText::new("Off, it reads only the current drawing. Alt-click with the brush picks into the stroke colour.")
                    .small()
                    .weak(),
            );
        }
        RasterTool::Marquee | RasterTool::Lasso => select_page(app, ui),
    }
}

/// Which drawing the tools work on, or why none.
fn layer_line(app: &EditorApp, ui: &mut Ui) {
    let text = match super::check_layer(app) {
        Ok(()) => match super::find_canvas(app) {
            Ok(Some(canvas)) => {
                let bounds = super::raster_ref(&app.state.project, canvas.asset_id)
                    .and_then(RasterAsset::exact_bounds);
                match bounds {
                    Some(b) => format!(
                        "Drawing {}: {} x {} px",
                        canvas.asset_id,
                        b.width(),
                        b.height()
                    ),
                    None => format!("Drawing {}: empty", canvas.asset_id),
                }
            }
            _ => "No drawing on this frame yet: painting makes one.".to_string(),
        },
        Err(why) => why.message().to_string(),
    };
    ui.label(egui::RichText::new(text).small().weak());
    ui.add_space(4.0);
}

fn brush_page(app: &mut EditorApp, ui: &mut Ui, family: Family) {
    let all = presets::family(family);
    let (chosen, mut settings) = app.settings.raster.preset(family);
    let color = match family {
        Family::Paint => super::paint_color(app)
            .map(|c| Color32::from_rgb(c.r, c.g, c.b))
            .unwrap_or(ui.visuals().text_color()),
        _ => ui.visuals().text_color(),
    };
    // The library.
    let columns = ((ui.available_width() + 4.0) / (TILE.x + 4.0))
        .floor()
        .max(1.0) as usize;
    egui::Grid::new(("raster_brushes", family as u8))
        .spacing(vec2(4.0, 4.0))
        .show(ui, |ui| {
            for (index, preset) in all.iter().enumerate() {
                let tweaked = app.settings.raster.changed.get(preset.key).copied();
                let look = tweaked.unwrap_or(preset.settings).sanitized();
                if brush_tile(
                    ui,
                    preset,
                    &look,
                    color,
                    preset.key == chosen.key,
                    tweaked.is_some(),
                )
                .clicked()
                {
                    app.settings.raster.choose(family, preset.key);
                }
                if (index + 1) % columns == 0 {
                    ui.end_row();
                }
            }
        });
    ui.add_space(6.0);
    let mut reset = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(chosen.label).strong());
        if app.settings.raster.changed.contains_key(chosen.key)
            && ui
                .small_button("Reset")
                .on_hover_text("Back to the brush as it came")
                .clicked()
        {
            reset = true;
        }
    });
    ui.label(egui::RichText::new(chosen.hint).small().weak());
    ui.add_space(4.0);
    if reset {
        app.settings.raster.changed.remove(chosen.key);
        settings = app.settings.raster.preset(family).1;
    }
    let edited = settings_controls(ui, &mut settings, family);
    if edited {
        app.settings
            .raster
            .set_settings(family, settings.sanitized());
    }
}

/// A brush in the library: its name over a stroke it painted.
fn brush_tile(
    ui: &mut Ui,
    preset: &Preset,
    settings: &BrushSettings,
    color: Color32,
    selected: bool,
    tweaked: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(TILE, Sense::click());
    if ui.is_rect_visible(rect) {
        crate::ui::paint_control(ui, &response, rect, 6.0, selected);
        // Painted once in white and tinted here, so a colour being dragged
        // in the picker never repaints the library.
        let paints = settings.blend == Blend::Normal || settings.blend == Blend::Behind;
        let texture = preview_texture(ui.ctx(), preset.key, settings);
        let image = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(rect.width() - 8.0, 26.0));
        ui.painter().image(
            texture.id(),
            image,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            if paints { color } else { Color32::WHITE },
        );
        let text_color = if selected {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        };
        let label = if tweaked {
            format!("{} *", preset.label)
        } else {
            preset.label.to_string()
        };
        crate::ui::clipped_label(
            ui.painter(),
            Rect::from_min_max(
                egui::pos2(rect.left() + 5.0, rect.bottom() - 15.0),
                rect.max - vec2(4.0, 1.0),
            ),
            &label,
            egui::FontId::proportional(11.0),
            text_color,
        );
    }
    response.on_hover_text(preset.hint)
}

#[derive(Clone, Default)]
struct Previews(HashMap<&'static str, (u64, TextureHandle)>);

/// A stroke the brush painted in white, kept until the brush changes.
fn preview_texture(
    ctx: &egui::Context,
    key: &'static str,
    settings: &BrushSettings,
) -> TextureHandle {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{settings:?}").hash(&mut hasher);
    let stamp = hasher.finish();
    let id = Id::new("raster_brush_previews");
    if let Some(texture) = ctx.data(|data| {
        data.get_temp::<Previews>(id).and_then(|previews| {
            previews
                .0
                .get(key)
                .filter(|(s, _)| *s == stamp)
                .map(|(_, t)| t.clone())
        })
    }) {
        return texture;
    }
    let image = render_preview(settings, Color32::WHITE);
    let texture = ctx.load_texture(
        format!("raster_preview_{key}"),
        image,
        TextureOptions::LINEAR,
    );
    ctx.data_mut(|data| {
        let previews = data.get_temp_mut_or_default::<Previews>(id);
        previews.0.insert(key, (stamp, texture.clone()));
    });
    texture
}

const PREVIEW_W: i32 = 168;
const PREVIEW_H: i32 = 52;

/// Paints an S stroke, light to firm to light, at a size that fits.
fn render_preview(settings: &BrushSettings, color: Color32) -> ColorImage {
    let rect = PixelRect::new(0, 0, PREVIEW_W, PREVIEW_H);
    let mut drawing = RasterAsset::new(0);
    let mut brush = *settings;
    brush.size = brush.size.clamp(2.0, 26.0);
    brush.taper = brush.taper.min(40.0);
    brush.stabilizer = 0.0;
    brush.build_up = false;
    let path: Vec<(f32, f32, Option<f32>)> = (0..=80)
        .map(|i| {
            let t = i as f32 / 80.0;
            let x = 14.0 + t * (PREVIEW_W as f32 - 28.0);
            let y = PREVIEW_H as f32 * 0.5
                + (t * std::f32::consts::TAU).sin() * PREVIEW_H as f32 * 0.22;
            let pressure = 0.25 + 0.75 * (t * std::f32::consts::PI).sin();
            (x, y, Some(pressure))
        })
        .collect();
    let [r, g, b, _] = color.to_array();
    match settings.blend {
        Blend::Erase | Blend::Smudge | Blend::Blur => {
            // Something to take away or move: bands of colour.
            let band = BrushSettings {
                size: PREVIEW_H as f32 * 0.5,
                hardness: 0.95,
                pressure_size: 0.0,
                ..BrushSettings::default()
            };
            let colours: [[u8; 3]; 3] = [[230, 90, 70], [80, 150, 230], [250, 200, 60]];
            for (i, colour) in colours.iter().enumerate() {
                let x0 = 6.0 + i as f32 * PREVIEW_W as f32 / 3.0;
                let x1 = x0 + PREVIEW_W as f32 / 3.0 - 4.0;
                let line = [
                    (x0, PREVIEW_H as f32 * 0.5, None),
                    (x1, PREVIEW_H as f32 * 0.5, None),
                ];
                engine::paint_path(&mut drawing, band, *colour, &line);
            }
            engine::paint_path(&mut drawing, brush, [r, g, b], &path);
        }
        _ => {
            engine::paint_path(&mut drawing, brush, [r, g, b], &path);
        }
    }
    let rgba = drawing.compose(rect);
    ColorImage::from_rgba_premultiplied([PREVIEW_W as usize, PREVIEW_H as usize], &rgba)
}

fn slider(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
    hint: &str,
) -> bool {
    let response = ui.add(egui::Slider::new(value, range).text(label));
    if hint.is_empty() {
        response.changed()
    } else {
        response.on_hover_text(hint).changed()
    }
}

/// The brush's settings. Returns whether any changed.
fn settings_controls(ui: &mut Ui, s: &mut BrushSettings, family: Family) -> bool {
    let mut changed = false;
    egui::CollapsingHeader::new("Nib")
        .default_open(true)
        .show(ui, |ui| {
            let mut size = s.size;
            let response = ui
                .add(
                    egui::Slider::new(&mut size, engine::SIZE_RANGE)
                        .logarithmic(true)
                        .suffix(" px")
                        .text("Size"),
                )
                .on_hover_text("Nib diameter in stage pixels. [ and ] on the stage.");
            if response.changed() {
                s.size = size;
                changed = true;
            }
            if family != Family::Smudge {
                changed |= slider(
                    ui,
                    &mut s.opacity,
                    0.0..=1.0,
                    "Opacity",
                    "The most one stroke lays down: it never gets darker where it crosses itself.",
                );
                changed |= slider(
                    ui,
                    &mut s.flow,
                    0.0..=1.0,
                    "Flow",
                    "What each dab lays down. Low flow builds up softly.",
                );
            } else {
                changed |= slider(
                    ui,
                    &mut s.strength,
                    0.0..=1.0,
                    "Strength",
                    "How strongly it drags or softens.",
                );
            }
            changed |= slider(
                ui,
                &mut s.hardness,
                0.0..=1.0,
                "Hardness",
                "Crisp edge at 1, soft all the way in at 0.",
            );
            changed |= slider(
                ui,
                &mut s.spacing,
                0.02..=2.0,
                "Spacing",
                "Gap between dabs, as a share of the size.",
            );
            changed |= slider(
                ui,
                &mut s.roundness,
                0.05..=1.0,
                "Roundness",
                "Flatten the nib into an oval, for calligraphy and markers.",
            );
            changed |= slider(
                ui,
                &mut s.angle,
                -90.0..=90.0,
                "Angle",
                "The flat nib's angle.",
            );
            let mut follow = s.follow;
            if crate::ui::switch(ui, &mut follow, "Turn with the stroke").changed() {
                s.follow = follow;
                changed = true;
            }
            if family == Family::Paint {
                egui::ComboBox::from_label("Tip")
                    .selected_text(match s.tip {
                        Tip::Round => "Round",
                        Tip::Bristles => "Bristles",
                        Tip::Spray => "Spray",
                    })
                    .show_ui(ui, |ui| {
                        for (tip, label) in [
                            (Tip::Round, "Round"),
                            (Tip::Bristles, "Bristles"),
                            (Tip::Spray, "Spray"),
                        ] {
                            if ui.selectable_value(&mut s.tip, tip, label).changed() {
                                changed = true;
                            }
                        }
                    });
                if s.tip != Tip::Round {
                    changed |= slider(
                        ui,
                        &mut s.density,
                        0.0..=1.0,
                        "Density",
                        "Hairs on a bristle tip, drops on a spray.",
                    );
                }
            }
        });
    egui::CollapsingHeader::new("Pressure and line")
        .default_open(true)
        .show(ui, |ui| {
            changed |= slider(
                ui,
                &mut s.pressure_size,
                0.0..=1.0,
                "Pressure: size",
                "How much a light touch thins the line.",
            );
            changed |= slider(
                ui,
                &mut s.pressure_opacity,
                0.0..=1.0,
                "Pressure: opacity",
                "How much a light touch lightens it.",
            );
            changed |= slider(
                ui,
                &mut s.min_size,
                0.02..=1.0,
                "Thinnest",
                "The size left at no pressure and at a taper's point.",
            );
            changed |= slider(
                ui,
                &mut s.stabilizer,
                0.0..=1.0,
                "Stabilizer",
                "The nib trails the pointer on a rope for steady, smooth lines.",
            );
            changed |= slider(
                ui,
                &mut s.taper,
                0.0..=200.0,
                "Taper",
                "Thin, pointed ends this long, even with a mouse.",
            );
        });
    if family == Family::Paint || family == Family::Erase {
        egui::CollapsingHeader::new("Texture")
            .default_open(false)
            .show(ui, |ui| {
                egui::ComboBox::from_label("Grain")
                    .selected_text(s.grain.label())
                    .show_ui(ui, |ui| {
                        for grain in Grain::ALL {
                            if ui
                                .selectable_value(&mut s.grain, grain, grain.label())
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });
                if s.grain != Grain::None {
                    changed |= slider(
                        ui,
                        &mut s.grain_amount,
                        0.0..=1.0,
                        "Grain amount",
                        "How much the paper's tooth breaks the stroke.",
                    );
                    changed |= slider(ui, &mut s.grain_scale, 0.25..=4.0, "Grain size", "");
                }
                changed |= slider(
                    ui,
                    &mut s.wet_edges,
                    0.0..=1.0,
                    "Wet edges",
                    "Paint pools at the stroke's rim, as watercolour does.",
                );
                changed |= slider(
                    ui,
                    &mut s.scatter,
                    0.0..=3.0,
                    "Scatter",
                    "Dabs stray from the line.",
                );
                changed |= slider(ui, &mut s.size_jitter, 0.0..=1.0, "Size jitter", "");
                changed |= slider(ui, &mut s.flow_jitter, 0.0..=1.0, "Flow jitter", "");
                changed |= slider(ui, &mut s.angle_jitter, 0.0..=1.0, "Angle jitter", "");
            });
    }
    if family != Family::Smudge {
        egui::CollapsingHeader::new("Paint")
            .default_open(family == Family::Paint)
            .show(ui, |ui| {
                let mut accumulate = s.accumulate;
                if crate::ui::switch(ui, &mut accumulate, "Build up within a stroke").changed() {
                    s.accumulate = accumulate;
                    changed = true;
                }
                let mut build = s.build_up;
                if crate::ui::switch(ui, &mut build, "Keep spraying while held").changed() {
                    s.build_up = build;
                    changed = true;
                }
                if family == Family::Paint {
                    let mut behind = s.blend == Blend::Behind;
                    if crate::ui::switch(ui, &mut behind, "Paint behind")
                        .on_hover_text("Paint goes under what is there: colour line art on its own layer.")
                        .changed()
                    {
                        s.blend = if behind { Blend::Behind } else { Blend::Normal };
                        changed = true;
                    }
                    let mut lock = s.lock_alpha;
                    if crate::ui::switch(ui, &mut lock, "Keep transparency")
                        .on_hover_text("Paints only where there is paint already: recolour lines without going over their edges.")
                        .changed()
                    {
                        s.lock_alpha = lock;
                        changed = true;
                    }
                }
            });
    } else {
        let mut blur = s.blend == Blend::Blur;
        if crate::ui::switch(ui, &mut blur, "Soften instead of drag").changed() {
            s.blend = if blur { Blend::Blur } else { Blend::Smudge };
            changed = true;
        }
    }
    changed
}

fn fill_page(app: &mut EditorApp, ui: &mut Ui) {
    let fill = &mut app.settings.raster.fill;
    ui.add(egui::Slider::new(&mut fill.tolerance, 0.0..=1.0).text("Tolerance"))
        .on_hover_text("How different a colour may be and still be filled.");
    let mut gap = f32::from(fill.gap);
    if ui
        .add(
            egui::Slider::new(&mut gap, 0.0..=f32::from(super::fill::MAX_GAP))
                .suffix(" px")
                .text("Close gaps"),
        )
        .on_hover_text("Gaps in the lines up to this wide still hold the paint.")
        .changed()
    {
        fill.gap = gap.round() as u8;
    }
    let mut grow = f32::from(fill.grow);
    if ui
        .add(
            egui::Slider::new(&mut grow, 0.0..=f32::from(super::fill::MAX_GROW))
                .suffix(" px")
                .text("Under the lines"),
        )
        .on_hover_text(
            "How far the colour reaches under the lines' soft edges, so no pale halo is left.",
        )
        .changed()
    {
        fill.grow = grow.round() as u8;
    }
    let mut all = fill.all_layers;
    if crate::ui::switch(ui, &mut all, "Lines on other raster layers count")
        .on_hover_text("Fills between the lines of every visible raster layer: keep colour on its own layer under the ink.")
        .changed()
    {
        fill.all_layers = all;
    }
    ui.label(egui::RichText::new("Pours the fill colour.").small().weak());
}

fn select_page(app: &mut EditorApp, ui: &mut Ui) {
    ui.label(
        egui::RichText::new("Drag to select; Shift adds, Alt takes away. Drag inside to move, the corners to size, just outside them to turn (Shift keeps the shape and steps the turn).")
            .small()
            .weak(),
    );
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Select all")
            .on_hover_text("Ctrl+A on the stage")
            .clicked()
        {
            super::select::select_all(app);
        }
        if ui.button("Place").on_hover_text("Enter").clicked() {
            super::select::let_go(app);
        }
        if ui.button("Delete").on_hover_text("Delete").clicked() {
            super::select::delete(app);
        }
    });
}

/// The toolbox icon of a raster tool, in `rect`. Each carries a small grid
/// of pixels at its foot, so it reads as raster beside its vector cousin.
pub fn paint_icon(
    painter: &egui::Painter,
    rect: Rect,
    tool: RasterTool,
    primary: Color32,
    secondary: Color32,
) {
    use egui::{pos2, Shape, Stroke};
    let s = rect.width() / 19.0;
    let p = |x: f32, y: f32| pos2(rect.left() + x * s, rect.top() + y * s);
    let line = |points: Vec<egui::Pos2>, width: f32, color: Color32| {
        painter.add(Shape::line(points, Stroke::new(width * s, color)));
    };
    match tool {
        RasterTool::Brush => {
            // Handle, ferrule, tuft: a round brush from top right.
            line(vec![p(16.5, 2.5), p(10.5, 8.5)], 2.0, primary);
            painter.add(Shape::convex_polygon(
                vec![p(10.0, 7.0), p(12.0, 9.0), p(10.4, 10.6), p(8.4, 8.6)],
                secondary,
                Stroke::NONE,
            ));
            painter.add(Shape::convex_polygon(
                vec![
                    p(8.6, 8.2),
                    p(10.8, 10.4),
                    p(7.4, 13.6),
                    p(4.0, 15.0),
                    p(5.4, 11.6),
                ],
                primary,
                Stroke::NONE,
            ));
        }
        RasterTool::Eraser => {
            painter.add(Shape::convex_polygon(
                vec![p(11.0, 3.0), p(16.0, 8.0), p(10.0, 14.0), p(5.0, 9.0)],
                primary,
                Stroke::NONE,
            ));
            painter.add(Shape::convex_polygon(
                vec![p(5.0, 9.0), p(10.0, 14.0), p(8.2, 15.8), p(3.2, 10.8)],
                secondary,
                Stroke::NONE,
            ));
        }
        RasterTool::Smudge => {
            // A smear, then the fingertip at its end.
            line(
                vec![p(3.0, 15.0), p(6.0, 12.5), p(9.0, 11.0)],
                2.6,
                secondary.gamma_multiply(0.6),
            );
            painter.add(Shape::line(
                vec![p(15.5, 3.0), p(10.0, 8.5)],
                Stroke::new(4.2 * s, primary),
            ));
            painter.circle_filled(p(9.6, 9.0), 2.1 * s, primary);
        }
        RasterTool::Fill => {
            painter.add(Shape::convex_polygon(
                vec![p(4.0, 7.0), p(11.0, 3.0), p(15.0, 10.0), p(8.0, 14.0)],
                primary,
                Stroke::NONE,
            ));
            line(vec![p(4.0, 7.0), p(2.6, 11.5)], 1.6, secondary);
            painter.circle_filled(p(2.6, 13.2), 1.5 * s, secondary);
        }
        RasterTool::Picker => {
            line(vec![p(4.0, 15.0), p(11.0, 8.0)], 1.8, primary);
            line(vec![p(9.5, 6.5), p(12.5, 9.5)], 2.0, primary);
            painter.circle_filled(p(13.6, 5.4), 2.6 * s, primary);
            painter.circle_filled(p(4.2, 14.8), 1.2 * s, secondary);
        }
        RasterTool::Marquee | RasterTool::Lasso => {
            let outline: Vec<egui::Pos2> = if tool == RasterTool::Marquee {
                vec![
                    p(3.0, 3.0),
                    p(16.0, 3.0),
                    p(16.0, 13.0),
                    p(3.0, 13.0),
                    p(3.0, 3.0),
                ]
            } else {
                (0..=24)
                    .map(|i| {
                        let t = i as f32 / 24.0 * std::f32::consts::TAU;
                        p(9.5 + t.cos() * 6.5, 7.5 + t.sin() * 4.6)
                    })
                    .collect()
            };
            painter.extend(Shape::dashed_line(
                &outline,
                Stroke::new(1.4 * s, primary),
                2.2 * s,
                1.6 * s,
            ));
            if tool == RasterTool::Lasso {
                line(vec![p(6.0, 11.4), p(5.0, 15.0), p(7.0, 17.0)], 1.3, primary);
            }
        }
    }
    // The pixel badge: a 2 x 2 checker at the foot.
    let cell = 2.0 * s;
    let origin = p(13.0, 13.0);
    for (i, j) in [(0, 0), (1, 1)] {
        painter.rect_filled(
            Rect::from_min_size(
                origin + vec2(i as f32 * cell, j as f32 * cell),
                vec2(cell, cell),
            ),
            0.0,
            secondary,
        );
    }
    for (i, j) in [(1, 0), (0, 1)] {
        painter.rect_filled(
            Rect::from_min_size(
                origin + vec2(i as f32 * cell, j as f32 * cell),
                vec2(cell, cell),
            ),
            0.0,
            primary.gamma_multiply(0.55),
        );
    }
}

/// A raster layer's icon in the timeline: a sheet of pixels.
pub fn paint_layer_icon(painter: &egui::Painter, rect: Rect, foreground: Color32, accent: Color32) {
    let side = rect.width().min(rect.height()) - 1.0;
    let cell = side / 3.0;
    let origin = rect.center() - vec2(side, side) * 0.5;
    for row in 0..3 {
        for column in 0..3 {
            let cell_rect = Rect::from_min_size(
                origin + vec2(column as f32 * cell, row as f32 * cell),
                vec2(cell - 0.6, cell - 0.6),
            );
            let color = if (row + column) % 2 == 0 {
                accent
            } else {
                foreground.gamma_multiply(0.45)
            };
            painter.rect_filled(cell_rect, 0.6, color);
        }
    }
}
