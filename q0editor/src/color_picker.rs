//! The open palette owns outside clicks: pick a displayed screen pixel, then close.
use egui::{Color32, Context, Id, Key, Pos2, Rect, Response, Sense, Ui};
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
        if ctx
            .pointer_hover_pos()
            .is_some_and(|point| !request.rect.contains(point))
        {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }
}

pub(crate) fn color_edit_button_srgba(ui: &mut Ui, color: &mut Color32) -> Response {
    color_edit_button(ui, color, egui::color_picker::Alpha::BlendOrAdditive)
}

pub(crate) fn color_edit_button_srgb(ui: &mut Ui, color: &mut [u8; 3]) -> Response {
    let mut rgba = Color32::from_rgb(color[0], color[1], color[2]);
    let response = color_edit_button(ui, &mut rgba, egui::color_picker::Alpha::Opaque);
    *color = [rgba.r(), rgba.g(), rgba.b()];
    response
}

fn color_edit_button(
    ui: &mut Ui,
    color: &mut Color32,
    alpha: egui::color_picker::Alpha,
) -> Response {
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
        // A screen pixel has RGB, not the paint's authored opacity.
        let rgba = color.to_srgba_unmultiplied();
        let sample = Color32::from_rgba_unmultiplied(sample.r(), sample.g(), sample.b(), rgba[3]);
        sampled_change = *color != sample;
        *color = sample;
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
        egui::color_picker::show_color_at(ui.painter(), *color, paint_rect);
        ui.painter()
            .rect_stroke(paint_rect, visuals.rounding.at_most(2.0), visuals.bg_stroke);
    }
    response = response.on_hover_text("Click to edit color");
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
                    if egui::color_picker::color_picker_color32(ui, color, alpha) {
                        response.mark_changed();
                    }
                    ui.small("Click outside to pick a screen color · Esc to close");
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
    response
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
