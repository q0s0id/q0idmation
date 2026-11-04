//! The open palette owns outside clicks: pick a displayed screen pixel, then close.
use crate::paint::PaintGradient;
use egui::{Color32, Context, Id, Key, Pos2, Rect, Response, Sense, Ui};
use q0s_format::v2::{GradientKind, Rgba};
#[cfg(windows)]
use std::sync::Arc;

pub(crate) type PickerScope = (Option<u64>, u16);

#[derive(Clone)]
struct PickRequest {
    owner: Id,
    scope: PickerScope,
    last_seen: u64,
    rect: Rect,
    #[cfg(windows)]
    desktop: Option<Arc<desktop::Sampler>>,
    screenshot: Option<(egui::ViewportId, Pos2)>,
}

#[derive(Clone, Default)]
struct PickerState {
    scope: PickerScope,
    request: Option<PickRequest>,
    result: Option<(Id, PickerScope, Color32)>,
}

fn state_id() -> Id {
    Id::new("screen_color_picker")
}

/// On a touch screen a tap outside a popup means "close it", so picking a
/// colour off the screen waits until the person asks for it.
fn armed_id(owner: Id) -> Id {
    owner.with("screen_pick_armed")
}

fn picks_from_screen(ctx: &Context, owner: Id) -> bool {
    !crate::touch::ENABLED
        || ctx.data(|data| data.get_temp::<bool>(armed_id(owner)).unwrap_or(false))
}

pub(crate) fn set_scope(ctx: &Context, scope: PickerScope) {
    let closed = ctx.data_mut(|data| {
        let state = data.get_temp_mut_or_default::<PickerState>(state_id());
        if state.scope == scope {
            return None;
        }
        let owner = state.request.as_ref().map(|request| request.owner);
        *state = PickerState {
            scope,
            ..Default::default()
        };
        owner
    });
    if closed.is_some_and(|owner| ctx.memory(|memory| memory.is_popup_open(owner))) {
        ctx.memory_mut(|memory| memory.close_popup());
    }
}

/// Run before editor input. The native sampler consumes the complete outside
/// click before it can reach another widget, the canvas, or another application.
pub(crate) fn update(ctx: &Context, scope: PickerScope) {
    set_scope(ctx, scope);
    let request = ctx
        .data(|data| data.get_temp::<PickerState>(state_id()))
        .and_then(|state| state.request);
    let Some(request) = request else {
        return;
    };
    if !ctx.memory(|memory| memory.is_popup_open(request.owner))
        || ctx.frame_nr() > request.last_seen + 1
    {
        clear_request(ctx);
        return;
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape)) {
        clear_request(ctx);
        ctx.memory_mut(|memory| memory.close_popup());
        return;
    }
    #[cfg(windows)]
    if let Some(sampler) = &request.desktop {
        if let Some(color) = sampler.take_color() {
            finish_pick(ctx, &request, color);
        }
        return;
    }
    // Other backends can still sample the full editor framebuffer, including UI.
    // Native Windows uses GetPixel and also works outside the editor window.
    if let Some((viewport, point)) = request.screenshot {
        let picked = ctx.input(|input| {
            input.events.iter().find_map(|event| {
                let egui::Event::Screenshot { viewport_id, image } = event else {
                    return None;
                };
                if *viewport_id != viewport || image.size[0] == 0 || image.size[1] == 0 {
                    return None;
                }
                let x = ((point.x * image.size[0] as f32).floor() as usize).min(image.size[0] - 1);
                let y = ((point.y * image.size[1] as f32).floor() as usize).min(image.size[1] - 1);
                image
                    .pixels
                    .get(y * image.size[0] + x)
                    .map(|color| color.to_opaque())
            })
        });
        if let Some(color) = picked {
            finish_pick(ctx, &request, color);
        }
        return;
    }
    let point = ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            } if !request.rect.contains(*pos) => Some(*pos),
            _ => None,
        })
    });
    if point.is_some() && !picks_from_screen(ctx, request.owner) {
        // A tap outside closes the palette, and goes no further: it must
        // not also draw on the canvas behind it.
        clear_request(ctx);
        ctx.memory_mut(|memory| memory.close_popup());
        ctx.input_mut(|input| {
            input.pointer = Default::default();
            input
                .events
                .retain(|event| !matches!(event, egui::Event::PointerButton { .. }));
        });
        return;
    }
    if let Some(point) = point {
        let screen = ctx.screen_rect();
        let viewport = ctx.viewport_id();
        let normalized = Pos2::new(
            (point.x - screen.left()) / screen.width().max(1.0),
            (point.y - screen.top()) / screen.height().max(1.0),
        );
        ctx.data_mut(|data| {
            if let Some(request) = data
                .get_temp_mut_or_default::<PickerState>(state_id())
                .request
                .as_mut()
            {
                request.screenshot = Some((viewport, normalized));
            }
        });
        ctx.input_mut(|input| {
            input.pointer = Default::default();
            input
                .events
                .retain(|event| !matches!(event, egui::Event::PointerButton { .. }));
        });
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        ctx.request_repaint();
    }
}

fn clear_request(ctx: &Context) {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<PickerState>(state_id())
            .request = None;
    });
}

fn finish_pick(ctx: &Context, request: &PickRequest, color: Color32) {
    ctx.data_mut(|data| {
        let state = data.get_temp_mut_or_default::<PickerState>(state_id());
        state.result = Some((request.owner, request.scope, color));
        state.request = None;
        data.remove::<bool>(armed_id(request.owner));
    });
    ctx.memory_mut(|memory| memory.close_popup());
    ctx.request_repaint();
}

/// Apply after all panels so ordinary tool hover cursors cannot override this.
pub(crate) fn finish_frame(ctx: &Context) {
    let request = ctx
        .data(|data| data.get_temp::<PickerState>(state_id()))
        .and_then(|state| state.request);
    if let Some(request) = request {
        if request.last_seen != ctx.frame_nr()
            || !ctx.memory(|memory| memory.is_popup_open(request.owner))
        {
            clear_request(ctx);
            if ctx.memory(|memory| memory.is_popup_open(request.owner)) {
                ctx.memory_mut(|memory| memory.close_popup());
            }
            return;
        }
        if picks_from_screen(ctx, request.owner)
            && ctx
                .pointer_hover_pos()
                .is_some_and(|point| !request.rect.contains(point))
        {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }
}

pub(crate) fn color_edit_button_srgba(ui: &mut Ui, color: &mut Color32) -> Response {
    color_edit_button(ui, color, egui::color_picker::Alpha::BlendOrAdditive, None).0
}

pub(crate) fn color_edit_button_srgb(ui: &mut Ui, color: &mut [u8; 3]) -> Response {
    let mut rgba = Color32::from_rgb(color[0], color[1], color[2]);
    let response = color_edit_button(ui, &mut rgba, egui::color_picker::Alpha::Opaque, None).0;
    *color = [rgba.r(), rgba.g(), rgba.b()];
    response
}

/// What a paint box changed this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PaintChange {
    #[default]
    None,
    /// the solid colour (`Response::changed` says so too)
    Color,
    /// a gradient's colours or their places
    Stops,
    /// gradient on or off, its kind, angle or size: it has to be fitted again
    Shape,
}

/// A fill's colour box: a colour, or a linear or radial gradient, picked in
/// the same popup.
pub(crate) fn paint_edit_button(
    ui: &mut Ui,
    color: &mut Color32,
    gradient: &mut Option<PaintGradient>,
) -> (Response, PaintChange) {
    color_edit_button(
        ui,
        color,
        egui::color_picker::Alpha::BlendOrAdditive,
        Some(gradient),
    )
}

fn to_color32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

fn to_rgba(c: Color32) -> Rgba {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Rgba { r, g, b, a }
}

fn checker(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, 0.0, Color32::from_gray(200));
    let cell = 5.0;
    let mut y = rect.top();
    let mut row = 0;
    while y < rect.bottom() {
        let mut x = rect.left() + if row % 2 == 0 { 0.0 } else { cell };
        while x < rect.right() {
            let square = Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell, cell));
            painter.rect_filled(square.intersect(rect), 0.0, Color32::from_gray(130));
            x += cell * 2.0;
        }
        y += cell;
        row += 1;
    }
}

/// The gradient's colours across `rect`: a ramp left to right, or rings
/// for a radial one when `rings` is set.
pub(crate) fn paint_gradient(
    painter: &egui::Painter,
    rect: Rect,
    gradient: &PaintGradient,
    rings: bool,
) {
    checker(painter, rect);
    let mut cuts: Vec<f32> = std::iter::once(0.0)
        .chain(gradient.stops.iter().map(|stop| stop.position))
        .chain(std::iter::once(1.0))
        .collect();
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    let mut mesh = egui::Mesh::default();
    if rings && gradient.kind == GradientKind::Radial {
        painter.rect_filled(rect, 0.0, to_color32(gradient.sample(1.0)));
        let center = rect.center();
        let radius = rect.size() * 0.5;
        let segments = 28;
        let point = |t: f32, k: usize| {
            let a = k as f32 / segments as f32 * std::f32::consts::TAU;
            center + egui::vec2(a.cos() * radius.x * t, a.sin() * radius.y * t)
        };
        for pair in cuts.windows(2) {
            let (inner, outer) = (
                to_color32(gradient.sample(pair[0])),
                to_color32(gradient.sample(pair[1])),
            );
            for k in 0..segments {
                let base = mesh.vertices.len() as u32;
                mesh.colored_vertex(point(pair[0], k), inner);
                mesh.colored_vertex(point(pair[0], k + 1), inner);
                mesh.colored_vertex(point(pair[1], k + 1), outer);
                mesh.colored_vertex(point(pair[1], k), outer);
                mesh.add_triangle(base, base + 1, base + 2);
                mesh.add_triangle(base, base + 2, base + 3);
            }
        }
    } else {
        for pair in cuts.windows(2) {
            let x0 = rect.left() + rect.width() * pair[0];
            let x1 = rect.left() + rect.width() * pair[1];
            let (left, right) = (
                to_color32(gradient.sample(pair[0])),
                to_color32(gradient.sample(pair[1])),
            );
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(egui::pos2(x0, rect.top()), left);
            mesh.colored_vertex(egui::pos2(x1, rect.top()), right);
            mesh.colored_vertex(egui::pos2(x1, rect.bottom()), right);
            mesh.colored_vertex(egui::pos2(x0, rect.bottom()), left);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// The stop bar, its controls, presets and the selected stop's colour.
fn gradient_editor(
    ui: &mut Ui,
    owner: Id,
    gradient: &mut PaintGradient,
    alpha: egui::color_picker::Alpha,
) -> PaintChange {
    let mut change = PaintChange::None;
    let selected_id = owner.with("gradient_stop");
    let mut selected = ui
        .data(|data| data.get_temp::<usize>(selected_id))
        .unwrap_or(0)
        .min(gradient.stops.len().saturating_sub(1));
    let width = ui.spacing().slider_width.max(200.0);
    let (bar, bar_response) = ui.allocate_exact_size(egui::vec2(width, 26.0), Sense::click());
    paint_gradient(ui.painter(), bar, gradient, false);
    ui.painter()
        .rect_stroke(bar, 2.0, ui.visuals().widgets.noninteractive.bg_stroke);
    let handle = if crate::touch::ENABLED {
        egui::vec2(18.0, 22.0)
    } else {
        egui::vec2(11.0, 15.0)
    };
    let (row, _) = ui.allocate_exact_size(egui::vec2(width, handle.y + 2.0), Sense::hover());
    let mut remove = None;
    for (index, stop) in gradient.stops.iter_mut().enumerate() {
        let x = bar.left() + bar.width() * stop.position;
        let rect = Rect::from_center_size(egui::pos2(x, row.center().y), handle);
        let response = ui
            .interact(rect, selected_id.with(index), Sense::click_and_drag())
            .on_hover_text("drag to move · drag down or double-click to remove");
        if response.clicked() || response.drag_started() {
            selected = index;
        }
        if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                stop.position = ((pointer.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
                change = PaintChange::Stops;
            }
        }
        let pulled_off = response.drag_stopped()
            && response
                .interact_pointer_pos()
                .is_some_and(|pointer| pointer.y > row.bottom() + 26.0);
        if pulled_off || response.double_clicked() || response.secondary_clicked() {
            remove = Some(index);
        }
        let stroke = if index == selected {
            egui::Stroke::new(2.0_f32, ui.visuals().strong_text_color())
        } else {
            ui.visuals().widgets.inactive.fg_stroke
        };
        let tip = egui::pos2(x, rect.top() - 3.0);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                tip,
                egui::pos2(x + 4.0, rect.top() + 1.0),
                egui::pos2(x - 4.0, rect.top() + 1.0),
            ],
            stroke.color,
            egui::Stroke::NONE,
        ));
        let body = Rect::from_min_max(egui::pos2(rect.left(), rect.top() + 1.0), rect.max);
        checker(ui.painter(), body);
        ui.painter().rect_filled(body, 2.0, to_color32(stop.color));
        ui.painter().rect_stroke(body, 2.0, stroke);
    }
    if bar_response.clicked() {
        if let Some(pointer) = bar_response.interact_pointer_pos() {
            let position = ((pointer.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
            let color = gradient.sample(position);
            gradient.stops.push(crate::paint::stop(position, color));
            selected = gradient.stops.len() - 1;
            change = PaintChange::Stops;
        }
    }
    if let Some(index) = remove.filter(|_| gradient.stops.len() > 2) {
        gradient.stops.remove(index);
        selected = selected.min(gradient.stops.len() - 1);
        change = PaintChange::Stops;
    }
    // keep the same stop selected through the sort
    if change != PaintChange::None {
        let picked = gradient.stops[selected];
        gradient.sort();
        selected = gradient
            .stops
            .iter()
            .position(|stop| *stop == picked)
            .unwrap_or(0);
    }

    ui.horizontal(|ui| {
        let mut percent = gradient.stops[selected].position * 100.0;
        ui.label("Stop at");
        if ui
            .add(
                egui::DragValue::new(&mut percent)
                    .clamp_range(0.0..=100.0)
                    .suffix("%")
                    .fixed_decimals(0),
            )
            .changed()
        {
            gradient.stops[selected].position = percent / 100.0;
            let picked = gradient.stops[selected];
            gradient.sort();
            selected = gradient
                .stops
                .iter()
                .position(|stop| *stop == picked)
                .unwrap_or(0);
            change = PaintChange::Stops;
        }
        if ui
            .add_enabled(
                gradient.stops.len() > 2,
                egui::Button::new("Remove").small(),
            )
            .clicked()
        {
            gradient.stops.remove(selected);
            selected = selected.min(gradient.stops.len() - 1);
            change = PaintChange::Stops;
        }
        if ui.small_button("⇄ Reverse").clicked() {
            gradient.reverse();
            selected = gradient.stops.len() - 1 - selected;
            change = PaintChange::Stops;
        }
        if ui
            .small_button("Even")
            .on_hover_text("Space the stops evenly")
            .clicked()
        {
            let last = (gradient.stops.len() - 1).max(1) as f32;
            for (index, stop) in gradient.stops.iter_mut().enumerate() {
                stop.position = index as f32 / last;
            }
            change = PaintChange::Stops;
        }
    });
    ui.horizontal(|ui| {
        if gradient.kind == GradientKind::Linear {
            ui.label("Angle");
            if ui
                .add(
                    egui::DragValue::new(&mut gradient.angle)
                        .clamp_range(-180.0..=180.0)
                        .speed(1.0)
                        .suffix("°")
                        .fixed_decimals(0),
                )
                .changed()
            {
                change = PaintChange::Shape;
            }
            for (arrow, angle) in [("→", 0.0), ("↘", 45.0), ("↓", 90.0), ("↙", 135.0)] {
                if ui
                    .selectable_label((gradient.angle - angle).abs() < 0.5, arrow)
                    .clicked()
                {
                    gradient.angle = angle;
                    change = PaintChange::Shape;
                }
            }
        }
        ui.label("Size");
        let mut size = gradient.size * 100.0;
        if ui
            .add(
                egui::DragValue::new(&mut size)
                    .clamp_range(10.0..=400.0)
                    .suffix("%")
                    .fixed_decimals(0),
            )
            .on_hover_text("100% spans the shape; less packs the colours tighter")
            .changed()
        {
            gradient.size = size / 100.0;
            change = PaintChange::Shape;
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for (index, (name, _)) in crate::paint::PRESETS.iter().enumerate() {
            let preset = PaintGradient::preset(index, gradient.kind, gradient.angle);
            let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 16.0), Sense::click());
            paint_gradient(ui.painter(), rect, &preset, false);
            let response = response.on_hover_text(*name);
            if response.clicked() {
                gradient.stops = preset.stops;
                selected = 0;
                change = PaintChange::Stops;
            }
        }
        let fade = {
            let mut fade = gradient.clone();
            let first = gradient.stops[0].color;
            fade.stops = vec![
                crate::paint::stop(0.0, first),
                crate::paint::stop(1.0, Rgba { a: 0, ..first }),
            ];
            fade
        };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 16.0), Sense::click());
        paint_gradient(ui.painter(), rect, &fade, false);
        if response.on_hover_text("Fade out").clicked() {
            gradient.stops = fade.stops;
            selected = 0;
            change = PaintChange::Stops;
        }
    });
    let mut stop_color = to_color32(gradient.stops[selected].color);
    if egui::color_picker::color_picker_color32(ui, &mut stop_color, alpha) {
        gradient.stops[selected].color = to_rgba(stop_color);
        change = PaintChange::Stops;
    }
    ui.data_mut(|data| data.insert_temp(selected_id, selected));
    change
}

fn color_edit_button(
    ui: &mut Ui,
    color: &mut Color32,
    alpha: egui::color_picker::Alpha,
    mut gradient: Option<&mut Option<PaintGradient>>,
) -> (Response, PaintChange) {
    let mut change = PaintChange::None;
    let owner = ui.auto_id_with("screen_color_popup");
    let result = ui.ctx().data_mut(|data| {
        let state = data.get_temp_mut_or_default::<PickerState>(state_id());
        if state
            .result
            .as_ref()
            .is_some_and(|(id, scope, _)| *id == owner && *scope == state.scope)
        {
            state.result.take().map(|(_, _, color)| color)
        } else {
            None
        }
    });
    let mut sampled_change = false;
    if let Some(sample) = result {
        if let Some(active) = gradient.as_mut().and_then(|g| g.as_mut()) {
            // a picked pixel paints the selected stop
            let selected = ui
                .data(|data| data.get_temp::<usize>(owner.with("gradient_stop")))
                .unwrap_or(0)
                .min(active.stops.len().saturating_sub(1));
            if let Some(stop) = active.stops.get_mut(selected) {
                stop.color = Rgba {
                    a: stop.color.a,
                    ..to_rgba(sample)
                };
                change = PaintChange::Stops;
            }
        } else {
            // A screen pixel has RGB, not the paint's authored opacity.
            let rgba = color.to_srgba_unmultiplied();
            let sample =
                Color32::from_rgba_unmultiplied(sample.r(), sample.g(), sample.b(), rgba[3]);
            sampled_change = *color != sample;
            *color = sample;
        }
    }
    let open = ui.memory(|memory| memory.is_popup_open(owner));
    let (rect, mut response) = ui.allocate_exact_size(ui.spacing().interact_size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::new(egui::WidgetType::ColorButton));
    if ui.is_rect_visible(rect) {
        let visuals = if open {
            &ui.visuals().widgets.open
        } else {
            ui.style().interact(&response)
        };
        let paint_rect = rect.expand(visuals.expansion);
        match gradient.as_deref().and_then(|g| g.as_ref()) {
            Some(active) => paint_gradient(ui.painter(), paint_rect, active, true),
            None => egui::color_picker::show_color_at(ui.painter(), *color, paint_rect),
        }
        ui.painter()
            .rect_stroke(paint_rect, visuals.rounding.at_most(2.0), visuals.bg_stroke);
    }
    response = response.on_hover_text(if gradient.is_some() {
        "Click to edit the colour or gradient"
    } else {
        "Click to edit color"
    });
    if sampled_change {
        response.mark_changed();
    }
    if response.clicked() {
        ui.memory_mut(|memory| memory.toggle_popup(owner));
    }
    if ui.memory(|memory| memory.is_popup_open(owner)) {
        let area = egui::Area::new(owner)
            .order(egui::Order::Foreground)
            .fixed_pos(response.rect.max)
            .constrain(true)
            .show(ui.ctx(), |ui| {
                ui.spacing_mut().slider_width = 275.0;
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if crate::touch::ENABLED {
                            let armed_key = armed_id(owner);
                            let mut armed =
                                ui.data(|data| data.get_temp::<bool>(armed_key).unwrap_or(false));
                            if ui
                                .selectable_label(armed, "Pick from screen")
                                .on_hover_text("Then tap anywhere to take its colour")
                                .clicked()
                            {
                                armed = !armed;
                                ui.data_mut(|data| data.insert_temp(armed_key, armed));
                            }
                        } else {
                            ui.small("Click outside to pick a screen color · Esc to close");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").on_hover_text("Close").clicked() {
                                ui.memory_mut(|memory| memory.close_popup());
                                ui.data_mut(|data| data.remove::<bool>(armed_id(owner)));
                            }
                        });
                    });
                    if let Some(gradient) = gradient.as_deref_mut() {
                        ui.horizontal(|ui| {
                            let kind = gradient.as_ref().map(|g| g.kind);
                            if ui.selectable_label(kind.is_none(), "Solid").clicked()
                                && kind.is_some()
                            {
                                *gradient = None;
                                change = PaintChange::Shape;
                            }
                            for (choice, label) in [
                                (GradientKind::Linear, "Linear"),
                                (GradientKind::Radial, "Radial"),
                            ] {
                                if ui
                                    .selectable_label(
                                        kind == Some(choice),
                                        format!("{label} gradient"),
                                    )
                                    .clicked()
                                    && kind != Some(choice)
                                {
                                    match gradient {
                                        Some(active) => active.kind = choice,
                                        None => {
                                            *gradient = Some(PaintGradient::starting_from(
                                                to_rgba(*color),
                                                choice,
                                            ))
                                        }
                                    }
                                    change = PaintChange::Shape;
                                }
                            }
                        });
                        ui.separator();
                    }
                    match gradient.as_mut().and_then(|g| g.as_mut()) {
                        Some(active) => {
                            let edited = gradient_editor(ui, owner, active, alpha);
                            if edited != PaintChange::None && change != PaintChange::Shape {
                                change = edited;
                            }
                        }
                        None => {
                            if egui::color_picker::color_picker_color32(ui, color, alpha) {
                                response.mark_changed();
                            }
                        }
                    }
                    if crate::touch::ENABLED {
                        let armed =
                            ui.data(|data| data.get_temp::<bool>(armed_id(owner)).unwrap_or(false));
                        ui.small(if armed {
                            "Tap anywhere to take its colour"
                        } else {
                            "Tap outside to close"
                        });
                    }
                });
            })
            .response;
        let ctx = ui.ctx();
        let last_seen = ctx.frame_nr();
        #[cfg(windows)]
        let screen_rect = ctx
            .input(|input| input.viewport().inner_rect)
            .map(|window| area.rect.translate(window.min.to_vec2()) * ctx.pixels_per_point());
        // Clone the handle before accessing ctx again; never nest egui's data lock.
        #[cfg(windows)]
        let previous = ctx
            .data(|data| data.get_temp::<PickerState>(state_id()))
            .and_then(|state| state.request)
            .filter(|request| request.owner == owner)
            .and_then(|request| request.desktop);
        #[cfg(windows)]
        let desktop = screen_rect.and_then(|rect| {
            if let Some(sampler) = previous {
                sampler.set_rect(rect);
                Some(sampler)
            } else {
                desktop::Sampler::new(ctx.clone(), rect).map(Arc::new)
            }
        });
        ctx.data_mut(|data| {
            let state = data.get_temp_mut_or_default::<PickerState>(state_id());
            if let Some(request) = state
                .request
                .as_mut()
                .filter(|request| request.owner == owner)
            {
                request.last_seen = last_seen;
                request.rect = area.rect;
                #[cfg(windows)]
                {
                    request.desktop = desktop;
                }
            } else {
                state.request = Some(PickRequest {
                    owner,
                    scope: state.scope,
                    last_seen,
                    rect: area.rect,
                    #[cfg(windows)]
                    desktop,
                    screenshot: None,
                });
            }
        });
    }
    if change == PaintChange::None && response.changed() {
        change = PaintChange::Color;
    }
    (response, change)
}

#[cfg(windows)]
mod desktop {
    use super::*;
    use std::sync::{Mutex, OnceLock, Weak};
    use windows_sys::Win32::{
        Foundation::{LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::{GetDC, GetPixel, ReleaseDC, CLR_INVALID},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, MSLLHOOKSTRUCT,
            WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP,
        },
    };

    struct Capture {
        rect: Rect,
        pressed: Option<Color32>,
        swallow_release: bool,
        ready: Option<Color32>,
        ctx: Context,
    }
    fn active() -> &'static Mutex<Weak<Mutex<Capture>>> {
        static ACTIVE: OnceLock<Mutex<Weak<Mutex<Capture>>>> = OnceLock::new();
        ACTIVE.get_or_init(|| Mutex::new(Weak::new()))
    }

    pub(super) struct Sampler {
        hook: usize,
        capture: Arc<Mutex<Capture>>,
    }
    impl Sampler {
        pub(super) fn new(ctx: Context, rect: Rect) -> Option<Self> {
            let capture = Arc::new(Mutex::new(Capture {
                rect,
                pressed: None,
                swallow_release: false,
                ready: None,
                ctx,
            }));
            // The hook runs on the editor's UI thread, whose native event loop
            // pumps messages. It exists only while a color popup is open.
            let hook = unsafe {
                SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(mouse_hook),
                    GetModuleHandleW(std::ptr::null()),
                    0,
                )
            };
            if hook.is_null() {
                return None;
            }
            if let Ok(mut slot) = active().lock() {
                *slot = Arc::downgrade(&capture);
            } else {
                unsafe {
                    UnhookWindowsHookEx(hook);
                }
                return None;
            }
            Some(Self {
                hook: hook as usize,
                capture,
            })
        }
        pub(super) fn set_rect(&self, rect: Rect) {
            if let Ok(mut capture) = self.capture.lock() {
                capture.rect = rect;
            }
        }
        pub(super) fn take_color(&self) -> Option<Color32> {
            self.capture.lock().ok()?.ready.take()
        }
    }
    impl Drop for Sampler {
        fn drop(&mut self) {
            unsafe {
                UnhookWindowsHookEx(self.hook as HHOOK);
            }
            if let Ok(mut slot) = active().lock() {
                if slot.ptr_eq(&Arc::downgrade(&self.capture)) {
                    *slot = Weak::new();
                }
            }
        }
    }

    unsafe extern "system" fn mouse_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
        if code >= 0 && matches!(message as u32, WM_LBUTTONDOWN | WM_LBUTTONUP) {
            let capture = active().lock().ok().and_then(|slot| slot.upgrade());
            if let Some(capture) = capture {
                // Windows supplies a valid MSLLHOOKSTRUCT for these hook messages.
                let event = &*(data as *const MSLLHOOKSTRUCT);
                let point = Pos2::new(event.pt.x as f32, event.pt.y as f32);
                if let Ok(mut capture) = capture.lock() {
                    if message as u32 == WM_LBUTTONUP && capture.swallow_release {
                        capture.swallow_release = false;
                        capture.ready = capture.pressed.take();
                        capture.ctx.request_repaint();
                        return 1;
                    }
                    if message as u32 == WM_LBUTTONDOWN && !capture.rect.contains(point) {
                        // Read exactly one desktop pixel; do not copy/encode a screenshot.
                        let dc = GetDC(std::ptr::null_mut());
                        if !dc.is_null() {
                            let pixel = GetPixel(dc, event.pt.x, event.pt.y);
                            ReleaseDC(std::ptr::null_mut(), dc);
                            if pixel != CLR_INVALID {
                                capture.pressed = Some(Color32::from_rgb(
                                    pixel as u8,
                                    (pixel >> 8) as u8,
                                    (pixel >> 16) as u8,
                                ));
                                capture.swallow_release = true;
                                return 1;
                            }
                        }
                    }
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, message, data)
    }
}
