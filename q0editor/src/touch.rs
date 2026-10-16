//! Canvas-first layout for touch screens.
//!
//! The desktop workspace docks five panels around the stage. Here the stage
//! gets nearly the whole screen: tools sit in a slim column, the timeline
//! folds down to a transport strip, and the library and properties slide in
//! over the canvas only when asked for.

use std::time::Duration;

use egui::{pos2, vec2, Align, Align2, Context, Layout, Rect, Ui};

use crate::app::{Action, EditorApp};
use crate::ui::{
    paint_icon, sized_icon_button, sized_icon_toggle, touch_icon_button, touch_icon_toggle, Icon,
    TOUCH_CONTROL_SIZE,
};

/// Whether this build lays the editor out for touch.
pub(crate) const ENABLED: bool = cfg!(target_os = "android");

const BAR_HEIGHT: f32 = TOUCH_CONTROL_SIZE + 8.0;
/// The folded timeline is used less than the canvas next to it, so it is
/// kept smaller than the other touch controls.
const TRANSPORT_CONTROL_SIZE: f32 = 32.0;
const DRAWER_MAX_WIDTH: f32 = 340.0;
/// How long a status message stays up before fading away.
const STATUS_SECONDS: f64 = 2.5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Drawer {
    #[default]
    Closed,
    Library,
    Properties,
}

/// What the touch layout remembers between frames.
#[derive(Default)]
pub(crate) struct TouchLayout {
    drawer: Drawer,
    timeline_open: bool,
    /// Modifier keys latched on screen, standing in for held keys.
    pub(crate) modifiers: egui::Modifiers,
    shown_status: String,
    status_since: f64,
    /// Whether the main menu was open last frame.
    menu_open: bool,
    /// Back asked for an Escape press on the next frame.
    pub(crate) press_escape: bool,
}

impl TouchLayout {
    fn toggle_drawer(&mut self, drawer: Drawer) {
        self.drawer = if self.drawer == drawer {
            Drawer::Closed
        } else {
            drawer
        };
    }
}

/// The one bar across the top: menus, documents, undo and the panel toggles.
pub(crate) fn top_bar(app: &mut EditorApp, ctx: &Context) {
    egui::TopBottomPanel::top("touch_bar")
        .resizable(false)
        .exact_height(BAR_HEIGHT)
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                main_menu(app, ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let editing = app.has_active_project() && !app.home_visible();
                    if editing {
                        panel_toggles(app, ui);
                        ui.separator();
                        history_buttons(app, ui);
                        ui.separator();
                        crate::panels::stage::zoom_controls(app, ui);
                        ui.separator();
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        // Inside a symbol the way back out matters more than
                        // the other open documents.
                        if editing && !app.session.breadcrumb.is_empty() {
                            crate::panels::stage::breadcrumbs(app, ui);
                        } else {
                            crate::panels::tabs::strip(app, ui);
                        }
                    });
                });
            });
        });
}

fn main_menu(app: &mut EditorApp, ui: &mut Ui) {
    let (response, open) = ui
        .scope(|ui| {
            // Blank text sized like the other bar buttons; the icon is painted over it.
            ui.spacing_mut().button_padding = vec2(14.0, 10.0);
            let menu = ui.menu_button("   ", |ui| {
                crate::panels::menu::menus(app, ui);
                ui.separator();
                if app.has_active_project() && !app.home_visible() {
                    if ui.button("Project...").clicked() {
                        app.queue(Action::ToggleProjectTree);
                        ui.close_menu();
                    }
                    if ui.button("Export (q0enc)...").clicked() {
                        app.queue(Action::ToggleQ0Enc);
                        ui.close_menu();
                    }
                }
                if ui.button("Import Animate files (q0fla)...").clicked() {
                    app.queue(Action::ToggleQ0Fla);
                    ui.close_menu();
                }
                if ui.button("Settings...").clicked() {
                    app.queue(Action::ToggleSettings);
                    ui.close_menu();
                }
            });
            (menu.response, menu.inner.is_some())
        })
        .inner;
    app.touch.menu_open = open;
    paint_icon(
        ui.painter(),
        Rect::from_center_size(response.rect.center(), vec2(20.0, 20.0)),
        Icon::Menu,
        ui.visuals().text_color(),
    );
}

/// The system Back button: closes the most passing thing that is open, and
/// with nothing left to close sends the editor to the background, the way
/// Back leaves other apps. Windows stay as they are either way.
pub(crate) fn back(app: &mut EditorApp, ctx: &Context) {
    if app.touch.drawer != Drawer::Closed {
        app.touch.drawer = Drawer::Closed;
        ctx.request_repaint();
        return;
    }
    let dismissable = app.touch.menu_open
        || app.text_field_focused
        || app.session.text_editor.is_some()
        || ctx.memory(|memory| memory.any_popup_open())
        || !matches!(app.session.tool_state, crate::state::ToolState::Idle);
    if dismissable {
        // Escape closes menus and popups, ends text editing and cancels
        // gestures in progress all over the editor already.
        app.touch.press_escape = true;
        ctx.request_repaint();
    } else {
        crate::platform::leave();
    }
}

fn history_buttons(app: &mut EditorApp, ui: &mut Ui) {
    // Right to left: the redo button ends up right of undo.
    if touch_icon_button(ui, app.history.can_redo(), Icon::Redo, "Redo").clicked() {
        app.queue(Action::Redo);
    }
    if touch_icon_button(ui, app.history.can_undo(), Icon::Undo, "Undo").clicked() {
        app.queue(Action::Undo);
    }
}

fn panel_toggles(app: &mut EditorApp, ui: &mut Ui) {
    let drawer = app.touch.drawer;
    if touch_icon_toggle(
        ui,
        Icon::Properties,
        "Properties",
        drawer == Drawer::Properties,
    )
    .clicked()
    {
        app.touch.toggle_drawer(Drawer::Properties);
    }
    if touch_icon_toggle(ui, Icon::Symbol, "Library", drawer == Drawer::Library).clicked() {
        app.touch.toggle_drawer(Drawer::Library);
    }
    if touch_icon_toggle(ui, Icon::Frame, "Timeline", app.touch.timeline_open).clicked() {
        app.touch.timeline_open = !app.touch.timeline_open;
    }
}

/// The project workspace: tool column, timeline, stage and the open drawer.
pub(crate) fn workspace(app: &mut EditorApp, ctx: &Context) {
    egui::SidePanel::left("touch_tools")
        .resizable(false)
        .exact_width(TOUCH_CONTROL_SIZE + 12.0)
        .show(ctx, |ui| {
            // A latched key changes what every tap does, so the keys stay in
            // sight instead of scrolling away with the tools.
            egui::TopBottomPanel::bottom("touch_modifiers")
                .frame(egui::Frame::none())
                .show_inside(ui, |ui| modifier_keys(app, ui));
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| crate::panels::toolbar::render(app, ui));
        });
    timeline(app, ctx);

    let free = ctx.available_rect();
    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show(ctx, |ui| crate::panels::stage::render(app, ui));
    drawer(app, ctx, free);
    status_toast(app, ctx, free);
}

/// Latching stand-ins for the modifier keys that tools read while dragging.
fn modifier_keys(app: &mut EditorApp, ui: &mut Ui) {
    let modifiers = &mut app.touch.modifiers;
    ui.vertical_centered(|ui| {
        latch(
            ui,
            &mut modifiers.shift,
            "Shift",
            "Constrain shapes and add to the selection",
        );
        latch(
            ui,
            &mut modifiers.ctrl,
            "Ctrl",
            "Toggle single frames and scale without the pivot",
        );
        latch(
            ui,
            &mut modifiers.alt,
            "Alt",
            "Move the pivot and drag out copies",
        );
    });
    // Shortcuts written for Mac read Command; elsewhere it means Ctrl.
    modifiers.command = modifiers.ctrl;
}

fn latch(ui: &mut Ui, on: &mut bool, label: &str, hint: &str) {
    let response = ui
        .add_sized(
            [TOUCH_CONTROL_SIZE, 30.0],
            egui::SelectableLabel::new(*on, label),
        )
        .on_hover_text(hint);
    if response.clicked() {
        *on = !*on;
    }
}

fn timeline(app: &mut EditorApp, ctx: &Context) {
    if app.touch.timeline_open {
        let screen = ctx.screen_rect().height();
        egui::TopBottomPanel::bottom("touch_timeline")
            .resizable(true)
            .default_height(screen * 0.3)
            .height_range(120.0..=screen * 0.6)
            .show(ctx, |ui| {
                crate::panels::timeline::render(app, ui);
                app.session
                    .clear_inactive_frame_selection(&app.state.project);
            });
    } else {
        egui::TopBottomPanel::bottom("touch_transport")
            .resizable(false)
            .exact_height(TRANSPORT_CONTROL_SIZE + 6.0)
            .show(ctx, |ui| transport(app, ui));
    }
}

/// The folded timeline: playback and frame stepping only.
fn transport(app: &mut EditorApp, ui: &mut Ui) {
    ui.horizontal_centered(|ui| {
        let (icon, label) = if app.session.playing {
            (Icon::Pause, "Pause")
        } else {
            (Icon::Play, "Play")
        };
        if sized_icon_button(ui, icon, label, TRANSPORT_CONTROL_SIZE).clicked() {
            app.queue(Action::TogglePlay);
        }
        let recording = app.session.record_held;
        if sized_icon_toggle(
            ui,
            Icon::Record,
            "Record held objects: dragging an object keys every frame",
            recording,
            TRANSPORT_CONTROL_SIZE,
        )
        .clicked()
        {
            app.session.record_held = !recording;
        }
        let key = sized_icon_button(
            ui,
            Icon::Keyframe,
            "Insert keyframe; tap with a second finger while holding an object",
            TRANSPORT_CONTROL_SIZE,
        );
        if key.clicked() || tapped_while_holding(app, ui, &key) {
            app.queue(Action::InsertKeyframe);
        }
        for (icon, label, action) in [
            (Icon::First, "First frame", Action::FirstFrame),
            (Icon::Previous, "Previous frame", Action::PreviousFrame),
        ] {
            if sized_icon_button(ui, icon, label, TRANSPORT_CONTROL_SIZE).clicked() {
                app.queue(action);
            }
        }
        let frames = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == app.session.current_q0rg_id)
            .map_or(1, |q0rg| q0rg.frame_count);
        ui.label(format!("{} / {frames}", app.session.timeline_frame() + 1));
        for (icon, label, action) in [
            (Icon::Next, "Next frame", Action::NextFrame),
            (Icon::Last, "Last frame", Action::LastFrame),
        ] {
            if sized_icon_button(ui, icon, label, TRANSPORT_CONTROL_SIZE).clicked() {
                app.queue(action);
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if sized_icon_button(ui, Icon::Up, "Show the timeline", TRANSPORT_CONTROL_SIZE)
                .clicked()
            {
                app.touch.timeline_open = true;
            }
        });
    });
}

/// A second finger tapping `button` while the first holds an object on the
/// canvas. egui follows only the first finger, so the button never sees that
/// tap as a click of its own.
fn tapped_while_holding(app: &EditorApp, ui: &Ui, button: &egui::Response) -> bool {
    !crate::app::held_object_layers(&app.session.tool_state, app.session.current_q0rg_id).is_empty()
        && ui.input(|input| {
            input.raw.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Touch {
                        phase: egui::TouchPhase::Start,
                        pos,
                        ..
                    } if button.rect.contains(*pos)
                )
            })
        })
}

/// Library or properties, laid over the right side of the canvas so opening
/// them never resizes or moves the drawing.
fn drawer(app: &mut EditorApp, ctx: &Context, free: Rect) {
    let drawer = app.touch.drawer;
    if drawer == Drawer::Closed {
        return;
    }
    let width = (free.width() * 0.85).min(DRAWER_MAX_WIDTH);
    let margin = 8.0;
    let inner = vec2(width, free.height()) - vec2(2.0 * margin, 2.0 * margin);
    egui::Area::new(egui::Id::new("touch_drawer"))
        .fixed_pos(pos2(free.right() - width, free.top()))
        .constrain(false)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(ui.visuals().panel_fill)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .inner_margin(margin)
                .show(ui, |ui| {
                    ui.set_min_size(inner);
                    ui.set_max_size(inner);
                    match drawer {
                        Drawer::Library => crate::panels::library::render(app, ui),
                        Drawer::Properties => crate::panels::properties::render(app, ui),
                        Drawer::Closed => {}
                    }
                });
        });
}

/// Status messages appear briefly over the canvas instead of taking a bar.
fn status_toast(app: &mut EditorApp, ctx: &Context, free: Rect) {
    let now = ctx.input(|input| input.time);
    if app.session.status != app.touch.shown_status {
        app.touch.shown_status = app.session.status.clone();
        app.touch.status_since = now;
    }
    let age = now - app.touch.status_since;
    if app.touch.shown_status.is_empty() || age > STATUS_SECONDS {
        return;
    }
    ctx.request_repaint_after(Duration::from_secs_f64(STATUS_SECONDS - age));
    egui::Area::new(egui::Id::new("touch_status"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(free.center().x, free.bottom() - 12.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(free.width() * 0.8);
                ui.label(app.touch.shown_status.as_str());
            });
        });
}
