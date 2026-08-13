use egui::{Color32, ScrollArea, Ui};
use q0s_format::v2::{
    Asset, BlendMode, BlurFx, DropShadowFx, GlowFx, PlacementFx, Rgba, Stroke as VStroke,
};

use crate::app::{Action, EditorApp};
use crate::state::{BrushLibraryFilter, BrushSizePreview, Selection, Tool};

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    ui.heading("Properties");
    ui.separator();

    ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let timeline_tweens = crate::easing::selected_tween_targets(app);
            if !timeline_tweens.is_empty() {
                crate::easing::render_selected_tween_properties(app, ui, &timeline_tweens);
                return;
            }
            match app.session.selection.clone() {
                Selection::None => stage_properties(app, ui),
                Selection::Asset(id) => asset_properties(app, ui, id),
                Selection::Q0rg(id) => q0rg_properties(app, ui, id),
                Selection::Placement {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                } => placement_properties(app, ui, q0rg_id, layer_id, placement_idx),
                Selection::Path {
                    q0rg_id,
                    layer_id,
                    placement_idx,
                    path_idx,
                } => raw_path_properties(app, ui, q0rg_id, layer_id, placement_idx, path_idx),
                Selection::Paths(refs) => {
                    ui.label(egui::RichText::new(format!("{} contours", refs.len())).strong());
                    ui.label(
                        egui::RichText::new("Delete removes only these selected contours")
                            .color(app.settings.theme.text_dim.to_color32())
                            .small(),
                    );
                }
                Selection::PathPoints { anchor_indices, .. } => {
                    ui.label(
                        egui::RichText::new(format!(
                            "Partial fill: {} boundary points",
                            anchor_indices.len()
                        ))
                        .strong(),
                    );
                    ui.label(
                        egui::RichText::new("V drag reshapes only this selected area")
                            .color(app.settings.theme.text_dim.to_color32())
                            .small(),
                    );
                }
                Selection::RawArea {
                    placements,
                    objects,
                    ..
                } => {
                    ui.label(
                        egui::RichText::new(format!(
                            "Selected fill area across {} drawing(s) + {} object(s)",
                            placements.len(),
                            objects.len()
                        ))
                        .strong(),
                    );
                    ui.label(
                        egui::RichText::new("The original geometry is unchanged until you drag")
                            .color(app.settings.theme.text_dim.to_color32())
                            .small(),
                    );
                }
                Selection::Mixed { paths, objects } => {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} contours + {} objects",
                            paths.len(),
                            objects.len()
                        ))
                        .strong(),
                    );
                    ui.label(
                        egui::RichText::new("Free transform edits them as one group")
                            .color(app.settings.theme.text_dim.to_color32())
                            .small(),
                    );
                }
                Selection::Multi(refs) => {
                    ui.label(egui::RichText::new(format!("{} objects", refs.len())).strong());
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Multiple objects selected. Click one to edit it.")
                            .color(app.settings.theme.text_dim.to_color32())
                            .small(),
                    );
                }
            }

            // Tool style must remain available while artwork is selected. Selecting
            // the Rectangle tool used to leave the previous raw selection active,
            // which hid the fill colour control behind selection-only properties.
            render_tool_properties(app, ui);
        });
}

fn raw_path_properties(
    app: &EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    path_idx: usize,
) {
    let path = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
        .and_then(|layer| layer.placements.get(placement_idx))
        .and_then(|placement| match placement.target {
            q0s_format::v2::Target::Asset(asset_id) => app
                .state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == asset_id),
            q0s_format::v2::Target::Q0rg(_) => None,
        })
        .and_then(|asset| match asset {
            Asset::Vector(vector) => vector.paths.get(path_idx),
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) => None,
        });
    let Some(path) = path else {
        ui.label("Selection is no longer available");
        return;
    };
    ui.label(egui::RichText::new("Contour").strong());
    ui.label(format!("Contour {}", path_idx + 1));
    ui.label(format!("{} boundary points", path.anchors.len()));
    ui.label(if path.closed {
        "Filled / closed"
    } else {
        "Line / open"
    });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Drag to move this contour. Delete removes only this contour.")
            .color(app.settings.theme.text_dim.to_color32())
            .small(),
    );
}

fn stage_properties(app: &mut EditorApp, ui: &mut Ui) {
    ui.label(egui::RichText::new("Stage").strong());
    let mut dirty = false;
    let snapshot_before = app.state.project.clone();
    let mut wants_snapshot = false;
    egui::Grid::new("stage_props")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Name");
            let r = ui.text_edit_singleline(&mut app.state.project.meta.name);
            if r.gained_focus() {
                wants_snapshot = true;
            }
            if r.changed() {
                dirty = true;
            }
            ui.end_row();

            ui.label("Width");
            let r = ui.add(
                egui::DragValue::new(&mut app.state.project.meta.stage_width).clamp_range(1..=8192),
            );
            if r.drag_started() || r.gained_focus() {
                wants_snapshot = true;
            }
            if r.changed() {
                dirty = true;
            }
            ui.end_row();

            ui.label("Height");
            let r = ui.add(
                egui::DragValue::new(&mut app.state.project.meta.stage_height)
                    .clamp_range(1..=8192),
            );
            if r.drag_started() || r.gained_focus() {
                wants_snapshot = true;
            }
            if r.changed() {
                dirty = true;
            }
            ui.end_row();

            ui.label("FPS");
            let r =
                ui.add(egui::DragValue::new(&mut app.state.project.meta.fps).clamp_range(1..=240));
            if r.drag_started() || r.gained_focus() {
                wants_snapshot = true;
            }
            if r.changed() {
                dirty = true;
            }
            ui.end_row();
        });
    if wants_snapshot {
        app.history.snapshot(&snapshot_before);
    }
    if dirty {
        app.state.mark_dirty();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPropertyKind {
    Select,
    Hand,
    Subselect,
    Pen,
    Pencil,
    Brush,
    Eraser,
    Line,
    ClosedShape,
    Fill,
    Eyedropper,
    Rig,
}

fn tool_property_kind(tool: Tool) -> ToolPropertyKind {
    match tool {
        Tool::Select => ToolPropertyKind::Select,
        Tool::Hand => ToolPropertyKind::Hand,
        Tool::Subselect => ToolPropertyKind::Subselect,
        Tool::Pen => ToolPropertyKind::Pen,
        Tool::Pencil => ToolPropertyKind::Pencil,
        Tool::Brush => ToolPropertyKind::Brush,
        Tool::Eraser => ToolPropertyKind::Eraser,
        Tool::Line => ToolPropertyKind::Line,
        Tool::Rectangle | Tool::Oval => ToolPropertyKind::ClosedShape,
        Tool::Bucket => ToolPropertyKind::Fill,
        Tool::Eyedropper => ToolPropertyKind::Eyedropper,
        Tool::Rig => ToolPropertyKind::Rig,
    }
}

fn render_tool_properties(app: &mut EditorApp, ui: &mut Ui) {
    ui.add_space(10.0);
    ui.separator();
    ui.label(
        egui::RichText::new(app.session.current_tool.label())
            .strong()
            .size(15.0),
    );

    match tool_property_kind(app.session.current_tool) {
        ToolPropertyKind::Select => tool_hint(
            app,
            ui,
            "Select connected raw fills, partial areas, or display objects. Drag the selection to move it.",
        ),
        ToolPropertyKind::Hand => tool_hint(
            app,
            ui,
            "Drag the stage to pan. Hold Space from any other tool for a temporary hand.",
        ),
        ToolPropertyKind::Subselect => tool_hint(
            app,
            ui,
            "Edit individual vector anchors and handles. This tool has no drawing style.",
        ),
        ToolPropertyKind::Pen => stroke_tool_properties(app, ui, "Pen path", true, true),
        ToolPropertyKind::Pencil => {
            stroke_tool_properties(app, ui, "Freehand line", false, true)
        }
        ToolPropertyKind::Brush => brush_properties(app, ui),
        ToolPropertyKind::Eraser => eraser_properties(app, ui),
        ToolPropertyKind::Line => stroke_tool_properties(app, ui, "Straight line", false, true),
        ToolPropertyKind::ClosedShape => {
            stroke_tool_properties(app, ui, "Closed shape", true, false)
        }
        ToolPropertyKind::Fill => fill_tool_properties(app, ui),
        ToolPropertyKind::Eyedropper => tool_hint(
            app,
            ui,
            "Click visible vector paint to copy its fill or stroke settings.",
        ),
        ToolPropertyKind::Rig => crate::rigging::render_properties(app, ui),
    }
}

fn tool_hint(app: &EditorApp, ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .color(app.settings.theme.text_dim.to_color32())
            .small(),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrushLibrarySource {
    Builtin,
    Created,
}

impl BrushLibrarySource {
    const fn label(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Created => "created",
        }
    }
}

#[derive(Debug, Clone)]
struct BrushLibraryEntry {
    name: String,
    settings: crate::advanced_brush::AdvancedBrushSettings,
    source: BrushLibrarySource,
}

fn advanced_brush_library_entries(
    custom: &[crate::settings::AdvancedBrushPreset],
) -> Vec<BrushLibraryEntry> {
    crate::advanced_brush::builtin_presets()
        .into_iter()
        .map(|(name, settings)| BrushLibraryEntry {
            name: name.to_string(),
            settings,
            source: BrushLibrarySource::Builtin,
        })
        .chain(custom.iter().map(|preset| BrushLibraryEntry {
            name: preset.name.clone(),
            settings: preset.settings.to_runtime(),
            source: BrushLibrarySource::Created,
        }))
        .collect()
}

fn brush_library_entry_visible(filter: BrushLibraryFilter, source: BrushLibrarySource) -> bool {
    match filter {
        BrushLibraryFilter::All => true,
        BrushLibraryFilter::Builtin => source == BrushLibrarySource::Builtin,
        BrushLibraryFilter::Created => source == BrushLibrarySource::Created,
    }
}

fn render_advanced_brush_library(app: &mut EditorApp, ui: &mut Ui) {
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Brush library").strong());
        for filter in BrushLibraryFilter::ALL {
            ui.selectable_value(
                &mut app.session.advanced_brush_library_filter,
                filter,
                filter.label(),
            );
        }
    });

    let entries = advanced_brush_library_entries(&app.settings.advanced_brush_presets);
    let filter = app.session.advanced_brush_library_filter;
    let mut visible = 0usize;
    for entry in entries
        .iter()
        .filter(|entry| brush_library_entry_visible(filter, entry.source))
    {
        visible += 1;
        let selected = match entry.source {
            BrushLibrarySource::Builtin => {
                app.session.advanced_brush_selected_preset.is_none()
                    && app.session.advanced_brush_preset_name == entry.name
            }
            BrushLibrarySource::Created => app
                .session
                .advanced_brush_selected_preset
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(&entry.name)),
        };

        if advanced_brush_library_card(ui, entry, selected) {
            app.session.advanced_brush = entry.settings;
            app.session.advanced_brush_preset_name = entry.name.clone();
            app.session.advanced_brush_selected_preset = match entry.source {
                BrushLibrarySource::Builtin => None,
                BrushLibrarySource::Created => Some(entry.name.clone()),
            };
        }
    }

    if visible == 0 {
        ui.label(
            egui::RichText::new("No created brushes yet")
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
    }
    ui.add_space(4.0);
}

fn advanced_brush_library_card(ui: &mut Ui, entry: &BrushLibraryEntry, selected: bool) -> bool {
    let width = ui.available_width().max(120.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 54.0), egui::Sense::click());
    let visuals = ui.visuals();
    let widget = if selected {
        &visuals.widgets.active
    } else if response.hovered() {
        &visuals.widgets.hovered
    } else {
        &visuals.widgets.inactive
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 6.0, widget.bg_fill);
    painter.rect_stroke(rect, 6.0, widget.bg_stroke);

    painter.text(
        rect.left_top() + egui::vec2(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        &entry.name,
        egui::FontId::proportional(12.5),
        visuals.text_color(),
    );
    painter.text(
        rect.right_top() + egui::vec2(-8.0, 7.0),
        egui::Align2::RIGHT_TOP,
        entry.source.label(),
        egui::FontId::proportional(9.5),
        visuals.weak_text_color(),
    );

    let preview_rect = egui::Rect::from_min_max(
        rect.left_top() + egui::vec2(8.0, 25.0),
        rect.right_bottom() - egui::vec2(8.0, 6.0),
    );
    paint_advanced_brush_preview(ui, preview_rect, entry.settings);
    response.clicked()
}

fn paint_advanced_brush_preview(
    ui: &Ui,
    rect: egui::Rect,
    settings: crate::advanced_brush::AdvancedBrushSettings,
) {
    if rect.width() <= 1.0 || rect.height() <= 1.0 {
        return;
    }

    let mut preview_settings = settings;
    preview_settings.size = (4.5 + settings.size.sqrt()).clamp(5.0, 12.0);
    let mut time = 0.0f64;
    let sample_count = 34usize;
    let samples = (0..sample_count)
        .map(|index| {
            let t = index as f32 / (sample_count - 1) as f32;
            let x = egui::lerp(rect.left()..=rect.right(), t);
            let wave = (t * std::f32::consts::TAU * 1.15).sin() * rect.height() * 0.10;
            let wobble = (t * std::f32::consts::TAU * 6.0).sin() * rect.height() * 0.035;
            let y = rect.center().y + wave + wobble;
            if index > 0 {
                time += if t < 0.58 { 0.032 } else { 0.008 };
            }
            let pressure = 0.28 + 0.72 * (std::f32::consts::PI * t).sin().sqrt();
            crate::advanced_brush::AdvancedBrushSample {
                position: q0s_format::v2::Vec2::new(x, y),
                pressure: Some(pressure),
                time_seconds: time,
            }
        })
        .collect::<Vec<_>>();
    let stroke = crate::advanced_brush::AdvancedBrushStroke {
        samples,
        settings: preview_settings,
    };
    let dabs = crate::advanced_brush::advanced_dabs(&stroke);
    let base = ui.visuals().strong_text_color();
    let alpha = settings.color.a.max(96);
    let ink = Color32::from_rgba_premultiplied(base.r(), base.g(), base.b(), alpha);

    if settings.glow {
        let halo = Color32::from_rgba_premultiplied(base.r(), base.g(), base.b(), 34);
        let halo_scale = (1.4 + settings.glow_radius / settings.size.max(1.0) * 0.22).min(3.0);
        for dab in &dabs {
            paint_preview_dab(ui.painter(), *dab, halo, halo_scale);
        }
    }
    for dab in dabs {
        paint_preview_dab(ui.painter(), dab, ink, 1.0);
    }
}

fn paint_preview_dab(
    painter: &egui::Painter,
    dab: crate::advanced_brush::AdvancedDab,
    fill: Color32,
    scale: f32,
) {
    let cos_a = dab.angle_radians.cos();
    let sin_a = dab.angle_radians.sin();
    let points = (0..12)
        .map(|index| {
            let phase = std::f32::consts::TAU * index as f32 / 12.0;
            let local_x = phase.cos() * dab.major_radius * scale;
            let local_y = phase.sin() * dab.minor_radius * scale;
            egui::pos2(
                dab.center.x + local_x * cos_a - local_y * sin_a,
                dab.center.y + local_x * sin_a + local_y * cos_a,
            )
        })
        .collect();
    painter.add(egui::Shape::convex_polygon(
        points,
        fill,
        egui::Stroke::NONE,
    ));
}

fn update_brush_size_preview(
    app: &mut EditorApp,
    response: &egui::Response,
    kind: BrushSizePreview,
) {
    let active = response.is_pointer_button_down_on() || response.changed();
    if active {
        app.session.brush_size_preview = Some(kind);
    } else if app.session.brush_size_preview == Some(kind) {
        app.session.brush_size_preview = None;
    }
}

fn brush_properties(app: &mut EditorApp, ui: &mut Ui) {
    let mode_before = app.session.brush_mode;
    let classic_before = app.session.brush;
    let advanced_before = app.session.advanced_brush;
    let presets_before = app.settings.advanced_brush_presets.clone();

    ui.horizontal(|ui| {
        ui.label("Mode");
        for mode in crate::advanced_brush::BrushMode::ALL {
            ui.selectable_value(&mut app.session.brush_mode, mode, mode.label());
        }
    });
    ui.separator();

    match app.session.brush_mode {
        crate::advanced_brush::BrushMode::Classic => {
            tool_hint(
                app,
                ui,
                "Flash-style vector brush: continuous fill-only sweep and boundary smoothing, with optional pressure/speed size dynamics. No stabilizer, taper or raster materials.",
            );
            egui::Grid::new("classic_brush_settings")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Fill color");
                    let mut color = rgba_to_color32(app.session.brush.color);
                    if ui.color_edit_button_srgba(&mut color).changed() {
                        app.session.brush.color = color32_to_rgba(color);
                    }
                    ui.end_row();

                    ui.label("Opacity");
                    opacity_control(ui, &mut app.session.brush.color.a);
                    ui.end_row();

                    ui.label("Nib");
                    egui::ComboBox::from_id_source("classic_brush_nib")
                        .selected_text(app.session.brush.nib.label())
                        .show_ui(ui, |ui| {
                            for nib in crate::brush::BrushNib::ALL {
                                ui.selectable_value(&mut app.session.brush.nib, nib, nib.label());
                            }
                        });
                    ui.end_row();

                    ui.label("Size");
                    let size_response = ui.add(
                        egui::DragValue::new(&mut app.session.brush.size)
                            .speed(0.25)
                            .clamp_range(0.1..=512.0),
                    );
                    update_brush_size_preview(app, &size_response, BrushSizePreview::Size);
                    ui.end_row();

                    ui.label("Smoothing");
                    ui.add(egui::Slider::new(&mut app.session.brush.smoothing, 0..=100));
                    ui.end_row();

                    ui.label("Pressure size");
                    ui.checkbox(&mut app.session.brush.pressure_size, "");
                    ui.end_row();

                    ui.label("Speed size");
                    ui.checkbox(&mut app.session.brush.velocity_size, "");
                    ui.end_row();

                    let dynamics_enabled =
                        app.session.brush.pressure_size || app.session.brush.velocity_size;
                    ui.label("Sensitivity");
                    ui.add_enabled(
                        dynamics_enabled,
                        egui::Slider::new(&mut app.session.brush.dynamics_sensitivity, 0..=100),
                    );
                    ui.end_row();

                    ui.label("Minimum size");
                    let min_size_response = ui.add_enabled(
                        dynamics_enabled,
                        egui::Slider::new(&mut app.session.brush.dynamics_min_size, 0.01..=1.0),
                    );
                    update_brush_size_preview(
                        app,
                        &min_size_response,
                        BrushSizePreview::MinimumSize,
                    );
                    ui.end_row();

                    ui.label("Scale with stage");
                    ui.checkbox(&mut app.session.brush.scale_with_stage, "");
                    ui.end_row();

                    ui.label("Sync with eraser");
                    ui.checkbox(&mut app.session.brush.sync_with_eraser, "");
                    ui.end_row();
                });
        }
        crate::advanced_brush::BrushMode::Advanced => {
            tool_hint(
                app,
                ui,
                "Dynamic vector brush with a GPU-mesh live preview. Stabilizer, pressure, velocity, taper and tip dynamics are resolved into compact fill geometry; Glow is an optional material effect.",
            );
            ui.label(
                egui::RichText::new("GPU preview: OpenGL / eframe Glow")
                    .small()
                    .color(app.settings.theme.text_dim.to_color32()),
            );

            render_advanced_brush_library(app, ui);

            egui::Grid::new("advanced_brush_settings")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Color");
                    let mut color = rgba_to_color32(app.session.advanced_brush.color);
                    if ui.color_edit_button_srgba(&mut color).changed() {
                        app.session.advanced_brush.color = color32_to_rgba(color);
                    }
                    ui.end_row();

                    ui.label("Opacity");
                    opacity_control(ui, &mut app.session.advanced_brush.color.a);
                    ui.end_row();

                    ui.label("Size");
                    let size_response = ui.add(
                        egui::DragValue::new(&mut app.session.advanced_brush.size)
                            .speed(0.25)
                            .clamp_range(0.1..=1024.0),
                    );
                    update_brush_size_preview(app, &size_response, BrushSizePreview::Size);
                    ui.end_row();

                    ui.label("Smoothing");
                    ui.add(egui::Slider::new(
                        &mut app.session.advanced_brush.smoothing,
                        0..=100,
                    ));
                    ui.end_row();

                    ui.label("Stabilizer");
                    ui.add(egui::Slider::new(
                        &mut app.session.advanced_brush.stabilizer,
                        0..=100,
                    ));
                    ui.end_row();

                    ui.label("Roundness");
                    ui.add(egui::Slider::new(
                        &mut app.session.advanced_brush.roundness,
                        0.05..=1.0,
                    ));
                    ui.end_row();

                    ui.label("Angle");
                    ui.add(
                        egui::DragValue::new(&mut app.session.advanced_brush.angle_degrees)
                            .speed(0.5)
                            .clamp_range(-180.0..=180.0)
                            .suffix("°"),
                    );
                    ui.end_row();

                    ui.label("Auto angle");
                    ui.checkbox(&mut app.session.advanced_brush.auto_angle, "");
                    ui.end_row();

                    ui.label("Taper start");
                    ui.add(egui::Slider::new(
                        &mut app.session.advanced_brush.taper_start,
                        0.0..=0.95,
                    ));
                    ui.end_row();

                    ui.label("Taper end");
                    ui.add(egui::Slider::new(
                        &mut app.session.advanced_brush.taper_end,
                        0.0..=0.95,
                    ));
                    ui.end_row();

                    ui.label("Pressure size");
                    ui.checkbox(&mut app.session.advanced_brush.pressure_size, "");
                    ui.end_row();

                    ui.label("Pressure min size");
                    let min_size_response = ui.add_enabled(
                        app.session.advanced_brush.pressure_size,
                        egui::Slider::new(
                            &mut app.session.advanced_brush.pressure_min_size,
                            0.01..=1.0,
                        ),
                    );
                    update_brush_size_preview(
                        app,
                        &min_size_response,
                        BrushSizePreview::MinimumSize,
                    );
                    ui.end_row();

                    ui.label("Velocity size");
                    ui.add(egui::Slider::new(
                        &mut app.session.advanced_brush.velocity_size,
                        0.0..=1.0,
                    ));
                    ui.end_row();

                    #[cfg(feature = "appearance-mask-eraser")]
                    {
                        ui.label("Glow");
                        ui.checkbox(&mut app.session.advanced_brush.glow, "");
                        ui.end_row();

                        ui.label("Glow radius");
                        ui.add_enabled(
                            app.session.advanced_brush.glow,
                            egui::DragValue::new(&mut app.session.advanced_brush.glow_radius)
                                .speed(0.25)
                                .clamp_range(0.25..=256.0),
                        );
                        ui.end_row();

                        ui.label("Glow opacity");
                        ui.add_enabled(
                            app.session.advanced_brush.glow,
                            egui::Slider::new(
                                &mut app.session.advanced_brush.glow_opacity,
                                0.0..=1.0,
                            ),
                        );
                        ui.end_row();
                    }

                    ui.label("Scale with stage");
                    ui.checkbox(&mut app.session.advanced_brush.scale_with_stage, "");
                    ui.end_row();
                });

            ui.separator();
            ui.label("Save current brush");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut app.session.advanced_brush_preset_name)
                        .desired_width(120.0),
                );
                if ui.button("Save / update").clicked() {
                    let name: String = app
                        .session
                        .advanced_brush_preset_name
                        .trim()
                        .chars()
                        .take(48)
                        .collect();
                    if !name.is_empty() {
                        let settings = crate::settings::AdvancedBrushPreferences::from_runtime(
                            app.session.advanced_brush,
                        );
                        let original = app.session.advanced_brush_selected_preset.clone();
                        let existing_index = original
                            .as_deref()
                            .and_then(|original| {
                                app.settings
                                    .advanced_brush_presets
                                    .iter()
                                    .position(|preset| preset.name.eq_ignore_ascii_case(original))
                            })
                            .or_else(|| {
                                app.settings
                                    .advanced_brush_presets
                                    .iter()
                                    .position(|preset| preset.name.eq_ignore_ascii_case(&name))
                            });
                        if let Some(index) = existing_index {
                            app.settings.advanced_brush_presets[index].name = name.clone();
                            app.settings.advanced_brush_presets[index].settings = settings;
                            app.session.advanced_brush_selected_preset = Some(name);
                        } else if app.settings.advanced_brush_presets.len() < 64 {
                            app.settings.advanced_brush_presets.push(
                                crate::settings::AdvancedBrushPreset {
                                    name: name.clone(),
                                    settings,
                                },
                            );
                            app.session.advanced_brush_selected_preset = Some(name);
                        }
                    }
                }
                let can_delete = app.session.advanced_brush_selected_preset.is_some();
                if ui
                    .add_enabled(can_delete, egui::Button::new("Delete"))
                    .clicked()
                {
                    if let Some(original) = app.session.advanced_brush_selected_preset.take() {
                        app.settings
                            .advanced_brush_presets
                            .retain(|preset| !preset.name.eq_ignore_ascii_case(&original));
                    }
                }
            });
        }
    }

    app.session.advanced_brush = app.session.advanced_brush.sanitized();
    if app.session.brush_mode != mode_before
        || app.session.brush != classic_before
        || app.session.advanced_brush != advanced_before
        || app.settings.advanced_brush_presets != presets_before
    {
        app.settings.brush_mode = app.session.brush_mode.into();
        app.settings.brush.update_from_runtime(app.session.brush);
        app.settings
            .advanced_brush
            .update_from_runtime(app.session.advanced_brush);
        app.settings.save();
    }
}

fn eraser_properties(app: &mut EditorApp, ui: &mut Ui) {
    tool_hint(
        app,
        ui,
        "Subtracts one continuous circular coverage area from raw graphics.",
    );
    let brush_before = app.session.brush;
    egui::Grid::new("classic_eraser_settings")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Nib");
            ui.label("Circle");
            ui.end_row();

            ui.label("Sync with brush");
            ui.checkbox(&mut app.session.brush.sync_with_eraser, "");
            ui.end_row();

            ui.label("Size");
            if app.session.brush.sync_with_eraser {
                ui.add(
                    egui::DragValue::new(&mut app.session.brush.size)
                        .speed(0.25)
                        .clamp_range(0.1..=512.0),
                );
            } else {
                ui.add(
                    egui::DragValue::new(&mut app.session.eraser_size)
                        .speed(0.25)
                        .clamp_range(0.1..=512.0),
                );
            }
            ui.end_row();

            ui.label("Scale with stage");
            ui.checkbox(&mut app.session.brush.scale_with_stage, "");
            ui.end_row();
        });
    persist_brush_preferences(app, brush_before);
}

fn persist_brush_preferences(app: &mut EditorApp, before: crate::brush::BrushSettings) {
    if app.session.brush != before {
        app.settings.brush.update_from_runtime(app.session.brush);
        app.settings.save();
    }
}

fn stroke_tool_properties(
    app: &mut EditorApp,
    ui: &mut Ui,
    title: &str,
    supports_fill: bool,
    supports_cap: bool,
) {
    tool_hint(app, ui, title);
    egui::Grid::new(format!("tool_style_{:?}", app.session.current_tool))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Stroke color");
            let mut stroke = rgba_to_color32(app.session.stroke_color);
            if ui.color_edit_button_srgba(&mut stroke).changed() {
                app.session.stroke_color = color32_to_rgba(stroke);
            }
            ui.end_row();

            ui.label("Stroke opacity");
            opacity_control(ui, &mut app.session.stroke_color.a);
            ui.end_row();

            ui.label("Width");
            ui.add(
                egui::DragValue::new(&mut app.session.stroke_width)
                    .speed(0.1)
                    .clamp_range(0.1..=64.0),
            );
            ui.end_row();

            if supports_cap {
                cap_control(app, ui);
            }
            if supports_fill {
                optional_fill_control(app, ui);
            }
        });
}

fn cap_control(app: &mut EditorApp, ui: &mut Ui) {
    ui.label("Cap");
    let mut cap_idx = match app.session.brush_cap {
        q0s_format::geom::CapShape::Round => 0,
        q0s_format::geom::CapShape::Butt => 1,
    };
    let previous = cap_idx;
    egui::ComboBox::from_id_source(format!("cap_{:?}", app.session.current_tool))
        .selected_text(if cap_idx == 0 { "Round" } else { "Butt (flat)" })
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut cap_idx, 0, "Round");
            ui.selectable_value(&mut cap_idx, 1, "Butt (flat)");
        });
    if cap_idx != previous {
        app.session.brush_cap = if cap_idx == 0 {
            q0s_format::geom::CapShape::Round
        } else {
            q0s_format::geom::CapShape::Butt
        };
        app.settings.brush_cap = match app.session.brush_cap {
            q0s_format::geom::CapShape::Round => crate::settings::BrushCap::Round,
            q0s_format::geom::CapShape::Butt => crate::settings::BrushCap::Butt,
        };
        app.settings.save();
    }
    ui.end_row();
}

fn optional_fill_control(app: &mut EditorApp, ui: &mut Ui) {
    ui.label("Fill");
    let mut enabled = app.session.fill_color.is_some();
    if ui.checkbox(&mut enabled, "Enabled").changed() {
        set_optional_fill_enabled(&mut app.session.fill_color, enabled);
    }
    ui.end_row();

    if let Some(fill) = app.session.fill_color.as_mut() {
        ui.label("Fill color");
        let mut color = rgba_to_color32(*fill);
        if ui.color_edit_button_srgba(&mut color).changed() {
            *fill = color32_to_rgba(color);
        }
        ui.end_row();

        ui.label("Fill opacity");
        opacity_control(ui, &mut fill.a);
        ui.end_row();
    }
}

fn set_optional_fill_enabled(fill: &mut Option<Rgba>, enabled: bool) {
    match (enabled, fill.is_some()) {
        (true, false) => *fill = Some(default_fill_color()),
        (false, true) => *fill = None,
        _ => {}
    }
}

fn fill_tool_properties(app: &mut EditorApp, ui: &mut Ui) {
    tool_hint(
        app,
        ui,
        "Changes the connected fill region under the cursor.",
    );
    let mut fill = app.session.fill_color.unwrap_or_else(default_fill_color);
    egui::Grid::new("bucket_fill_settings")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Fill color");
            let mut color = rgba_to_color32(fill);
            if ui.color_edit_button_srgba(&mut color).changed() {
                fill = color32_to_rgba(color);
            }
            ui.end_row();

            ui.label("Opacity");
            opacity_control(ui, &mut fill.a);
            ui.end_row();
        });
    app.session.fill_color = Some(fill);
}

fn opacity_control(ui: &mut Ui, alpha: &mut u8) {
    let mut opacity = f32::from(*alpha) * 100.0 / 255.0;
    if ui
        .add(egui::Slider::new(&mut opacity, 0.0..=100.0).suffix("%"))
        .changed()
    {
        *alpha = (opacity * 255.0 / 100.0).round().clamp(0.0, 255.0) as u8;
    }
}

fn default_fill_color() -> Rgba {
    Rgba {
        r: 0xCC,
        g: 0xCC,
        b: 0xCC,
        a: 255,
    }
}

fn rgba_to_color32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

fn color32_to_rgba(c: Color32) -> Rgba {
    let [r, g, b, a] = c.to_array();
    Rgba { r, g, b, a }
}

fn asset_properties(app: &mut EditorApp, ui: &mut Ui, id: u16) {
    let Some(asset_idx) = app.state.project.assets.iter().position(|a| a.id() == id) else {
        ui.label("Selection is no longer available");
        return;
    };
    ui.label(egui::RichText::new("Asset").strong());

    let mut dirty = false;
    let asset = &mut app.state.project.assets[asset_idx];
    match asset {
        Asset::Bitmap(b) => {
            ui.label(format!("Bitmap {} x {}", b.width, b.height));
        }
        Asset::Q0v(v) => match q0video::q0v::Q0vFile::parse(v.bytes.clone()) {
            Ok(media) => {
                ui.label(format!("q0v {} x {}", media.spec.width, media.spec.height));
                ui.label(format!(
                    "{} frame(s) at {} fps",
                    media.spec.timeline_frames, media.spec.fps
                ));
                ui.label(if media.spec.audio {
                    format!(
                        "audio: {} hz / {} channel(s)",
                        media.spec.audio_sample_rate, media.spec.audio_channels
                    )
                } else {
                    "audio: none".to_string()
                });
            }
            Err(error) => {
                ui.label(format!("invalid q0v: {error}"));
            }
        },
        Asset::Rig(rig) => {
            ui.label(format!("Rig for q0rg {}", rig.owner_q0rg_id));
            ui.label(format!(
                "{} bones / {} controls / {} constraints",
                rig.nodes.len(),
                rig.controls.len(),
                rig.constraints.len()
            ));
        }
        Asset::Vector(v) => {
            ui.label(format!("Vector: {} path(s)", v.paths.len()));
            let total_anchors: usize = v.paths.iter().map(|p| p.anchors.len()).sum();
            ui.label(format!("Boundary points: {total_anchors}"));

            ui.add_space(4.0);
            egui::Grid::new(format!("asset_style_{id}"))
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Stroke");
                    let mut has_stroke = v.stroke.is_some();
                    if ui.checkbox(&mut has_stroke, "").changed() {
                        v.stroke = if has_stroke {
                            Some(VStroke {
                                color: Rgba {
                                    r: 0,
                                    g: 0,
                                    b: 0,
                                    a: 255,
                                },
                                width: 1.0,
                                cap: q0s_format::geom::CapShape::Round,
                            })
                        } else {
                            None
                        };
                        dirty = true;
                    }
                    if let Some(s) = v.stroke.as_mut() {
                        let mut c = rgba_to_color32(s.color);
                        if ui.color_edit_button_srgba(&mut c).changed() {
                            s.color = color32_to_rgba(c);
                            dirty = true;
                        }
                    }
                    ui.end_row();

                    if let Some(s) = v.stroke.as_mut() {
                        ui.label("Width");
                        if ui
                            .add(
                                egui::DragValue::new(&mut s.width)
                                    .speed(0.1)
                                    .clamp_range(0.1..=64.0),
                            )
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.end_row();

                        // Per-asset cap shape, persisted in project and export files.
                        ui.label("Cap");
                        let mut cap_idx = match s.cap {
                            q0s_format::geom::CapShape::Round => 0,
                            q0s_format::geom::CapShape::Butt => 1,
                        };
                        let prev = cap_idx;
                        egui::ComboBox::from_id_source(format!("asset_cap_{id}"))
                            .selected_text(if cap_idx == 0 { "Round" } else { "Butt (flat)" })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut cap_idx, 0, "Round");
                                ui.selectable_value(&mut cap_idx, 1, "Butt (flat)");
                            });
                        if cap_idx != prev {
                            s.cap = if cap_idx == 0 {
                                q0s_format::geom::CapShape::Round
                            } else {
                                q0s_format::geom::CapShape::Butt
                            };
                            dirty = true;
                        }
                        ui.end_row();
                    }

                    ui.label("Fill");
                    let mut has_fill = v.fill.is_some();
                    if ui.checkbox(&mut has_fill, "").changed() {
                        v.fill = if has_fill {
                            Some(Rgba {
                                r: 0xCC,
                                g: 0xCC,
                                b: 0xCC,
                                a: 255,
                            })
                        } else {
                            None
                        };
                        dirty = true;
                    }
                    if let Some(fill) = v.fill.as_mut() {
                        let mut c = rgba_to_color32(*fill);
                        if ui.color_edit_button_srgba(&mut c).changed() {
                            *fill = color32_to_rgba(c);
                            dirty = true;
                        }
                    }
                    ui.end_row();
                });
        }
    }

    if dirty {
        app.state.mark_dirty();
    }
}

fn q0rg_properties(app: &mut EditorApp, ui: &mut Ui, id: u16) {
    let Some(q0rg_idx) = app.state.project.q0rgs.iter().position(|q| q.q0rg_id == id) else {
        ui.label("Selection is no longer available");
        return;
    };
    let symbol_name = app.state.project.q0rgs[q0rg_idx].name.clone();
    ui.label(egui::RichText::new(format!("Symbol: {symbol_name}")).strong());

    let before = app.state.project.clone();
    let mut wants_snapshot = false;
    let mut dirty = false;
    let mut queued_open_q0rg_id: Option<u16> = None;
    let mut requested_frame_count: Option<u16> = None;
    {
        let q0rg = &mut app.state.project.q0rgs[q0rg_idx];
        egui::Grid::new("q0rg_props").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            let r = ui.text_edit_singleline(&mut q0rg.name);
            if r.gained_focus() {
                wants_snapshot = true;
            }
            if r.changed() {
                dirty = true;
            }
            ui.end_row();
            ui.label("Frames");
            let mut frame_count = q0rg.frame_count;
            let r = ui.add(egui::DragValue::new(&mut frame_count).clamp_range(1..=u16::MAX as i32));
            if r.changed() {
                requested_frame_count = Some(frame_count);
            }
            ui.end_row();
        });

        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Script (q0lang)")
                .color(app.settings.theme.text_dim.to_color32())
                .small(),
        );
        // Compact preview: line / char count + first non-empty line. The
        // full editing surface lives in its own resizable window opened
        // via the button below (or F9), so we don't cram a tiny
        // unhighlighted multiline TextEdit into the side panel.
        let line_count = q0rg.script.lines().count().max(1);
        let char_count = q0rg.script.chars().count();
        let preview_line = q0rg
            .script
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let preview = if preview_line.is_empty() {
            "<empty>".to_string()
        } else if preview_line.chars().count() > 48 {
            let mut s: String = preview_line.chars().take(45).collect();
            s.push_str("...");
            s
        } else {
            preview_line.to_string()
        };
        ui.label(
            egui::RichText::new(format!(
                "{line_count} line{}, {char_count} char{}",
                if line_count == 1 { "" } else { "s" },
                if char_count == 1 { "" } else { "s" },
            ))
            .small()
            .color(app.settings.theme.text_dim.to_color32()),
        );
        ui.label(
            egui::RichText::new(preview)
                .small()
                .italics()
                .color(app.settings.theme.text_dim.to_color32()),
        );
        let queued_open = ui
            .button("Open script editor...")
            .on_hover_text(
                "Full q0lang editor with syntax highlighting, line numbers, font picker (F9)",
            )
            .clicked();
        if queued_open {
            // Defer the action via the queue so we don't double-borrow `app`
            // (this fn already holds `&mut q0rg` from `app.state.project`).
            queued_open_q0rg_id = Some(id);
        }
    }

    if wants_snapshot {
        app.history.snapshot(&before);
    }
    if dirty {
        app.state.mark_dirty();
    }
    if let Some(id) = queued_open_q0rg_id {
        app.queue(Action::OpenQ0langEditor(Some(id)));
    }
    if let Some(frame_count) = requested_frame_count {
        app.queue(Action::SetQ0rgFrameCount(id, frame_count));
    }
}

fn blend_mode_name(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "Normal",
        BlendMode::Multiply => "Multiply",
        BlendMode::Screen => "Screen",
        BlendMode::Add => "Add",
        BlendMode::Overlay => "Overlay",
    }
}

fn note_property_response(response: &egui::Response, dirty: &mut bool, wants_snapshot: &mut bool) {
    if response.drag_started() || response.gained_focus() || response.changed() {
        *wants_snapshot = true;
    }
    if response.changed() {
        *dirty = true;
    }
}

fn placement_properties(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
) {
    let before = app.state.project.clone();
    let current_frame = app.session.current_frame;
    let Some((source_frame, mut transform, tween, mut fx)) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
        .and_then(|layer| {
            let source = layer.placements.get(placement_idx)?;
            let active = q0s_format::raster::active_placement_states_at(layer, current_frame)
                .into_iter()
                .find(|active| active.index == placement_idx)?;
            Some((source.frame, active.transform, source.tween, active.fx))
        })
    else {
        ui.label("Selection is no longer available");
        return;
    };

    let mut wants_snapshot = false;
    let mut transform_dirty = false;
    let mut fx_dirty = false;
    ui.label(egui::RichText::new("Placement").strong());
    if source_frame != current_frame {
        ui.label(
            egui::RichText::new(format!(
                "held from frame {}; editing creates a keyframe at {}",
                source_frame + 1,
                current_frame + 1
            ))
            .small()
            .color(app.settings.theme.text_dim.to_color32()),
        );
    }
    egui::Grid::new("placement_props")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Keyframe");
            ui.label(format!("{}", source_frame + 1));
            ui.end_row();

            let mut row =
                |ui: &mut Ui, label: &str, widget: egui::DragValue<'_>| -> egui::Response {
                    ui.label(label);
                    let response = ui.add(widget);
                    if response.drag_started() || response.gained_focus() {
                        wants_snapshot = true;
                    }
                    if response.changed() {
                        transform_dirty = true;
                    }
                    ui.end_row();
                    response
                };
            row(ui, "X", egui::DragValue::new(&mut transform.tx).speed(0.5));
            row(ui, "Y", egui::DragValue::new(&mut transform.ty).speed(0.5));
            row(
                ui,
                "Scale X",
                egui::DragValue::new(&mut transform.sx)
                    .speed(0.01)
                    .clamp_range(0.01..=64.0),
            );
            row(
                ui,
                "Scale Y",
                egui::DragValue::new(&mut transform.sy)
                    .speed(0.01)
                    .clamp_range(0.01..=64.0),
            );
            let mut rotation_degrees = transform.rotation.to_degrees();
            let rotation_response = row(
                ui,
                "Rotation (deg)",
                egui::DragValue::new(&mut rotation_degrees).speed(1.0),
            );
            if rotation_response.changed() {
                transform.rotation = rotation_degrees.to_radians();
            }
        });

    crate::easing::render_tween_properties(app, ui, q0rg_id, layer_id, placement_idx, tween);

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("FX").strong());
        let reset = ui.add_enabled(!fx.is_identity(), egui::Button::new("Reset"));
        if reset.clicked() {
            fx = PlacementFx::default();
            fx_dirty = true;
            wants_snapshot = true;
        }
    });

    egui::Grid::new("placement_fx_global")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Alpha");
            let mut percent = fx.opacity * 100.0;
            let response = ui.add(
                egui::Slider::new(&mut percent, 0.0..=100.0)
                    .suffix("%")
                    .show_value(true),
            );
            note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
            if response.changed() {
                fx.opacity = (percent / 100.0).clamp(0.0, 1.0);
            }
            ui.end_row();

            ui.label("Blend");
            let blend_before = fx.blend_mode;
            egui::ComboBox::from_id_source("placement_fx_blend")
                .selected_text(blend_mode_name(fx.blend_mode))
                .show_ui(ui, |ui| {
                    for mode in [
                        BlendMode::Normal,
                        BlendMode::Multiply,
                        BlendMode::Screen,
                        BlendMode::Add,
                        BlendMode::Overlay,
                    ] {
                        ui.selectable_value(&mut fx.blend_mode, mode, blend_mode_name(mode));
                    }
                });
            if fx.blend_mode != blend_before {
                fx_dirty = true;
                wants_snapshot = true;
            }
            ui.end_row();
        });

    ui.add_space(4.0);
    ui.label(egui::RichText::new("Filters").small().strong());

    let mut glow_enabled = fx.glow.is_some();
    let glow_toggle = ui.checkbox(&mut glow_enabled, "Glow");
    if glow_toggle.changed() {
        fx.glow = glow_enabled.then_some(GlowFx {
            color: Rgba {
                r: 255,
                g: 32,
                b: 24,
                a: 255,
            },
            radius: 8.0,
            strength: 1.0,
        });
        fx_dirty = true;
        wants_snapshot = true;
    }
    if let Some(glow) = fx.glow.as_mut() {
        ui.indent("placement_glow", |ui| {
            egui::Grid::new("placement_glow_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Color");
                    let mut color = rgba_to_color32(glow.color);
                    let response = ui.color_edit_button_srgba(&mut color);
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    if response.changed() {
                        glow.color = color32_to_rgba(color);
                    }
                    ui.end_row();

                    ui.label("Radius");
                    let response = ui.add(
                        egui::DragValue::new(&mut glow.radius)
                            .speed(0.25)
                            .clamp_range(0.0..=256.0)
                            .suffix(" px"),
                    );
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    ui.end_row();

                    ui.label("Strength");
                    let response = ui.add(
                        egui::Slider::new(&mut glow.strength, 0.0..=4.0)
                            .suffix("?")
                            .show_value(true),
                    );
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    ui.end_row();
                });
        });
    }

    let mut blur_enabled = fx.blur.is_some();
    let blur_toggle = ui.checkbox(&mut blur_enabled, "Blur");
    if blur_toggle.changed() {
        fx.blur = blur_enabled.then_some(BlurFx { radius: 4.0 });
        fx_dirty = true;
        wants_snapshot = true;
    }
    if let Some(blur) = fx.blur.as_mut() {
        ui.indent("placement_blur", |ui| {
            ui.horizontal(|ui| {
                ui.label("Radius");
                let response = ui.add(
                    egui::DragValue::new(&mut blur.radius)
                        .speed(0.25)
                        .clamp_range(0.0..=256.0)
                        .suffix(" px"),
                );
                note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
            });
        });
    }

    let mut shadow_enabled = fx.shadow.is_some();
    let shadow_toggle = ui.checkbox(&mut shadow_enabled, "Drop shadow");
    if shadow_toggle.changed() {
        fx.shadow = shadow_enabled.then_some(DropShadowFx {
            color: Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 180,
            },
            blur_radius: 6.0,
            offset_x: 4.0,
            offset_y: 4.0,
            strength: 1.0,
        });
        fx_dirty = true;
        wants_snapshot = true;
    }
    if let Some(shadow) = fx.shadow.as_mut() {
        ui.indent("placement_shadow", |ui| {
            egui::Grid::new("placement_shadow_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Color");
                    let mut color = rgba_to_color32(shadow.color);
                    let response = ui.color_edit_button_srgba(&mut color);
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    if response.changed() {
                        shadow.color = color32_to_rgba(color);
                    }
                    ui.end_row();

                    ui.label("Blur");
                    let response = ui.add(
                        egui::DragValue::new(&mut shadow.blur_radius)
                            .speed(0.25)
                            .clamp_range(0.0..=256.0)
                            .suffix(" px"),
                    );
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    ui.end_row();

                    ui.label("Offset X");
                    let response = ui.add(egui::DragValue::new(&mut shadow.offset_x).speed(0.25));
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    ui.end_row();

                    ui.label("Offset Y");
                    let response = ui.add(egui::DragValue::new(&mut shadow.offset_y).speed(0.25));
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    ui.end_row();

                    ui.label("Strength");
                    let response = ui.add(
                        egui::Slider::new(&mut shadow.strength, 0.0..=4.0)
                            .suffix("?")
                            .show_value(true),
                    );
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    ui.end_row();
                });
        });
    }

    ui.separator();
    let break_apart_fx_block = app.break_apart_fx_block_reason();
    let break_apart = ui.add_enabled(
        app.can_break_apart_selection(),
        egui::Button::new("Break Apart  (Ctrl+B)"),
    );
    let break_apart = if let Some(reason) = break_apart_fx_block {
        break_apart.on_hover_text(reason)
    } else {
        break_apart
    };
    if break_apart.clicked() {
        app.queue(Action::BreakApartSelection);
    }
    if let Some(reason) = break_apart_fx_block {
        ui.label(
            egui::RichText::new(reason)
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
    }

    if !transform_dirty && !fx_dirty {
        return;
    }
    if wants_snapshot {
        app.history.snapshot(&before);
    }
    if let Some(new_idx) = apply_placement_edit_at_frame(
        app,
        q0rg_id,
        layer_id,
        placement_idx,
        current_frame,
        transform,
        fx,
    ) {
        app.session.selection = Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx: new_idx,
        };
        app.state.mark_dirty();
    }
}

fn apply_placement_edit_at_frame(
    app: &mut EditorApp,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    frame: u16,
    transform: q0s_format::v2::Transform2D,
    fx: PlacementFx,
) -> Option<usize> {
    let new_idx = crate::tools::materialize_placement_keyframe_for_edit(
        &mut app.state.project,
        q0rg_id,
        layer_id,
        placement_idx,
        frame,
    )?;
    let placement = app
        .state
        .project
        .q0rgs
        .iter_mut()
        .find(|q| q.q0rg_id == q0rg_id)?
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == layer_id)?
        .placements
        .get_mut(new_idx)?;
    placement.transform = transform;
    placement.fx = fx;
    Some(new_idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brush_library_unifies_builtin_and_created_entries_with_source_filters() {
        let custom = vec![crate::settings::AdvancedBrushPreset {
            name: "My Needle".to_string(),
            settings: crate::settings::AdvancedBrushPreferences::from_runtime(
                crate::advanced_brush::AdvancedBrushSettings {
                    size: 7.0,
                    roundness: 0.2,
                    ..Default::default()
                },
            ),
        }];
        let entries = advanced_brush_library_entries(&custom);
        let builtin_count = crate::advanced_brush::builtin_presets().len();

        assert_eq!(entries.len(), builtin_count + 1);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| brush_library_entry_visible(
                    BrushLibraryFilter::Builtin,
                    entry.source
                ))
                .count(),
            builtin_count
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| brush_library_entry_visible(
                    BrushLibraryFilter::Created,
                    entry.source
                ))
                .count(),
            1
        );
        let created = entries
            .iter()
            .find(|entry| entry.source == BrushLibrarySource::Created)
            .expect("created brush must be present in the unified library");
        assert_eq!(created.name, "My Needle");
        assert_eq!(created.settings.size, 7.0);
        assert_eq!(created.settings.roundness, 0.2);
    }

    #[test]
    fn properties_transform_on_held_object_creates_current_keyframe() {
        let mut app = EditorApp::default();
        app.state.project.q0rgs[0].frame_count = 12;
        app.state.project.q0rgs[0].layers[0].placements = vec![q0s_format::v2::Placement {
            instance_id: 0,
            frame: 0,
            target: q0s_format::v2::Target::Asset(77),
            transform: q0s_format::v2::Transform2D::IDENTITY,
            tween: q0s_format::v2::Tween::None,
            fx: Default::default(),
        }];
        let changed = q0s_format::v2::Transform2D {
            tx: 42.0,
            ty: 13.0,
            ..q0s_format::v2::Transform2D::IDENTITY
        };

        let changed_fx = PlacementFx {
            opacity: 0.6,
            glow: Some(GlowFx {
                color: Rgba {
                    r: 255,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                radius: 8.0,
                strength: 1.0,
            }),
            ..Default::default()
        };
        let new_idx = apply_placement_edit_at_frame(&mut app, 1, 1, 0, 5, changed, changed_fx)
            .expect("properties edit must create keyframe");

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(new_idx, 1);
        assert_eq!(layer.placements[0].frame, 0);
        assert_eq!(
            layer.placements[0].transform,
            q0s_format::v2::Transform2D::IDENTITY
        );
        assert_eq!(layer.placements[1].frame, 5);
        assert_eq!(layer.placements[1].transform, changed);
        assert_eq!(layer.placements[1].fx, changed_fx);
        assert!(layer.placements[0].fx.is_identity());
    }

    fn collect_painted_text(shape: &egui::epaint::Shape, text: &mut String) {
        match shape {
            egui::epaint::Shape::Text(label) => {
                text.push_str(&label.galley.job.text);
                text.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_painted_text(shape, text);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn selected_raw_graphics_do_not_hide_closed_shape_fill_controls() {
        let context = egui::Context::default();
        let mut app = EditorApp::default();
        app.session.current_tool = Tool::Rectangle;
        app.session.fill_color = Some(default_fill_color());
        app.session.selection = Selection::Path {
            q0rg_id: u16::MAX,
            layer_id: u16::MAX,
            placement_idx: usize::MAX,
            path_idx: usize::MAX,
        };

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(420.0, 900.0),
            )),
            ..Default::default()
        };
        let _first_frame = context.run(input.clone(), |context| {
            egui::CentralPanel::default().show(context, |ui| render(&mut app, ui));
        });
        let output = context.run(input, |context| {
            egui::CentralPanel::default().show(context, |ui| render(&mut app, ui));
        });
        let mut painted_text = String::new();
        for clipped in &output.shapes {
            collect_painted_text(&clipped.shape, &mut painted_text);
        }

        assert!(painted_text.contains("Rectangle"));
        assert!(painted_text.contains("Fill color"));
        assert!(painted_text.contains("Fill opacity"));
    }

    #[test]
    fn optional_fill_toggle_enables_a_visible_default_and_can_disable_it() {
        let mut fill = None;
        set_optional_fill_enabled(&mut fill, true);
        assert_eq!(fill, Some(default_fill_color()));
        set_optional_fill_enabled(&mut fill, false);
        assert_eq!(fill, None);
    }

    #[test]
    fn optional_fill_toggle_does_not_replace_an_existing_colour() {
        let chosen = Rgba {
            r: 12,
            g: 34,
            b: 56,
            a: 78,
        };
        let mut fill = Some(chosen);
        set_optional_fill_enabled(&mut fill, true);
        assert_eq!(fill, Some(chosen));
    }

    #[test]
    fn every_tool_gets_a_specific_properties_kind() {
        let kinds = [
            tool_property_kind(Tool::Select),
            tool_property_kind(Tool::Subselect),
            tool_property_kind(Tool::Pen),
            tool_property_kind(Tool::Pencil),
            tool_property_kind(Tool::Brush),
            tool_property_kind(Tool::Eraser),
            tool_property_kind(Tool::Line),
            tool_property_kind(Tool::Rectangle),
            tool_property_kind(Tool::Bucket),
            tool_property_kind(Tool::Eyedropper),
            tool_property_kind(Tool::Rig),
        ];
        for first in 0..kinds.len() {
            for second in (first + 1)..kinds.len() {
                assert_ne!(kinds[first], kinds[second]);
            }
        }
        assert_eq!(
            tool_property_kind(Tool::Rectangle),
            tool_property_kind(Tool::Oval),
            "rectangle and oval intentionally share the closed-shape panel"
        );
    }
}
