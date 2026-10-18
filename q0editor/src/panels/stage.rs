use egui::{
    pos2, vec2, Align2, Color32, FontId, Key, PointerButton, Rect, Sense, Shape, Stroke, Ui,
};

use crate::app::{Action, EditorApp};
use crate::render::{
    centered_target_transform, render_target_preview_with_revision, StageRenderPass, StageView,
};
use crate::state::{LibraryDragPayload, LibraryItem};
use crate::ui::FitToScreen;

pub const ZOOM_MIN: f32 = 0.05;
pub const ZOOM_MAX: f32 = 32.0;
const ZOOM_WHEEL_FACTOR: f32 = 0.0015;
const SYMBOL_EDIT_MASK_ALPHA: u8 = 48;

pub(crate) fn render_animation_preview(app: &mut EditorApp, ctx: &egui::Context) {
    if !app.session.show_animation_preview {
        app.session.animation_preview_playing = false;
        if app.animation_preview_revision.take().is_some() {
            app.animation_preview_textures = Default::default();
        }
        return;
    }
    let root = app.state.project.meta.entry_q0rg_id;
    let frame_count = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == root)
        .map(|q| q.frame_count.max(1))
        .unwrap_or(1);
    let (loop_start, loop_end) = app
        .session
        .preview_loop_ranges
        .get(&root)
        .filter(|range| range.0)
        .map(|range| {
            let start = range.1.min(frame_count - 1);
            (start, range.2.max(start).min(frame_count - 1))
        })
        .unwrap_or((0, frame_count - 1));
    app.session.animation_preview_frame = app.session.animation_preview_frame.min(frame_count - 1);
    if app.session.animation_preview_playing {
        let interval =
            std::time::Duration::from_secs_f64(1.0 / f64::from(app.state.project.meta.fps.max(1)));
        let elapsed = app.session.animation_preview_tick.elapsed();
        let steps = elapsed.as_nanos() / interval.as_nanos().max(1);
        if steps > 0 {
            let current = app.session.animation_preview_frame;
            let current = if current < loop_start || current > loop_end {
                loop_start
            } else {
                current
            };
            app.session.animation_preview_frame = loop_start
                + ((u128::from(current - loop_start) + steps)
                    % u128::from(loop_end - loop_start + 1)) as u16;
            app.session.animation_preview_tick = std::time::Instant::now()
                - std::time::Duration::from_nanos(
                    (elapsed.as_nanos() % interval.as_nanos().max(1)) as u64,
                );
        }
        ctx.request_repaint_after(interval);
    }
    let mut open = true;
    egui::Window::new("Animation Preview")
        .id(egui::Id::new("animation_camera_preview"))
        .open(&mut open)
        .default_size(vec2(560.0, 390.0))
        .fit_to_screen(ctx)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button(if app.session.animation_preview_playing {
                        "Pause"
                    } else {
                        "Play"
                    })
                    .clicked()
                {
                    app.session.animation_preview_playing = !app.session.animation_preview_playing;
                    if app.session.animation_preview_playing
                        && (app.session.animation_preview_frame < loop_start
                            || app.session.animation_preview_frame > loop_end)
                    {
                        app.session.animation_preview_frame = loop_start;
                    }
                    app.session.animation_preview_tick = std::time::Instant::now();
                }
                if ui.button("Start").clicked() {
                    app.session.animation_preview_frame = loop_start;
                }
                let mut display_frame = app.session.animation_preview_frame + 1;
                if ui
                    .add(egui::Slider::new(&mut display_frame, 1..=frame_count).text("Frame"))
                    .changed()
                {
                    app.session.animation_preview_frame = display_frame - 1;
                    app.session.animation_preview_tick = std::time::Instant::now();
                }
            });
            let width = f32::from(app.state.project.meta.stage_width).max(1.0);
            let height = f32::from(app.state.project.meta.stage_height).max(1.0);
            let available = ui.available_size();
            let scale = (available.x.max(1.0) / width).min(available.y.max(1.0) / height);
            let (rect, _) =
                ui.allocate_exact_size(vec2(width * scale, height * scale), Sense::hover());
            let painter = ui.painter_at(rect);
            let bg = app.state.project.meta.background_color;
            painter.rect_filled(
                rect,
                0.0,
                Color32::from_rgba_unmultiplied(bg.r, bg.g, bg.b, bg.a),
            );
            let frame = app.session.animation_preview_frame;
            let view = StageView {
                origin: rect.min,
                scale,
                stage_rect: rect,
                edit_transform: if app.state.project.runtime.camera.enabled {
                    app.state.project.camera_affine_at(frame)
                } else {
                    q0s_format::transform::Affine::IDENTITY
                },
            };
            let revision = (
                app.active_project_tab_id().unwrap_or(0),
                app.state.revision(),
            );
            if app.animation_preview_revision != Some(revision) {
                app.animation_preview_textures.invalidate();
                app.animation_preview_revision = Some(revision);
            }
            app.animation_preview_textures.video_playback_active =
                app.session.animation_preview_playing;
            let pass = StageRenderPass::new_with_revision(
                &app.state.project,
                &mut app.animation_preview_textures,
                Some(revision),
            );
            pass.render(
                &painter,
                root,
                frame,
                &view,
                &mut app.animation_preview_textures,
                ctx,
            );
        });
    if !open {
        app.session.show_animation_preview = false;
        app.session.animation_preview_playing = false;
    }
}

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    let stage_started = std::time::Instant::now();
    if !crate::touch::ENABLED {
        ui.horizontal(|ui| {
            breadcrumbs(app, ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                zoom_controls(app, ui);
            });
        });
        ui.separator();
    }

    let avail = ui.available_size_before_wrap();
    let (canvas_rect, response) = ui.allocate_exact_size(
        vec2(avail.x.max(40.0), avail.y.max(40.0)),
        Sense::click_and_drag(),
    );
    let painter = ui.painter_at(canvas_rect);

    // Clicking the stage leaves any Properties/TextEdit field behind. Without
    // this, egui keeps keyboard focus in the last text box and correctly blocks
    // Backspace/Ctrl+G as text-editing shortcuts even though the user is already
    // manipulating artwork on the canvas.
    let canvas_interacted = response.clicked_by(PointerButton::Primary)
        || response.drag_started_by(PointerButton::Primary)
        || response.secondary_clicked();
    release_canvas_keyboard_focus(
        ui.ctx(),
        canvas_interacted && app.session.text_editor.is_none(),
    );
    if canvas_interacted {
        // A stale row selection makes Copy/Cut treat stage artwork as a
        // whole-layer operation. A canvas interaction returns focus to stage.
        app.session.timeline_selection = None;
        app.session.timeline_cell_selection = None;
        app.session.timeline_layer_selection = None;
        app.session.pending_timeline_frame = None;
    }

    // Canvas backdrop — area outside the stage paper. An optional theme
    // bitmap is cropped to cover the viewport, with the theme colour as fallback.
    let backdrop_path = app.settings.theme.canvas_background_image.clone();
    let backdrop_darkening = app.settings.theme.canvas_background_darkening;
    let painted_image = backdrop_path.as_deref().and_then(|path| {
        app.textures
            .canvas_background_texture(ui.ctx(), path)
            .map(|(texture, [width, height])| {
                let image_size = vec2(width as f32, height as f32);
                let scale =
                    (canvas_rect.width() / image_size.x).max(canvas_rect.height() / image_size.y);
                let image_rect = Rect::from_center_size(canvas_rect.center(), image_size * scale);
                painter.image(
                    texture,
                    image_rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            })
    });
    if painted_image.is_none() {
        painter.rect_filled(canvas_rect, 0.0, app.settings.theme.canvas_bg.to_color32());
    } else if backdrop_darkening > 0 {
        let alpha = ((u16::from(backdrop_darkening) * 255) / 100) as u8;
        painter.rect_filled(canvas_rect, 0.0, Color32::from_black_alpha(alpha));
    }

    let stage_w = app.state.project.meta.stage_width as f32;
    let stage_h = app.state.project.meta.stage_height as f32;

    // Auto-fit base scale; user zoom is a multiplier on top of that, so zoom=1
    // always means "fits the canvas no matter the canvas size".
    let fit_scale = (canvas_rect.width() * 0.9 / stage_w)
        .min(canvas_rect.height() * 0.9 / stage_h)
        .max(ZOOM_MIN);

    handle_viewport_input(app, &response, canvas_rect, fit_scale, ui.ctx());
    let frame_time = ui.ctx().input(|input| input.predicted_dt);
    if app.session.viewport.step_glide(frame_time) {
        ui.ctx().request_repaint();
    }

    let scale = (fit_scale * app.session.viewport.zoom).clamp(ZOOM_MIN, fit_scale * ZOOM_MAX);
    let scaled = vec2(stage_w * scale, stage_h * scale);
    let center =
        canvas_rect.center() + vec2(app.session.viewport.pan.x, app.session.viewport.pan.y);
    let stage_rect = Rect::from_min_size(
        pos2(center.x - scaled.x * 0.5, center.y - scaled.y * 0.5),
        scaled,
    );

    // Drop shadow — fixed translucent black, theme-independent.
    painter.rect_filled(
        stage_rect.translate(vec2(2.0, 2.0)),
        0.0,
        Color32::from_black_alpha(110),
    );
    // Stage paper — always white (it's the user's drawing canvas, not
    // chrome). The border colour does follow the theme so dark/light
    // backdrops can frame it appropriately.
    let bg = app.state.project.meta.background_color;
    painter.rect_filled(
        stage_rect,
        0.0,
        Color32::from_rgba_unmultiplied(bg.r, bg.g, bg.b, bg.a),
    );
    painter.rect_stroke(
        stage_rect,
        0.0,
        Stroke::new(1.0_f32, app.settings.theme.stage_border.to_color32()),
    );

    let view = StageView {
        origin: stage_rect.min,
        scale,
        stage_rect,
        edit_transform: app
            .session
            .edit_in_place
            .last()
            .map(|context| context.child_to_stage)
            .unwrap_or(q0s_format::transform::Affine::IDENTITY),
    };
    app.textures.editing_mask_q0rg_id = Some(app.session.current_q0rg_id);
    if let Some(pointer) = ui
        .ctx()
        .pointer_hover_pos()
        .filter(|pointer| canvas_rect.contains(*pointer))
    {
        let stage_cursor = view.screen_to_stage(pointer);
        if stage_cursor.x.is_finite() && stage_cursor.y.is_finite() {
            app.session.stage_cursor = Some(stage_cursor);
        }
    }

    // Resolve interaction before painting the project. egui is immediate-mode:
    // if tools mutate the model after `render_stage`, artwork is one UI frame
    // behind its selection handles. Input-first keeps both on the same state.
    let editing_text = app.session.text_editor.is_some() || app.session.text_canvas_click_consumed;
    let library_payload = response.dnd_hover_payload::<LibraryDragPayload>();
    let library_drag_active = library_payload.is_some();
    let ctx = ui.ctx().clone();
    let camera_editing = app.camera_track_is_selected();
    if camera_editing && !library_drag_active && !editing_text {
        handle_camera_stage_input(app, &response, &view, &ctx);
    }
    if app.session.current_q0rg_id == app.state.project.meta.entry_q0rg_id
        && app.state.project.runtime.camera.enabled
    {
        draw_camera_background_outside_stage(
            app,
            &painter,
            &view,
            canvas_rect,
            app.session.current_frame,
        );
    }
    if !library_drag_active && !editing_text && !camera_editing {
        let tools_started = std::time::Instant::now();
        if !crate::frame_edit::handle(app, &response, &view, &ctx) {
            crate::tools::handle(app, &response, &painter, &view, &ctx);
        }
        app.perf
            .record(crate::perf::PerfSpan::ToolHandle, tools_started.elapsed());
    }
    // Tool input can enter another q0rg on double-click, so resolve the scene
    // identity only after interaction has finished for this frame.
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    let outlined_layers = app
        .session
        .outline_layers
        .iter()
        .filter_map(|(outlined_q0rg, layer_id)| (*outlined_q0rg == q0rg_id).then_some(*layer_id))
        .collect::<std::collections::HashSet<_>>();
    let text_preview = crate::text_tool::scene_preview(app, &ctx);
    // A running physics preview draws its baked copy of the project instead.
    let physics_preview = text_preview
        .is_none()
        .then(|| crate::physics::stage_preview(app))
        .flatten();
    let preview_state_id = egui::Id::new("painted_text_scene_preview");
    let previous_preview = ctx.data(|data| data.get_temp::<(u64, u16)>(preview_state_id));
    let current_preview = text_preview.as_ref().map(|p| (p.revision, p.asset_id));
    if previous_preview != current_preview {
        if let Some((_, id)) = current_preview.or(previous_preview) {
            app.textures.invalidate_asset(id);
        }
        ctx.data_mut(|data| {
            if let Some(current) = current_preview {
                data.insert_temp(preview_state_id, current);
            } else {
                data.remove::<(u64, u16)>(preview_state_id);
            }
        });
    }
    app.textures.hidden_text_placement = if text_preview.is_some() {
        None
    } else {
        app.session
            .text_editor
            .as_ref()
            .and_then(|draft| draft.placement)
            .map(|r| (r.q0rg_id, r.layer_id, r.placement_idx))
    };
    app.textures.interactive_transform = ui.input(|i| i.pointer.primary_down());
    app.textures.gpu_filter_cache_enabled = !app.session.playing
        && !app.textures.interactive_transform
        && !app.session.onion.enabled
        && !app.session.onion.edit_frames;

    // Editing inside a q0rg uses a subtle full-stage veil. The veil is drawn
    // before the symbol, so the current q0rg remains crisp while the canvas
    // behind it recedes. Its RGB comes from the active theme/.q7s.
    if !breadcrumb_is_at_root(app) {
        draw_symbol_edit_backdrop(app, &painter, &view);
    }

    // A single render pass owns immutable id lookups and rig-pose memoization for
    // every onion frame plus the live frame painted below.
    let render_revision = (
        app.active_project_tab_id().unwrap_or(0),
        text_preview
            .as_ref()
            .map(|p| p.revision)
            .or(physics_preview.as_ref().map(|(_, revision)| *revision))
            .unwrap_or(app.state.render_revision()),
    );
    let hidden_mask_art_layers = app
        .session
        .hidden_mask_art_layers
        .iter()
        .filter_map(|(project_id, hidden_q0rg_id, layer_id)| {
            (*project_id == render_revision.0).then_some((*hidden_q0rg_id, *layer_id))
        })
        .collect::<std::collections::HashSet<_>>();
    if app.textures.hidden_mask_art_layers != hidden_mask_art_layers {
        app.textures.mask_visibility_revision =
            app.textures.mask_visibility_revision.wrapping_add(1);
        app.textures.hidden_mask_art_layers = hidden_mask_art_layers;
    }
    app.textures.editing_mask_q0rg_id = Some(q0rg_id);
    app.textures.video_playback_active = app.session.playing;
    app.textures.reset_stage_traversal_stats();
    q0glblend::reset_paint_diagnostics();
    let scene_started = std::time::Instant::now();
    let render_pass = StageRenderPass::new_with_revision(
        text_preview
            .as_ref()
            .map(|p| p.project.as_ref())
            .or(physics_preview
                .as_ref()
                .map(|(project, _)| project.as_ref()))
            .unwrap_or(&app.state.project),
        &mut app.textures,
        Some(render_revision),
    );
    if let Some(context) = app.session.edit_in_place.last().copied() {
        let parent_view = StageView {
            edit_transform: context.parent_to_stage,
            ..view
        };
        render_pass.render_tinted_excluding_placement(
            &painter,
            context.parent_q0rg_id,
            context.parent_frame,
            &parent_view,
            &mut app.textures,
            ui.ctx(),
            Color32::from_rgba_unmultiplied(175, 175, 185, 150),
            context.layer_id,
            context.placement_idx,
        );
    }
    if app.session.outline_mode {
        crate::render::render_stage_outline(
            &painter,
            &render_pass,
            q0rg_id,
            frame,
            &view,
            &app.textures.hidden_mask_art_layers,
        );
    } else {
        // Onion skin: render `before` past frames + `after` upcoming frames with
        // a colour-tinted alpha behind the live frame. Past frames get a cool
        // (blueish) tint, upcoming a warm (orange) tint — Flash convention.
        let onion_split = if app.session.onion.enabled && !app.session.onion.all_unlocked_layers {
            app.state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == q0rg_id)
                .and_then(|q| {
                    q.layers
                        .iter()
                        .position(|layer| {
                            layer.layer_id == app.session.current_layer_id
                                && app.state.project.layer_is_visible(q0rg_id, layer.layer_id)
                                && !app.state.project.layer_is_locked(q0rg_id, layer.layer_id)
                                && !app.state.project.layer_is_folder(q0rg_id, layer.layer_id)
                        })
                        .map(|index| {
                            let below: std::collections::HashSet<_> = q.layers[..index]
                                .iter()
                                .map(|layer| layer.layer_id)
                                .collect();
                            let above: std::collections::HashSet<_> = q.layers[index..]
                                .iter()
                                .map(|layer| layer.layer_id)
                                .collect();
                            (below, above)
                        })
                })
        } else {
            None
        };
        if let Some((_, above)) = &onion_split {
            let excluded: std::collections::HashSet<_> =
                above.union(&outlined_layers).copied().collect();
            render_pass.render_tinted_excluding_layers(
                &painter,
                q0rg_id,
                frame,
                &view,
                &mut app.textures,
                ui.ctx(),
                Color32::WHITE,
                &excluded,
            );
        }
        if app.session.onion.edit_frames {
            for (sample, layers) in crate::frame_edit::preview_layers(app) {
                let excluded = app
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q| q.q0rg_id == q0rg_id)
                    .into_iter()
                    .flat_map(|q| &q.layers)
                    .filter(|l| !layers.contains(&l.layer_id))
                    .map(|l| l.layer_id)
                    .collect();
                render_pass.render_tinted_excluding_layers(
                    &painter,
                    q0rg_id,
                    sample,
                    &view,
                    &mut app.textures,
                    ui.ctx(),
                    Color32::WHITE,
                    &excluded,
                );
            }
        }
        if app.session.onion.enabled {
            let excluded_layers: std::collections::HashSet<_> = app
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == q0rg_id)
                .into_iter()
                .flat_map(|q| &q.layers)
                .filter(|layer| {
                    app.state.project.layer_is_locked(q0rg_id, layer.layer_id)
                        || (!app.session.onion.all_unlocked_layers
                            && layer.layer_id != app.session.current_layer_id)
                })
                .map(|layer| layer.layer_id)
                .collect();
            let has_onion_layers = app
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == q0rg_id)
                .is_some_and(|q| {
                    q.layers.iter().any(|layer| {
                        !excluded_layers.contains(&layer.layer_id)
                            && !app.state.project.layer_is_folder(q0rg_id, layer.layer_id)
                            && app.state.project.layer_is_visible(q0rg_id, layer.layer_id)
                    })
                });
            let frame_count = app
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == q0rg_id)
                .map(|q| q.frame_count)
                .unwrap_or(0);
            if frame_count > 0 && has_onion_layers {
                for offset in (-(app.session.onion.before as i32)
                    ..=(app.session.onion.after as i32))
                    .filter(|o| *o != 0)
                {
                    let target = frame as i32 + offset;
                    if target < 0 || target >= frame_count as i32 {
                        continue;
                    }
                    let tint = onion_tint_for(
                        offset,
                        app.session.onion.before,
                        app.session.onion.after,
                        app.settings.theme.onion_past.to_color32(),
                        app.settings.theme.onion_future.to_color32(),
                    );
                    render_pass.render_tinted_excluding_layers(
                        &painter,
                        q0rg_id,
                        target as u16,
                        &view,
                        &mut app.textures,
                        ui.ctx(),
                        tint,
                        &excluded_layers,
                    );
                }
            }
        }

        if let Some((below, _)) = &onion_split {
            let excluded: std::collections::HashSet<_> =
                below.union(&outlined_layers).copied().collect();
            render_pass.render_tinted_excluding_layers(
                &painter,
                q0rg_id,
                frame,
                &view,
                &mut app.textures,
                ui.ctx(),
                Color32::WHITE,
                &excluded,
            );
            if !outlined_layers.is_empty() {
                crate::render::render_stage_outline_layers(
                    &painter,
                    &render_pass,
                    q0rg_id,
                    frame,
                    &view,
                    &outlined_layers,
                    &app.textures.hidden_mask_art_layers,
                );
            }
        } else if outlined_layers.is_empty() {
            render_pass.render(&painter, q0rg_id, frame, &view, &mut app.textures, ui.ctx());
        } else {
            render_pass.render_tinted_excluding_layers(
                &painter,
                q0rg_id,
                frame,
                &view,
                &mut app.textures,
                ui.ctx(),
                Color32::WHITE,
                &outlined_layers,
            );
            crate::render::render_stage_outline_layers(
                &painter,
                &render_pass,
                q0rg_id,
                frame,
                &view,
                &outlined_layers,
                &app.textures.hidden_mask_art_layers,
            );
        }
    }
    let scene_elapsed = scene_started.elapsed();
    let traversal = app.textures.stage_traversal_stats();
    app.textures.record_stage_work_timings(&mut app.perf);
    let gpu = q0glblend::take_paint_diagnostics();
    let gpu_timing = q0glblend::take_gpu_callback_timing();
    app.perf.set_scene_context(crate::perf::ScenePerfContext {
        zoom: app.session.viewport.zoom,
        view_scale: view.effective_scale(),
        pan_x: app.session.viewport.pan.x,
        pan_y: app.session.viewport.pan.y,
        pointer_over_stage: response.hovered(),
        cursor_stage: response.hover_pos().map(|position| {
            let stage = view.screen_to_stage(position);
            [stage.x, stage.y]
        }),
        q0rg_id,
        frame,
        tool: app.session.current_tool.label(),
        q0rg_visits: traversal.q0rg_visits,
        active_placements: traversal.active_placements,
        vector_instances_batched: traversal.vector_instances_batched,
        vector_batches: traversal.vector_batches,
        vector_vertices: traversal.vector_vertices,
        vector_indices: traversal.vector_indices,
        vector_fallbacks: traversal.vector_fallbacks,
        gpu_callbacks: gpu.callbacks,
        filtered_callbacks: gpu.filtered_callbacks,
        blur_passes: gpu.blur_passes,
        glow_passes: gpu.glow_passes,
        filter_pixel_passes: gpu.filter_pixel_passes,
        max_filter_region_pixels: gpu.max_filter_region_pixels,
        gpu_timer_initialized: gpu_timing.timer_queries_initialized,
        gpu_timer_supported: gpu_timing.timer_queries_supported,
        gpu_filter_samples: gpu_timing.gpu_samples,
        gpu_filter_total_ns: gpu_timing.gpu_total_ns,
        gpu_filter_max_ns: gpu_timing.gpu_max_ns,
        gpu_filter_dropped_samples: gpu_timing.gpu_dropped_samples,
        gpu_filter_cpu_callbacks: gpu_timing.cpu_samples,
        gpu_filter_cpu_total_ns: gpu_timing.cpu_total_ns,
        gpu_filter_cpu_max_ns: gpu_timing.cpu_max_ns,
        gpu_filter_cpu_max_region_width: gpu_timing.cpu_max_region_width,
        gpu_filter_cpu_max_region_height: gpu_timing.cpu_max_region_height,
        gpu_filter_cpu_max_blur_passes: gpu_timing.cpu_max_blur_passes,
        gpu_filter_cpu_max_glow_passes: gpu_timing.cpu_max_glow_passes,
        gpu_filter_cpu_max_primitive_count: gpu_timing.cpu_max_primitive_count,
        gpu_filter_cpu_max_vertex_count: gpu_timing.cpu_max_vertex_count,
        gpu_filter_cpu_max_index_count: gpu_timing.cpu_max_index_count,
        gpu_filter_cpu_max_target_setup_ns: gpu_timing.cpu_max_target_setup_ns,
        gpu_filter_cpu_max_raster_ns: gpu_timing.cpu_max_raster_ns,
        gpu_filter_cpu_max_raster_clear_ns: gpu_timing.cpu_max_raster_clear_ns,
        gpu_filter_cpu_max_raster_upload_ns: gpu_timing.cpu_max_raster_upload_ns,
        gpu_filter_cpu_max_raster_draw_ns: gpu_timing.cpu_max_raster_draw_ns,
        gpu_filter_cpu_max_raster_resolve_ns: gpu_timing.cpu_max_raster_resolve_ns,
        gpu_filter_cpu_max_raster_other_ns: gpu_timing.cpu_max_raster_other_ns,
        gpu_filter_cpu_max_filter_ns: gpu_timing.cpu_max_filter_ns,
        gpu_filter_cpu_max_composite_ns: gpu_timing.cpu_max_composite_ns,
        gpu_filter_cache_hits: gpu_timing.filtered_cache_hits,
        gpu_filter_cache_misses: gpu_timing.filtered_cache_misses,
    });
    app.perf
        .record(crate::perf::PerfSpan::SceneRender, scene_elapsed);

    if q0rg_id == app.state.project.meta.entry_q0rg_id && app.state.project.runtime.camera.enabled {
        draw_camera_overlay(app, &painter, &view, frame);
    }

    // Library drag-and-drop uses the real renderer for items from the active
    // project. Cross-project ids may collide with the active texture cache, so
    // foreign items use their source project's real bounds as a safe ghost until
    // drop imports/remaps the graph into this project.
    if let (Some(payload), Some(pointer)) =
        (library_payload.as_deref(), ui.ctx().pointer_interact_pos())
    {
        if crate::panels::library::library_drag_item_is_audio_only(app, *payload) {
            // Audio is timeline media, never a display object.
        } else {
            let stage_position = view.screen_to_stage(pointer);
            if Some(payload.source_tab_id) == app.active_project_tab_id() {
                if let Some(target) = library_item_target(&app.state.project, payload.item) {
                    let transform =
                        centered_target_transform(&app.state.project, target, stage_position);
                    render_target_preview_with_revision(
                        &painter,
                        &app.state.project,
                        target,
                        frame,
                        transform,
                        &view,
                        &mut app.textures,
                        ui.ctx(),
                        Color32::from_white_alpha(176),
                        Some(render_revision),
                    );
                    let preview = q0s_format::v2::Placement {
                        instance_id: 0,
                        frame,
                        target,
                        transform,
                        tween: q0s_format::v2::Tween::None,
                        fx: Default::default(),
                    };
                    if let Some((min_x, min_y, max_x, max_y)) =
                        crate::render::placement_bbox(&app.state.project, &preview)
                    {
                        painter.rect_stroke(
                            view.stage_bounds_to_screen(
                                q0s_format::v2::Vec2::new(min_x, min_y),
                                q0s_format::v2::Vec2::new(max_x, max_y),
                            ),
                            0.0,
                            Stroke::new(1.5_f32, app.settings.theme.accent.to_color32()),
                        );
                    }
                }
            } else if let Some(source) = app.project_for_document_tab(payload.source_tab_id) {
                if let Some(target) = library_item_target(source, payload.item) {
                    let transform = centered_target_transform(source, target, stage_position);
                    if let Some((min_x, min_y, max_x, max_y)) =
                        crate::render::target_local_bbox(source, target, None)
                    {
                        let affine = q0s_format::transform::Affine::from_transform(transform);
                        let corners = [
                            affine.apply(q0s_format::v2::Vec2::new(min_x, min_y)),
                            affine.apply(q0s_format::v2::Vec2::new(max_x, min_y)),
                            affine.apply(q0s_format::v2::Vec2::new(max_x, max_y)),
                            affine.apply(q0s_format::v2::Vec2::new(min_x, max_y)),
                        ];
                        let min_x = corners.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
                        let min_y = corners.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
                        let max_x = corners
                            .iter()
                            .map(|p| p.x)
                            .fold(f32::NEG_INFINITY, f32::max);
                        let max_y = corners
                            .iter()
                            .map(|p| p.y)
                            .fold(f32::NEG_INFINITY, f32::max);
                        painter.rect_stroke(
                            view.stage_bounds_to_screen(
                                q0s_format::v2::Vec2::new(min_x, min_y),
                                q0s_format::v2::Vec2::new(max_x, max_y),
                            ),
                            0.0,
                            Stroke::new(1.5_f32, app.settings.theme.accent.to_color32()),
                        );
                    }
                }
            }
        }
    }
    if let Some(payload) = response.dnd_release_payload::<LibraryDragPayload>() {
        if crate::panels::library::library_drag_item_is_audio_only(app, *payload) {
            app.session.status = "drop audio onto a timeline frame".to_string();
        } else if let Some(pointer) = ui.ctx().pointer_interact_pos() {
            let stage_position = view.screen_to_stage(pointer);
            app.queue(Action::PlaceLibraryDragAt(*payload, stage_position));
        }
    }

    // Canvas text UI and transient overlays stay above the freshly rendered scene.
    let editing_text = crate::text_tool::render_canvas(app, ui, &view);
    if !editing_text && app.session.onion.edit_frames {
        crate::frame_edit::draw_overlay(app, &painter, &view);
    } else if !editing_text {
        crate::tools::draw_overlay(app, &response, &painter, &view);
        let selection_started = std::time::Instant::now();
        crate::tools::draw_selection_overlay(app, &painter, &view);
        app.perf.record(
            crate::perf::PerfSpan::SelectionOverlay,
            selection_started.elapsed(),
        );
    }
    crate::tools::draw_brush_size_preview(app, &painter, &view);

    // Right-click context menu — pre-select the placement under cursor so the
    // menu's actions act on what the user actually right-clicked.
    if response.secondary_clicked() && !app.session.onion.edit_frames && !editing_text {
        if let Some(p) = response.interact_pointer_pos() {
            let stage_pos = view.screen_to_stage(p);
            let clicked_selection = crate::tools::selection_at_point_cached_for_scale(
                &app.state.project,
                &mut app.textures,
                q0rg_id,
                app.session.current_frame,
                stage_pos,
                view.effective_scale(),
            )
            .unwrap_or(crate::state::Selection::None);
            let clicked_objects =
                crate::selection_edit::selected_object_refs(&app.state.project, &clicked_selection);
            let selected_objects = crate::selection_edit::selected_object_refs(
                &app.state.project,
                &app.session.selection,
            );
            let clicked_inside_selection = !clicked_objects.is_empty()
                && clicked_objects
                    .iter()
                    .all(|clicked| selected_objects.contains(clicked));
            if !clicked_inside_selection {
                app.session.selection = clicked_selection;
            }
        }
    }
    if !editing_text {
        response.context_menu(|ui| stage_context_menu(app, ui));
    }

    if !breadcrumb_is_at_root(app) {
        let inner = Rect::from_min_size(stage_rect.min, vec2(stage_rect.width(), 18.0));
        painter.rect_filled(inner, 0.0, Color32::from_black_alpha(64));
        painter.text(
            inner.left_center() + vec2(6.0, 0.0),
            Align2::LEFT_CENTER,
            format!("inside {}", current_q0rg_name(app)),
            FontId::proportional(11.0),
            Color32::WHITE,
        );
    }
    app.textures.editing_mask_q0rg_id = None;
    app.perf
        .record(crate::perf::PerfSpan::Stage, stage_started.elapsed());
}

fn handle_camera_stage_input(
    app: &mut EditorApp,
    response: &egui::Response,
    view: &StageView,
    ctx: &egui::Context,
) {
    if response.drag_started_by(PointerButton::Primary) {
        app.history
            .snapshot_with_session(&app.state.project, &app.session);
    }
    if !response.dragged_by(PointerButton::Primary) {
        return;
    }
    let delta = ctx.input(|input| input.pointer.delta());
    if !delta.x.is_finite() || !delta.y.is_finite() || delta == egui::Vec2::ZERO {
        return;
    }
    let frame = app.session.current_frame;
    let state = app.state.project.camera_state_at(frame);
    if !app.state.project.runtime.camera.has_keyframe(frame) {
        for key in &mut app.state.project.runtime.camera.keys {
            if key.frame < frame
                && key
                    .tween
                    .to_frame()
                    .is_some_and(|to_frame| frame <= to_frame)
            {
                key.tween = key.tween.with_to_frame(frame);
            }
        }
        app.state
            .project
            .runtime
            .camera
            .keys
            .push(q0s_format::v2::CameraKey {
                frame,
                state,
                tween: q0s_format::v2::Tween::None,
            });
        app.state
            .project
            .runtime
            .camera
            .keys
            .sort_by_key(|key| key.frame);
    }
    if let Some(key) = app
        .state
        .project
        .runtime
        .camera
        .keys
        .iter_mut()
        .find(|key| key.frame == frame)
    {
        let stage_origin = view.screen_to_stage(pos2(0.0, 0.0));
        let stage_end = view.screen_to_stage(pos2(delta.x, delta.y));
        let stage_delta =
            q0s_format::v2::Vec2::new(stage_end.x - stage_origin.x, stage_end.y - stage_origin.y);
        key.state.center_x += stage_delta.x;
        key.state.center_y += stage_delta.y;
        app.state.mark_dirty();
        app.session.status = format!("camera moved at frame {}", frame + 1);
    }
}

fn draw_camera_overlay(app: &EditorApp, painter: &egui::Painter, view: &StageView, frame: u16) {
    let state = app.state.project.camera_state_at(frame);
    let corners = camera_viewport_corners(app, view, frame);
    let selected = app.session.camera_track_selected;
    let color = if selected {
        app.settings.theme.accent.to_color32()
    } else {
        app.settings.theme.accent.to_color32().gamma_multiply(0.65)
    };
    let stroke = Stroke::new(if selected { 2.0_f32 } else { 1.25_f32 }, color);
    for index in 0..4 {
        painter.line_segment([corners[index], corners[(index + 1) % 4]], stroke);
    }
    if selected {
        for corner in corners {
            painter.rect_filled(Rect::from_center_size(corner, vec2(7.0, 7.0)), 1.0, color);
        }
        let center =
            view.stage_to_screen(q0s_format::v2::Vec2::new(state.center_x, state.center_y));
        painter.circle_stroke(center, 5.0, Stroke::new(1.5_f32, color));
        painter.line_segment(
            [
                pos2(center.x - 8.0, center.y),
                pos2(center.x + 8.0, center.y),
            ],
            Stroke::new(1.0_f32, color),
        );
        painter.line_segment(
            [
                pos2(center.x, center.y - 8.0),
                pos2(center.x, center.y + 8.0),
            ],
            Stroke::new(1.0_f32, color),
        );
    }
}

fn draw_camera_background_outside_stage(
    app: &EditorApp,
    painter: &egui::Painter,
    view: &StageView,
    canvas_rect: Rect,
    frame: u16,
) {
    let camera = Shape::convex_polygon(
        camera_viewport_corners(app, view, frame).to_vec(),
        {
            let bg = app.state.project.meta.background_color;
            Color32::from_rgba_unmultiplied(bg.r, bg.g, bg.b, bg.a)
        },
        Stroke::NONE,
    );
    let visible_stage = view.stage_rect.intersect(canvas_rect);
    if !visible_stage.is_positive() {
        painter.add(camera);
        return;
    }

    // The stage already has the project background. Extend that same color
    // only into the camera viewport beyond the visible stage paper.
    let outside_stage = [
        (
            canvas_rect.min,
            pos2(canvas_rect.right(), visible_stage.top()),
        ),
        (
            pos2(canvas_rect.left(), visible_stage.bottom()),
            canvas_rect.max,
        ),
        (
            pos2(canvas_rect.left(), visible_stage.top()),
            pos2(visible_stage.left(), visible_stage.bottom()),
        ),
        (
            pos2(visible_stage.right(), visible_stage.top()),
            pos2(canvas_rect.right(), visible_stage.bottom()),
        ),
    ];
    for (min, max) in outside_stage {
        let clip = Rect::from_min_max(min, max);
        if clip.is_positive() {
            painter.with_clip_rect(clip).add(camera.clone());
        }
    }
}

fn camera_viewport_corners(app: &EditorApp, view: &StageView, frame: u16) -> [egui::Pos2; 4] {
    let state = app.state.project.camera_state_at(frame);
    let half_w = f32::from(app.state.project.meta.stage_width) * 0.5 / state.zoom.max(0.01);
    let half_h = f32::from(app.state.project.meta.stage_height) * 0.5 / state.zoom.max(0.01);
    let (sin, cos) = state.rotation.sin_cos();
    let to_screen = |dx: f32, dy: f32| {
        let x = state.center_x + cos * dx - sin * dy;
        let y = state.center_y + sin * dx + cos * dy;
        view.stage_to_screen(q0s_format::v2::Vec2::new(x, y))
    };
    [
        to_screen(-half_w, -half_h),
        to_screen(half_w, -half_h),
        to_screen(half_w, half_h),
        to_screen(-half_w, half_h),
    ]
}

fn library_item_target(
    project: &q0s_format::v2::ProjectV2,
    item: LibraryItem,
) -> Option<q0s_format::v2::Target> {
    match item {
        LibraryItem::Q0rg(id) => project
            .q0rgs
            .iter()
            .any(|q0rg| q0rg.q0rg_id == id)
            .then_some(q0s_format::v2::Target::Q0rg(id)),
        LibraryItem::Asset(id) => project
            .assets
            .iter()
            .any(|asset| asset.id() == id)
            .then_some(q0s_format::v2::Target::Asset(id)),
    }
}

fn draw_symbol_edit_backdrop(app: &EditorApp, painter: &egui::Painter, view: &StageView) {
    let (rect, color) =
        symbol_edit_backdrop_spec(view.stage_rect, app.settings.theme.symbol_edit_mask);
    painter.rect_filled(rect, 0.0, color);
}

fn symbol_edit_backdrop_spec(stage: Rect, mask: crate::settings::ColorRgb) -> (Rect, Color32) {
    (
        stage,
        Color32::from_rgba_unmultiplied(mask.r, mask.g, mask.b, SYMBOL_EDIT_MASK_ALPHA),
    )
}

/// Wheel-zoom (zoom-around-cursor), middle-mouse pan, and drag-stop reset of
/// the panning flag.  Tools see `Sense::click_and_drag` on the primary button
/// only, so middle-button gestures don't bleed into pen/brush/select.
fn handle_viewport_input(
    app: &mut crate::app::EditorApp,
    response: &egui::Response,
    canvas_rect: Rect,
    fit_scale: f32,
    ctx: &egui::Context,
) {
    if touch_navigation(app, canvas_rect, fit_scale, ctx) {
        return;
    }

    // --- Wheel zoom (only when hovering the canvas) -------------------------
    if response.contains_pointer() {
        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.5 {
            let cursor = ctx
                .input(|i| i.pointer.hover_pos())
                .unwrap_or(canvas_rect.center());
            zoom_around(
                app,
                cursor,
                canvas_rect,
                fit_scale,
                scroll * ZOOM_WHEEL_FACTOR,
            );
        }
    }

    // --- Hand tool + temporary Space override + middle-click pan -----------
    // H selects Tool::Hand through the normal shortcut path. Space does not
    // replace the selected tool: while held over the stage, primary drag pans
    // exactly like MMB and releasing it restores the underlying tool.
    let typing = ctx.wants_keyboard_input();
    let hand_held = hand_mode_requested(
        app.session.current_tool,
        typing,
        app.session.tool_state.is_drawing(),
        ctx.input(|i| i.key_down(Key::Space)),
    );
    let middle_pressed = ctx.input(|i| i.pointer.button_pressed(PointerButton::Middle));
    let middle_released = ctx.input(|i| i.pointer.button_released(PointerButton::Middle));
    let middle_down = ctx.input(|i| i.pointer.button_down(PointerButton::Middle));
    let primary_pressed = ctx.input(|i| i.pointer.button_pressed(PointerButton::Primary));
    let primary_released = ctx.input(|i| i.pointer.button_released(PointerButton::Primary));
    let primary_down = ctx.input(|i| i.pointer.button_down(PointerButton::Primary));

    if middle_pressed && response.contains_pointer() {
        app.session.viewport.panning = true;
    }
    if hand_held && primary_pressed && response.contains_pointer() {
        app.session.viewport.panning = true;
    }

    // Keep the hand latched until the current primary drag ends, even if
    // temporary Space is released a fraction earlier than the mouse button.
    app.session.viewport.hand_active =
        hand_held || (app.session.viewport.panning && primary_down && !middle_down);

    if middle_released || (primary_released && !middle_down) {
        app.session.viewport.panning = false;
        if !hand_held {
            app.session.viewport.hand_active = false;
        }
    }
    if !middle_down && !primary_down && !hand_held {
        app.session.viewport.panning = false;
        app.session.viewport.hand_active = false;
    }

    if app.session.viewport.panning {
        let d = ctx.input(|i| i.pointer.delta());
        if d != egui::Vec2::ZERO {
            app.session.viewport.glide = None;
            app.session.viewport.pan.x += d.x;
            app.session.viewport.pan.y += d.y;
        }
    }
}

/// A two-finger touch shorter than this, that barely moved, counts as a tap.
const TOUCH_TAP_SECONDS: f64 = 0.3;
const TOUCH_TAP_TRAVEL: f32 = 12.0;

/// Two or more fingers on the canvas navigate instead of drawing: pinching
/// zooms around them and dragging pans. A quick tap with two fingers undoes,
/// with three it redoes.
///
/// Returns true while the gesture owns the canvas, including the frame in
/// which the last finger lifts, so tools never see that release.
fn touch_navigation(
    app: &mut crate::app::EditorApp,
    canvas_rect: Rect,
    fit_scale: f32,
    ctx: &egui::Context,
) -> bool {
    let gesture = app.session.viewport.touch;
    if let Some(touch) = ctx.input(|i| i.multi_touch()) {
        if !gesture.active {
            if !canvas_rect.contains(touch.start_pos) {
                return false;
            }
            match touch_gesture_effect(&app.session.tool_state) {
                TouchGestureEffect::Blocked => return false,
                TouchGestureEffect::DiscardStroke => {
                    app.session.tool_state = crate::state::ToolState::Idle;
                }
                TouchGestureEffect::Keep => {}
            }
            app.session.viewport.touch = crate::state::TouchGesture {
                active: true,
                started: touch.start_time,
                center: touch.start_pos,
                ..Default::default()
            };
        }
        let gesture = &mut app.session.viewport.touch;
        gesture.most_fingers = gesture.most_fingers.max(touch.num_touches);
        gesture.center += touch.translation_delta;
        gesture.travel +=
            touch.translation_delta.length() + touch.zoom_delta.ln().abs() * canvas_rect.width();
        let center = gesture.center;
        app.session.viewport.glide = None;
        app.session.viewport.pan.x += touch.translation_delta.x;
        app.session.viewport.pan.y += touch.translation_delta.y;
        if touch.zoom_delta != 1.0 {
            zoom_around(app, center, canvas_rect, fit_scale, touch.zoom_delta.ln());
        }
        app.session.viewport.panning = true;
        return true;
    }
    if !gesture.active {
        return false;
    }
    app.session.viewport.panning = true;
    // Lifting all but one finger does not hand the touch back to the tool.
    if ctx.input(|i| i.any_touches()) {
        return true;
    }
    let quick = ctx.input(|i| i.time) - gesture.started <= TOUCH_TAP_SECONDS;
    if quick && gesture.travel <= TOUCH_TAP_TRAVEL {
        match gesture.most_fingers {
            2 => app.queue(Action::Undo),
            3 => app.queue(Action::Redo),
            _ => {}
        }
    }
    app.session.viewport.touch = crate::state::TouchGesture::default();
    true
}

enum TouchGestureEffect {
    /// Navigate and leave the tool as it is.
    Keep,
    /// The touch was the start of a stroke that has not been committed yet.
    DiscardStroke,
    /// Artwork is being edited live and cannot be abandoned halfway.
    Blocked,
}

fn touch_gesture_effect(state: &crate::state::ToolState) -> TouchGestureEffect {
    use crate::state::ToolState;
    match state {
        // Pen anchors are placed by taps, so zooming between them must not
        // lose the path.
        ToolState::Idle | ToolState::PenDrawing { .. } => TouchGestureEffect::Keep,
        ToolState::LassoDrawing { .. }
        | ToolState::FreehandDrawing { .. }
        | ToolState::BrushDrawing { .. }
        | ToolState::AdvancedBrushDrawing { .. }
        | ToolState::EraserDrawing { .. }
        | ToolState::PrimitiveDrawing { .. }
        | ToolState::Marquee { .. } => TouchGestureEffect::DiscardStroke,
        _ => TouchGestureEffect::Blocked,
    }
}

fn release_canvas_keyboard_focus(ctx: &egui::Context, interacted: bool) {
    if !interacted {
        return;
    }
    if let Some(focused) = ctx.memory(|memory| memory.focused()) {
        ctx.memory_mut(|memory| memory.surrender_focus(focused));
    }
}

fn hand_mode_requested(
    current_tool: crate::state::Tool,
    typing: bool,
    drawing: bool,
    space_down: bool,
) -> bool {
    current_tool == crate::state::Tool::Hand || (!typing && !drawing && space_down)
}

/// Apply a zoom delta keeping `cursor` (in screen px) anchored to the same
/// stage point — i.e. Photoshop/Flash-style zoom-to-cursor.
pub fn zoom_around(
    app: &mut crate::app::EditorApp,
    cursor: egui::Pos2,
    canvas_rect: Rect,
    fit_scale: f32,
    log_factor: f32,
) {
    let stage_w = app.state.project.meta.stage_width as f32;
    let stage_h = app.state.project.meta.stage_height as f32;
    app.session.viewport.glide = None;

    let old_zoom = app.session.viewport.zoom;
    let scale_old = (fit_scale * old_zoom).clamp(ZOOM_MIN, fit_scale * ZOOM_MAX);
    let center_old =
        canvas_rect.center() + vec2(app.session.viewport.pan.x, app.session.viewport.pan.y);
    let origin_old = pos2(
        center_old.x - stage_w * scale_old * 0.5,
        center_old.y - stage_h * scale_old * 0.5,
    );
    // Stage point under cursor at the old zoom
    let stage_x = (cursor.x - origin_old.x) / scale_old;
    let stage_y = (cursor.y - origin_old.y) / scale_old;

    let new_zoom = (old_zoom * (log_factor.exp())).clamp(0.05, ZOOM_MAX);
    if (new_zoom - old_zoom).abs() < f32::EPSILON {
        return;
    }
    app.session.viewport.zoom = new_zoom;

    // Solve for pan such that the same stage point lands under the same cursor.
    // origin_new = canvas.center + pan_new - half_stage_screen_new
    // cursor.x = origin_new.x + stage_x * scale_new
    // → pan_new.x = cursor.x - stage_x*scale_new - canvas.center.x + half_w*scale_new
    let scale_new = (fit_scale * new_zoom).clamp(ZOOM_MIN, fit_scale * ZOOM_MAX);
    app.session.viewport.pan.x =
        cursor.x - stage_x * scale_new - canvas_rect.center().x + stage_w * scale_new * 0.5;
    app.session.viewport.pan.y =
        cursor.y - stage_y * scale_new - canvas_rect.center().y + stage_h * scale_new * 0.5;
    app.session.status = format!("zoom {:.0}%", new_zoom * 100.0);
}

/// The way back out of nested q0rgs: a back button and the chain of parents.
/// Shows nothing at the root.
pub(crate) fn breadcrumbs(app: &mut EditorApp, ui: &mut Ui) {
    if !app.session.breadcrumb.is_empty()
        && crate::ui::icon_button(ui, crate::ui::Icon::Back, "Return to parent timeline").clicked()
    {
        app.queue(Action::ExitQ0rg);
    }
    // Build the breadcrumb chain: each entry is a clickable jump-to depth.
    // depth=0 is the root stage. depth>=1 walks down into nested q0rgs.
    let mut chain: Vec<(usize, String, bool)> = Vec::new();
    // Root: the q0rg at the bottom of the breadcrumb stack, or the current
    // q0rg itself if breadcrumb is empty.
    let root_q0rg_id = app
        .session
        .breadcrumb
        .first()
        .copied()
        .unwrap_or(app.session.current_q0rg_id);
    if !app.session.breadcrumb.is_empty() {
        let root_name = q0rg_name(app, root_q0rg_id, "Stage");
        chain.push((0, root_name, false));
    }
    for (i, parent_id) in app.session.breadcrumb.iter().enumerate().skip(1) {
        let nm = q0rg_name(app, *parent_id, "Unknown");
        chain.push((i, nm, false));
    }
    // Final tail: the q0rg we're currently editing (only show if nested).
    if !app.session.breadcrumb.is_empty() {
        let nm = q0rg_name(app, app.session.current_q0rg_id, "Unknown");
        chain.push((app.session.breadcrumb.len(), nm, true));
    }

    let mut jump: Option<usize> = None;
    for (i, (depth, name, is_current)) in chain.iter().enumerate() {
        if i > 0 {
            ui.label(
                egui::RichText::new(">")
                    .color(app.settings.theme.text_dim.to_color32())
                    .small(),
            );
        }
        let display_name = truncate_label(name, 24);
        let resp = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(display_name)
                        .color(if *is_current {
                            app.settings.theme.text.to_color32()
                        } else {
                            app.settings.theme.text_dim.to_color32()
                        })
                        .small(),
                )
                .sense(Sense::click()),
            )
            .on_hover_text(name.as_str());
        if resp.clicked() && !is_current {
            jump = Some(*depth);
        }
    }
    if let Some(d) = jump {
        app.queue(Action::BreadcrumbJumpTo(d));
    }
}

/// Fit-to-window and the current zoom, laid out right to left.
pub(crate) fn zoom_controls(app: &mut EditorApp, ui: &mut Ui) {
    if crate::ui::icon_button(
        ui,
        crate::ui::Icon::Fit,
        &app.settings.hotkeys.hint(
            crate::hotkeys::ShortcutCommand::ZoomReset,
            "Fit stage to window",
        ),
    )
    .clicked()
    {
        app.queue(Action::ZoomReset);
    }
    ui.label(
        egui::RichText::new(format!("{:.0}%", app.session.viewport.zoom * 100.0,))
            .color(app.settings.theme.text_dim.to_color32())
            .small(),
    )
    .on_hover_text(format!(
        "{} x {} · {} fps · frame {}",
        app.state.project.meta.stage_width,
        app.state.project.meta.stage_height,
        app.state.project.meta.fps,
        app.session.current_frame + 1
    ));
}

fn q0rg_name(app: &EditorApp, id: u16, fallback: &str) -> String {
    app.state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == id)
        .map(|q| q.name.clone())
        .unwrap_or_else(|| fallback.to_string())
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut shortened: String = text.chars().take(keep).collect();
    shortened.push_str("...");
    shortened
}

fn breadcrumb_is_at_root(app: &EditorApp) -> bool {
    app.session.breadcrumb.is_empty()
}

fn current_q0rg_name(app: &EditorApp) -> String {
    q0rg_name(app, app.session.current_q0rg_id, "Unknown")
}

fn stage_context_menu(app: &mut EditorApp, ui: &mut egui::Ui) {
    if app.session.onion.edit_frames {
        if ui.button("Select all objects in range").clicked() {
            crate::frame_edit::select_all(app);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                !app.session.frame_edit_selection.is_empty(),
                egui::Button::new("Delete selected objects"),
            )
            .clicked()
        {
            app.queue(Action::DeleteSelection);
            ui.close_menu();
        }
        if ui.button("Exit multiple-frame editing").clicked() {
            crate::frame_edit::disable(app);
            ui.close_menu();
        }
        return;
    }
    use crate::state::Selection;
    let has_selection = !matches!(app.session.selection, Selection::None);
    let has_clip = app.has_clipboard();
    let is_placement = matches!(app.session.selection, Selection::Placement { .. });
    let can_clip = crate::selection_edit::selection_can_clip(&app.session.selection);
    let selected_q0rg_placement = match app.session.selection.clone() {
        Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } => app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| layer.placements.get(placement_idx))
            .and_then(|placement| {
                matches!(placement.target, q0s_format::v2::Target::Q0rg(_)).then_some((
                    q0rg_id,
                    layer_id,
                    placement_idx,
                ))
            }),
        _ => None,
    };

    if can_clip {
        if ui
            .add(crate::ui::shortcut_button(
                &app.settings.hotkeys,
                crate::hotkeys::ShortcutCommand::Cut,
                "Cut",
            ))
            .clicked()
        {
            app.queue(Action::CutSelection);
            ui.close_menu();
        }
        if ui
            .add(crate::ui::shortcut_button(
                &app.settings.hotkeys,
                crate::hotkeys::ShortcutCommand::Copy,
                "Copy",
            ))
            .clicked()
        {
            app.queue(Action::CopySelection);
            ui.close_menu();
        }
    }
    if ui
        .add_enabled(
            has_clip,
            crate::ui::shortcut_button(
                &app.settings.hotkeys,
                crate::hotkeys::ShortcutCommand::Paste,
                "Paste",
            ),
        )
        .clicked()
    {
        app.queue(Action::Paste);
        ui.close_menu();
    }
    if has_selection {
        if can_clip
            && ui
                .add(crate::ui::shortcut_button(
                    &app.settings.hotkeys,
                    crate::hotkeys::ShortcutCommand::Duplicate,
                    "Duplicate",
                ))
                .clicked()
        {
            app.queue(Action::DuplicateSelection);
            ui.close_menu();
        }
        if ui
            .add(crate::ui::shortcut_button(
                &app.settings.hotkeys,
                crate::hotkeys::ShortcutCommand::Delete,
                "Delete",
            ))
            .clicked()
        {
            app.queue(Action::DeleteSelection);
            ui.close_menu();
        }
        ui.separator();
        let can_convert_to_symbol =
            crate::selection_edit::selection_can_clip(&app.session.selection);
        if can_convert_to_symbol
            && ui
                .add(crate::ui::shortcut_button(
                    &app.settings.hotkeys,
                    crate::hotkeys::ShortcutCommand::ConvertToSymbol,
                    "Convert to Symbol",
                ))
                .clicked()
        {
            app.queue(Action::ConvertSelectionToQ0rg);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                app.can_break_apart_selection(),
                crate::ui::shortcut_button(
                    &app.settings.hotkeys,
                    crate::hotkeys::ShortcutCommand::BreakApart,
                    "Break Apart",
                ),
            )
            .clicked()
        {
            app.queue(Action::BreakApartSelection);
            ui.close_menu();
        }
        let spread_objects =
            crate::selection_edit::selected_object_refs(&app.state.project, &app.session.selection);
        let spread_between_keyframes_enabled = spread_objects.first().is_some_and(|first| {
            spread_objects.iter().all(|reference| {
                reference.q0rg_id == first.q0rg_id && reference.layer_id == first.layer_id
            }) && !app
                .state
                .project
                .layer_is_locked(first.q0rg_id, first.layer_id)
                && !app
                    .state
                    .project
                    .layer_is_folder(first.q0rg_id, first.layer_id)
        });
        let spread_between_layers_enabled = spread_between_keyframes_enabled
            && spread_objects.first().is_some_and(|first| {
                app.state
                    .project
                    .layer_metadata(first.q0rg_id, first.layer_id)
                    .kind
                    == q0s_format::v2::LayerKind::Normal
            });
        if ui
            .add_enabled(
                spread_between_layers_enabled,
                egui::Button::new("Spread between layers"),
            )
            .on_hover_text(
                "Move selected depth tracks onto named layers; a lone object names its existing layer",
            )
            .clicked()
        {
            app.queue(Action::SpreadBetweenLayers);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                spread_between_keyframes_enabled,
                egui::Button::new("Spread between keyframes"),
            )
            .on_hover_text(
                "Create consecutive keyframes in depth order; unselected objects stay present and later keys on this layer shift right",
            )
            .clicked()
        {
            app.queue(Action::SpreadBetweenKeyframes);
            ui.close_menu();
        }
        if is_placement {
            let flip_horizontal = app
                .settings
                .hotkeys
                .display(crate::hotkeys::ShortcutCommand::FlipHorizontal)
                .to_lowercase();
            if ui
                .button(format!("flip horizontally ({flip_horizontal})"))
                .clicked()
            {
                app.queue(Action::FlipHorizontal);
                ui.close_menu();
            }
            let flip_vertical = app
                .settings
                .hotkeys
                .display(crate::hotkeys::ShortcutCommand::FlipVertical)
                .to_lowercase();
            if ui
                .button(format!("flip vertically ({flip_vertical})"))
                .clicked()
            {
                app.queue(Action::FlipVertical);
                ui.close_menu();
            }
            ui.separator();
            if let Some((q0rg_id, layer_id, placement_idx)) = selected_q0rg_placement {
                if ui.button("Edit In Place").clicked() {
                    if let Some(q0s_format::v2::Target::Q0rg(child)) = app
                        .state
                        .project
                        .q0rgs
                        .iter()
                        .find(|q| q.q0rg_id == q0rg_id)
                        .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
                        .and_then(|l| l.placements.get(placement_idx))
                        .map(|p| p.target)
                    {
                        app.queue(Action::EnterQ0rgInPlace {
                            q0rg_id: child,
                            layer_id,
                            placement_idx,
                        });
                    }
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        !app.state.project.layer_is_locked(q0rg_id, layer_id),
                        egui::Button::new("Duplicate Symbol"),
                    )
                    .on_hover_text(
                        "Create an independent library symbol and use it on this keyframe",
                    )
                    .clicked()
                {
                    app.queue(Action::DuplicateSymbol(q0rg_id, layer_id, placement_idx));
                    ui.close_menu();
                }
                if ui.button("Replace q0rg").clicked() {
                    app.open_replace_q0rg_dialog(q0rg_id, layer_id, placement_idx);
                    ui.close_menu();
                }
            }
            // Stroke → Fill: only enabled when the selected placement points
            // at a vector asset that actually has a stroke.
            let can_convert = selected_has_stroke(app);
            if ui
                .add_enabled(can_convert, egui::Button::new("Convert Stroke to Fill"))
                .clicked()
            {
                app.queue(Action::ConvertStrokeToFill);
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Bring to Front").clicked() {
                app.queue(Action::BringToFront);
                ui.close_menu();
            }
            if ui.button("Send to Back").clicked() {
                app.queue(Action::SendToBack);
                ui.close_menu();
            }
            if ui
                .add(crate::ui::shortcut_button(
                    &app.settings.hotkeys,
                    crate::hotkeys::ShortcutCommand::BringForward,
                    "Bring Forward",
                ))
                .on_hover_text("Move one step forward in the layer's drawing order")
                .clicked()
            {
                app.queue(Action::BringForward);
                ui.close_menu();
            }
            if ui
                .add(crate::ui::shortcut_button(
                    &app.settings.hotkeys,
                    crate::hotkeys::ShortcutCommand::SendBackward,
                    "Send Backward",
                ))
                .on_hover_text("Move one step backward in the layer's drawing order")
                .clicked()
            {
                app.queue(Action::SendBackward);
                ui.close_menu();
            }
        }
    } else {
        ui.label(
            egui::RichText::new("(empty scene)")
                .color(app.settings.theme.text_dim.to_color32())
                .small(),
        );
    }
}

fn selected_has_stroke(app: &EditorApp) -> bool {
    let crate::state::Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx,
    } = app.session.selection.clone()
    else {
        return false;
    };
    let target = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
        .and_then(|l| l.placements.get(placement_idx))
        .map(|p| p.target);
    match target {
        Some(q0s_format::v2::Target::Asset(id)) => app
            .state
            .project
            .assets
            .iter()
            .find(|a| a.id() == id)
            .map(|a| matches!(a, q0s_format::v2::Asset::Vector(v) if v.stroke.is_some()))
            .unwrap_or(false),
        _ => false,
    }
}

/// Pick the onion-skin tint for a frame `offset` away from the current
/// frame. Past frames (offset < 0) get a cool blue, future ones a warm
/// orange. Alpha falls off with distance so the closest neighbour reads
/// strongest, the furthest barely a ghost.
fn onion_tint_for(
    offset: i32,
    before: u16,
    after: u16,
    past: egui::Color32,
    future: egui::Color32,
) -> egui::Color32 {
    let distance = offset.unsigned_abs() as f32;
    let span = if offset < 0 {
        before.max(1) as f32
    } else {
        after.max(1) as f32
    };
    let t = (distance / span).clamp(0.0, 1.0);
    // Closer (t→0) = stronger; farther (t→1) = ghostly. Range 180→60.
    let alpha = (180.0 - t * 120.0).round() as u8;
    let color = if offset < 0 { past } else { future };
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

#[cfg(test)]
mod tests {
    use super::{hand_mode_requested, release_canvas_keyboard_focus, symbol_edit_backdrop_spec};
    use crate::state::Tool;

    #[test]
    fn symbol_edit_backdrop_covers_the_whole_stage_and_stays_subtle() {
        let stage = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0));
        let (rect, color) =
            symbol_edit_backdrop_spec(stage, crate::settings::ColorRgb::new(20, 30, 40));
        assert_eq!(rect, stage);
        let rgba = color.to_srgba_unmultiplied();
        assert!(rgba[0].abs_diff(20) <= 1);
        assert!(rgba[1].abs_diff(30) <= 1);
        assert!(rgba[2].abs_diff(40) <= 1);
        assert_eq!(rgba[3], 48);
    }

    #[test]
    fn h_selects_a_persistent_hand_tool() {
        assert!(hand_mode_requested(Tool::Hand, false, false, false));
        assert!(hand_mode_requested(Tool::Hand, false, false, true));
    }

    #[test]
    fn space_is_only_a_temporary_hand_override() {
        assert!(hand_mode_requested(Tool::Brush, false, false, true));
        assert!(!hand_mode_requested(Tool::Brush, false, false, false));
        assert!(!hand_mode_requested(Tool::Brush, true, false, true));
        assert!(!hand_mode_requested(Tool::Brush, false, true, true));
    }

    #[test]
    fn clicking_canvas_releases_properties_keyboard_focus() {
        let ctx = egui::Context::default();
        let field = egui::Id::new("properties-field");
        ctx.memory_mut(|memory| memory.request_focus(field));
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(field));

        release_canvas_keyboard_focus(&ctx, true);
        assert_eq!(ctx.memory(|memory| memory.focused()), None);
    }
}
