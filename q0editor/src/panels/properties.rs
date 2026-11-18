use egui::{Color32, ScrollArea, Ui};
use q0s_format::raster::ActivePlacementState;
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Asset, BlendMode, BlurFx, DropShadowFx, GlowFx, GradientFill, Placement, PlacementFx,
    ProjectV2, Q0rgPlayback, Q0rgPlaybackMode, Rgba, Stroke as VStroke, Target, VectorAppearance,
    VectorMaterial,
};

use crate::app::{Action, EditorApp};
use crate::color_picker::PaintChange;

/// The paint box for a tool's fill: its colour and gradient.
fn tool_paint_box(
    ui: &mut Ui,
    color: &mut Rgba,
    gradient: &mut Option<crate::paint::PaintGradient>,
) {
    let mut shown = rgba_to_color32(*color);
    let (_, change) = crate::color_picker::paint_edit_button(ui, &mut shown, gradient);
    if change == PaintChange::Color {
        *color = color32_to_rgba(shown);
    }
    if let Some(first) = gradient.as_ref().and_then(|g| g.stops.first()) {
        // the drawing's own colour follows the gradient's first stop, so a
        // gradient turned off later leaves a colour from it
        if change == PaintChange::Stops || change == PaintChange::Shape {
            *color = first.color;
        }
    }
}
use crate::state::{BrushLibraryFilter, BrushSizePreview, PlacementRef, Selection, Tool};

/// The Properties panel's pages, as tabs along its top. The panel brings
/// forward the page of what was just worked with: picking something shows
/// its page, picking a tool the tool's, clicking away the stage's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    Selection,
    Tool,
    #[default]
    Stage,
}

/// What the panel saw last frame, to tell what was just touched.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    selection: Selection,
    tool: Tool,
    /// The selection page's name, `None` when nothing is selected.
    context: Option<&'static str>,
}

/// The page to show this frame.
fn follow_focus(stored: Option<Tab>, before: Option<&Seen>, now: &Seen) -> Tab {
    // With a drawing tool in hand its page comes first; with a pointer, the
    // selection's, or the stage's when nothing is picked.
    let resting = if !pointer_tool(now.tool) {
        Tab::Tool
    } else if now.context.is_some() {
        Tab::Selection
    } else {
        Tab::Stage
    };
    let mut tab = stored.unwrap_or(if now.context.is_some() && selection_takes_focus(now) {
        Tab::Selection
    } else {
        resting
    });
    if let Some(before) = before {
        if before.tool != now.tool {
            tab = Tab::Tool;
        }
        if before.selection != now.selection || before.context != now.context {
            if now.context.is_some() {
                if selection_takes_focus(now) {
                    tab = Tab::Selection;
                }
            } else if tab == Tab::Selection {
                tab = resting;
            }
        }
    }
    if tab == Tab::Selection && now.context.is_none() {
        tab = resting;
    }
    tab
}

/// Tools that pick and move things rather than draw them.
fn pointer_tool(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::Select | Tool::Subselect | Tool::Lasso | Tool::Hand
    )
}

/// Whether a new selection is something the user picked, rather than the
/// stroke a drawing tool just left selected: that stays on the tool's page.
fn selection_takes_focus(now: &Seen) -> bool {
    matches!(
        now.tool,
        Tool::Select | Tool::Subselect | Tool::Lasso | Tool::Hand | Tool::Rig | Tool::Text
    ) || matches!(now.selection, Selection::Q0rg(_) | Selection::Asset(_))
        || matches!(now.context, Some("Camera" | "Sound" | "Tween"))
}

/// The name of the selection's page, or `None` with nothing selected.
fn selection_label(app: &EditorApp) -> Option<&'static str> {
    if crate::text_tool::has_text_selection(app) {
        return Some("Text");
    }
    if app.camera_track_is_selected() {
        return Some("Camera");
    }
    let audio = app.session.timeline_selection.is_some_and(|selection| {
        crate::audio::audio_clip_at_frame(
            &app.state.project,
            app.session.current_q0rg_id,
            selection.focus_layer_id,
            selection.focus_frame,
        )
        .is_some()
    });
    if audio {
        return Some("Sound");
    }
    if !crate::easing::selected_tween_targets(app).is_empty() {
        return Some("Tween");
    }
    Some(match &app.session.selection {
        Selection::None => return None,
        Selection::Asset(_) => "Asset",
        Selection::Q0rg(_) => "Symbol",
        Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } => {
            let symbol = app
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == *q0rg_id)
                .and_then(|q| q.layers.iter().find(|l| l.layer_id == *layer_id))
                .and_then(|l| l.placements.get(*placement_idx))
                .is_some_and(|p| matches!(p.target, Target::Q0rg(_)));
            if symbol {
                "Symbol"
            } else {
                "Object"
            }
        }
        Selection::Multi(_) => "Objects",
        _ => "Shape",
    })
}

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    crate::ui::inspector_style(ui);
    let tab_id = egui::Id::new("properties_tab");
    let seen_id = egui::Id::new("properties_seen");
    let seen = Seen {
        selection: app.session.selection.clone(),
        tool: app.session.current_tool,
        context: selection_label(app),
    };
    let (stored, before) = ui
        .ctx()
        .data(|d| (d.get_temp::<Tab>(tab_id), d.get_temp::<Seen>(seen_id)));
    let mut tab = follow_focus(stored, before.as_ref(), &seen);
    let mut tabs = Vec::with_capacity(3);
    if let Some(label) = seen.context {
        tabs.push((Tab::Selection, label));
    }
    tabs.push((Tab::Tool, app.session.current_tool.label()));
    tabs.push((Tab::Stage, "Stage"));
    tab_strip(ui, &tabs, &mut tab);
    ui.ctx().data_mut(|d| {
        d.insert_temp(tab_id, tab);
        d.insert_temp(seen_id, seen);
    });
    ScrollArea::both()
        .id_source(("properties_scroll", tab as u8))
        .auto_shrink([false, true])
        .show(ui, |ui| match tab {
            Tab::Selection => selection_tab(app, ui),
            Tab::Tool => render_tool_properties(app, ui),
            Tab::Stage => stage_properties(app, ui),
        });
}

/// The pages' tabs, sharing the panel's width, worn like the tool buttons.
fn tab_strip(ui: &mut Ui, tabs: &[(Tab, &str)], current: &mut Tab) {
    const GAP: f32 = 4.0;
    let count = tabs.len().max(1) as f32;
    let width = ((ui.available_width() - GAP * (count - 1.0)) / count).max(24.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for &(tab, label) in tabs {
            let selected = *current == tab;
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, selected, label)
            });
            if ui.is_rect_visible(rect) {
                crate::ui::paint_control(ui, &response, rect, 6.0, selected);
                let color = if selected {
                    ui.visuals().strong_text_color()
                } else {
                    ui.visuals().text_color()
                };
                let font = egui::FontId::proportional(13.0);
                let galley = ui
                    .painter()
                    .layout_no_wrap(label.to_owned(), font.clone(), color);
                if galley.size().x <= rect.width() - 12.0 {
                    ui.painter()
                        .galley(rect.center() - galley.size() * 0.5, galley, color);
                } else {
                    crate::ui::clipped_label(
                        ui.painter(),
                        rect.shrink2(egui::vec2(6.0, 0.0)),
                        label,
                        font,
                        color,
                    );
                }
            }
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                *current = tab;
            }
        }
    });
    ui.add_space(4.0);
}

/// The page of what is selected: a symbol, shapes, text, a tween…
fn selection_tab(app: &mut EditorApp, ui: &mut Ui) {
    if crate::text_tool::has_text_selection(app) {
        crate::text_tool::render_properties(app, ui);
        if app.session.text_editor.is_none() {
            if let Selection::Placement {
                q0rg_id,
                layer_id,
                placement_idx,
            } = app.session.selection
            {
                ui.separator();
                placement_properties(app, ui, q0rg_id, layer_id, placement_idx);
            }
        }
        return;
    }
    if app.camera_track_is_selected() {
        let camera_tweens = crate::easing::selected_tween_targets(app);
        if !camera_tweens.is_empty() {
            crate::easing::render_selected_tween_properties(app, ui, &camera_tweens);
        } else {
            render_camera_properties(app, ui);
        }
        return;
    }
    if render_audio_clip_properties(app, ui) {
        return;
    }
    if let Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = app.session.selection
    {
        let symbol = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
            .and_then(|l| l.placements.get(placement_idx))
            .and_then(|p| match p.target {
                Target::Q0rg(id) => app.state.project.q0rgs.iter().find(|q| q.q0rg_id == id),
                _ => None,
            });
        if let Some(symbol) = symbol {
            crate::ui::section_header(ui, &symbol.name);
        }
    }
    let timeline_tweens = crate::easing::selected_tween_targets(app);
    if !timeline_tweens.is_empty() {
        crate::easing::render_selected_tween_properties(app, ui, &timeline_tweens);
        return;
    }
    match app.session.selection.clone() {
        Selection::None => {}
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
        } => {
            raw_path_properties(app, ui, q0rg_id, layer_id, placement_idx, path_idx);
            selected_stroke_properties(app, ui);
        }
        Selection::Paths(refs) => {
            crate::ui::section_header(ui, &format!("{} contours", refs.len()));
            ui.label(
                egui::RichText::new("Delete removes only these selected contours")
                    .color(app.settings.theme.text_dim.to_color32())
                    .small(),
            );
            selected_stroke_properties(app, ui);
        }
        Selection::StrokeSegment(segment) => {
            crate::ui::section_header(ui, "Stroke segment");
            ui.label(
                egui::RichText::new(format!("Segment {}", segment.segment_idx + 1))
                    .color(app.settings.theme.text_dim.to_color32())
                    .small(),
            );
            ui.label(
                egui::RichText::new("Drag to detach this part of the outline")
                    .color(app.settings.theme.text_dim.to_color32())
                    .small(),
            );
            selected_stroke_properties(app, ui);
        }
        Selection::RawParts {
            fills,
            strokes,
            stroke_segments,
            objects,
        } => {
            ui.label(
                egui::RichText::new(format!(
                    "{} fill part(s) + {} outline(s) + {} outline segment(s) + {} object(s)",
                    fills.len(),
                    strokes.len(),
                    stroke_segments.len(),
                    objects.len()
                ))
                .strong(),
            );
            ui.label(
                egui::RichText::new("Drag to detach only the selected raw parts")
                    .color(app.settings.theme.text_dim.to_color32())
                    .small(),
            );
            if !fills.is_empty() || !strokes.is_empty() || !stroke_segments.is_empty() {
                selected_stroke_properties(app, ui);
            }
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
        Selection::SubselectPoints {
            path,
            anchor_indices,
            ..
        } => {
            let more: Vec<usize> = app
                .session
                .subselect_more
                .iter()
                .filter(|(other, _)| *other != path)
                .map(|(_, indices)| indices.len())
                .collect();
            let points = anchor_indices.len() + more.iter().sum::<usize>();
            ui.label(
                egui::RichText::new(if more.is_empty() {
                    format!("Subselect: {points} vector point(s)")
                } else {
                    format!(
                        "Subselect: {points} vector point(s) on {} paths",
                        more.len() + 1
                    )
                })
                .strong(),
            );
            ui.label(
                egui::RichText::new("Switching to Select transforms the whole raw path")
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
            selected_stroke_properties(app, ui);
        }
        Selection::Multi(refs) => {
            crate::ui::section_header(ui, &format!("{} objects", refs.len()));
        }
    }

    let selected_objects = match &app.session.selection {
        Selection::Multi(objects)
        | Selection::Mixed { objects, .. }
        | Selection::RawParts { objects, .. }
        | Selection::RawArea { objects, .. } => objects.clone(),
        _ => Vec::new(),
    };
    if !selected_objects.is_empty() {
        multiple_placement_properties(app, ui, &selected_objects);
    }
}

fn render_camera_properties(app: &mut EditorApp, ui: &mut Ui) {
    let frame = app.session.current_frame;
    let before = app.state.project.clone();
    let mut state = app
        .state
        .project
        .camera_state_in(app.session.current_q0rg_id, frame);
    let has_exact_key = app.camera().has_keyframe(frame);
    let mut changed = false;
    let mut wants_snapshot = false;

    crate::ui::section_header(ui, "Camera");
    ui.label(
        egui::RichText::new(if has_exact_key {
            format!("keyframe {}", frame + 1)
        } else {
            format!("held/interpolated state at frame {}", frame + 1)
        })
        .small()
        .color(app.settings.theme.text_dim.to_color32()),
    );
    ui.add_space(6.0);

    egui::Grid::new("camera_properties")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("X");
            let response = ui.add(egui::DragValue::new(&mut state.center_x).speed(1.0));
            wants_snapshot |= response.drag_started() || response.gained_focus();
            changed |= response.changed();
            ui.end_row();

            ui.label("Y");
            let response = ui.add(egui::DragValue::new(&mut state.center_y).speed(1.0));
            wants_snapshot |= response.drag_started() || response.gained_focus();
            changed |= response.changed();
            ui.end_row();

            ui.label("Zoom");
            let mut zoom_percent = state.zoom * 100.0;
            let response = ui.add(
                egui::DragValue::new(&mut zoom_percent)
                    .speed(1.0)
                    .clamp_range(1.0..=6400.0)
                    .suffix("%"),
            );
            wants_snapshot |= response.drag_started() || response.gained_focus();
            if response.changed() {
                state.zoom = (zoom_percent / 100.0).clamp(0.01, 64.0);
                changed = true;
            }
            ui.end_row();

            ui.label("Rotation");
            let mut degrees = state.rotation.to_degrees();
            let response = ui.add(egui::DragValue::new(&mut degrees).speed(0.5).suffix(" deg"));
            wants_snapshot |= response.drag_started() || response.gained_focus();
            if response.changed() {
                state.rotation = degrees.to_radians();
                changed = true;
            }
            ui.end_row();
        });

    ui.add_space(6.0);
    if ui.button("Reset camera at this frame").clicked() {
        state = app.state.project.default_camera_state();
        changed = true;
        wants_snapshot = true;
    }
    ui.label(
        egui::RichText::new("F6 creates a camera key. Tween animates position, zoom and rotation.")
            .small()
            .color(app.settings.theme.text_dim.to_color32()),
    );

    if changed {
        if wants_snapshot {
            app.history.snapshot_with_session(&before, &app.session);
        }
        app.camera_mut().insert_key(frame, state);
        if let Some(key) = app
            .state
            .project
            .runtime
            .camera
            .keys
            .iter_mut()
            .find(|key| key.frame == frame)
        {
            key.state = state;
        }
        app.state.mark_dirty();
        app.session.status = format!("camera updated at frame {}", frame + 1);
    }
}

fn render_audio_clip_properties(app: &mut EditorApp, ui: &mut Ui) -> bool {
    let Some(selection) = app.session.timeline_selection else {
        return false;
    };
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = selection.focus_layer_id;
    let frame = selection.focus_frame;
    let Some(reference) =
        crate::audio::audio_clip_at_frame(&app.state.project, q0rg_id, layer_id, frame)
    else {
        return false;
    };
    let Some(header) = app
        .state
        .project
        .assets
        .iter()
        .find(|a| a.id() == reference.asset_id)
        .and_then(|a| match a {
            Asset::Q0v(media) => q0video::q0v::probe_header(&media.bytes).ok(),
            _ => None,
        })
    else {
        return false;
    };
    let original = app.state.project.audio_clips[reference.clip_idx];
    let mut clip = original;
    let name = app
        .state
        .project
        .asset_names
        .get(&clip.asset_id)
        .cloned()
        .unwrap_or_else(|| format!("Audio {}", clip.asset_id));
    let source_seconds =
        header.audio_samples_per_channel as f64 / f64::from(header.spec.audio_sample_rate.max(1));
    crate::ui::section_header(ui, &name);
    ui.label(
        egui::RichText::new(format!(
            "{source_seconds:.2}s · {} Hz · {} channels",
            header.spec.audio_sample_rate, header.spec.audio_channels
        ))
        .small()
        .color(app.settings.theme.text_dim.to_color32()),
    );
    ui.add_space(6.0);
    let locked = app.state.project.layer_is_locked(q0rg_id, layer_id);
    let mut snapshot = false;
    let timeline_length = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .map(|q| q.frame_count)
        .unwrap_or(1);
    ui.add_enabled_ui(!locked, |ui| {
        egui::Grid::new("audio_clip_properties")
            .num_columns(2)
            .spacing([10.0, 7.0])
            .show(ui, |ui| {
                ui.label("Timeline");
                ui.label(format!(
                    "{}–{} · {:.2}s",
                    clip.start_frame + 1,
                    reference.end_frame_exclusive,
                    f64::from(reference.end_frame_exclusive - clip.start_frame)
                        / f64::from(app.state.project.meta.fps.max(1))
                ));
                ui.end_row();
                ui.label("Source start");
                let mut seconds = clip.source_offset_samples as f64 / 48_000.0;
                let response = ui.add(
                    egui::DragValue::new(&mut seconds)
                        .speed(0.01)
                        .suffix(" s")
                        .clamp_range(0.0..=(source_seconds - 1.0 / 48_000.0).max(0.0)),
                );
                snapshot |= response.drag_started() || response.gained_focus();
                if response.changed() {
                    clip.source_offset_samples = (seconds * 48_000.0).round() as u64;
                }
                ui.end_row();
                ui.label("Length");
                ui.horizontal(|ui| {
                    let mut full = clip.duration_frames.is_none();
                    let response = crate::ui::switch(ui, &mut full, "Full source");
                    snapshot |= response.changed();
                    if response.changed() {
                        clip.duration_frames = if full {
                            None
                        } else {
                            Some(reference.end_frame_exclusive - clip.start_frame)
                        };
                    }
                    if let Some(mut frames) = clip.duration_frames {
                        let response = ui.add(
                            egui::DragValue::new(&mut frames)
                                .suffix(" frames")
                                .clamp_range(
                                    1..=timeline_length.saturating_sub(clip.start_frame).max(1),
                                ),
                        );
                        snapshot |= response.drag_started() || response.gained_focus();
                        if response.changed() {
                            clip.duration_frames = Some(frames);
                        }
                    }
                });
                ui.end_row();
                ui.label("Volume");
                let mut percent = clip.gain * 100.0;
                let response = ui.add(egui::Slider::new(&mut percent, 0.0..=400.0).suffix("%"));
                snapshot |= response.drag_started() || response.gained_focus();
                if response.changed() {
                    clip.gain = percent / 100.0;
                }
                ui.end_row();
                ui.label("Balance");
                let response = ui.add(egui::Slider::new(&mut clip.pan, -1.0..=1.0).text("L / R"));
                snapshot |= response.drag_started() || response.gained_focus();
                ui.end_row();
                for (label, value) in [
                    ("Fade in", &mut clip.fade_in_ms),
                    ("Fade out", &mut clip.fade_out_ms),
                ] {
                    ui.label(label);
                    let response = ui.add(
                        egui::DragValue::new(value)
                            .speed(10.0)
                            .suffix(" ms")
                            .clamp_range(0..=60_000),
                    );
                    snapshot |= response.drag_started() || response.gained_focus();
                    ui.end_row();
                }
                ui.label("Mute");
                snapshot |= crate::ui::switch(ui, &mut clip.muted, "").changed();
                ui.end_row();
            });
    });
    ui.add_space(6.0);
    let mut keyframe_action = None;
    ui.horizontal_wrapped(|ui| {
        if ui.button("Play clip").clicked() {
            app.session.current_layer_id = layer_id;
            app.session
                .set_timeline_frame(clip.start_frame, timeline_length);
            app.session.timeline_selection = Some(crate::state::TimelineSelection::single(
                layer_id,
                clip.start_frame,
            ));
            if app.session.playing {
                let _ = app.refresh_audio_playback();
            } else {
                app.queue(Action::TogglePlay);
            }
        }
        if ui
            .add_enabled(
                !locked && frame > clip.start_frame,
                egui::Button::new("Split here"),
            )
            .on_hover_text(
                "Split into two clips, preserving the position in the source recording (F6)",
            )
            .clicked()
        {
            keyframe_action = Some(true);
            return;
        }
        if ui
            .add_enabled(!locked, egui::Button::new("Stop here"))
            .on_hover_text("Stop audio at a blank keyframe (F7)")
            .clicked()
        {
            keyframe_action = Some(false);
            return;
        }
        if ui
            .add_enabled(!locked, egui::Button::new("Reset audio settings"))
            .clicked()
        {
            clip = q0s_format::v2::AudioClip {
                gain: 1.0,
                source_offset_samples: original.source_offset_samples,
                duration_frames: original.duration_frames,
                ..original
            };
            clip.pan = 0.0;
            clip.fade_in_ms = 0;
            clip.fade_out_ms = 0;
            clip.muted = false;
            snapshot = true;
        }
    });
    ui.label(
        egui::RichText::new("The next keyframe stops this clip. Source audio stays intact.")
            .small()
            .color(app.settings.theme.text_dim.to_color32()),
    );
    if snapshot {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
    }
    if clip != original && !locked {
        app.state.project.audio_clips[reference.clip_idx] = clip;
        app.state.mark_dirty();
        if app.session.playing {
            let _ = app.refresh_audio_playback();
        }
    }
    if let Some(continue_audio) = keyframe_action {
        app.audio_keyframe_at(q0rg_id, layer_id, frame, continue_audio);
    }
    true
}

/// A change to the selected outlines.
type Restyle = Box<dyn Fn(&mut VStroke)>;

/// Stroke and fill of the selected raw parts, editable in place.
fn selected_stroke_properties(app: &mut EditorApp, ui: &mut Ui) {
    use q0s_format::geom::{CapShape, JoinShape};
    let crate::tools::SelectedPaint {
        stroke,
        fill,
        fill_gradient,
    } = crate::tools::selected_paint(&app.state.project, &app.session.selection);
    let dim = app.settings.theme.text_dim.to_color32();
    if let Some((stroke, mixed)) = stroke {
        crate::ui::section_header(ui, "Stroke");
        if mixed {
            ui.label(
                egui::RichText::new("Mixed styles - what you change is made the same")
                    .color(dim)
                    .small(),
            );
        }
        let mut snapshot = false;
        let mut restyle: Option<Restyle> = None;
        let mut rejoin: Option<JoinShape> = None;
        egui::Grid::new("selected_stroke_style")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Color");
                let mut color = rgba_to_color32(stroke.color);
                let response = crate::color_picker::color_edit_button_srgba(ui, &mut color);
                if response.gained_focus() || response.clicked() {
                    snapshot = true;
                }
                if response.changed() {
                    let picked = color32_to_rgba(color);
                    restyle = Some(Box::new(move |stroke| stroke.color = picked));
                }
                ui.end_row();

                ui.label("Opacity");
                let mut opacity = f32::from(stroke.color.a) / 2.55;
                let response = ui.add(
                    egui::Slider::new(&mut opacity, 0.0..=100.0)
                        .suffix("%")
                        .fixed_decimals(0),
                );
                if response.drag_started() || response.gained_focus() {
                    snapshot = true;
                }
                if response.changed() {
                    let alpha = (opacity * 2.55).round().clamp(0.0, 255.0) as u8;
                    restyle = Some(Box::new(move |stroke| stroke.color.a = alpha));
                }
                ui.end_row();

                ui.label("Width");
                let mut width = stroke.width;
                let response = ui.add(
                    egui::DragValue::new(&mut width)
                        .speed(0.1)
                        .clamp_range(0.1..=64.0),
                );
                if response.drag_started() || response.gained_focus() {
                    snapshot = true;
                }
                if response.changed() {
                    restyle = Some(Box::new(move |stroke| stroke.width = width));
                }
                ui.end_row();

                ui.label("Ends");
                ui.horizontal(|ui| {
                    for (cap, label) in [
                        (CapShape::Round, "Round"),
                        (CapShape::Butt, "Flat"),
                        (CapShape::Square, "Square"),
                    ] {
                        if ui.selectable_label(stroke.cap == cap, label).clicked()
                            && stroke.cap != cap
                        {
                            snapshot = true;
                            restyle = Some(Box::new(move |stroke| stroke.cap = cap));
                        }
                    }
                });
                ui.end_row();

                ui.label("Corners");
                ui.horizontal(|ui| {
                    for (join, label) in [
                        (JoinShape::Round, "Round"),
                        (JoinShape::Miter, "Sharp"),
                        (JoinShape::Bevel, "Bevel"),
                    ] {
                        if ui.selectable_label(stroke.join == join, label).clicked()
                            && stroke.join != join
                        {
                            snapshot = true;
                            rejoin = Some(join);
                        }
                    }
                });
                ui.end_row();
            });
        if snapshot {
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
        }
        if let Some(join) = rejoin {
            crate::tools::set_selected_stroke_join(app, join);
        }
        if let Some(restyle) = restyle {
            if crate::tools::restyle_selected_strokes(app, restyle) {
                if let Some((stroke, _)) =
                    crate::tools::selected_paint(&app.state.project, &app.session.selection).stroke
                {
                    app.session.stroke_width = stroke.width;
                }
            }
        }
    }
    if let Some((color, mixed)) = fill {
        crate::ui::section_header(ui, "Fill");
        if mixed {
            ui.label(
                egui::RichText::new("Mixed colours - what you pick paints them all")
                    .color(dim)
                    .small(),
            );
        }
        let mut snapshot = false;
        let mut picked = None;
        let mut gradient = fill_gradient;
        let mut gradient_change = PaintChange::None;
        egui::Grid::new("selected_fill_style")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Paint");
                let mut shown = rgba_to_color32(color);
                let (response, change) =
                    crate::color_picker::paint_edit_button(ui, &mut shown, &mut gradient);
                if response.gained_focus() || response.clicked() {
                    snapshot = true;
                }
                match change {
                    PaintChange::Color => picked = Some(color32_to_rgba(shown)),
                    PaintChange::Stops | PaintChange::Shape => gradient_change = change,
                    PaintChange::None => {}
                }
                ui.end_row();

                ui.label("Opacity");
                let mut opacity = f32::from(color.a) / 2.55;
                let response = ui.add(
                    egui::Slider::new(&mut opacity, 0.0..=100.0)
                        .suffix("%")
                        .fixed_decimals(0),
                );
                if response.drag_started() || response.gained_focus() {
                    snapshot = true;
                }
                if response.changed() {
                    picked = Some(Rgba {
                        a: (opacity * 2.55).round().clamp(0.0, 255.0) as u8,
                        ..color
                    });
                }
                ui.end_row();
            });
        if snapshot {
            app.history
                .snapshot_with_session(&app.state.project, &app.session);
        }
        if let Some(picked) = picked {
            crate::tools::set_selected_fill(app, picked);
        }
        if gradient_change != PaintChange::None {
            crate::tools::paint_selected_fills(
                app,
                crate::tools::FillPaint::Gradient {
                    pick: gradient.as_ref(),
                    refit: gradient_change == PaintChange::Shape,
                },
            );
        }
    }
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
            Asset::Bitmap(_) | Asset::Q0v(_) | Asset::Rig(_) | Asset::Model(_) => None,
        });
    let Some(path) = path else {
        ui.label("Selection is no longer available");
        return;
    };
    crate::ui::section_header(ui, "Contour");
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
    scene_properties(app, ui);
    crate::ui::section_header(ui, "Stage");
    let mut dirty = false;
    // Stage controls are drawn on every frame. Keep only the small metadata
    // value eagerly, then clone the full project if an edit actually starts.
    let meta_before = app.state.project.meta.clone();
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

            ui.label("Background");
            let mut background = rgba_to_color32(app.state.project.meta.background_color);
            let r = crate::color_picker::color_edit_button_srgba(ui, &mut background);
            if r.gained_focus() || r.clicked() {
                wants_snapshot = true;
            }
            if r.changed() {
                app.state.project.meta.background_color = color32_to_rgba(background);
                app.state.project.meta.background_color.a = 255;
                dirty = true;
            }
            ui.end_row();
        });
    if wants_snapshot {
        let mut snapshot_before = app.state.project.clone();
        snapshot_before.meta = meta_before;
        app.history
            .snapshot_with_session(&snapshot_before, &app.session);
    }
    if dirty {
        app.state.mark_dirty();
    }
}

fn scene_properties(app: &mut EditorApp, ui: &mut Ui) {
    use crate::ui::{icon_button, icon_button_enabled, Icon};
    let scenes = app.state.project.scenes();
    let active_scene_q0rg = app
        .session
        .breadcrumb
        .first()
        .copied()
        .unwrap_or(app.session.current_q0rg_id);
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        crate::ui::section_title(ui, "Scenes");
        if icon_button(ui, Icon::Add, "Add scene").clicked() {
            app.queue(Action::AddScene);
        }
    });
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(4.0))
        .show(ui, |ui| {
            ScrollArea::vertical()
                .id_source("project_scenes")
                .max_height(104.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Each row is laid out from the right: the buttons first,
                    // then the name takes what is left. Sizing the name by a
                    // guess at the buttons' width overflowed, and every row
                    // that overflowed widened the list for the rows below.
                    let row_width = ui.available_width();
                    for scene in &scenes {
                        let row = egui::Layout::right_to_left(egui::Align::Center);
                        ui.allocate_ui_with_layout(
                            egui::vec2(row_width, crate::ui::ICON_CONTROL_SIZE),
                            row,
                            |ui| {
                                if icon_button_enabled(
                                    ui,
                                    scenes.len() > 1,
                                    Icon::Delete,
                                    if scenes.len() > 1 {
                                        "Delete scene"
                                    } else {
                                        "The project needs at least one scene"
                                    },
                                )
                                .clicked()
                                {
                                    app.queue(Action::DeleteScene(scene.scene_id));
                                }
                                if icon_button(ui, Icon::Rename, "Rename scene").clicked() {
                                    app.session.scene_rename = Some(crate::state::SceneRename {
                                        scene_id: scene.scene_id,
                                        draft: scene.name.clone(),
                                        focus_requested: true,
                                    });
                                }
                                let active = scene.q0rg_id == active_scene_q0rg;
                                if crate::ui::name_button(
                                    ui,
                                    &scene.name,
                                    active,
                                    ui.available_width().max(0.0),
                                )
                                .on_hover_text("Switch to this scene")
                                .clicked()
                                {
                                    app.queue(Action::SwitchScene(scene.scene_id));
                                }
                            },
                        );
                    }
                });
        });

    let mut commit = None;
    let mut cancel = false;
    if let Some(rename) = app.session.scene_rename.as_mut() {
        if scenes.iter().any(|scene| scene.scene_id == rename.scene_id) {
            ui.horizontal_wrapped(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut rename.draft)
                        .desired_width((ui.available_width() - 65.0).max(50.0)),
                );
                if rename.focus_requested {
                    response.request_focus();
                    rename.focus_requested = false;
                }
                if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
                if !rename.draft.trim().is_empty()
                    && response.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter))
                {
                    commit = Some((rename.scene_id, rename.draft.clone()));
                }
                if icon_button_enabled(
                    ui,
                    !rename.draft.trim().is_empty(),
                    Icon::Check,
                    "Save scene name",
                )
                .clicked()
                {
                    commit = Some((rename.scene_id, rename.draft.clone()));
                }
                if icon_button(ui, Icon::Close, "Cancel scene rename").clicked() {
                    cancel = true;
                }
            });
        } else {
            cancel = true;
        }
    }
    if cancel {
        app.session.scene_rename = None;
    }
    if let Some((scene_id, name)) = commit {
        app.queue(Action::RenameScene(scene_id, name));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPropertyKind {
    Select,
    Lasso,
    Hand,
    Subselect,
    Pen,
    Pencil,
    Brush,
    Eraser,
    Knife,
    Line,
    ClosedShape,
    Fill,
    Eyedropper,
    Text,
    Rig,
}

fn tool_property_kind(tool: Tool) -> ToolPropertyKind {
    match tool {
        Tool::Select => ToolPropertyKind::Select,
        Tool::Lasso => ToolPropertyKind::Lasso,
        Tool::Hand => ToolPropertyKind::Hand,
        Tool::Subselect => ToolPropertyKind::Subselect,
        Tool::Pen => ToolPropertyKind::Pen,
        Tool::Pencil => ToolPropertyKind::Pencil,
        Tool::Brush => ToolPropertyKind::Brush,
        Tool::Eraser => ToolPropertyKind::Eraser,
        Tool::Knife => ToolPropertyKind::Knife,
        Tool::Line => ToolPropertyKind::Line,
        Tool::Rectangle | Tool::Oval => ToolPropertyKind::ClosedShape,
        Tool::Bucket => ToolPropertyKind::Fill,
        Tool::Eyedropper => ToolPropertyKind::Eyedropper,
        Tool::Text => ToolPropertyKind::Text,
        Tool::Rig => ToolPropertyKind::Rig,
    }
}

fn render_tool_properties(app: &mut EditorApp, ui: &mut Ui) {
    // The tool's shortcut sits by its name, quietly, to change it right here.
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        crate::ui::section_title(ui, app.session.current_tool.label());
        let command = crate::hotkeys::ShortcutCommand::for_tool(app.session.current_tool);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            crate::panels::shortcuts::chip(app, ui, command);
        });
    });

    match tool_property_kind(app.session.current_tool) {
        ToolPropertyKind::Select => {
            tool_hint(
                app,
                ui,
                "Drag artwork to move it; Alt-drag artwork to duplicate. Alt-drag the anchor cross to reposition it. Double-click a line to pick every line joined to it.",
            );
            aid_checkbox(
                app,
                ui,
                |aids| &mut aids.transform_box,
                "Transform box",
                "Off: the Select tool only moves things, with no handles to catch by mistake",
            );
        }
        ToolPropertyKind::Lasso => tool_hint(
            app,
            ui,
            "Draw a loop to select contours and objects inside or touching it.",
        ),
        ToolPropertyKind::Text => {
            crate::text_tool::render_tool_defaults(app, ui);
            tool_hint(
                app,
                ui,
                "Click the stage to add text. Click existing text to edit it.",
            );
        }
        ToolPropertyKind::Hand => tool_hint(
            app,
            ui,
            "Drag the stage to pan. Hold Space from any other tool for a temporary hand.",
        ),
        ToolPropertyKind::Subselect => tool_hint(
            app,
            ui,
            "Edit individual vector anchors and handles. Drag over empty stage to pick many points, on several lines at once, then drag one of them to move them all. Drag a line's middle to bend it.",
        ),
        ToolPropertyKind::Pen => {
            stroke_tool_properties(app, ui, "Pen path", true, true);
            crossing_option(app, ui);
            tool_hint(
                app,
                ui,
                "Click for a corner, drag for a curve (Alt: one handle, Shift: 45°). Click the first point to close; Enter or double-click ends. Backspace takes a point back.",
            );
        }
        ToolPropertyKind::Pencil => {
            stroke_tool_properties(app, ui, "Freehand line", false, true);
            if ui.add(egui::Slider::new(&mut app.settings.pencil_smoothing, 0..=100).text("Smoothing")).changed() {
                app.settings.save();
            }
        }
        ToolPropertyKind::Brush => brush_properties(app, ui),
        ToolPropertyKind::Eraser => eraser_properties(app, ui),
        ToolPropertyKind::Knife => knife_properties(app, ui),
        ToolPropertyKind::Line => {
            stroke_tool_properties(app, ui, "Straight line", false, true);
            middle_option(app, ui);
            crossing_option(app, ui);
        }
        ToolPropertyKind::ClosedShape => {
            stroke_tool_properties(app, ui, "Closed shape", true, false);
            middle_option(app, ui);
        }
        ToolPropertyKind::Fill => fill_tool_properties(app, ui),
        ToolPropertyKind::Eyedropper => tool_hint(
            app,
            ui,
            "Click vector paint to copy its fill or stroke, or anything else to take the colour shown there. Drag from the stage to pick from anywhere on screen. Shift picks into the stroke.",
        ),
        ToolPropertyKind::Rig => crate::rigging::render_properties(app, ui),
    }
}

fn knife_properties(app: &mut EditorApp, ui: &mut Ui) {
    tool_hint(
        app,
        ui,
        "Drag across shapes on this layer to cut them apart; Shift cuts straight. Fills fall into pieces you can move or delete, outlines open along the cut.",
    );
    let mut gap = app.settings.aids.knife_gap;
    if ui
        .add(
            egui::Slider::new(&mut gap, 0..=24)
                .text("Gap")
                .suffix(" px"),
        )
        .on_hover_text("How wide a crack the cut leaves; 0 only parts the pieces")
        .changed()
    {
        app.settings.aids.knife_gap = gap;
        app.settings.save();
    }
    aid_checkbox(
        app,
        ui,
        |aids| &mut aids.knife_jagged,
        "Jagged crack",
        "Cut along a zigzag following the line drawn, for broken and cracked looks",
    );
}

fn middle_option(app: &mut EditorApp, ui: &mut Ui) {
    aid_checkbox(
        app,
        ui,
        |aids| &mut aids.from_middle,
        "Draw from the middle",
        "The drag starts in the middle and grows both ways. Hold Alt for the same; Shift keeps it square or at 45 degrees",
    );
}

fn crossing_option(app: &mut EditorApp, ui: &mut Ui) {
    aid_checkbox(
        app,
        ui,
        |aids| &mut aids.split_crossings,
        "Cut lines where they cross",
        "New lines and the lines they cross are cut where they meet, so each piece can be picked on its own",
    );
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
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        crate::ui::section_title(ui, "Brush library");
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
                    ui.label("Fill");
                    tool_paint_box(
                        ui,
                        &mut app.session.brush.color,
                        &mut app.session.brush_gradient,
                    );
                    ui.end_row();

                    ui.label("Opacity");
                    opacity_control(ui, &mut app.session.brush.color.a);
                    ui.end_row();

                    ui.label("Mode");
                    let mut mode = app.settings.aids.paint_mode;
                    egui::ComboBox::from_id_source("classic_brush_mode")
                        .selected_text(mode.label())
                        .show_ui(ui, |ui| {
                            for choice in crate::settings::PaintMode::ALL {
                                ui.selectable_value(&mut mode, choice, choice.label())
                                    .on_hover_text(choice.hint());
                            }
                        })
                        .response
                        .on_hover_text(mode.hint());
                    if mode != app.settings.aids.paint_mode {
                        app.settings.aids.paint_mode = mode;
                        app.settings.save();
                    }
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
                    crate::ui::switch(ui, &mut app.session.brush.pressure_size, "");
                    ui.end_row();

                    ui.label("Speed size");
                    crate::ui::switch(ui, &mut app.session.brush.velocity_size, "");
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
                    crate::ui::switch(ui, &mut app.session.brush.scale_with_stage, "");
                    ui.end_row();

                    ui.label("Sync with eraser");
                    crate::ui::switch(ui, &mut app.session.brush.sync_with_eraser, "");
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
                    ui.label("Paint");
                    tool_paint_box(
                        ui,
                        &mut app.session.advanced_brush.color,
                        &mut app.session.brush_gradient,
                    );
                    ui.end_row();

                    ui.label("Opacity");
                    opacity_control(ui, &mut app.session.advanced_brush.color.a);
                    ui.end_row();

                    ui.label("Mode");
                    let mut mode = app.settings.aids.paint_mode;
                    egui::ComboBox::from_id_source("advanced_brush_mode")
                        .selected_text(mode.label())
                        .show_ui(ui, |ui| {
                            for choice in crate::settings::PaintMode::ALL {
                                ui.selectable_value(&mut mode, choice, choice.label())
                                    .on_hover_text(choice.hint());
                            }
                        })
                        .response
                        .on_hover_text(mode.hint());
                    if mode != app.settings.aids.paint_mode {
                        app.settings.aids.paint_mode = mode;
                        app.settings.save();
                    }
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
                    crate::ui::switch(ui, &mut app.session.advanced_brush.auto_angle, "");
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
                    crate::ui::switch(ui, &mut app.session.advanced_brush.pressure_size, "");
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
                        crate::ui::switch(ui, &mut app.session.advanced_brush.glow, "");
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
                    crate::ui::switch(ui, &mut app.session.advanced_brush.scale_with_stage, "");
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
        "A brush that takes away: erases raw graphics along its stroke. Synced, it is the brush exactly; otherwise it keeps its own nib, size, smoothing and dynamics.",
    );
    let brush_before = app.session.brush;
    let eraser_before = app.session.eraser;
    egui::Grid::new("classic_eraser_settings")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Sync with brush");
            crate::ui::switch(ui, &mut app.session.brush.sync_with_eraser, "");
            ui.end_row();

            let settings = if app.session.brush.sync_with_eraser {
                &mut app.session.brush
            } else {
                &mut app.session.eraser
            };
            ui.label("Nib");
            egui::ComboBox::from_id_source("classic_eraser_nib")
                .selected_text(settings.nib.label())
                .show_ui(ui, |ui| {
                    for nib in crate::brush::BrushNib::ALL {
                        ui.selectable_value(&mut settings.nib, nib, nib.label());
                    }
                });
            ui.end_row();

            ui.label("Size");
            ui.add(
                egui::DragValue::new(&mut settings.size)
                    .speed(0.25)
                    .clamp_range(0.1..=512.0),
            );
            ui.end_row();

            ui.label("Smoothing");
            ui.add(egui::Slider::new(&mut settings.smoothing, 0..=100));
            ui.end_row();

            ui.label("Pressure size");
            crate::ui::switch(ui, &mut settings.pressure_size, "");
            ui.end_row();

            ui.label("Speed size");
            crate::ui::switch(ui, &mut settings.velocity_size, "");
            ui.end_row();

            let dynamics = settings.pressure_size || settings.velocity_size;
            ui.label("Sensitivity");
            ui.add_enabled(
                dynamics,
                egui::Slider::new(&mut settings.dynamics_sensitivity, 0..=100),
            );
            ui.end_row();

            ui.label("Minimum size");
            ui.add_enabled(
                dynamics,
                egui::Slider::new(&mut settings.dynamics_min_size, 0.01..=1.0),
            );
            ui.end_row();

            ui.label("Scale with stage");
            crate::ui::switch(ui, &mut settings.scale_with_stage, "");
            ui.end_row();
        });
    persist_brush_preferences(app, brush_before);
    if app.session.eraser != eraser_before {
        app.settings.eraser.update_from_runtime(app.session.eraser);
        app.settings.save();
    }
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
            // Shapes may drop their stroke (not their fill too); a line is
            // its stroke, so it only takes a colour.
            tool_stroke_box(app, ui, supports_fill);
            ui.end_row();

            if app.session.stroke_none {
                if !supports_fill {
                    ui.label("");
                    ui.label(
                        egui::RichText::new("The stroke is none: pick a colour to draw lines")
                            .small()
                            .color(app.settings.theme.text_dim.to_color32()),
                    );
                    ui.end_row();
                }
            } else {
                ui.label("Stroke opacity");
                tool_opacity(app, ui, crate::color_panel::Slot::Stroke);
                ui.end_row();
            }

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

fn cap_label(cap: q0s_format::geom::CapShape) -> &'static str {
    match cap {
        q0s_format::geom::CapShape::Round => "Round",
        q0s_format::geom::CapShape::Butt => "Flat",
        q0s_format::geom::CapShape::Square => "Square",
    }
}

fn join_label(join: q0s_format::geom::JoinShape) -> &'static str {
    match join {
        q0s_format::geom::JoinShape::Miter => "Sharp",
        q0s_format::geom::JoinShape::Round => "Round",
        q0s_format::geom::JoinShape::Bevel => "Cut",
    }
}

/// Nib (end) and corner pickers; returns whether either changed.
fn stroke_shape_pickers(
    ui: &mut Ui,
    id: impl std::fmt::Debug,
    cap: &mut q0s_format::geom::CapShape,
    join: &mut q0s_format::geom::JoinShape,
) -> bool {
    use q0s_format::geom::{CapShape, JoinShape};
    let before = (*cap, *join);
    ui.label("Ends");
    egui::ComboBox::from_id_source(format!("stroke_cap_{id:?}"))
        .selected_text(cap_label(*cap))
        .show_ui(ui, |ui| {
            for option in [CapShape::Round, CapShape::Butt, CapShape::Square] {
                ui.selectable_value(cap, option, cap_label(option));
            }
        })
        .response
        .on_hover_text("Round nib, flat cut at the end, or a square nib reaching past it");
    ui.end_row();
    ui.label("Corners");
    egui::ComboBox::from_id_source(format!("stroke_join_{id:?}"))
        .selected_text(join_label(*join))
        .show_ui(ui, |ui| {
            for option in [JoinShape::Miter, JoinShape::Round, JoinShape::Bevel] {
                ui.selectable_value(join, option, join_label(option));
            }
        })
        .response
        .on_hover_text("Sharp points, rounded corners, or corners cut flat");
    ui.end_row();
    before != (*cap, *join)
}

fn cap_control(app: &mut EditorApp, ui: &mut Ui) {
    let mut cap = app.session.brush_cap;
    let mut join = app.session.brush_join;
    if stroke_shape_pickers(ui, app.session.current_tool, &mut cap, &mut join) {
        app.session.brush_cap = cap;
        app.session.brush_join = join;
        app.settings.brush_cap = cap.into();
        app.settings.brush_join = join.into();
        app.settings.save();
    }
}

/// The fill of the shape tools and the pen: a colour, a gradient, or none
/// (picked in its popup), as long as the stroke is not none as well.
fn optional_fill_control(app: &mut EditorApp, ui: &mut Ui) {
    ui.label("Fill paint");
    tool_fill_box(app, ui, true);
    ui.end_row();

    if app.session.fill_color.is_some() {
        ui.label("Fill opacity");
        tool_opacity(app, ui, crate::color_panel::Slot::Fill);
        ui.end_row();
    }
}

/// A colour as the paint stores it: unmultiplied.
fn unmultiplied(color: Color32) -> Rgba {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    Rgba { r, g, b, a }
}

/// The tools' fill box: a colour or a gradient, and none where `allow_none`
/// and the stroke is not none already.
fn tool_fill_box(app: &mut EditorApp, ui: &mut Ui, allow_none: bool) {
    use crate::color_panel::{self as colors, Slot};
    let look = colors::tool_look(&app.session, Slot::Fill);
    let mut shown = rgba_to_color32(look.color);
    let mut gradient = look.gradient.clone();
    let mut none = look.none && allow_none;
    let (_, change) = if allow_none {
        crate::color_picker::optional_paint_edit_button(
            ui,
            &mut shown,
            &mut gradient,
            &mut none,
            colors::none_allowed(&app.session, Slot::Fill),
        )
    } else {
        crate::color_picker::paint_edit_button(ui, &mut shown, &mut gradient)
    };
    if change != PaintChange::None {
        let paint = colors::Look {
            color: unmultiplied(shown),
            gradient,
            none,
        }
        .paint(change == PaintChange::Shape);
        colors::set_tool_paint(&mut app.session, Slot::Fill, &paint);
    }
}

/// The tools' stroke colour box, with none where `allow_none` and the fill
/// is not none already.
fn tool_stroke_box(app: &mut EditorApp, ui: &mut Ui, allow_none: bool) {
    use crate::color_panel::{self as colors, Paint, Slot};
    let look = colors::tool_look(&app.session, Slot::Stroke);
    let mut shown = rgba_to_color32(look.color);
    let mut none = look.none;
    let changed = if allow_none {
        crate::color_picker::optional_color_edit_button(
            ui,
            &mut shown,
            &mut none,
            colors::none_allowed(&app.session, Slot::Stroke),
        )
        .changed()
    } else {
        let changed = crate::color_picker::color_edit_button_srgba(ui, &mut shown).changed();
        if changed {
            none = false;
        }
        changed
    };
    if changed {
        let paint = if none {
            Paint::None
        } else {
            Paint::Color(unmultiplied(shown))
        };
        colors::set_tool_paint(&mut app.session, Slot::Stroke, &paint);
    }
}

/// The opacity of the tools' fill or stroke.
fn tool_opacity(app: &mut EditorApp, ui: &mut Ui, slot: crate::color_panel::Slot) {
    use crate::color_panel::Slot;
    let mut alpha = crate::color_panel::tool_look(&app.session, slot).color.a;
    let before = alpha;
    opacity_control(ui, &mut alpha);
    if alpha == before {
        return;
    }
    match slot {
        Slot::Fill => {
            if let Some(fill) = app.session.fill_color.as_mut() {
                fill.a = alpha;
            }
            app.session.fill_kept.a = alpha;
        }
        Slot::Stroke => app.session.stroke_color.a = alpha,
    }
}

fn fill_tool_properties(app: &mut EditorApp, ui: &mut Ui) {
    tool_hint(
        app,
        ui,
        "Fills enclosed raw regions on the active layer. Existing fills are recolored without changing their geometry.",
    );
    egui::Grid::new("bucket_fill_settings")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Fill paint");
            tool_fill_box(app, ui, false);
            ui.end_row();

            ui.label("Opacity");
            tool_opacity(app, ui, crate::color_panel::Slot::Fill);
            ui.end_row();

            ui.label("Close gaps");
            let before = app.settings.aids.bucket_gap;
            egui::ComboBox::from_id_source("bucket_gap")
                .selected_text(before.label())
                .show_ui(ui, |ui| {
                    for gap in crate::settings::GapSize::ALL {
                        ui.selectable_value(&mut app.settings.aids.bucket_gap, gap, gap.label());
                    }
                })
                .response
                .on_hover_text("Fill outlines that are not quite closed");
            if app.settings.aids.bucket_gap != before {
                app.settings.save();
            }
            ui.end_row();
        });
}

/// A switch kept in the settings, saved as soon as it is flipped.
fn aid_checkbox(
    app: &mut EditorApp,
    ui: &mut Ui,
    value: fn(&mut crate::settings::DrawingAids) -> &mut bool,
    label: &str,
    hint: &str,
) {
    let mut on = *value(&mut app.settings.aids);
    if crate::ui::switch(ui, &mut on, label)
        .on_hover_text(hint)
        .changed()
    {
        *value(&mut app.settings.aids) = on;
        app.settings.save();
    }
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

fn rgba_to_color32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

fn color32_to_rgba(c: Color32) -> Rgba {
    let [r, g, b, a] = c.to_array();
    Rgba { r, g, b, a }
}

/// Where a gradient sits on its drawing, exactly. The paint box covers
/// angle and size; this is for nudging its centre and axes by hand.
fn edit_gradient_placement(ui: &mut Ui, gradient: &mut GradientFill) -> bool {
    let mut changed = false;
    let labels = [
        ("Centre X", &mut gradient.transform.tx),
        ("Centre Y", &mut gradient.transform.ty),
        ("X axis X", &mut gradient.transform.a11),
        ("X axis Y", &mut gradient.transform.a21),
        ("Y axis X", &mut gradient.transform.a12),
        ("Y axis Y", &mut gradient.transform.a22),
    ];
    for (label, value) in labels {
        ui.label(label);
        if ui.add(egui::DragValue::new(value).speed(0.25)).changed() {
            changed = true;
        }
        ui.end_row();
    }
    changed
}

fn empty_vector_appearance() -> VectorAppearance {
    VectorAppearance {
        material: VectorMaterial::Solid,
        erase_mask: Vec::new(),
        material_source: Vec::new(),
        clip_mask: Vec::new(),
        field_transform: Affine::IDENTITY,
        fill_gradient: None,
    }
}

fn appearance_is_empty(appearance: &VectorAppearance) -> bool {
    matches!(appearance.material, VectorMaterial::Solid)
        && appearance.erase_mask.is_empty()
        && appearance.material_source.is_empty()
        && appearance.clip_mask.is_empty()
        && appearance.field_transform == Affine::IDENTITY
        && appearance.fill_gradient.is_none()
}

fn asset_properties(app: &mut EditorApp, ui: &mut Ui, id: u16) {
    let Some(asset_idx) = app.state.project.assets.iter().position(|a| a.id() == id) else {
        ui.label("Selection is no longer available");
        return;
    };
    crate::ui::section_header(ui, "Asset");

    let mut gradient = app
        .state
        .project
        .asset_appearances
        .get(&id)
        .and_then(|appearance| appearance.fill_gradient.clone());
    let mut gradient_dirty = false;
    let mut dirty = false;
    let asset = &mut app.state.project.assets[asset_idx];
    match asset {
        Asset::Bitmap(b) => {
            ui.label(format!("Bitmap {} x {}", b.width, b.height));
        }
        Asset::Q0v(v) => match q0video::q0v::probe_header(&v.bytes) {
            Ok(header) => {
                ui.label(format!(
                    "q0v {} x {}",
                    header.spec.width, header.spec.height
                ));
                ui.label(format!(
                    "{} frame(s) at {} fps",
                    header.spec.timeline_frames, header.spec.fps
                ));
                ui.label(if header.spec.audio {
                    format!(
                        "audio: {} hz / {} channel(s)",
                        header.spec.audio_sample_rate, header.spec.audio_channels
                    )
                } else {
                    "audio: none".to_string()
                });
            }
            Err(error) => {
                ui.label(format!("invalid q0v: {error}"));
            }
        },
        Asset::Model(model) => {
            let data = &model.data;
            ui.label(format!("3D model: {} mesh(es)", data.meshes.len()));
            ui.label(format!(
                "{} vertices / {} triangles",
                data.vertex_count(),
                data.triangle_count()
            ));
            let bones = data.nodes.iter().filter(|node| node.bone).count();
            ui.label(format!("{bones} bone(s) / {} clip(s)", data.clips.len()));
        }
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
                    // None in the picker takes the stroke off, while
                    // the drawing keeps a fill.
                    ui.label("Stroke");
                    let mut no_stroke = v.stroke.is_none();
                    let mut c = rgba_to_color32(
                        v.stroke
                            .map_or(crate::state::DEFAULT_STROKE, |stroke| stroke.color),
                    );
                    if crate::color_picker::optional_color_edit_button(
                        ui,
                        &mut c,
                        &mut no_stroke,
                        v.fill.is_some(),
                    )
                    .changed()
                    {
                        if no_stroke {
                            v.stroke = None;
                        } else {
                            let color = unmultiplied(c);
                            match v.stroke.as_mut() {
                                Some(stroke) => stroke.color = color,
                                None => {
                                    v.stroke = Some(VStroke {
                                        color,
                                        width: 1.0,
                                        cap: q0s_format::geom::CapShape::Round,
                                        join: q0s_format::geom::JoinShape::Bevel,
                                    })
                                }
                            }
                        }
                        dirty = true;
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

                        // Per-asset ends and corners, persisted in project and
                        // export files.
                        if stroke_shape_pickers(ui, ("asset", id), &mut s.cap, &mut s.join) {
                            dirty = true;
                        }
                    }

                    // A colour, a gradient, or none (while a stroke stays).
                    ui.label("Fill");
                    let bounds = crate::paint::vector_bounds(v).unwrap_or((
                        q0s_format::v2::Vec2::new(-50.0, -50.0),
                        q0s_format::v2::Vec2::new(50.0, 50.0),
                    ));
                    let mut no_fill = v.fill.is_none();
                    let mut c = rgba_to_color32(v.fill.unwrap_or(crate::state::DEFAULT_FILL));
                    let mut pick = gradient
                        .as_ref()
                        .map(|g| crate::paint::PaintGradient::from_fill(g, Some(bounds)));
                    let (_, change) = crate::color_picker::optional_paint_edit_button(
                        ui,
                        &mut c,
                        &mut pick,
                        &mut no_fill,
                        v.stroke.is_some(),
                    );
                    if no_fill {
                        if v.fill.take().is_some() {
                            dirty = true;
                        }
                    } else {
                        if v.fill.is_none() {
                            v.fill = Some(unmultiplied(c));
                            dirty = true;
                        }
                        match change {
                            PaintChange::None => {}
                            PaintChange::Color => {
                                v.fill = Some(unmultiplied(c));
                                dirty = true;
                            }
                            PaintChange::Stops => {
                                if let (Some(old), Some(pick)) = (gradient.as_mut(), pick.as_ref())
                                {
                                    old.stops = pick.stops.clone();
                                }
                                gradient_dirty = true;
                            }
                            PaintChange::Shape => {
                                gradient = pick.map(|pick| pick.fit(bounds));
                                gradient_dirty = true;
                            }
                        }
                    }
                    if v.fill.is_none() && gradient.take().is_some() {
                        gradient_dirty = true;
                    }
                    ui.end_row();

                    if let Some(fill_gradient) = gradient.as_mut() {
                        if edit_gradient_placement(ui, fill_gradient) {
                            gradient_dirty = true;
                        }
                    }
                });
        }
    }

    if gradient_dirty {
        if let Some(fill_gradient) = gradient {
            let appearance = app
                .state
                .project
                .asset_appearances
                .entry(id)
                .or_insert_with(empty_vector_appearance);
            appearance.fill_gradient = Some(fill_gradient);
        } else {
            let remove = if let Some(appearance) = app.state.project.asset_appearances.get_mut(&id)
            {
                appearance.fill_gradient = None;
                appearance_is_empty(appearance)
            } else {
                false
            };
            if remove {
                app.state.project.asset_appearances.remove(&id);
            }
        }
        app.textures.invalidate_assets([id]);
        dirty = true;
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
    crate::ui::section_header(ui, &format!("Symbol: {symbol_name}"));

    let mut wants_snapshot = false;
    let mut dirty = false;
    let mut queued_open_q0rg_id: Option<u16> = None;
    let mut requested_frame_count: Option<u16> = None;
    let mut name_draft = app.state.project.q0rgs[q0rg_idx].name.clone();
    {
        let q0rg = &mut app.state.project.q0rgs[q0rg_idx];
        egui::Grid::new("q0rg_props").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            let r = ui.text_edit_singleline(&mut name_draft);
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
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
    }
    if dirty {
        app.state.project.q0rgs[q0rg_idx].name = name_draft;
        app.state.mark_dirty();
    }
    if let Some(id) = queued_open_q0rg_id {
        app.queue(Action::OpenQ0langEditor(Some(id)));
    }
    if let Some(frame_count) = requested_frame_count {
        app.queue(Action::SetQ0rgFrameCount(id, frame_count));
    }
}

fn q0rg_playback_mode_name(mode: Q0rgPlaybackMode) -> &'static str {
    match mode {
        Q0rgPlaybackMode::Loop => "Loop",
        Q0rgPlaybackMode::PlayOnce => "Play once",
        Q0rgPlaybackMode::LoopReversed => "Loop reversed",
        Q0rgPlaybackMode::PlayReversedOnce => "Play reversed once",
        Q0rgPlaybackMode::SingleFrame => "Single frame",
        Q0rgPlaybackMode::Manual => "manual",
    }
}

fn set_q0rg_first_frame(playback: &mut Q0rgPlayback, first: u16, frame_count: u16) {
    let last = playback
        .last_frame
        .unwrap_or(frame_count.saturating_sub(1))
        .max(first);
    playback.set_range(first, last, frame_count);
}

fn active_placement_at_frame(
    project: &ProjectV2,
    reference: PlacementRef,
    frame: u16,
) -> Option<(&Placement, ActivePlacementState)> {
    let layer = project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == reference.q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == reference.layer_id)?;
    let source = layer.placements.get(reference.placement_idx)?;
    let active = q0s_format::raster::active_placement_states_at(layer, frame)
        .into_iter()
        .find(|active| active.index == reference.placement_idx)?;
    Some((source, active))
}

fn q0rg_frame_grid(
    app: &mut EditorApp,
    ui: &mut Ui,
    child_id: u16,
    child_frames: u16,
    selected_frame: u16,
) -> Option<u16> {
    let render_revision = (
        app.active_project_tab_id().unwrap_or(0),
        app.state.render_revision(),
    );
    let light = app.settings.theme.library_preview_light.to_color32();
    let dark = app.settings.theme.library_preview_dark.to_color32();
    let mut picked = None;
    crate::ui::preview_card_grid(ui, usize::from(child_frames), None, |ui, index, rect| {
        let frame = index as u16;
        let (response, preview_rect) =
            crate::ui::preview_card_at(ui, &(frame + 1).to_string(), frame == selected_frame, rect);
        crate::panels::library::draw_cached_q0rg_frame_preview(
            &ui.painter_at(preview_rect),
            &app.state.project,
            child_id,
            frame,
            preview_rect,
            &mut app.textures,
            ui.ctx(),
            render_revision,
            light,
            dark,
        );
        if response.clicked() {
            picked = Some(frame);
        }
    });
    picked
}

pub(crate) fn render_frame_picker(app: &mut EditorApp, ui: &mut Ui) {
    let Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = app.session.selection
    else {
        ui.weak("Select a symbol instance on the stage");
        return;
    };
    let reference = PlacementRef {
        q0rg_id,
        layer_id,
        placement_idx,
    };
    let frame = crate::frame_edit::frame_for(app, reference);
    let project = &app.state.project;
    let symbol =
        active_placement_at_frame(project, reference, frame).and_then(|(source, active)| {
            let Target::Q0rg(child_id) = source.target else {
                return None;
            };
            project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == child_id)
                .map(|symbol| (symbol, active))
        });
    let Some((symbol, active)) = symbol else {
        ui.weak("Select a symbol instance on the stage");
        return;
    };

    let child_id = symbol.q0rg_id;
    let child_frames = symbol.frame_count.max(1);
    let locked = project.layer_is_locked(q0rg_id, layer_id);
    let mut fx = active.fx;
    let mode = fx.q0rg_playback.mode;
    let single = matches!(
        mode,
        Q0rgPlaybackMode::SingleFrame | Q0rgPlaybackMode::Manual
    );
    let (caption, selected_frame) = if single {
        ("Frame", fx.q0rg_playback.single_frame)
    } else {
        ("First frame", fx.q0rg_playback.first_frame)
    };
    let selected_frame = selected_frame.min(child_frames - 1);
    crate::ui::section_header(ui, &symbol.name);
    ui.weak(format!(
        "{} · {caption} {} / {child_frames}",
        q0rg_playback_mode_name(mode),
        selected_frame + 1
    ));
    if locked {
        ui.weak("Unlock this layer to pick a frame");
    }

    let project_tab = app.active_project_tab_id();
    let picked = ScrollArea::vertical()
        .id_source(("frame_picker", project_tab, child_id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_enabled_ui(!locked, |ui| {
                ui.push_id((q0rg_id, layer_id, placement_idx, frame), |ui| {
                    q0rg_frame_grid(app, ui, child_id, child_frames, selected_frame)
                })
                .inner
            })
            .inner
        })
        .inner;
    let Some(picked) = picked.filter(|picked| *picked != selected_frame) else {
        return;
    };
    if single {
        fx.q0rg_playback.single_frame = picked;
    } else {
        set_q0rg_first_frame(&mut fx.q0rg_playback, picked, child_frames);
    }

    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    if let Some(new_idx) = apply_placement_edit_at_frame(
        app,
        q0rg_id,
        layer_id,
        placement_idx,
        frame,
        active.transform,
        fx,
    ) {
        let manual = (mode == Q0rgPlaybackMode::Manual).then_some(fx.q0rg_playback);
        finish_placement_edit(app, reference, new_idx, frame, manual);
        ui.ctx().request_repaint();
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
    let reference = PlacementRef {
        q0rg_id,
        layer_id,
        placement_idx,
    };
    let current_frame = crate::frame_edit::frame_for(app, reference);
    let Some((source, active)) =
        active_placement_at_frame(&app.state.project, reference, current_frame)
    else {
        ui.label("Selection is no longer available");
        return;
    };
    let source_frame = source.frame;
    let target = source.target;
    let source_pivot = source.pivot;
    let tween = source.tween;
    let playback_start_frame = active.playback_start_frame;
    let mut fx = active.fx;
    let authored_transform = active.transform;
    let (mut transform, rig_bound) = crate::tools::placement_transform_for_edit(
        &app.state.project,
        q0rg_id,
        layer_id,
        placement_idx,
        current_frame,
    )
    .unwrap_or((authored_transform, false));

    let mut wants_snapshot = false;
    let mut transform_dirty = false;
    let mut fx_dirty = false;
    let mut manual_playback_edit = None;
    let mut rotation_pivot = None;
    crate::ui::section_header(ui, "Placement");
    if rig_bound {
        ui.label(
            egui::RichText::new("transform edits the rig pose; linked bones stay connected")
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
        crate::rigging::render_pose_tween_options(app, ui, q0rg_id);
    }
    if source_frame != current_frame {
        let held_message = if rig_bound {
            format!(
                "held from frame {}; bone binding stays connected",
                source_frame + 1
            )
        } else {
            format!(
                "held from frame {}; editing creates a keyframe at {}",
                source_frame + 1,
                current_frame + 1
            )
        };
        ui.label(
            egui::RichText::new(held_message)
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
            let scale_x_response = row(
                ui,
                "Scale X",
                egui::DragValue::new(&mut transform.sx)
                    .speed(0.01)
                    .clamp_range(-64.0..=64.0),
            );
            if scale_x_response.changed() {
                transform.sx = crate::tools::valid_scale(transform.sx);
            }
            let scale_y_response = row(
                ui,
                "Scale Y",
                egui::DragValue::new(&mut transform.sy)
                    .speed(0.01)
                    .clamp_range(-64.0..=64.0),
            );
            if scale_y_response.changed() {
                transform.sy = crate::tools::valid_scale(transform.sy);
            }
            let mut rotation_degrees = transform.rotation.to_degrees();
            let rotation_response = row(
                ui,
                "Rotation (deg)",
                egui::DragValue::new(&mut rotation_degrees).speed(1.0),
            );
            if rotation_response.changed() {
                // Numeric rotation must use the same local anchor as the stage
                // handles. Changing only the angle rotates off-origin artwork
                // around empty target coordinates instead of its visible pivot.
                let pivot = source_pivot
                    .or_else(|| {
                        app.session
                            .transform_pivot
                            .as_ref()
                            .filter(|pivot| pivot.selection == app.session.selection)
                            .and_then(|pivot| {
                                let plane_point = crate::tools::stage_point_to_placement_plane(
                                    &app.state.project,
                                    q0rg_id,
                                    layer_id,
                                    placement_idx,
                                    app.session.current_frame,
                                    pivot.point,
                                );
                                Affine::from_transform(transform)
                                    .inverse()
                                    .map(|inverse| inverse.apply(plane_point))
                            })
                    })
                    .or_else(|| {
                        let revision = (
                            app.active_project_tab_id().unwrap_or(0),
                            app.state.render_revision(),
                        );
                        app.textures.with_placement_bounds_cache(
                            &app.state.project,
                            revision,
                            |bounds, _| {
                                bounds.target_local_bbox(&app.state.project, target).map(
                                    |(x0, y0, x1, y1)| {
                                        q0s_format::v2::Vec2::new((x0 + x1) * 0.5, (y0 + y1) * 0.5)
                                    },
                                )
                            },
                        )
                    })
                    .unwrap_or(q0s_format::v2::Vec2::new(0.0, 0.0));
                let world = Affine::from_transform(transform).apply(pivot);
                transform = crate::tools::rotate_placement_about_local_point(
                    transform,
                    pivot,
                    world,
                    rotation_degrees.to_radians() - transform.rotation,
                );
                rotation_pivot = Some(pivot);
            }
        });

    crate::easing::render_tween_properties(app, ui, q0rg_id, layer_id, placement_idx, tween);

    if let Target::Q0rg(child_id) = target {
        let child_frames = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == child_id)
            .map(|q0rg| q0rg.frame_count.max(1))
            .unwrap_or(1);
        let max_frame = child_frames - 1;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            crate::ui::section_title(ui, "Symbol");
            let can_replace = !app.state.project.layer_is_locked(q0rg_id, layer_id);
            if ui
                .add_enabled(can_replace, egui::Button::new("Replace q0rg"))
                .clicked()
            {
                app.open_replace_q0rg_dialog(q0rg_id, layer_id, placement_idx);
            }
        });
        let unlocked = !app.state.project.layer_is_locked(q0rg_id, layer_id);
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(
                unlocked,
                egui::Button::new("Duplicate Symbol"),
            ).on_hover_text("Create an independent copy, including nested symbols, without changing the original").clicked() {
                app.queue(crate::app::Action::PromptSymbol(crate::symbol_prompt::SymbolRequest::Duplicate(
                    q0rg_id,
                    layer_id,
                    placement_idx,
                )));
            }
            if ui
                .add_enabled(unlocked, egui::Button::new("Make Variant"))
                .on_hover_text("Copy this symbol as a variant of it (a broken, dented or wounded version), show the copy here and start drawing on it")
                .clicked()
            {
                app.queue(crate::app::Action::MakeVariant(q0rg_id, layer_id, placement_idx));
            }
        });
        let family = crate::variants::family(&app.state.project, child_id);
        if family.len() > 1 {
            let index = family.iter().position(|id| *id == child_id).unwrap_or(0);
            ui.horizontal(|ui| {
                ui.label("Variant");
                if ui
                    .add_enabled(unlocked, egui::Button::new("<").small())
                    .on_hover_text("Show the previous variant here")
                    .clicked()
                {
                    app.queue(crate::app::Action::SwapVariant(
                        q0rg_id,
                        layer_id,
                        placement_idx,
                        -1,
                    ));
                }
                ui.label(format!("{} / {}", index + 1, family.len()));
                if ui
                    .add_enabled(unlocked, egui::Button::new(">").small())
                    .on_hover_text("Show the next variant here")
                    .clicked()
                {
                    app.queue(crate::app::Action::SwapVariant(
                        q0rg_id,
                        layer_id,
                        placement_idx,
                        1,
                    ));
                }
            });
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            crate::ui::section_title(ui, "Playback");
            if !crate::touch::ENABLED && ui.small_button("Frame Picker…").clicked() {
                app.queue(Action::OpenFramePicker);
            }
        });
        egui::Grid::new("placement_q0rg_playback")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Mode");
                let mode_before = fx.q0rg_playback.mode;
                egui::ComboBox::from_id_source("placement_q0rg_playback_mode")
                    .selected_text(q0rg_playback_mode_name(fx.q0rg_playback.mode))
                    .show_ui(ui, |ui| {
                        for mode in [
                            Q0rgPlaybackMode::Loop,
                            Q0rgPlaybackMode::PlayOnce,
                            Q0rgPlaybackMode::LoopReversed,
                            Q0rgPlaybackMode::PlayReversedOnce,
                            Q0rgPlaybackMode::SingleFrame,
                            Q0rgPlaybackMode::Manual,
                        ] {
                            ui.selectable_value(
                                &mut fx.q0rg_playback.mode,
                                mode,
                                q0rg_playback_mode_name(mode),
                            );
                        }
                    });
                if fx.q0rg_playback.mode != mode_before {
                    fx_dirty = true;
                    wants_snapshot = true;
                    if fx.q0rg_playback.mode == Q0rgPlaybackMode::Manual {
                        fx.q0rg_playback.single_frame =
                            q0s_format::v2::resolve_q0rg_playback_frame(
                                &app.state.project,
                                child_id,
                                current_frame,
                                playback_start_frame,
                                Q0rgPlayback {
                                    mode: mode_before,
                                    ..fx.q0rg_playback
                                },
                            )
                            .min(max_frame);
                        fx.q0rg_playback.placement_relative = true;
                        manual_playback_edit = Some(fx.q0rg_playback);
                    } else if fx.q0rg_playback.mode == Q0rgPlaybackMode::SingleFrame {
                        fx.q0rg_playback.placement_relative = true;
                    } else {
                        fx.q0rg_playback.placement_relative = true;
                        if fx.q0rg_playback.first_frame != 0 && fx.q0rg_playback.last_frame.is_none() {
                            fx.q0rg_playback.last_frame = Some(max_frame.max(fx.q0rg_playback.first_frame));
                        }
                    }
                }
                ui.end_row();

                ui.label("Speed");
                let runs_automatically = !matches!(fx.q0rg_playback.mode,
                    Q0rgPlaybackMode::SingleFrame | Q0rgPlaybackMode::Manual);
                ui.add_enabled_ui(runs_automatically, |ui| {
                    ui.horizontal(|ui| {
                        let mut speed = f32::from(fx.q0rg_playback.speed_percent) / 100.0;
                        let response = ui.add(egui::DragValue::new(&mut speed)
                            .clamp_range(0.01..=100.0).speed(0.01)
                            .max_decimals(2).suffix("×"))
                            .on_hover_text("Symbol playback speed. 0.5× is half speed; 2× is double speed.");
                        note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                        if response.changed() {
                            fx.q0rg_playback.speed_percent = (speed * 100.0).round().clamp(1.0, 10_000.0) as u16;
                            fx.q0rg_playback.placement_relative = true;
                        }
                        ui.menu_button("Presets", |ui| {
                            for (percent, label) in [(25, "0.25×"), (50, "0.5×"), (100, "1×"),
                                (150, "1.5×"), (200, "2×"), (300, "3×"), (400, "4×")] {
                                if ui.selectable_label(fx.q0rg_playback.speed_percent == percent, label).clicked() {
                                    if fx.q0rg_playback.speed_percent != percent {
                                        fx.q0rg_playback.speed_percent = percent;
                                        fx.q0rg_playback.placement_relative = true;
                                        fx_dirty = true;
                                        wants_snapshot = true;
                                    }
                                    ui.close_menu();
                                }
                            }
                        });
                    });
                });
                ui.end_row();

                if fx.q0rg_playback.mode == Q0rgPlaybackMode::SingleFrame {
                    ui.label("Frame");
                    let mut display = fx.q0rg_playback.single_frame.min(max_frame) + 1;
                    let response = ui.add(
                        egui::DragValue::new(&mut display)
                            .clamp_range(1..=child_frames)
                            .speed(1),
                    );
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    if response.changed() {
                        fx.q0rg_playback.single_frame = display.saturating_sub(1);
                    }
                    ui.end_row();
                } else if fx.q0rg_playback.mode != Q0rgPlaybackMode::Manual {
                    ui.label("First frame");
                    let mut first = fx.q0rg_playback.first_frame.saturating_add(1);
                    let response = ui.add(
                        egui::DragValue::new(&mut first)
                            .clamp_range(1..=u16::MAX)
                            .speed(1),
                    );
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    if response.changed() || (response.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                        let before = fx.q0rg_playback;
                        set_q0rg_first_frame(
                            &mut fx.q0rg_playback,
                            first.saturating_sub(1),
                            child_frames,
                        );
                        if fx.q0rg_playback != before { fx_dirty = true; wants_snapshot = true; }
                    }
                    ui.end_row();

                    ui.label(if fx.q0rg_playback.follows_symbol_length() {
                        "Last frame (auto)"
                    } else { "Last frame" }).on_hover_text(
                        "The full symbol range follows its length. Custom first/last frames stay fixed.",
                    );
                    let mut last = fx
                        .q0rg_playback
                        .last_frame
                        .unwrap_or(max_frame)
                        .saturating_add(1);
                    let response = ui.add(
                        egui::DragValue::new(&mut last)
                            .clamp_range(
                                fx.q0rg_playback.first_frame.saturating_add(1)..=u16::MAX,
                            )
                            .speed(1),
                    );
                    note_property_response(&response, &mut fx_dirty, &mut wants_snapshot);
                    if response.changed() || (response.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                        let before = fx.q0rg_playback;
                        fx.q0rg_playback.set_range(fx.q0rg_playback.first_frame,
                            last.saturating_sub(1), child_frames);
                        if fx.q0rg_playback != before { fx_dirty = true; wants_snapshot = true; }
                    }
                    ui.end_row();
                }

                let preview_frame = q0s_format::v2::resolve_q0rg_playback_frame(
                    &app.state.project,
                    child_id,
                    current_frame,
                    playback_start_frame,
                    fx.q0rg_playback,
                );
                ui.label("Preview frame");
                ui.label(format!("{} / {}", preview_frame + 1, child_frames));
                ui.end_row();
            });

        if fx.q0rg_playback.mode == Q0rgPlaybackMode::Manual {
            let selected_frame = fx.q0rg_playback.single_frame.min(max_frame);
            let picked = ScrollArea::vertical()
                .id_source((
                    "placement_q0rg_manual_frames",
                    q0rg_id,
                    layer_id,
                    placement_idx,
                ))
                .max_height(190.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    q0rg_frame_grid(app, ui, child_id, child_frames, selected_frame)
                })
                .inner;
            if let Some(frame) = picked {
                fx.q0rg_playback.single_frame = frame;
                manual_playback_edit = Some(fx.q0rg_playback);
                fx_dirty = true;
                wants_snapshot = true;
            }
        }
    }

    crate::scene3d::placement_3d_properties(
        app,
        ui,
        reference,
        target,
        &mut fx,
        current_frame,
        playback_start_frame,
        &mut fx_dirty,
        &mut wants_snapshot,
    );

    let fx_values = [fx];
    render_fx_controls(ui, &mut fx, &fx_values);
    if fx != fx_values[0] {
        fx_dirty = true;
        wants_snapshot = true;
    }

    ui.separator();
    let break_apart_fx_block = app.break_apart_fx_block_reason();
    let break_apart = ui.add_enabled(
        app.can_break_apart_selection(),
        crate::ui::shortcut_button(
            &app.settings.hotkeys,
            crate::hotkeys::ShortcutCommand::BreakApart,
            "Break Apart",
        ),
    );
    let break_apart = if let Some(reason) = break_apart_fx_block {
        break_apart.on_hover_text(reason)
    } else {
        break_apart
    };
    if break_apart.clicked() {
        app.queue(Action::BreakApartSelection);
    }
    if app.can_trace_bitmap_selection()
        && ui
            .button("Trace Bitmap…")
            .on_hover_text("Turn this bitmap into vector shapes")
            .clicked()
    {
        app.queue(Action::PromptTraceBitmap);
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
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
    }
    if rig_bound && transform_dirty && !fx_dirty {
        if crate::frame_edit::set_transform_at_frame(app, reference, current_frame, transform) {
            store_rotation_pivot(app, reference, rotation_pivot);
            app.mark_placement_transforms_changed(&[(q0rg_id, layer_id, placement_idx)]);
        } else {
            app.session.status = "could not update the rig-bound transform".to_string();
        }
        return;
    }
    if let Some(new_idx) = apply_placement_edit_at_frame(
        app,
        q0rg_id,
        layer_id,
        placement_idx,
        current_frame,
        if rig_bound {
            authored_transform
        } else {
            transform
        },
        fx,
    ) {
        store_rotation_pivot(
            app,
            PlacementRef {
                placement_idx: new_idx,
                ..reference
            },
            rotation_pivot,
        );
        if rig_bound && transform_dirty {
            if crate::frame_edit::set_transform_at_frame(
                app,
                PlacementRef {
                    placement_idx: new_idx,
                    ..reference
                },
                current_frame,
                transform,
            ) {
                app.mark_placement_transforms_changed(&[(q0rg_id, layer_id, new_idx)]);
            } else {
                app.session.status = "could not update the rig-bound transform".to_string();
            }
        }
        finish_placement_edit(app, reference, new_idx, current_frame, manual_playback_edit);
    }
}

fn finish_placement_edit(
    app: &mut EditorApp,
    reference: PlacementRef,
    new_idx: usize,
    frame: u16,
    manual_playback_edit: Option<Q0rgPlayback>,
) {
    let PlacementRef {
        q0rg_id, layer_id, ..
    } = reference;
    if let Some(playback) = manual_playback_edit {
        propagate_manual_q0rg_playback(
            &mut app.state.project,
            q0rg_id,
            layer_id,
            new_idx,
            frame,
            playback,
        );
    }
    app.session.selection = Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx: new_idx,
    };
    if app.session.onion.edit_frames {
        crate::frame_edit::remap(
            app,
            &[reference],
            &[PlacementRef {
                placement_idx: new_idx,
                ..reference
            }],
        );
    }
    app.state.mark_dirty();
    app.textures.invalidate();
}

#[derive(Clone, Copy)]
enum FxField {
    Opacity,
    Blend,
    TintEnabled,
    TintColor,
    TintAmount,
    GlowEnabled,
    GlowColor,
    GlowRadiusX,
    GlowRadiusY,
    GlowStrength,
    GlowQuality,
    BlurEnabled,
    BlurRadiusX,
    BlurRadiusY,
    BlurQuality,
    ShadowEnabled,
    ShadowColor,
    ShadowBlur,
    ShadowOffsetX,
    ShadowOffsetY,
    ShadowStrength,
    ShadowQuality,
    Reset,
}

impl FxField {
    fn mixed(self, values: &[PlacementFx]) -> bool {
        let Some(first) = values.first() else {
            return false;
        };
        values.iter().skip(1).any(|value| match self {
            Self::Opacity => value.opacity != first.opacity,
            Self::Blend => value.blend_mode != first.blend_mode,
            Self::TintEnabled => value.tint.is_some() != first.tint.is_some(),
            Self::TintColor => value.tint.map(|v| v.color) != first.tint.map(|v| v.color),
            Self::TintAmount => value.tint.map(|v| v.amount) != first.tint.map(|v| v.amount),
            Self::GlowEnabled => value.glow.is_some() != first.glow.is_some(),
            Self::GlowColor => value.glow.map(|v| v.color) != first.glow.map(|v| v.color),
            Self::GlowRadiusX => value.glow.map(|v| v.radius_x) != first.glow.map(|v| v.radius_x),
            Self::GlowRadiusY => value.glow.map(|v| v.radius_y) != first.glow.map(|v| v.radius_y),
            Self::GlowStrength => value.glow.map(|v| v.strength) != first.glow.map(|v| v.strength),
            Self::GlowQuality => value.glow.map(|v| v.quality) != first.glow.map(|v| v.quality),
            Self::BlurEnabled => value.blur.is_some() != first.blur.is_some(),
            Self::BlurRadiusX => value.blur.map(|v| v.radius_x) != first.blur.map(|v| v.radius_x),
            Self::BlurRadiusY => value.blur.map(|v| v.radius_y) != first.blur.map(|v| v.radius_y),
            Self::BlurQuality => value.blur.map(|v| v.quality) != first.blur.map(|v| v.quality),
            Self::ShadowEnabled => value.shadow.is_some() != first.shadow.is_some(),
            Self::ShadowColor => value.shadow.map(|v| v.color) != first.shadow.map(|v| v.color),
            Self::ShadowBlur => {
                value.shadow.map(|v| v.blur_radius) != first.shadow.map(|v| v.blur_radius)
            }
            Self::ShadowOffsetX => {
                value.shadow.map(|v| v.offset_x) != first.shadow.map(|v| v.offset_x)
            }
            Self::ShadowOffsetY => {
                value.shadow.map(|v| v.offset_y) != first.shadow.map(|v| v.offset_y)
            }
            Self::ShadowStrength => {
                value.shadow.map(|v| v.strength) != first.shadow.map(|v| v.strength)
            }
            Self::ShadowQuality => {
                value.shadow.map(|v| v.quality) != first.shadow.map(|v| v.quality)
            }
            Self::Reset => false,
        })
    }
}

#[derive(Default)]
struct FxEdits(u32);

impl FxEdits {
    fn mark(&mut self, field: FxField) {
        self.0 |= 1 << field as u32;
    }
    fn has(&self, field: FxField) -> bool {
        self.0 & (1 << field as u32) != 0
    }
    fn changed(&self) -> bool {
        self.0 != 0
    }
    fn response(&mut self, field: FxField, response: &egui::Response) {
        if response.changed() {
            self.mark(field);
        }
    }

    /// Apply only the controls edited by the user. In particular, a tint color
    /// must not copy the first object's alpha, filters or playback into others.
    fn apply(&self, value: &mut PlacementFx, draft: PlacementFx) {
        if self.has(FxField::Reset) {
            let playback = value.q0rg_playback;
            *value = PlacementFx::default();
            value.q0rg_playback = playback;
        }
        if self.has(FxField::Opacity) {
            value.opacity = draft.opacity;
        }
        if self.has(FxField::Blend) {
            value.blend_mode = draft.blend_mode;
        }
        macro_rules! optional {
            ($member:ident, $enabled:ident, $($field:ident => $property:ident),+ $(,)?) => {
                if self.has(FxField::$enabled) {
                    if let Some(source) = draft.$member { value.$member.get_or_insert(source); }
                    else { value.$member = None; }
                }
                if $(self.has(FxField::$field))||+ {
                    if let Some(source) = draft.$member {
                        let target = value.$member.get_or_insert(source);
                        $(if self.has(FxField::$field) { target.$property = source.$property; })+
                    }
                }
            };
        }
        optional!(tint, TintEnabled, TintColor => color, TintAmount => amount);
        optional!(glow, GlowEnabled, GlowColor => color, GlowRadiusX => radius_x,
            GlowRadiusY => radius_y, GlowStrength => strength, GlowQuality => quality);
        optional!(blur, BlurEnabled, BlurRadiusX => radius_x, BlurRadiusY => radius_y, BlurQuality => quality);
        optional!(shadow, ShadowEnabled, ShadowColor => color, ShadowBlur => blur_radius,
            ShadowOffsetX => offset_x, ShadowOffsetY => offset_y, ShadowStrength => strength,
            ShadowQuality => quality);
    }
}

fn fx_label(ui: &mut Ui, label: &str, field: FxField, values: &[PlacementFx]) {
    if field.mixed(values) {
        ui.label(format!("{label} (mixed)")).on_hover_text("The selected objects have different values. Editing this control applies its value to all selected objects.");
    } else {
        ui.label(label);
    }
}

fn fx_toggle(
    ui: &mut Ui,
    enabled: &mut bool,
    field: FxField,
    label: &str,
    values: &[PlacementFx],
) -> egui::Response {
    let mixed = field.mixed(values);
    if mixed {
        *enabled = false;
    }
    crate::ui::switch_mixed(ui, enabled, mixed, label)
}

fn multiple_placement_properties(app: &mut EditorApp, ui: &mut Ui, refs: &[PlacementRef]) {
    let mut refs = refs.to_vec();
    refs.sort_by_key(|r| (r.q0rg_id, r.layer_id, r.placement_idx));
    refs.dedup();
    let frames: Vec<_> = refs
        .iter()
        .map(|r| crate::frame_edit::frame_for(app, *r))
        .collect();
    let mut active_layers = std::collections::BTreeMap::new();
    for (reference, frame) in refs.iter().zip(&frames) {
        let key = (reference.q0rg_id, reference.layer_id, *frame);
        if active_layers.contains_key(&key) {
            continue;
        }
        if let Some(layer) = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == reference.q0rg_id)
            .and_then(|q| {
                q.layers
                    .iter()
                    .find(|layer| layer.layer_id == reference.layer_id)
            })
        {
            active_layers.insert(
                key,
                q0s_format::raster::active_placement_states_at(layer, *frame)
                    .into_iter()
                    .map(|active| (active.index, active.fx))
                    .collect::<std::collections::BTreeMap<_, _>>(),
            );
        }
    }
    let values: Option<Vec<_>> = refs
        .iter()
        .zip(&frames)
        .map(|(r, frame)| {
            active_layers
                .get(&(r.q0rg_id, r.layer_id, *frame))?
                .get(&r.placement_idx)
                .copied()
        })
        .collect();
    let Some(values) = values else {
        ui.label("Selection is no longer available on this frame");
        return;
    };
    let Some(mut draft) = values.first().copied() else {
        return;
    };
    crate::ui::section_header(ui, "Shared effects");
    ui.label(egui::RichText::new("Changes apply to every selected object").small());
    let locked = refs
        .iter()
        .any(|r| app.state.project.layer_is_locked(r.q0rg_id, r.layer_id));
    if locked {
        ui.label("Unlock the selected layers to edit their properties");
    }
    let edits = ui
        .add_enabled_ui(!locked, |ui| render_fx_controls(ui, &mut draft, &values))
        .inner;
    if !edits.changed() {
        return;
    }
    let updated: Vec<_> = values
        .iter()
        .copied()
        .map(|mut fx| {
            edits.apply(&mut fx, draft);
            fx
        })
        .collect();
    if updated == values {
        return;
    }

    // Stage the entire edit so a failed keyframe materialization cannot leave
    // half of a multi-layer selection changed. Held/interpolated FX are baked
    // at this frame; past keys and per-instance playback remain intact.
    let mut project = app.state.project.clone();
    let Some(mapped) = crate::frame_edit::materialize_refs(&mut project, &refs, &frames) else {
        app.session.status = "could not create keyframes for the selected objects".into();
        return;
    };
    for (reference, fx) in mapped.iter().zip(updated) {
        let Some(placement) = project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == reference.q0rg_id)
            .and_then(|q| {
                q.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == reference.layer_id)
            })
            .and_then(|layer| layer.placements.get_mut(reference.placement_idx))
        else {
            return;
        };
        placement.fx = fx;
    }
    app.history
        .snapshot_with_session(&app.state.project, &app.session);
    let mut edited_layers =
        std::collections::BTreeMap::<u16, std::collections::BTreeMap<u16, u16>>::new();
    for (reference, frame) in refs.iter().zip(&frames) {
        edited_layers
            .entry(reference.q0rg_id)
            .or_default()
            .insert(reference.layer_id, *frame);
    }
    for (q0rg_id, layers) in edited_layers {
        if app.session.onion.edit_frames {
            break;
        }
        crate::app::remap_selection_after_keyframes(
            &app.state.project,
            &project,
            &mut app.session.selection,
            &mut app.session.transform_pivot,
            q0rg_id,
            &layers,
        );
    }
    app.state.project = project;
    if app.session.onion.edit_frames {
        crate::frame_edit::remap(app, &refs, &mapped);
    }
    // Resolved layer frames and FX snapshots are keyed by render_revision.
    // Clearing textures alone leaves those frames displaying the previous FX.
    app.state.mark_render_dirty();
    app.textures.invalidate();
    ui.ctx().request_repaint();
    app.session.status = format!("updated effects for {} selected objects", refs.len());
}

fn render_fx_controls(ui: &mut Ui, fx: &mut PlacementFx, values: &[PlacementFx]) -> FxEdits {
    let mut edits = FxEdits::default();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        crate::ui::section_title(ui, "FX");
        let reset = ui.add_enabled(
            values.iter().any(|value| !value.is_identity()),
            egui::Button::new("Reset"),
        );
        if reset.clicked() {
            let playback = fx.q0rg_playback;
            *fx = PlacementFx::default();
            fx.q0rg_playback = playback;
            edits.mark(FxField::Reset);
        }
    });

    egui::Grid::new("placement_fx_global")
        .num_columns(2)
        .show(ui, |ui| {
            fx_label(ui, "Alpha", FxField::Opacity, values);
            let mut percent = fx.opacity * 100.0;
            let response = ui.add(
                egui::Slider::new(&mut percent, 0.0..=100.0)
                    .suffix("%")
                    .show_value(true),
            );
            edits.response(FxField::Opacity, &response);
            if response.changed() {
                fx.opacity = (percent / 100.0).clamp(0.0, 1.0);
            }
            ui.end_row();

            ui.label("Blend");
            egui::ComboBox::from_id_source("placement_fx_blend")
                .selected_text(if FxField::Blend.mixed(values) {
                    "Mixed"
                } else {
                    blend_mode_name(fx.blend_mode)
                })
                .show_ui(ui, |ui| {
                    for mode in [
                        BlendMode::Normal,
                        BlendMode::Multiply,
                        BlendMode::Screen,
                        BlendMode::Add,
                        BlendMode::Overlay,
                    ] {
                        if ui
                            .selectable_value(&mut fx.blend_mode, mode, blend_mode_name(mode))
                            .clicked()
                        {
                            edits.mark(FxField::Blend);
                        }
                    }
                });
            ui.end_row();
        });

    ui.add_space(4.0);
    let mut tint_enabled = fx.tint.is_some();
    let toggle = fx_toggle(
        ui,
        &mut tint_enabled,
        FxField::TintEnabled,
        "Tint Color Effect",
        values,
    );
    if toggle.changed() {
        fx.tint = tint_enabled.then_some(q0s_format::v2::TintFx {
            color: Rgba {
                r: 255,
                g: 64,
                b: 64,
                a: 255,
            },
            amount: 0.5,
        });
        edits.mark(FxField::TintEnabled);
    }
    if let Some(tint) = fx.tint.as_mut() {
        ui.horizontal(|ui| {
            fx_label(ui, "Color", FxField::TintColor, values);
            let mut color = [tint.color.r, tint.color.g, tint.color.b];
            let response = crate::color_picker::color_edit_button_srgb(ui, &mut color);
            edits.response(FxField::TintColor, &response);
            if response.changed() {
                tint.color = Rgba {
                    r: color[0],
                    g: color[1],
                    b: color[2],
                    a: 255,
                };
            }
            fx_label(ui, "Amount", FxField::TintAmount, values);
            let mut percent = tint.amount * 100.0;
            let response = ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%"));
            edits.response(FxField::TintAmount, &response);
            if response.changed() {
                tint.amount = percent / 100.0;
            }
        });
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Filters").small().strong());

    let mut glow_enabled = fx.glow.is_some();
    let glow_toggle = fx_toggle(ui, &mut glow_enabled, FxField::GlowEnabled, "Glow", values);
    if glow_toggle.changed() {
        fx.glow = glow_enabled.then_some(GlowFx {
            color: Rgba {
                r: 255,
                g: 32,
                b: 24,
                a: 255,
            },
            radius_x: 8.0,
            radius_y: 8.0,
            strength: 1.0,
            quality: 3,
        });
        edits.mark(FxField::GlowEnabled);
    }
    if let Some(glow) = fx.glow.as_mut() {
        ui.indent("placement_glow", |ui| {
            egui::Grid::new("placement_glow_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    fx_label(ui, "Color", FxField::GlowColor, values);
                    let mut color = rgba_to_color32(glow.color);
                    let response = crate::color_picker::color_edit_button_srgba(ui, &mut color);
                    edits.response(FxField::GlowColor, &response);
                    if response.changed() {
                        glow.color = color32_to_rgba(color);
                    }
                    ui.end_row();
                    fx_label(ui, "Radius X", FxField::GlowRadiusX, values);
                    let response = ui.add(
                        egui::DragValue::new(&mut glow.radius_x)
                            .speed(0.25)
                            .clamp_range(0.0..=256.0)
                            .suffix(" px"),
                    );
                    edits.response(FxField::GlowRadiusX, &response);
                    ui.end_row();

                    fx_label(ui, "Radius Y", FxField::GlowRadiusY, values);
                    let response = ui.add(
                        egui::DragValue::new(&mut glow.radius_y)
                            .speed(0.25)
                            .clamp_range(0.0..=256.0)
                            .suffix(" px"),
                    );
                    edits.response(FxField::GlowRadiusY, &response);
                    ui.end_row();

                    fx_label(ui, "Strength", FxField::GlowStrength, values);
                    let response = ui.add(
                        egui::Slider::new(&mut glow.strength, 0.0..=4.0)
                            .suffix("x")
                            .show_value(true),
                    );
                    edits.response(FxField::GlowStrength, &response);
                    ui.end_row();

                    fx_label(ui, "Quality", FxField::GlowQuality, values);
                    let response =
                        ui.add(egui::DragValue::new(&mut glow.quality).clamp_range(1..=15));
                    edits.response(FxField::GlowQuality, &response);
                    ui.end_row();
                });
        });
    }

    let mut blur_enabled = fx.blur.is_some();
    let blur_toggle = fx_toggle(ui, &mut blur_enabled, FxField::BlurEnabled, "Blur", values);
    if blur_toggle.changed() {
        fx.blur = blur_enabled.then_some(BlurFx {
            radius_x: 4.0,
            radius_y: 4.0,
            quality: 3,
        });
        edits.mark(FxField::BlurEnabled);
    }
    if let Some(blur) = fx.blur.as_mut() {
        ui.indent("placement_blur", |ui| {
            egui::Grid::new("placement_blur_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    fx_label(ui, "Radius X", FxField::BlurRadiusX, values);
                    let response = ui.add(
                        egui::DragValue::new(&mut blur.radius_x)
                            .speed(0.25)
                            .clamp_range(0.0..=256.0)
                            .suffix(" px"),
                    );
                    edits.response(FxField::BlurRadiusX, &response);
                    ui.end_row();

                    fx_label(ui, "Radius Y", FxField::BlurRadiusY, values);
                    let response = ui.add(
                        egui::DragValue::new(&mut blur.radius_y)
                            .speed(0.25)
                            .clamp_range(0.0..=256.0)
                            .suffix(" px"),
                    );
                    edits.response(FxField::BlurRadiusY, &response);
                    ui.end_row();

                    fx_label(ui, "Quality", FxField::BlurQuality, values);
                    let response =
                        ui.add(egui::DragValue::new(&mut blur.quality).clamp_range(1..=15));
                    edits.response(FxField::BlurQuality, &response);
                    ui.end_row();
                });
        });
    }

    let mut shadow_enabled = fx.shadow.is_some();
    let shadow_toggle = fx_toggle(
        ui,
        &mut shadow_enabled,
        FxField::ShadowEnabled,
        "Drop shadow",
        values,
    );
    if shadow_toggle.changed() {
        fx.shadow = shadow_enabled.then_some(DropShadowFx {
            quality: 3,
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
        edits.mark(FxField::ShadowEnabled);
    }
    if let Some(shadow) = fx.shadow.as_mut() {
        ui.indent("placement_shadow", |ui| {
            egui::Grid::new("placement_shadow_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    fx_label(ui, "Color", FxField::ShadowColor, values);
                    let mut color = rgba_to_color32(shadow.color);
                    let response = crate::color_picker::color_edit_button_srgba(ui, &mut color);
                    edits.response(FxField::ShadowColor, &response);
                    if response.changed() {
                        shadow.color = color32_to_rgba(color);
                    }
                    ui.end_row();

                    fx_label(ui, "Blur", FxField::ShadowBlur, values);
                    let response = ui.add(
                        egui::DragValue::new(&mut shadow.blur_radius)
                            .speed(0.25)
                            .clamp_range(0.0..=256.0)
                            .suffix(" px"),
                    );
                    edits.response(FxField::ShadowBlur, &response);
                    ui.end_row();

                    fx_label(ui, "Offset X", FxField::ShadowOffsetX, values);
                    let response = ui.add(egui::DragValue::new(&mut shadow.offset_x).speed(0.25));
                    edits.response(FxField::ShadowOffsetX, &response);
                    ui.end_row();

                    fx_label(ui, "Offset Y", FxField::ShadowOffsetY, values);
                    let response = ui.add(egui::DragValue::new(&mut shadow.offset_y).speed(0.25));
                    edits.response(FxField::ShadowOffsetY, &response);
                    ui.end_row();

                    fx_label(ui, "Strength", FxField::ShadowStrength, values);
                    let response = ui.add(
                        egui::Slider::new(&mut shadow.strength, 0.0..=4.0)
                            .suffix("x")
                            .show_value(true),
                    );
                    edits.response(FxField::ShadowStrength, &response);
                    ui.end_row();

                    fx_label(ui, "quality", FxField::ShadowQuality, values);
                    let response =
                        ui.add(egui::DragValue::new(&mut shadow.quality).clamp_range(1..=15));
                    edits.response(FxField::ShadowQuality, &response);
                    ui.end_row();
                });
        });
    }

    edits
}

fn propagate_manual_q0rg_playback(
    project: &mut q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    selected_placement_idx: usize,
    frame: u16,
    playback: q0s_format::v2::Q0rgPlayback,
) {
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
        return;
    };
    let Some(selected) = layer.placements.get(selected_placement_idx) else {
        return;
    };
    if selected.frame != frame {
        return;
    }
    let target = selected.target;
    if !matches!(target, Target::Q0rg(_)) {
        return;
    }
    let instance_id = selected.instance_id;
    if instance_id != 0 {
        for placement in &mut layer.placements {
            if placement.frame >= frame
                && placement.instance_id == instance_id
                && placement.target == target
            {
                placement.fx.q0rg_playback = playback;
            }
        }
        return;
    }

    // Older placements without persistent IDs follow their order at each key.
    let occurrence = layer
        .placements
        .iter()
        .take(selected_placement_idx)
        .filter(|placement| placement.frame == frame && placement.target == target)
        .count();
    let mut positions = std::collections::BTreeMap::<u16, usize>::new();
    for placement in &mut layer.placements {
        if placement.frame < frame || placement.target != target {
            continue;
        }
        let position = positions.entry(placement.frame).or_default();
        if *position == occurrence && placement.instance_id == 0 {
            placement.fx.q0rg_playback = playback;
        }
        *position += 1;
    }
}

/// Persist only after the numeric edit succeeded (and a held key was isolated).
fn store_rotation_pivot(
    app: &mut EditorApp,
    reference: PlacementRef,
    pivot: Option<q0s_format::v2::Vec2>,
) {
    let Some(pivot) = pivot else {
        return;
    };
    if let Some(placement) = app
        .state
        .project
        .q0rgs
        .iter_mut()
        .find(|q| q.q0rg_id == reference.q0rg_id)
        .and_then(|q| {
            q.layers
                .iter_mut()
                .find(|layer| layer.layer_id == reference.layer_id)
        })
        .and_then(|layer| layer.placements.get_mut(reference.placement_idx))
    {
        placement.pivot = Some(pivot);
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
            pivot: None,
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
                radius_x: 8.0,
                radius_y: 8.0,
                strength: 1.0,
                quality: 3,
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
        app.session.fill_color = Some(crate::state::DEFAULT_FILL);
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
        assert!(painted_text.contains("Fill paint"));
        assert!(painted_text.contains("Fill opacity"));
    }

    /// The text a frame of the Properties panel paints for `app`.
    fn painted_properties_text(app: &mut EditorApp) -> String {
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(420.0, 900.0),
            )),
            ..Default::default()
        };
        let _first_frame = context.run(input.clone(), |context| {
            egui::CentralPanel::default().show(context, |ui| render(app, ui));
        });
        let output = context.run(input, |context| {
            egui::CentralPanel::default().show(context, |ui| render(app, ui));
        });
        let mut painted_text = String::new();
        for clipped in &output.shapes {
            collect_painted_text(&clipped.shape, &mut painted_text);
        }
        painted_text
    }

    #[test]
    fn shape_tools_show_fill_and_stroke_with_no_on_off_switches() {
        let mut app = EditorApp::default();
        app.session.current_tool = Tool::Oval;
        let text = painted_properties_text(&mut app);
        assert!(text.contains("Stroke color") && text.contains("Fill paint"));
        assert!(text.contains("Stroke opacity") && text.contains("Fill opacity"));
        assert!(!text.contains("Enabled"), "none lives in the picker now");
    }

    #[test]
    fn a_paint_of_none_keeps_its_box_and_drops_its_opacity() {
        let mut app = EditorApp::default();
        app.session.current_tool = Tool::Rectangle;
        app.session.fill_color = None;
        let text = painted_properties_text(&mut app);
        assert!(
            text.contains("Fill paint"),
            "none is picked in the same box"
        );
        assert!(!text.contains("Fill opacity"));
        assert!(text.contains("Stroke opacity"));

        app.session.fill_color = Some(crate::state::DEFAULT_FILL);
        app.session.stroke_none = true;
        let text = painted_properties_text(&mut app);
        assert!(text.contains("Fill opacity") && !text.contains("Stroke opacity"));
    }

    #[test]
    fn the_bucket_pours_its_colour_even_when_the_fill_is_none() {
        let mut app = EditorApp::default();
        app.session.current_tool = Tool::Bucket;
        app.session.fill_color = None;
        app.session.fill_kept = Rgba {
            r: 12,
            g: 34,
            b: 56,
            a: 255,
        };
        let _ = painted_properties_text(&mut app);
        assert_eq!(
            app.session.fill_color, None,
            "showing the bucket changes nothing"
        );
    }

    #[test]
    fn every_tool_gets_a_specific_properties_kind() {
        let kinds = [
            tool_property_kind(Tool::Select),
            tool_property_kind(Tool::Lasso),
            tool_property_kind(Tool::Subselect),
            tool_property_kind(Tool::Pen),
            tool_property_kind(Tool::Pencil),
            tool_property_kind(Tool::Brush),
            tool_property_kind(Tool::Eraser),
            tool_property_kind(Tool::Line),
            tool_property_kind(Tool::Rectangle),
            tool_property_kind(Tool::Bucket),
            tool_property_kind(Tool::Eyedropper),
            tool_property_kind(Tool::Text),
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

#[cfg(test)]
mod tab_tests {
    use super::*;

    fn seen(selection: Selection, tool: Tool, context: Option<&'static str>) -> Seen {
        Seen {
            selection,
            tool,
            context,
        }
    }

    #[test]
    fn the_page_follows_what_was_touched() {
        let idle = seen(Selection::None, Tool::Select, None);
        assert_eq!(follow_focus(None, None, &idle), Tab::Stage);
        // Picking a symbol in the library brings its page forward.
        let symbol = seen(Selection::Q0rg(4), Tool::Select, Some("Symbol"));
        assert_eq!(
            follow_focus(Some(Tab::Stage), Some(&idle), &symbol),
            Tab::Selection
        );
        // Picking a tool brings the tool's page forward.
        let brush = seen(Selection::Q0rg(4), Tool::Brush, Some("Symbol"));
        assert_eq!(
            follow_focus(Some(Tab::Selection), Some(&symbol), &brush),
            Tab::Tool
        );
        // Clicking away leaves the selection's page for the stage's.
        let away = seen(Selection::None, Tool::Select, None);
        assert_eq!(
            follow_focus(Some(Tab::Selection), Some(&symbol), &away),
            Tab::Stage
        );
        // A page picked by hand stays while nothing changes.
        assert_eq!(follow_focus(Some(Tab::Tool), Some(&idle), &idle), Tab::Tool);
    }

    #[test]
    fn drawing_does_not_pull_the_page_away_from_the_tool() {
        let before = seen(Selection::None, Tool::Brush, None);
        let drawn = seen(Selection::Paths(Vec::new()), Tool::Brush, Some("Shape"));
        assert_eq!(
            follow_focus(Some(Tab::Tool), Some(&before), &drawn),
            Tab::Tool
        );
    }
}
