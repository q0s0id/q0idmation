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

/// The colour shown under the mouse pointer, read straight off the screen, so
/// it works over anything: the stage, the panels, other windows. Windows
/// only; elsewhere [`request_pixel`] takes a screenshot of the editor.
#[cfg(windows)]
pub(crate) fn pixel_under_pointer() -> Option<Color32> {
    desktop::pixel_under_pointer()
}

#[derive(Clone, Copy)]
struct PixelRequest {
    viewport: egui::ViewportId,
    /// The point as a fraction of the screenshot's size.
    at: Pos2,
}

fn pixel_request_id(owner: Id) -> Id {
    owner.with("pixel_request")
}

fn pixel_result_id(owner: Id) -> Id {
    owner.with("pixel_result")
}

/// Asks for the colour shown at `point`, in this window. [`take_pixel`] hands
/// it to `owner` once known: at once where the screen can be read directly,
/// otherwise a frame or two later from a screenshot.
pub(crate) fn request_pixel(ctx: &Context, owner: Id, point: Pos2) {
    #[cfg(windows)]
    if let Some(color) = pixel_under_pointer() {
        ctx.data_mut(|data| data.insert_temp(pixel_result_id(owner), color));
        return;
    }
    let screen = ctx.screen_rect();
    let request = PixelRequest {
        viewport: ctx.viewport_id(),
        at: Pos2::new(
            (point.x - screen.left()) / screen.width().max(1.0),
            (point.y - screen.top()) / screen.height().max(1.0),
        ),
    };
    ctx.data_mut(|data| data.insert_temp(pixel_request_id(owner), request));
    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
    ctx.request_repaint();
}

/// The colour [`request_pixel`] asked for, once it is known.
pub(crate) fn take_pixel(ctx: &Context, owner: Id) -> Option<Color32> {
    if let Some(color) = ctx.data_mut(|data| data.remove_temp::<Color32>(pixel_result_id(owner))) {
        return Some(color);
    }
    let request = ctx.data(|data| data.get_temp::<PixelRequest>(pixel_request_id(owner)))?;
    let color = ctx.input(|input| {
        input.events.iter().find_map(|event| {
            let egui::Event::Screenshot { viewport_id, image } = event else {
                return None;
            };
            if *viewport_id != request.viewport || image.size[0] == 0 || image.size[1] == 0 {
                return None;
            }
            let x = ((request.at.x * image.size[0] as f32).floor().max(0.0) as usize)
                .min(image.size[0] - 1);
            let y = ((request.at.y * image.size[1] as f32).floor().max(0.0) as usize)
                .min(image.size[1] - 1);
            image
                .pixels
                .get(y * image.size[0] + x)
                .map(|color| color.to_opaque())
        })
    })?;
    ctx.data_mut(|data| data.remove::<PixelRequest>(pixel_request_id(owner)));
    Some(color)
}

/// Whether a colour popup is open.
pub(crate) fn is_open(ctx: &Context) -> bool {
    ctx.data(|data| data.get_temp::<PickerState>(state_id()))
        .is_some_and(|state| state.request.is_some())
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
            #[cfg(windows)]
            let sample = pixel_under_pointer();
            #[cfg(not(windows))]
            let sample = None;
            crate::cursors::request_with_sample(
                ctx,
                crate::cursors::CursorKind::ScreenPick,
                sample,
            );
        }
    }
}

pub fn color_edit_button_srgba(ui: &mut Ui, color: &mut Color32) -> Response {
    color_edit_button(
        ui,
        color,
        egui::color_picker::Alpha::BlendOrAdditive,
        None,
        None,
    )
    .0
}

pub fn color_edit_button_srgb(ui: &mut Ui, color: &mut [u8; 3]) -> Response {
    let mut rgba = Color32::from_rgb(color[0], color[1], color[2]);
    let response =
        color_edit_button(ui, &mut rgba, egui::color_picker::Alpha::Opaque, None, None).0;
    *color = [rgba.r(), rgba.g(), rgba.b()];
    response
}

/// What a paint box changed this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaintChange {
    #[default]
    None,
    /// the solid colour (`Response::changed` says so too)
    Color,
    /// a gradient's colours or their places
    Stops,
    /// gradient on or off, its kind, angle or size, or none turned on or
    /// off: it has to be fitted again
    Shape,
}

/// What a fill's popup paints with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PaintKind {
    Solid,
    Linear,
    Radial,
    Nothing,
}

/// A paint box's "none": whether the paint is none now, and whether it may
/// be made none (a shape keeps at least its fill or its stroke).
pub(crate) struct NoneSlot<'a> {
    pub(crate) none: &'a mut bool,
    pub(crate) allowed: bool,
}

/// Why none cannot be chosen when it is not allowed.
pub(crate) const NONE_REFUSED: &str =
    "The other paint is none already: a shape keeps its fill or its stroke";

/// A fill's colour box: a colour, or a linear or radial gradient, picked in
/// the same popup.
pub fn paint_edit_button(
    ui: &mut Ui,
    color: &mut Color32,
    gradient: &mut Option<PaintGradient>,
) -> (Response, PaintChange) {
    color_edit_button(
        ui,
        color,
        egui::color_picker::Alpha::BlendOrAdditive,
        Some(gradient),
        None,
    )
}

/// `paint_edit_button` whose paint can also be none: the box shows white
/// with a red line, and the popup's None segment turns it on. `none_allowed`
/// false greys that segment out. Picking a colour turns none off.
pub fn optional_paint_edit_button(
    ui: &mut Ui,
    color: &mut Color32,
    gradient: &mut Option<PaintGradient>,
    none: &mut bool,
    none_allowed: bool,
) -> (Response, PaintChange) {
    color_edit_button(
        ui,
        color,
        egui::color_picker::Alpha::BlendOrAdditive,
        Some(gradient),
        Some(NoneSlot {
            none,
            allowed: none_allowed,
        }),
    )
}

/// A plain colour box (a stroke's, say) that can also be none.
/// `Response::changed` covers none turning on or off.
pub fn optional_color_edit_button(
    ui: &mut Ui,
    color: &mut Color32,
    none: &mut bool,
    none_allowed: bool,
) -> Response {
    color_edit_button(
        ui,
        color,
        egui::color_picker::Alpha::BlendOrAdditive,
        None,
        Some(NoneSlot {
            none,
            allowed: none_allowed,
        }),
    )
    .0
}

/// What a popup edits: a colour, maybe a gradient, maybe none.
pub(crate) struct PaintTarget<'a> {
    pub(crate) color: &'a mut Color32,
    pub(crate) gradient: Option<&'a mut Option<PaintGradient>>,
    pub(crate) none: Option<NoneSlot<'a>>,
}

/// A paint box drawn in `rect`: none, a gradient, or the colour.
pub(crate) fn paint_box_face(
    painter: &egui::Painter,
    rect: Rect,
    color: Color32,
    gradient: Option<&PaintGradient>,
    none: bool,
) {
    if none {
        crate::color_picker_ui::paint_none(painter, rect, 0.0);
    } else if let Some(gradient) = gradient {
        paint_gradient(painter, rect, gradient, true);
    } else {
        egui::color_picker::show_color_at(painter, color, rect);
    }
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

fn selected_stop_id(owner: Id) -> Id {
    owner.with("gradient_stop")
}

/// Which stop of `owner`'s gradient is selected, for an editor that shows
/// the stop bar without its own picker.
pub(crate) fn selected_stop(ctx: &Context, owner: Id, stops: usize) -> usize {
    ctx.data(|data| data.get_temp::<usize>(selected_stop_id(owner)))
        .unwrap_or(0)
        .min(stops.saturating_sub(1))
}

/// The stop bar, its controls and presets, `width` wide, without a picker
/// for the selected stop's colour: the caller edits it, see
/// [`selected_stop`].
pub(crate) fn gradient_stops_editor(
    ui: &mut Ui,
    owner: Id,
    gradient: &mut PaintGradient,
    width: f32,
) -> PaintChange {
    gradient_editor(
        ui,
        owner,
        gradient,
        egui::color_picker::Alpha::BlendOrAdditive,
        None,
        width,
        false,
    )
}

/// The stop bar, its controls, presets and, with `picker`, the selected
/// stop's colour, in a column exactly `width` wide. Bounded so its
/// right-aligned buttons stay beside the bar: in a popup that sizes itself,
/// an unbounded row would reach the edge of the screen.
fn gradient_editor(
    ui: &mut Ui,
    owner: Id,
    gradient: &mut PaintGradient,
    alpha: egui::color_picker::Alpha,
    armed: Option<&mut bool>,
    width: f32,
    picker: bool,
) -> PaintChange {
    ui.vertical(|ui| {
        ui.set_width(width);
        gradient_editor_rows(ui, owner, gradient, alpha, armed, width, picker)
    })
    .inner
}

fn gradient_editor_rows(
    ui: &mut Ui,
    owner: Id,
    gradient: &mut PaintGradient,
    alpha: egui::color_picker::Alpha,
    armed: Option<&mut bool>,
    width: f32,
    picker: bool,
) -> PaintChange {
    use crate::color_picker_ui as face;
    let mut change = PaintChange::None;
    let selected_id = selected_stop_id(owner);
    let mut selected = selected_stop(ui.ctx(), owner, gradient.stops.len());
    let touch = crate::touch::ENABLED;
    let bar_height = if touch { 30.0 } else { 24.0 };
    let (bar, bar_response) = ui.allocate_exact_size(egui::vec2(width, bar_height), Sense::click());
    let ramp = gradient.clone();
    face::ramp(ui, bar, 8.0, owner.with("gradient_bar"), |t| {
        to_color32(ramp.sample(t))
    });

    // Round stop pins under the bar, pointing up at their place.
    let pin = if touch { 10.0 } else { 7.5 };
    let (row, _) = ui.allocate_exact_size(egui::vec2(width, pin * 2.0 + 8.0), Sense::hover());
    let mut remove = None;
    for (index, stop) in gradient.stops.iter_mut().enumerate() {
        let x = bar.left() + bar.width() * stop.position;
        let center = egui::pos2(x, row.top() + 5.0 + pin);
        let rect =
            egui::Rect::from_center_size(center, egui::vec2(pin * 2.0 + 4.0, pin * 2.0 + 10.0));
        let response = ui
            .interact(rect, selected_id.with(index), Sense::click_and_drag())
            .on_hover_text("Drag to move \u{b7} drag down or double-click to remove");
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
        face::stop_pin(
            ui.painter(),
            center,
            pin,
            to_color32(stop.color),
            index == selected,
            response.hovered(),
            ui.visuals().selection.stroke.color,
        );
    }
    let bar_response = bar_response.on_hover_text("Click to add a colour here");
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

    // Rows that wrap where they would not fit, as in a narrow Color panel;
    // in the popup they keep their buttons at the right.
    let narrow = width < 250.0;
    let row = |ui: &mut Ui, contents: &mut dyn FnMut(&mut Ui)| {
        if narrow {
            ui.horizontal_wrapped(|ui| contents(ui));
        } else {
            ui.horizontal(|ui| contents(ui));
        }
    };
    let at_right = |ui: &mut Ui, contents: &mut dyn FnMut(&mut Ui)| {
        if narrow {
            contents(ui);
        } else {
            // Right-aligned within the editor's own width. In a popup the
            // room to the right runs to the edge of the screen, and a
            // right-to-left row given all of it stretched the popup there.
            let used = ui.cursor().min.x - ui.min_rect().min.x;
            let room = egui::vec2((width - used).max(0.0), ui.spacing().interact_size.y);
            ui.allocate_ui_with_layout(
                room,
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| contents(ui),
            );
        }
    };
    row(ui, &mut |ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let mut percent = gradient.stops[selected].position * 100.0;
        ui.label(egui::RichText::new("Stop").color(ui.visuals().weak_text_color()));
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
        at_right(ui, &mut |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if ui
                .add_enabled(
                    gradient.stops.len() > 2,
                    egui::Button::new("Remove").small(),
                )
                .on_hover_text("Remove this stop")
                .clicked()
            {
                gradient.stops.remove(selected);
                selected = selected.min(gradient.stops.len() - 1);
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
            if ui
                .small_button("Reverse")
                .on_hover_text("Turn the colours round")
                .clicked()
            {
                gradient.reverse();
                selected = gradient.stops.len() - 1 - selected;
                change = PaintChange::Stops;
            }
        });
    });
    row(ui, &mut |ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        if gradient.kind == GradientKind::Linear {
            ui.label(egui::RichText::new("Angle").color(ui.visuals().weak_text_color()));
            if ui
                .add(
                    egui::DragValue::new(&mut gradient.angle)
                        .clamp_range(-180.0..=180.0)
                        .speed(1.0)
                        .suffix("\u{b0}")
                        .fixed_decimals(0),
                )
                .changed()
            {
                change = PaintChange::Shape;
            }
            for angle in [0.0, 45.0, 90.0, 135.0] {
                if face::arrow_button(ui, angle, (gradient.angle - angle).abs() < 0.5)
                    .on_hover_text(format!("{angle:.0}\u{b0}"))
                    .clicked()
                {
                    gradient.angle = angle;
                    change = PaintChange::Shape;
                }
            }
        }
        at_right(ui, &mut |ui| {
            if narrow {
                ui.label(egui::RichText::new("Size").color(ui.visuals().weak_text_color()));
            }
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
            if !narrow {
                ui.label(egui::RichText::new("Size").color(ui.visuals().weak_text_color()));
            }
        });
    });
    // Ready-made ramps, and a fade of the first colour.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        let count = crate::paint::PRESETS.len() + 1;
        let tile_width = ((width - 4.0 * (count as f32 - 1.0)) / count as f32).floor();
        let tile = egui::vec2(tile_width, if touch { 22.0 } else { 16.0 });
        for (index, (name, _)) in crate::paint::PRESETS.iter().enumerate() {
            let preset = PaintGradient::preset(index, gradient.kind, gradient.angle);
            let (rect, response) = ui.allocate_exact_size(tile, Sense::click());
            face::ramp(ui, rect, 4.0, owner.with(("gradient_preset", index)), |t| {
                to_color32(preset.sample(t))
            });
            if response.hovered() {
                face::hover_ring(ui, rect, 4.0);
            }
            if response.on_hover_text(*name).clicked() {
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
        let (rect, response) = ui.allocate_exact_size(tile, Sense::click());
        face::ramp(ui, rect, 4.0, owner.with("gradient_fade"), |t| {
            to_color32(fade.sample(t))
        });
        if response.hovered() {
            face::hover_ring(ui, rect, 4.0);
        }
        if response.on_hover_text("Fade out").clicked() {
            gradient.stops = fade.stops;
            selected = 0;
            change = PaintChange::Stops;
        }
    });
    if picker {
        ui.add_space(4.0);
        let mut stop_color = to_color32(gradient.stops[selected].color);
        let outcome = crate::color_picker_ui::picker(
            ui,
            &mut stop_color,
            crate::color_picker_ui::Frame {
                owner,
                key: owner.with(("gradient_stop_picker", selected)),
                alpha: alpha != egui::color_picker::Alpha::Opaque,
                closable: false,
                armed,
            },
        );
        if outcome.changed {
            gradient.stops[selected].color = to_rgba(stop_color);
            change = PaintChange::Stops;
        }
    }
    ui.data_mut(|data| data.insert_temp(selected_id, selected));
    change
}

fn color_edit_button(
    ui: &mut Ui,
    color: &mut Color32,
    alpha: egui::color_picker::Alpha,
    gradient: Option<&mut Option<PaintGradient>>,
    none: Option<NoneSlot<'_>>,
) -> (Response, PaintChange) {
    let owner = ui.auto_id_with("screen_color_popup");
    let (rect, mut response) = ui.allocate_exact_size(ui.spacing().interact_size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::new(egui::WidgetType::ColorButton));
    response = response.on_hover_text(match (gradient.is_some(), none.is_some()) {
        (true, true) => "Click to edit the colour or gradient, or make it none",
        (true, false) => "Click to edit the colour or gradient",
        (false, true) => "Click to edit the colour, or make it none",
        (false, false) => "Click to edit color",
    });
    let mut gradient = gradient;
    let mut none = none;
    let (changed, change) = paint_popup(
        ui,
        owner,
        response.clicked(),
        rect,
        alpha,
        PaintTarget {
            color,
            gradient: gradient.as_deref_mut(),
            none: none.as_mut().map(|slot| NoneSlot {
                none: &mut *slot.none,
                allowed: slot.allowed,
            }),
        },
    );
    if changed {
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let open = ui.memory(|memory| memory.is_popup_open(owner));
        let visuals = if open {
            &ui.visuals().widgets.open
        } else {
            ui.style().interact(&response)
        };
        let paint_rect = rect.expand(visuals.expansion);
        paint_box_face(
            ui.painter(),
            paint_rect,
            *color,
            gradient.as_deref().and_then(|g| g.as_ref()),
            none.as_ref().is_some_and(|slot| *slot.none),
        );
        ui.painter()
            .rect_stroke(paint_rect, visuals.rounding.at_most(2.0), visuals.bg_stroke);
    }
    (response, change)
}

/// Whether a "none" slot is switched on.
fn is_none(none: &Option<NoneSlot<'_>>) -> bool {
    none.as_ref().is_some_and(|slot| *slot.none)
}

/// The paint popup of a box at `anchor`, owned by `owner`: `toggle` opens
/// or closes it. It takes a pixel picked off the screen, shows the kind
/// (solid, linear, radial, none) when there is a choice, and the picker or
/// gradient editor. Returns whether the paint changed this frame, and how.
pub(crate) fn paint_popup(
    ui: &mut Ui,
    owner: Id,
    toggle: bool,
    anchor: Rect,
    alpha: egui::color_picker::Alpha,
    target: PaintTarget<'_>,
) -> (bool, PaintChange) {
    let PaintTarget {
        color,
        mut gradient,
        mut none,
    } = target;
    let mut change = PaintChange::None;
    let mut changed = false;
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
    if let Some(sample) = result {
        let painting_none = is_none(&none);
        let active = gradient
            .as_mut()
            .and_then(|g| g.as_mut())
            .filter(|_| !painting_none);
        if let Some(active) = active {
            // a picked pixel paints the selected stop
            let selected = selected_stop(ui.ctx(), owner, active.stops.len());
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
            changed = *color != sample;
            *color = sample;
            if let Some(slot) = none.as_mut().filter(|slot| *slot.none) {
                // a colour picked is a colour wanted: none is over
                *slot.none = false;
                if let Some(gradient) = gradient.as_mut() {
                    **gradient = None;
                }
                changed = true;
                change = PaintChange::Shape;
            }
        }
    }
    let open = ui.memory(|memory| memory.is_popup_open(owner));
    if !open && crate::color_picker_ui::has_open(ui.ctx(), owner) {
        // Closed: a colour that changed joins the recent ones.
        crate::color_picker_ui::finish(ui.ctx(), owner);
    }
    if toggle {
        ui.memory_mut(|memory| memory.toggle_popup(owner));
    }
    if ui.memory(|memory| memory.is_popup_open(owner)) {
        let area = egui::Area::new(owner)
            .order(egui::Order::Foreground)
            .fixed_pos(anchor.max)
            .constrain(true)
            .show(ui.ctx(), |ui| {
                ui.spacing_mut().slider_width = crate::color_picker_ui::width();
                let touch = crate::touch::ENABLED;
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(if touch { 14.0 } else { 10.0 }))
                    .rounding(12.0)
                    .show(ui, |ui| {
                        // The popup is as wide as its picker, whatever the
                        // mode: nothing in it may push it wider.
                        ui.set_max_width(crate::color_picker_ui::width());
                        let armed_key = armed_id(owner);
                        let mut armed =
                            ui.data(|data| data.get_temp::<bool>(armed_key).unwrap_or(false));
                        let mut close = false;
                        let chooses_kind = gradient.is_some() || none.is_some();
                        if chooses_kind {
                            // Solid, linear, radial or none, and the close button.
                            let before = if is_none(&none) {
                                PaintKind::Nothing
                            } else {
                                match gradient.as_deref().and_then(|g| g.as_ref()).map(|g| g.kind) {
                                    None => PaintKind::Solid,
                                    Some(GradientKind::Linear) => PaintKind::Linear,
                                    Some(GradientKind::Radial) => PaintKind::Radial,
                                }
                            };
                            let mut options = vec![(PaintKind::Solid, "Solid")];
                            if gradient.is_some() {
                                options.push((PaintKind::Linear, "Linear"));
                                options.push((PaintKind::Radial, "Radial"));
                            }
                            if none.is_some() {
                                options.push((PaintKind::Nothing, "None"));
                            }
                            let none_allowed = none.as_ref().is_some_and(|slot| slot.allowed);
                            let mut kind = before;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let side = if touch { 32.0 } else { 24.0 };
                                crate::color_picker_ui::segmented_enabled(
                                    ui,
                                    crate::color_picker_ui::width() - side - 4.0,
                                    &mut kind,
                                    &options,
                                    |option| {
                                        (option == PaintKind::Nothing
                                            && !none_allowed
                                            && before != PaintKind::Nothing)
                                            .then_some(NONE_REFUSED)
                                    },
                                );
                                if crate::color_picker_ui::close_button(ui).clicked() {
                                    close = true;
                                }
                            });
                            if kind != before {
                                if let Some(slot) = none.as_mut() {
                                    *slot.none = kind == PaintKind::Nothing;
                                }
                                if let Some(gradient) = gradient.as_deref_mut() {
                                    let start = |kind| {
                                        Some(PaintGradient::starting_from(to_rgba(*color), kind))
                                    };
                                    match (kind, gradient.as_mut()) {
                                        (PaintKind::Nothing, _) => {}
                                        (PaintKind::Solid, _) => *gradient = None,
                                        (PaintKind::Linear, Some(active)) => {
                                            active.kind = GradientKind::Linear
                                        }
                                        (PaintKind::Radial, Some(active)) => {
                                            active.kind = GradientKind::Radial
                                        }
                                        (PaintKind::Linear, None) => {
                                            *gradient = start(GradientKind::Linear)
                                        }
                                        (PaintKind::Radial, None) => {
                                            *gradient = start(GradientKind::Radial)
                                        }
                                    }
                                }
                                change = PaintChange::Shape;
                                changed = true;
                            }
                            ui.add_space(4.0);
                        }
                        let armed_slot = touch.then_some(&mut armed);
                        let painting_none = is_none(&none);
                        let active = gradient
                            .as_mut()
                            .and_then(|g| g.as_mut())
                            .filter(|_| !painting_none);
                        match active {
                            Some(active) => {
                                let edited = gradient_editor(
                                    ui,
                                    owner,
                                    active,
                                    alpha,
                                    armed_slot,
                                    crate::color_picker_ui::width(),
                                    true,
                                );
                                if edited != PaintChange::None && change != PaintChange::Shape {
                                    change = edited;
                                }
                            }
                            None => {
                                let outcome = crate::color_picker_ui::picker(
                                    ui,
                                    color,
                                    crate::color_picker_ui::Frame {
                                        owner,
                                        key: owner,
                                        alpha: alpha != egui::color_picker::Alpha::Opaque,
                                        closable: !chooses_kind,
                                        armed: armed_slot,
                                    },
                                );
                                if outcome.changed {
                                    changed = true;
                                    if let Some(slot) = none.as_mut().filter(|slot| *slot.none) {
                                        // a colour picked is a colour wanted
                                        *slot.none = false;
                                        if let Some(gradient) = gradient.as_mut() {
                                            **gradient = None;
                                        }
                                        change = PaintChange::Shape;
                                    }
                                }
                                close |= outcome.close;
                            }
                        }
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(if !touch {
                                "Click outside to pick a colour off the screen \u{b7} Esc closes"
                            } else if armed {
                                "Tap anywhere to take its colour"
                            } else {
                                "Tap outside to close"
                            })
                            .small()
                            .color(ui.visuals().weak_text_color()),
                        );
                        if touch {
                            ui.data_mut(|data| data.insert_temp(armed_key, armed));
                        }
                        if close {
                            ui.memory_mut(|memory| memory.close_popup());
                            ui.data_mut(|data| data.remove::<bool>(armed_key));
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
    if change == PaintChange::None && changed {
        change = PaintChange::Color;
    }
    (change != PaintChange::None, change)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The popup's width over a few frames on a wide screen, painting
    /// `color` with `gradient` (a fill) and `none`, as given.
    fn popup_widths(gradient: Option<Option<PaintGradient>>, none: Option<bool>) -> Vec<f32> {
        let ctx = Context::default();
        let owner = Id::new("paint_popup_width_test");
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1920.0, 1080.0),
            )),
            ..Default::default()
        };
        let mut gradient = gradient;
        let mut none = none;
        let mut widths = Vec::new();
        for frame in 0..6 {
            let _ = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut color = Color32::from_rgb(200, 40, 40);
                    paint_popup(
                        ui,
                        owner,
                        frame == 0,
                        Rect::from_min_size(egui::pos2(40.0, 40.0), egui::vec2(20.0, 20.0)),
                        egui::color_picker::Alpha::BlendOrAdditive,
                        PaintTarget {
                            color: &mut color,
                            gradient: gradient.as_mut(),
                            none: none.as_mut().map(|none| NoneSlot {
                                none,
                                allowed: true,
                            }),
                        },
                    );
                });
            });
            if let Some(rect) = ctx.memory(|memory| memory.area_rect(owner)) {
                widths.push(rect.width());
            }
        }
        widths
    }

    #[test]
    fn the_paint_popup_keeps_its_width_in_every_mode() {
        let red = Rgba {
            r: 200,
            g: 40,
            b: 40,
            a: 255,
        };
        let modes = [
            ("a plain colour", None, None),
            ("a solid fill", Some(None), Some(false)),
            (
                "a linear gradient",
                Some(Some(PaintGradient::starting_from(
                    red,
                    GradientKind::Linear,
                ))),
                Some(false),
            ),
            (
                "a radial gradient",
                Some(Some(PaintGradient::starting_from(
                    red,
                    GradientKind::Radial,
                ))),
                Some(false),
            ),
            ("no fill", Some(None), Some(true)),
            ("no stroke", None, Some(true)),
        ];
        // the picker's width and the popup's margins, and a little for its edge
        let most = crate::color_picker_ui::width() + 2.0 * 10.0 + 4.0;
        for wheel in [false, true] {
            crate::color_picker_ui::set_prefers_wheel(wheel);
            for (name, gradient, none) in modes.clone() {
                let widths = popup_widths(gradient, none);
                assert!(widths.len() >= 4, "{name}: the popup opened");
                let settled = &widths[2..];
                assert!(
                    settled.iter().all(|width| *width <= most),
                    "{name} (wheel {wheel}) grew past {most}: {widths:?}"
                );
                assert!(
                    settled
                        .windows(2)
                        .all(|pair| (pair[0] - pair[1]).abs() < 0.5),
                    "{name} (wheel {wheel}) kept changing width: {widths:?}"
                );
            }
        }
        crate::color_picker_ui::set_prefers_wheel(false);
    }
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
            CallNextHookEx, GetCursorPos, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK,
            MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP,
        },
    };

    /// One desktop pixel, in physical screen coordinates.
    fn pixel(x: i32, y: i32) -> Option<Color32> {
        // Read exactly one desktop pixel; do not copy/encode a screenshot.
        unsafe {
            let dc = GetDC(std::ptr::null_mut());
            if dc.is_null() {
                return None;
            }
            let pixel = GetPixel(dc, x, y);
            ReleaseDC(std::ptr::null_mut(), dc);
            (pixel != CLR_INVALID)
                .then(|| Color32::from_rgb(pixel as u8, (pixel >> 8) as u8, (pixel >> 16) as u8))
        }
    }

    pub(super) fn pixel_under_pointer() -> Option<Color32> {
        let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut point) } == 0 {
            return None;
        }
        pixel(point.x, point.y)
    }

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
                        if let Some(color) = pixel(event.pt.x, event.pt.y) {
                            capture.pressed = Some(color);
                            capture.swallow_release = true;
                            return 1;
                        }
                    }
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, message, data)
    }
}
