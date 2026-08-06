use egui::{Color32, ScrollArea, Ui};
use q0s_format::v2::{Asset, Rgba, Stroke as VStroke};

use crate::app::{Action, EditorApp};
use crate::state::{Selection, Tool};

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
            Asset::Bitmap(_) | Asset::Q0v(_) => None,
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
    }
}

fn tool_hint(app: &EditorApp, ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .color(app.settings.theme.text_dim.to_color32())
            .small(),
    );
}

fn brush_properties(app: &mut EditorApp, ui: &mut Ui) {
    tool_hint(
        app,
        ui,
        "Static fill nib. The chosen pen shape keeps one fixed angle; smoothing is applied after the gesture.",
    );
    let brush_before = app.session.brush;
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
            ui.add(
                egui::DragValue::new(&mut app.session.brush.size)
                    .speed(0.25)
                    .clamp_range(0.1..=512.0),
            );
            ui.end_row();

            ui.label("Smoothing");
            ui.add(egui::Slider::new(&mut app.session.brush.smoothing, 0..=100));
            ui.end_row();

            ui.label("Scale with stage");
            ui.checkbox(&mut app.session.brush.scale_with_stage, "");
            ui.end_row();

            ui.label("Sync with eraser");
            ui.checkbox(&mut app.session.brush.sync_with_eraser, "");
            ui.end_row();
        });
    persist_brush_preferences(app, brush_before);
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

fn placement_properties(
    app: &mut EditorApp,
    ui: &mut Ui,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
) {
    let before = app.state.project.clone();
    let current_frame = app.session.current_frame;
    let Some((source_frame, mut transform, tween)) = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
        .and_then(|layer| {
            let source = layer.placements.get(placement_idx)?;
            let active =
                crate::render::active_transform_for_placement(layer, placement_idx, current_frame)?;
            Some((source.frame, active, source.tween))
        })
    else {
        ui.label("Selection is no longer available");
        return;
    };

    let mut wants_snapshot = false;
    let mut transform_dirty = false;
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
    if ui
        .add_enabled(
            app.can_break_apart_selection(),
            egui::Button::new("Break Apart  (Ctrl+B)"),
        )
        .clicked()
    {
        app.queue(Action::BreakApartSelection);
    }

    if !transform_dirty {
        return;
    }
    if wants_snapshot {
        app.history.snapshot(&before);
    }
    if let Some(new_idx) = apply_placement_transform_at_frame(
        app,
        q0rg_id,
        layer_id,
        placement_idx,
        current_frame,
        transform,
    ) {
        app.session.selection = Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx: new_idx,
        };
        app.state.mark_dirty();
    }
}

fn apply_placement_transform_at_frame(
    app: &mut EditorApp,
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    frame: u16,
    transform: q0s_format::v2::Transform2D,
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
    Some(new_idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_transform_on_held_object_creates_current_keyframe() {
        let mut app = EditorApp::default();
        app.state.project.q0rgs[0].frame_count = 12;
        app.state.project.q0rgs[0].layers[0].placements = vec![q0s_format::v2::Placement {
            frame: 0,
            target: q0s_format::v2::Target::Asset(77),
            transform: q0s_format::v2::Transform2D::IDENTITY,
            tween: q0s_format::v2::Tween::None,
        }];
        let changed = q0s_format::v2::Transform2D {
            tx: 42.0,
            ty: 13.0,
            ..q0s_format::v2::Transform2D::IDENTITY
        };

        let new_idx = apply_placement_transform_at_frame(&mut app, 1, 1, 0, 5, changed)
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
