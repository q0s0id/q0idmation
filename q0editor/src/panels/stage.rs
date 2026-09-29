use egui::{pos2, vec2, Align2, Color32, FontId, Key, PointerButton, Rect, Sense, Stroke, Ui};

use crate::app::{Action, EditorApp};
use crate::render::{centered_target_transform, render_target_preview, StageRenderPass, StageView};
use crate::state::{LibraryDragPayload, LibraryItem};

pub const ZOOM_MIN: f32 = 0.05;
pub const ZOOM_MAX: f32 = 32.0;
const ZOOM_WHEEL_FACTOR: f32 = 0.0015;
const SYMBOL_EDIT_MASK_ALPHA: u8 = 48;

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    let stage_started = std::time::Instant::now();
    breadcrumb_bar(app, ui);
    ui.separator();

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
        app.session.timeline_selection = None;
        app.session.pending_timeline_frame = None;
    }

    // Canvas backdrop — area outside the stage paper. Pulled from the
    // theme so dark/light/etc. presets all blend in consistently.
    painter.rect_filled(canvas_rect, 0.0, app.settings.theme.canvas_bg.to_color32());

    let stage_w = app.state.project.meta.stage_width as f32;
    let stage_h = app.state.project.meta.stage_height as f32;

    // Auto-fit base scale; user zoom is a multiplier on top of that, so zoom=1
    // always means "fits the canvas no matter the canvas size".
    let fit_scale = (canvas_rect.width() * 0.9 / stage_w)
        .min(canvas_rect.height() * 0.9 / stage_h)
        .max(ZOOM_MIN);

    handle_viewport_input(app, &response, canvas_rect, fit_scale, ui.ctx());

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
    };
    if let Some(pointer) = ui
        .ctx()
        .pointer_hover_pos()
        .filter(|pointer| canvas_rect.contains(*pointer))
    {
        let stage_cursor = q0s_format::v2::Vec2::new(
            (pointer.x - view.origin.x) / view.scale,
            (pointer.y - view.origin.y) / view.scale,
        );
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
    if !library_drag_active && !editing_text && !camera_editing {
        let tools_started = std::time::Instant::now();
        crate::tools::handle(app, &response, &painter, &view, &ctx);
        app.perf
            .record(crate::perf::PerfSpan::ToolHandle, tools_started.elapsed());
    }
    // Tool input can enter another q0rg on double-click, so resolve the scene
    // identity only after interaction has finished for this frame.
    let q0rg_id = app.session.current_q0rg_id;
    let frame = app.session.current_frame;
    app.textures.hidden_text_placement = app
        .session
        .text_editor
        .as_ref()
        .and_then(|draft| draft.placement)
        .map(|r| (r.q0rg_id, r.layer_id, r.placement_idx));
    app.textures.interactive_transform = ui.input(|i| i.pointer.primary_down());

    // Editing inside a q0rg uses a subtle full-stage veil. The veil is drawn
    // before the symbol, so the current q0rg remains crisp while the canvas
    // behind it recedes. Its RGB comes from the active theme/.q7s.
    if !breadcrumb_is_at_root(app) {
        draw_symbol_edit_backdrop(app, &painter, &view);
    }

    // A single render pass owns immutable id lookups and rig-pose memoization for
    // every onion frame plus the live frame painted below.
    let render_pass = StageRenderPass::new(&app.state.project, &mut app.textures);
    app.textures.reset_stage_traversal_stats();
    q0glblend::reset_paint_diagnostics();
    let scene_started = std::time::Instant::now();

    // Onion skin: render `before` past frames + `after` upcoming frames with
    // a colour-tinted alpha behind the live frame. Past frames get a cool
    // (blueish) tint, upcoming a warm (orange) tint — Flash convention.
    if app.session.onion.enabled {
        let frame_count = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .map(|q| q.frame_count)
            .unwrap_or(0);
        if frame_count > 0 {
            for offset in (-(app.session.onion.before as i32)..=(app.session.onion.after as i32))
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
                render_pass.render_tinted(
                    &painter,
                    q0rg_id,
                    target as u16,
                    &view,
                    &mut app.textures,
                    ui.ctx(),
                    tint,
                );
            }
        }
    }

    render_pass.render(&painter, q0rg_id, frame, &view, &mut app.textures, ui.ctx());
    let scene_elapsed = scene_started.elapsed();
    let traversal = app.textures.stage_traversal_stats();
    let gpu = q0glblend::take_paint_diagnostics();
    let gpu_timing = q0glblend::take_gpu_callback_timing();
    app.perf.set_scene_context(crate::perf::ScenePerfContext {
        zoom: app.session.viewport.zoom,
        view_scale: view.scale,
        pan_x: app.session.viewport.pan.x,
        pan_y: app.session.viewport.pan.y,
        pointer_over_stage: response.hovered(),
        cursor_stage: response.hover_pos().map(|position| {
            [
                (position.x - view.origin.x) / view.scale,
                (position.y - view.origin.y) / view.scale,
            ]
        }),
        q0rg_id,
        frame,
        tool: app.session.current_tool.label(),
        q0rg_visits: traversal.q0rg_visits,
        active_placements: traversal.active_placements,
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
    });
    drop(render_pass);
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
        if app.library_drag_item_is_audio_only(*payload) {
            // Audio is timeline media, never a display object.
        } else {
            let stage_position = q0s_format::v2::Vec2::new(
                (pointer.x - view.origin.x) / view.scale,
                (pointer.y - view.origin.y) / view.scale,
            );
            if Some(payload.source_tab_id) == app.active_project_tab_id() {
                if let Some(target) = library_item_target(&app.state.project, payload.item) {
                    let transform =
                        centered_target_transform(&app.state.project, target, stage_position);
                    render_target_preview(
                        &painter,
                        &app.state.project,
                        target,
                        frame,
                        transform,
                        &view,
                        &mut app.textures,
                        ui.ctx(),
                        Color32::from_white_alpha(176),
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
                            Rect::from_min_max(
                                pos2(
                                    view.origin.x + min_x * view.scale,
                                    view.origin.y + min_y * view.scale,
                                ),
                                pos2(
                                    view.origin.x + max_x * view.scale,
                                    view.origin.y + max_y * view.scale,
                                ),
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
                            Rect::from_min_max(
                                pos2(
                                    view.origin.x + min_x * view.scale,
                                    view.origin.y + min_y * view.scale,
                                ),
                                pos2(
                                    view.origin.x + max_x * view.scale,
                                    view.origin.y + max_y * view.scale,
                                ),
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
        if app.library_drag_item_is_audio_only(*payload) {
            app.session.status = "drop audio onto a timeline frame".to_string();
        } else if let Some(pointer) = ui.ctx().pointer_interact_pos() {
            let stage_position = q0s_format::v2::Vec2::new(
                (pointer.x - view.origin.x) / view.scale,
                (pointer.y - view.origin.y) / view.scale,
            );
            app.queue(Action::PlaceLibraryDragAt(*payload, stage_position));
        }
    }

    // Canvas text UI and transient overlays stay above the freshly rendered scene.
    let editing_text = crate::text_tool::render_canvas(app, ui, &view);
    if !editing_text {
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
    if response.secondary_clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            let stage_pos = q0s_format::v2::Vec2::new(
                (p.x - view.origin.x) / view.scale,
                (p.y - view.origin.y) / view.scale,
            );
            let selection = crate::tools::selection_at_point_cached_for_scale(
                &app.state.project,
                &mut app.textures,
                q0rg_id,
                app.session.current_frame,
                stage_pos,
                view.scale,
            )
            .unwrap_or(crate::state::Selection::None);
            app.session.selection = selection;
        }
    }
    response.context_menu(|ui| stage_context_menu(app, ui));

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
        app.history.snapshot(&app.state.project);
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
        key.state.center_x += delta.x / view.scale;
        key.state.center_y += delta.y / view.scale;
        app.state.mark_dirty();
        app.session.status = format!("camera moved at frame {}", frame + 1);
    }
}

fn draw_camera_overlay(app: &EditorApp, painter: &egui::Painter, view: &StageView, frame: u16) {
    let state = app.state.project.camera_state_at(frame);
    let half_w = f32::from(app.state.project.meta.stage_width) * 0.5 / state.zoom.max(0.01);
    let half_h = f32::from(app.state.project.meta.stage_height) * 0.5 / state.zoom.max(0.01);
    let (sin, cos) = state.rotation.sin_cos();
    let to_screen = |dx: f32, dy: f32| {
        let x = state.center_x + cos * dx - sin * dy;
        let y = state.center_y + sin * dx + cos * dy;
        pos2(
            view.origin.x + x * view.scale,
            view.origin.y + y * view.scale,
        )
    };
    let corners = [
        to_screen(-half_w, -half_h),
        to_screen(half_w, -half_h),
        to_screen(half_w, half_h),
        to_screen(-half_w, half_h),
    ];
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
        let center = pos2(
            view.origin.x + state.center_x * view.scale,
            view.origin.y + state.center_y * view.scale,
        );
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
            app.session.viewport.pan.x += d.x;
            app.session.viewport.pan.y += d.y;
        }
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

fn breadcrumb_bar(app: &mut EditorApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
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
        let root_name = q0rg_name(app, root_q0rg_id, "Stage");
        chain.push((0, root_name, app.session.breadcrumb.is_empty()));
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

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "stage {}x{} | {} fps | frame {} | zoom {:.0}%",
                    app.state.project.meta.stage_width,
                    app.state.project.meta.stage_height,
                    app.state.project.meta.fps,
                    app.session.current_frame + 1,
                    app.session.viewport.zoom * 100.0,
                ))
                .color(app.settings.theme.text_dim.to_color32())
                .small(),
            );
        });
    });
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
    use crate::state::Selection;
    let has_selection = !matches!(app.session.selection, Selection::None);
    let has_clip = app.session.clipboard.is_some();
    let is_placement = matches!(app.session.selection, Selection::Placement { .. });
    let can_clip = crate::selection_edit::selection_can_clip(&app.session.selection);

    if can_clip {
        if ui.button("Cut").clicked() {
            app.queue(Action::CutSelection);
            ui.close_menu();
        }
        if ui.button("Copy").clicked() {
            app.queue(Action::CopySelection);
            ui.close_menu();
        }
    }
    if ui
        .add_enabled(has_clip, egui::Button::new("Paste"))
        .clicked()
    {
        app.queue(Action::Paste);
        ui.close_menu();
    }
    if has_selection {
        if can_clip && ui.button("Duplicate").clicked() {
            app.queue(Action::DuplicateSelection);
            ui.close_menu();
        }
        if ui.button("Delete").clicked() {
            app.queue(Action::DeleteSelection);
            ui.close_menu();
        }
        ui.separator();
        let can_convert_to_symbol =
            crate::selection_edit::selection_can_clip(&app.session.selection);
        if can_convert_to_symbol && ui.button("Convert to Symbol").clicked() {
            app.queue(Action::ConvertSelectionToQ0rg);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                app.can_break_apart_selection(),
                egui::Button::new("Break Apart  (Ctrl+B)"),
            )
            .clicked()
        {
            app.queue(Action::BreakApartSelection);
            ui.close_menu();
        }
        if is_placement {
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
    before: u8,
    after: u8,
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
