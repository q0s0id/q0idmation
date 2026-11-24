//! Floating windows and dialogs, dressed like the docked panels: a title
//! strip in the theme's bar gradient with the title on the left and icon
//! buttons on the right, over the panel gradient, inside the theme's window
//! frame. Every editor window goes through [`Dialog`] instead of building an
//! `egui::Window` itself, so they all change together with the theme.
//!
//! The strip is drawn inside the window rather than by egui: egui's own
//! title bar centres a heading, tints the front window's header and draws a
//! bare cross, none of which the rest of the editor does.

use egui::{
    pos2, vec2, Align2, Color32, Context, Id, InnerResponse, Margin, Pos2, Rect, Response,
    RichText, Rounding, Sense, Shape, Stroke, Ui, Vec2, WidgetText,
};

use crate::ui::{FitToScreen, Icon, MIN_SCREEN_SIZE};

/// Height of a dialog's title strip. A little taller than a dock tab strip,
/// so the title reads as the window's name rather than as a tab.
pub(crate) const TITLE_HEIGHT: f32 = 32.0;
/// Side of the icon buttons in the title strip.
const TITLE_BUTTON: f32 = 22.0;
/// Room left and right of the strip's contents.
const TITLE_PAD: f32 = 12.0;
/// How long a window takes to roll up or down, in seconds.
const ROLL_TIME: f32 = 0.22;
/// How long a window takes to appear, and how small it starts.
const OPEN_TIME: f32 = 0.18;
const OPEN_SCALE: f32 = 0.95;
/// How long a closed window takes to fade away, and how small it ends.
const CLOSE_TIME: f32 = 0.14;
const CLOSE_SCALE: f32 = 0.97;
/// A window drawn with more shapes than this is not kept for fading out.
const MAX_GHOST_SHAPES: usize = 6000;

/// An `egui::Window` with the editor's chrome. The builder methods are the
/// window's own; only the title strip, frame, body margin and scrolling are
/// handled here.
#[must_use = "a dialog does nothing until shown"]
pub(crate) struct Dialog<'open> {
    window: egui::Window<'static>,
    title: WidgetText,
    id: Id,
    open: Option<&'open mut bool>,
    collapsible: bool,
    resizable: bool,
    scroll: [bool; 2],
    margin: Option<Margin>,
}

impl<'open> Dialog<'open> {
    pub(crate) fn new(title: impl Into<WidgetText>) -> Self {
        let title = title.into();
        // The same id egui gives a window, so positions and sizes it already
        // remembers carry over.
        let id = Id::new(title.text());
        Self {
            window: egui::Window::new(title.clone()),
            title,
            id,
            open: None,
            collapsible: true,
            resizable: true,
            scroll: [false, false],
            margin: None,
        }
    }

    pub(crate) fn id(mut self, id: Id) -> Self {
        self.id = id;
        self.window = self.window.id(id);
        self
    }

    /// Shows a close button; it, like egui's, writes `false` here.
    pub(crate) fn open(mut self, open: &'open mut bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Whether the title strip offers to roll the window up to just itself.
    pub(crate) fn collapsible(mut self, collapsible: bool) -> Self {
        self.collapsible = collapsible;
        self
    }

    pub(crate) fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub(crate) fn vscroll(mut self, vscroll: bool) -> Self {
        self.scroll[1] = vscroll;
        self
    }

    /// Room around the body. The theme's window margin when not set.
    pub(crate) fn inner_margin(mut self, margin: impl Into<Margin>) -> Self {
        self.margin = Some(margin.into());
        self
    }

    pub(crate) fn default_pos(mut self, pos: impl Into<Pos2>) -> Self {
        self.window = self.window.default_pos(pos);
        self
    }

    pub(crate) fn fixed_pos(mut self, pos: impl Into<Pos2>) -> Self {
        self.window = self.window.fixed_pos(pos);
        self
    }

    pub(crate) fn anchor(mut self, align: Align2, offset: impl Into<Vec2>) -> Self {
        self.window = self.window.anchor(align, offset);
        self
    }

    pub(crate) fn pivot(mut self, pivot: Align2) -> Self {
        self.window = self.window.pivot(pivot);
        self
    }

    pub(crate) fn constrain_to(mut self, rect: Rect) -> Self {
        self.window = self.window.constrain_to(rect);
        self
    }

    pub(crate) fn default_size(mut self, size: impl Into<Vec2>) -> Self {
        self.window = self.window.default_size(size);
        self
    }

    pub(crate) fn default_width(mut self, width: f32) -> Self {
        self.window = self.window.default_width(width);
        self
    }

    pub(crate) fn default_height(mut self, height: f32) -> Self {
        self.window = self.window.default_height(height);
        self
    }

    pub(crate) fn min_size(mut self, size: impl Into<Vec2>) -> Self {
        self.window = self.window.min_size(size);
        self
    }

    pub(crate) fn min_width(mut self, width: f32) -> Self {
        self.window = self.window.min_width(width);
        self
    }

    pub(crate) fn min_height(mut self, height: f32) -> Self {
        self.window = self.window.min_height(height);
        self
    }

    pub(crate) fn max_width(mut self, width: f32) -> Self {
        self.window = self.window.max_width(width);
        self
    }

    pub(crate) fn max_height(mut self, height: f32) -> Self {
        self.window = self.window.max_height(height);
        self
    }

    /// Like `egui::Window::show`: `None` while closed, and an inner `None`
    /// while rolled up.
    pub(crate) fn show<R>(
        self,
        ctx: &Context,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> Option<InnerResponse<Option<R>>> {
        let Dialog {
            window,
            title,
            id,
            mut open,
            collapsible,
            resizable,
            scroll,
            margin,
        } = self;
        let collapsed_id = id.with("dialog_collapsed");
        let open_size_id = id.with("dialog_open_size");
        let collapsed =
            collapsible && ctx.data(|data| data.get_temp::<bool>(collapsed_id).unwrap_or(false));
        // 0 while open, 1 once rolled up, eased in between.
        let roll = if collapsible {
            ease_in_out(ctx.animate_bool_with_time(id.with("dialog_roll"), collapsed, ROLL_TIME))
        } else {
            0.0
        };
        // The window's size the last time it was all the way open: where a
        // roll starts from and where it goes back to.
        let open_size = (roll > 0.0)
            .then(|| ctx.data(|data| data.get_temp::<Vec2>(open_size_id)))
            .flatten();
        // Closed by its caller: nothing to draw. The window's last picture
        // fades out from `fade_closed`, rather than egui's empty frame.
        if matches!(open.as_deref(), Some(false)) {
            return None;
        }
        let appear = appear_progress(ctx, id);
        let style = ctx.style();
        let mut frame = egui::Frame::window(&style).inner_margin(Margin::ZERO);
        if appear < 1.0 {
            frame.fill = frame.fill.gamma_multiply(appear);
            frame.stroke.color = frame.stroke.color.gamma_multiply(appear);
            frame.shadow.color = frame.shadow.color.gamma_multiply(appear);
        }
        let margin = margin.unwrap_or(style.spacing.window_margin);
        // Rolling or rolled up, the window takes the size drawn here. Its own
        // size stays in egui's memory and comes back once it is open again.
        let resizable = resizable && roll == 0.0;
        let buttons = Buttons {
            close: open.is_some(),
            collapse: collapsible.then_some(collapsed),
        };
        let window = window
            .title_bar(false)
            .collapsible(false)
            .frame(frame)
            .resizable(resizable);
        let mut window: egui::Window<'_> = window;
        if let Some(open) = open.as_deref_mut() {
            window = window.open(open);
        }
        let mut pressed = None;
        let response = window.show(ctx, |ui| {
            if appear < 1.0 {
                ui.set_opacity(appear);
            }
            let backdrop = ui.painter().add(Shape::Noop);
            let strip = ui.painter().add(Shape::Noop);
            // The body starts right under the strip; its contents get the
            // usual spacing back inside.
            let spacing = ui.spacing().item_spacing;
            ui.spacing_mut().item_spacing.y = 0.0;
            let title_min = title_min_width(ui, &title, buttons);
            // Mid-roll: the share of the open window still showing.
            let rolling = open_size.map(|size| {
                let width = egui::lerp(size.x.max(title_min)..=title_min, roll);
                let body = vec2(size.x, (size.y - TITLE_HEIGHT).max(0.0));
                (width, body)
            });
            let title_width = rolling.map_or(title_min, |(width, _)| width);
            let (title_rect, _) =
                ui.allocate_exact_size(vec2(title_width, TITLE_HEIGHT), Sense::hover());
            let inner = (!(collapsed && roll >= 1.0)).then(|| {
                // Always in a child of its own, so the widgets keep their ids
                // (and their scroll, open and focus state) while rolling.
                let room = ui.available_rect_before_wrap();
                let (max_rect, shown) = match rolling {
                    Some((width, body)) => (
                        Rect::from_min_size(room.min, body),
                        Some(Rect::from_min_size(
                            room.min,
                            vec2(width, body.y * (1.0 - roll)),
                        )),
                    ),
                    None => (room, None),
                };
                let mut child = ui.child_ui_with_id_source(max_rect, *ui.layout(), "dialog_body");
                if let Some(shown) = shown {
                    child.set_clip_rect(shown.intersect(child.clip_rect()));
                }
                let inner = body(&mut child, margin, scroll, spacing, add_contents);
                ui.advance_cursor_after_rect(shown.unwrap_or_else(|| child.min_rect()));
                inner
            });
            // The window's size this frame, as `egui::Resize` will settle it.
            let mut rect = ui.min_rect();
            if resizable {
                rect = rect.union(ui.max_rect());
            }
            if collapsible && roll == 0.0 {
                ui.ctx()
                    .data_mut(|data| data.insert_temp(open_size_id, rect.size()));
            }
            pressed = finish_title(
                ui,
                TitleSlots { backdrop, strip },
                rect,
                title_rect,
                &title,
                buttons,
                frame,
            );
            inner
        });
        match pressed {
            Some(TitleButton::Close) => {
                if let Some(open) = open {
                    *open = false;
                }
            }
            Some(TitleButton::Collapse) => {
                ctx.data_mut(|data| data.insert_temp(collapsed_id, !collapsed));
            }
            None => {}
        }
        if let Some(shown) = &response {
            let layer = shown.response.layer_id;
            let rect = shown.response.rect;
            remember(ctx, id, layer, rect);
            if appear < 1.0 {
                let scale = egui::lerp(OPEN_SCALE..=1.0, appear);
                ctx.transform_layer_shapes(layer, scale_about(rect.center(), scale));
            }
        }
        response.map(|response| InnerResponse {
            inner: response.inner.flatten(),
            response: response.response,
        })
    }
}

impl FitToScreen for Dialog<'_> {
    fn fit_to_screen(mut self, ctx: &Context) -> Self {
        let screen_rect = ctx.screen_rect();
        self = self.constrain_to(screen_rect);
        let screen = screen_rect.size();
        if screen.x < MIN_SCREEN_SIZE.x || screen.y < MIN_SCREEN_SIZE.y {
            self.scroll = [true, true];
        }
        self
    }
}

#[derive(Clone, Copy)]
struct Buttons {
    close: bool,
    /// Whether the window is rolled up, when it can be.
    collapse: Option<bool>,
}

impl Buttons {
    fn width(self) -> f32 {
        let count = usize::from(self.close) + usize::from(self.collapse.is_some());
        count as f32 * (TITLE_BUTTON + 2.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TitleButton {
    Close,
    Collapse,
}

struct TitleSlots {
    backdrop: egui::layers::ShapeIdx,
    strip: egui::layers::ShapeIdx,
}

fn title_font() -> egui::FontId {
    egui::FontId::proportional(14.0)
}

/// The narrowest the strip can be: the title and its buttons. A rolled-up
/// window shrinks to this.
fn title_min_width(ui: &Ui, title: &WidgetText, buttons: Buttons) -> f32 {
    let text = ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(title.text().to_owned(), title_font(), egui::Color32::WHITE)
            .size()
            .x
    });
    TITLE_PAD * 2.0 + text + 8.0 + buttons.width()
}

/// Smoothstep: starts and lands softly.
fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn body<R>(
    ui: &mut Ui,
    margin: Margin,
    scroll: [bool; 2],
    spacing: Vec2,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let framed = |ui: &mut Ui| {
        egui::Frame::none()
            .inner_margin(margin)
            .show(ui, |ui| {
                body_style(ui, spacing);
                add_contents(ui)
            })
            .inner
    };
    if scroll.iter().any(|&on| on) {
        egui::ScrollArea::new(scroll).show(ui, framed).inner
    } else {
        framed(ui)
    }
}

/// Paints the backdrop and the strip once the window's size is known, then
/// lays the title and its buttons over the strip.
fn finish_title(
    ui: &mut Ui,
    slots: TitleSlots,
    rect: Rect,
    title_rect: Rect,
    title: &WidgetText,
    buttons: Buttons,
    frame: egui::Frame,
) -> Option<TitleButton> {
    let chrome = crate::ui::chrome(ui.ctx());
    let visuals = ui.visuals().clone();
    // The frame's stroke lies just outside the contents: the backdrop fills
    // exactly what it encloses.
    let radius = (frame.rounding.nw - frame.stroke.width * 0.5).max(0.0);
    // From the window's own painter, so it fades in with the window.
    let painter = ui.painter().with_clip_rect(rect.expand(1.0));
    if chrome.enabled {
        painter.set(
            slots.backdrop,
            crate::ui::gradient_shape(ui.ctx(), rect, radius, chrome.panel),
        );
    }
    let strip = Rect::from_x_y_ranges(rect.x_range(), title_rect.y_range());
    let top = Rounding {
        nw: radius,
        ne: radius,
        sw: 0.0,
        se: 0.0,
    };
    let fill = chrome.pick(chrome.bar, visuals.extreme_bg_color);
    painter.set(
        slots.strip,
        Shape::Vec(vec![
            crate::ui::gradient_shape(ui.ctx(), strip, top, fill),
            Shape::hline(
                strip.x_range(),
                strip.bottom() - 0.5,
                visuals.widgets.noninteractive.bg_stroke,
            ),
        ]),
    );

    let mut pressed = None;
    let mut right = strip.right() - TITLE_PAD + 4.0;
    let mut button = |ui: &mut Ui, icon: Icon, label: &str| {
        let at = Rect::from_center_size(
            pos2(right - TITLE_BUTTON * 0.5, strip.center().y),
            Vec2::splat(TITLE_BUTTON),
        );
        right -= TITLE_BUTTON + 2.0;
        // Own ids: a plain child Ui would share one with the body's frame.
        let mut child = ui.child_ui_with_id_source(
            at,
            egui::Layout::left_to_right(egui::Align::Center),
            ("dialog_title_button", label),
        );
        crate::ui::sized_icon_button(&mut child, icon, label, TITLE_BUTTON).clicked()
    };
    // Double-clicking the strip rolls the window up or down, as egui's did.
    let double_clicked = buttons.collapse.is_some()
        && ui
            .interact(
                strip.shrink2(vec2(buttons.width() + TITLE_PAD, 0.0)),
                ui.id().with("dialog_title"),
                Sense::click(),
            )
            .double_clicked();
    if buttons.close && button(ui, Icon::Close, "Close") {
        pressed = Some(TitleButton::Close);
    }
    if let Some(collapsed) = buttons.collapse {
        let (icon, label) = if collapsed {
            (Icon::Down, "Show the window")
        } else {
            (Icon::Up, "Roll up to the title")
        };
        if button(ui, icon, label) || double_clicked {
            pressed = Some(TitleButton::Collapse);
        }
    }
    let text = Rect::from_min_max(
        pos2(strip.left() + TITLE_PAD, strip.top()),
        pos2(right - 6.0, strip.bottom()),
    );
    crate::ui::clipped_label(
        &painter,
        text,
        title.text(),
        title_font(),
        visuals.strong_text_color(),
    );
    pressed
}

/// The look of everything inside a dialog: a little more room than egui's
/// default and softer, rounder controls, so a dialog's contents read as part
/// of the same family as its title strip.
fn body_style(ui: &mut Ui, spacing: Vec2) {
    let style = ui.style_mut();
    style.spacing.item_spacing = vec2(spacing.x.max(8.0), spacing.y.max(7.0));
    style.spacing.button_padding = vec2(12.0, 5.0);
    style.spacing.interact_size.y = style.spacing.interact_size.y.max(28.0);
    let widgets = &mut style.visuals.widgets;
    for widget in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.rounding = Rounding::same(7.0);
    }
}

// ---------------------------------------------------------------------------
// Appearing and fading away

/// How far through appearing the window `id` is, eased: 0 the frame it
/// shows after being away, 1 once it has fully arrived.
fn appear_progress(ctx: &Context, id: Id) -> f32 {
    let key = id.with("dialog_appear");
    let frame = ctx.frame_nr();
    let now = ctx.input(|input| input.time);
    let (last, started) = ctx
        .data(|data| data.get_temp::<(u64, f64)>(key))
        .unwrap_or((u64::MAX, now));
    // Shown last frame too: still the same appearance.
    let started = if last == frame || last.wrapping_add(1) == frame {
        started
    } else {
        now
    };
    ctx.data_mut(|data| data.insert_temp(key, (frame, started)));
    let t = ((now - started) as f32 / OPEN_TIME).clamp(0.0, 1.0);
    if t < 1.0 {
        ctx.request_repaint();
    }
    ease_out(t)
}

/// Fast at first, then landing softly.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Scales graphics by `scale` about `center`.
fn scale_about(center: Pos2, scale: f32) -> egui::emath::TSTransform {
    egui::emath::TSTransform::new(center.to_vec2() * (1.0 - scale), scale)
}

/// A window's picture from the last frame it was drawn.
#[derive(Clone)]
struct Picture {
    frame: u64,
    layer: egui::LayerId,
    rect: Rect,
    shapes: std::sync::Arc<Vec<egui::epaint::ClippedShape>>,
}

/// A closed window's last picture, fading away.
#[derive(Clone)]
struct Ghost {
    picture: Picture,
    started: f64,
}

#[derive(Clone, Default)]
struct Pictures {
    drawn: std::collections::HashMap<Id, Picture>,
    fading: Vec<Ghost>,
}

const PICTURES_ID: &str = "dialog_pictures";

fn with_pictures<R>(ctx: &Context, f: impl FnOnce(&mut Pictures) -> R) -> R {
    ctx.data_mut(|data| f(data.get_temp_mut_or_default::<Pictures>(Id::new(PICTURES_ID))))
}

/// Keeps what the window `id` drew this frame, to fade out from once its
/// caller stops showing it.
fn remember(ctx: &Context, id: Id, layer: egui::LayerId, rect: Rect) {
    let shapes = ctx.graphics_mut(|graphics| {
        let list = graphics.get(layer)?;
        if list.all_entries().len() > MAX_GHOST_SHAPES {
            return None;
        }
        Some(
            list.all_entries()
                // A paint callback draws from state its window may drop.
                .filter(|clipped| !matches!(clipped.shape, Shape::Callback(_)))
                .cloned()
                .collect::<Vec<_>>(),
        )
    });
    let frame = ctx.frame_nr();
    with_pictures(ctx, |pictures| {
        // Shown again: drop what was still fading on its layer.
        pictures.fading.retain(|ghost| ghost.picture.layer != layer);
        match shapes {
            Some(shapes) => {
                pictures.drawn.insert(
                    id,
                    Picture {
                        frame,
                        layer,
                        rect,
                        shapes: std::sync::Arc::new(shapes),
                    },
                );
            }
            None => {
                pictures.drawn.remove(&id);
            }
        }
    });
}

/// Fades out every window that was drawn last frame and not this one.
/// Call once a frame, after all windows have been shown.
pub(crate) fn fade_closed(ctx: &Context) {
    let frame = ctx.frame_nr();
    let now = ctx.input(|input| input.time);
    let ghosts = with_pictures(ctx, |pictures| {
        let closed: Vec<Id> = pictures
            .drawn
            .iter()
            .filter(|(_, picture)| picture.frame != frame)
            .map(|(id, _)| *id)
            .collect();
        for id in closed {
            if let Some(picture) = pictures.drawn.remove(&id) {
                // Only a window shown up to last frame fades; one gone
                // longer has nothing current to show.
                if picture.frame.wrapping_add(1) == frame {
                    pictures.fading.push(Ghost {
                        picture,
                        started: now,
                    });
                }
            }
        }
        pictures
            .fading
            .retain(|ghost| now - ghost.started < f64::from(CLOSE_TIME));
        pictures.fading.clone()
    });
    for ghost in &ghosts {
        let t = ((now - ghost.started) as f32 / CLOSE_TIME).clamp(0.0, 1.0);
        let gone = t * t;
        let picture = &ghost.picture;
        let mut ui = Ui::new(
            ctx.clone(),
            picture.layer,
            Id::new("dialog_ghost"),
            picture.rect,
            ctx.screen_rect(),
        );
        ui.set_opacity(1.0 - gone);
        let painter = ui.painter();
        for clipped in picture.shapes.iter() {
            painter
                .with_clip_rect(clipped.clip_rect)
                .add(clipped.shape.clone());
        }
        let scale = egui::lerp(1.0..=CLOSE_SCALE, gone);
        ctx.transform_layer_shapes(picture.layer, scale_about(picture.rect.center(), scale));
    }
    if !ghosts.is_empty() {
        ctx.request_repaint();
    }
}

// ---------------------------------------------------------------------------
// Contents shared by dialogs

/// What a dialog's message is about, for its badge and buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Info,
    Warning,
    Danger,
}

impl Tone {
    fn color(self, ui: &Ui) -> Color32 {
        match self {
            Self::Info => accent(ui),
            Self::Warning => Color32::from_rgb(0xF0, 0xA8, 0x30),
            Self::Danger => DANGER,
        }
    }
}

/// A red that reads as "this destroys something" on dark and light themes.
const DANGER: Color32 = Color32::from_rgb(0xE0, 0x4A, 0x4F);

fn accent(ui: &Ui) -> Color32 {
    match crate::ui::chrome(ui.ctx()).accent {
        Color32::TRANSPARENT => ui.visuals().selection.bg_fill,
        accent => accent,
    }
}

/// The picture in a dialog's badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Badge {
    Icon(Icon),
    Glyph(char),
}

/// How long a dialog's badge takes to arrive.
const BADGE_SECS: f32 = 0.45;

/// How far a dialog's badge has arrived, 0 to 1, since the dialog `id`
/// first showed. A dialog that went away starts over when it comes back.
fn arrival(ui: &Ui, id: Id) -> f32 {
    let ctx = ui.ctx();
    let now = ctx.input(|input| input.time);
    let frame = ctx.frame_nr();
    let seen: Option<(f64, u64)> = ctx.data(|data| data.get_temp(id));
    let since = match seen {
        Some((since, last)) if last + 2 >= frame => since,
        _ => now,
    };
    ctx.data_mut(|data| data.insert_temp(id, (since, frame)));
    let t = ((now - since) as f32 / BADGE_SECS).clamp(0.0, 1.0);
    if t < 1.0 {
        ctx.request_repaint();
    }
    t
}

/// Past 1 and back: what pops in overshoots a little before it settles.
fn ease_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let back = 1.6;
    1.0 + (back + 1.0) * (t - 1.0).powi(3) + back * (t - 1.0).powi(2)
}

/// A dialog's main message: a badge, a headline, and quieter lines of
/// detail under it.
///
/// The badge is a tinted tile with the tone's picture drawn on it: a
/// warning triangle, an "i", a bin, or any of the editor's icons. It pops
/// in as the dialog opens and sends one ring out. `urge` (0 to 1) is how
/// near the person is to the button that destroys something: the bin's lid
/// lifts with it.
pub(crate) fn lead(
    ui: &mut Ui,
    id: Id,
    tone: Tone,
    badge: Badge,
    headline: &str,
    detail: &[String],
    urge: f32,
) {
    let color = tone.color(ui);
    let arrived = arrival(ui, id.with("dialog_badge_arrival"));
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(50.0), Sense::hover());
        if ui.is_rect_visible(rect) {
            paint_badge(ui.painter(), rect.center(), color, badge, arrived, urge);
        }
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.set_max_width(360.0);
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.add_space(3.0);
            ui.label(
                RichText::new(headline)
                    .strong()
                    .size(16.5)
                    .color(ui.visuals().strong_text_color()),
            );
            for line in detail {
                ui.label(
                    RichText::new(line)
                        .size(13.5)
                        .color(ui.visuals().weak_text_color()),
                );
            }
        });
    });
}

/// A dialog's badge at `center`: a rounded tile tinted with `color`, its
/// picture on it. See [`lead`].
fn paint_badge(
    painter: &egui::Painter,
    center: Pos2,
    color: Color32,
    badge: Badge,
    arrived: f32,
    urge: f32,
) {
    let pop = 0.6 + 0.4 * ease_back(arrived);
    let half = 23.0 * pop;
    let tile = Rect::from_center_size(center, Vec2::splat(half * 2.0));
    // One ring goes out as it lands.
    if arrived < 1.0 {
        let ring = arrived;
        painter.rect_stroke(
            tile.expand(2.0 + 12.0 * ring),
            15.0 + 6.0 * ring,
            Stroke::new(2.0 * (1.0 - ring), color.gamma_multiply(0.7 * (1.0 - ring))),
        );
    }
    painter.rect_filled(
        tile.translate(vec2(0.0, 2.0)),
        15.0,
        Color32::from_black_alpha(40),
    );
    painter.rect_filled(tile, 15.0, color.gamma_multiply(0.16 + 0.08 * urge));
    painter.rect_stroke(tile, 15.0, Stroke::new(1.0_f32, color.gamma_multiply(0.55)));
    // A sheen across its top half.
    painter.rect_filled(
        Rect::from_min_max(
            tile.min + vec2(3.0, 3.0),
            pos2(tile.max.x - 3.0, tile.center().y),
        ),
        12.0,
        Color32::from_white_alpha(10),
    );
    let unit = half / 23.0;
    let stroke = Stroke::new(2.4 * unit, color);
    match badge {
        Badge::Glyph('!') => {
            // A warning triangle, rounded at its corners, rocking once as it lands.
            let rock = (1.0 - arrived) * (arrived * 14.0).sin() * 0.25;
            let turn = |v: Vec2| {
                let (sin, cos) = rock.sin_cos();
                center + vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos) * unit
            };
            let corners = [vec2(0.0, -11.0), vec2(11.5, 8.5), vec2(-11.5, 8.5)];
            let mut outline = Vec::new();
            for index in 0..3 {
                let at = corners[index];
                let before = corners[(index + 2) % 3];
                let after = corners[(index + 1) % 3];
                outline.push(turn(at + (before - at).normalized() * 2.5));
                outline.push(turn(at + (after - at).normalized() * 2.5));
            }
            painter.add(Shape::convex_polygon(
                outline.clone(),
                color.gamma_multiply(0.18),
                Stroke::NONE,
            ));
            painter.add(Shape::closed_line(outline, stroke));
            painter.line_segment([turn(vec2(0.0, -4.0)), turn(vec2(0.0, 2.0))], stroke);
            painter.circle_filled(turn(vec2(0.0, 5.6)), 1.5 * unit, color);
        }
        Badge::Glyph('i') => {
            painter.circle_stroke(center, 11.0 * unit, stroke);
            painter.circle_filled(center + vec2(0.0, -5.0) * unit, 1.5 * unit, color);
            painter.line_segment(
                [
                    center + vec2(0.0, -1.0) * unit,
                    center + vec2(0.0, 6.0) * unit,
                ],
                stroke,
            );
        }
        Badge::Icon(Icon::Delete) => paint_bin(painter, center, unit, color, arrived, urge),
        Badge::Icon(icon) => crate::ui::paint_icon(
            painter,
            Rect::from_center_size(center, Vec2::splat(22.0 * unit)),
            icon,
            color,
        ),
        Badge::Glyph(glyph) => {
            painter.text(
                center,
                Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(21.0 * unit),
                color,
            );
        }
    }
}

/// A bin: its body with three ribs, and a lid that hops as the dialog
/// lands and lifts open (`urge`) while the pointer is on the button that
/// throws things away.
fn paint_bin(
    painter: &egui::Painter,
    center: Pos2,
    unit: f32,
    color: Color32,
    arrived: f32,
    urge: f32,
) {
    let stroke = Stroke::new(2.2 * unit, color);
    let p = |x: f32, y: f32| center + vec2(x, y) * unit;
    let body = vec![p(-7.5, -4.0), p(7.5, -4.0), p(6.0, 10.5), p(-6.0, 10.5)];
    painter.add(Shape::convex_polygon(
        body.clone(),
        color.gamma_multiply(0.18),
        Stroke::NONE,
    ));
    painter.add(Shape::closed_line(body, stroke));
    for x in [-3.0, 0.0, 3.0] {
        painter.line_segment(
            [p(x, -0.5), p(x * 0.85, 7.5)],
            Stroke::new(1.6 * unit, color),
        );
    }
    // The lid turns open about its right end.
    let hop = (1.0 - arrived) * (arrived * 12.0).sin().abs() * 0.35;
    let open = 0.55 * urge + hop;
    let hinge = p(9.0, -7.0);
    let lid = |x: f32, y: f32| {
        let v = vec2(x, y) * unit - vec2(9.0, -7.0) * unit;
        let (sin, cos) = (-open).sin_cos();
        hinge + vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
    };
    painter.line_segment([lid(-9.0, -7.0), lid(9.0, -7.0)], stroke);
    painter.add(Shape::line(
        vec![
            lid(-3.0, -7.0),
            lid(-3.0, -10.0),
            lid(3.0, -10.0),
            lid(3.0, -7.0),
        ],
        Stroke::new(2.0 * unit, color),
    ));
}

/// How strongly a dialog button asks to be pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The way forward: filled with the accent.
    Primary,
    /// A way forward that destroys something: filled red.
    Danger,
    /// Everything else, Cancel included.
    Secondary,
}

/// A dialog button. Primary and danger buttons are filled; secondary ones
/// are soft and light up like the tool buttons.
pub(crate) fn button(ui: &mut Ui, text: &str, kind: Kind) -> Response {
    let fill = match kind {
        Kind::Primary => Some(accent(ui)),
        Kind::Danger => Some(DANGER),
        Kind::Secondary => None,
    };
    let color = match fill {
        Some(fill) => crate::theme::on_accent(fill),
        None => ui.visuals().text_color(),
    };
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(13.5), color);
    let size = vec2((galley.size().x + 30.0).max(88.0), 30.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, text));
    if ui.is_rect_visible(rect) {
        let lit = crate::ui::hover_strength(ui, &response);
        let rounding = 7.0;
        match fill {
            Some(fill) => {
                let fill = crate::ui::blend(fill, Color32::WHITE, 0.12 * lit);
                ui.painter().rect_filled(rect, rounding, fill);
                // A hairline of light along the top, as on the selected tools.
                ui.painter().line_segment(
                    [
                        pos2(rect.left() + rounding, rect.top() + 1.0),
                        pos2(rect.right() - rounding, rect.top() + 1.0),
                    ],
                    Stroke::new(1.0_f32, Color32::from_white_alpha(36)),
                );
            }
            None => {
                let rest =
                    crate::ui::blend(ui.visuals().window_fill, ui.visuals().text_color(), 0.07);
                ui.painter().rect(
                    rect,
                    rounding,
                    rest,
                    ui.visuals().widgets.inactive.bg_stroke,
                );
                crate::ui::paint_control(ui, &response, rect, rounding, false);
            }
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(2.0),
                rounding + 2.0,
                ui.visuals().selection.stroke,
            );
        }
        // A pressed button sinks by half a point.
        let sink = if response.is_pointer_button_down_on() {
            0.5
        } else {
            0.0
        };
        let at = rect.center() - galley.size() * 0.5 + vec2(0.0, sink);
        ui.painter().galley(at, galley, color);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A dialog's buttons on a band of their own along its bottom edge, laid
/// out from the right: add them in the order they should read from the
/// right, the way forward first. Put it last in the dialog.
pub(crate) fn footer<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(12.0);
    let band = ui.painter().add(Shape::Noop);
    let top = ui.cursor().top();
    // One button high: given the rest of the dialog, the buttons sat in the
    // middle of it and a short dialog grew to egui's default height.
    let row = vec2(ui.available_width(), 30.0);
    let inner = ui
        .allocate_ui_with_layout(
            row,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                add(ui)
            },
        )
        .inner;
    let margin = ui.spacing().window_margin;
    let content = ui.min_rect();
    let rect = Rect::from_min_max(
        pos2(content.left() - margin.left, top - 12.0),
        pos2(
            content.right() + margin.right,
            content.bottom() + margin.bottom,
        ),
    );
    let visuals = ui.visuals();
    let radius = (visuals.window_rounding.sw - visuals.window_stroke.width * 0.5).max(0.0);
    ui.painter().set(
        band,
        Shape::Vec(vec![
            Shape::rect_filled(
                rect,
                Rounding {
                    sw: radius,
                    se: radius,
                    ..Rounding::ZERO
                },
                visuals.extreme_bg_color.gamma_multiply(0.4),
            ),
            Shape::hline(
                rect.x_range(),
                rect.top() + 0.5,
                visuals.widgets.noninteractive.bg_stroke,
            ),
        ]),
    );
    inner
}

/// A question in a small dialog of its own: a badge, a headline, detail,
/// and buttons on a footer. Escape takes the cancel button and Enter the
/// default one, when the dialog has them.
#[must_use = "a question does nothing until shown"]
pub(crate) struct Confirm<'a> {
    title: &'a str,
    id: Option<Id>,
    tone: Tone,
    badge: Badge,
    headline: String,
    detail: Vec<String>,
    actions: Vec<(String, Kind)>,
    cancel: Option<usize>,
    default: Option<usize>,
}

impl<'a> Confirm<'a> {
    pub(crate) fn new(title: &'a str, tone: Tone, headline: impl Into<String>) -> Self {
        Self {
            title,
            id: None,
            tone,
            badge: match tone {
                Tone::Danger => Badge::Icon(Icon::Delete),
                Tone::Warning => Badge::Glyph('!'),
                Tone::Info => Badge::Glyph('i'),
            },
            headline: headline.into(),
            detail: Vec::new(),
            actions: Vec::new(),
            cancel: None,
            default: None,
        }
    }

    pub(crate) fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    pub(crate) fn badge(mut self, badge: Badge) -> Self {
        self.badge = badge;
        self
    }

    pub(crate) fn detail(mut self, line: impl Into<String>) -> Self {
        self.detail.push(line.into());
        self
    }

    /// A button, left of those added before it.
    pub(crate) fn action(mut self, label: impl Into<String>, kind: Kind) -> Self {
        self.actions.push((label.into(), kind));
        self
    }

    /// The button Enter presses: the way forward when it destroys nothing.
    pub(crate) fn default_action(mut self) -> Self {
        self.default = self.actions.len().checked_sub(1);
        self
    }

    /// The button Escape presses.
    pub(crate) fn cancel(mut self, label: impl Into<String>) -> Self {
        self.cancel = Some(self.actions.len());
        self.actions.push((label.into(), Kind::Secondary));
        self
    }

    /// The index of the button chosen this frame, in the order added.
    pub(crate) fn show(self, ctx: &Context) -> Option<usize> {
        let mut chosen = None;
        let mut dialog = Dialog::new(self.title)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .fit_to_screen(ctx);
        if let Some(id) = self.id {
            dialog = dialog.id(id);
        }
        // The bin's lid lifts while the pointer rests on a red button; last
        // frame's word on that, as the badge is drawn before the buttons.
        let urge_id = Id::new(("dialog_urge", self.title));
        let urged: bool = ctx.data(|data| data.get_temp(urge_id)).unwrap_or(false);
        let urge = ctx.animate_bool_with_time(urge_id.with("eased"), urged, 0.16);
        let mut urging = false;
        dialog.show(ctx, |ui| {
            lead(
                ui,
                urge_id,
                self.tone,
                self.badge,
                &self.headline,
                &self.detail,
                urge,
            );
            footer(ui, |ui| {
                for (index, (label, kind)) in self.actions.iter().enumerate() {
                    let response = button(ui, label, *kind);
                    urging |= *kind == Kind::Danger && response.hovered();
                    if response.clicked() {
                        chosen = Some(index);
                    }
                }
            });
        });
        ctx.data_mut(|data| data.insert_temp(urge_id, urging));
        if chosen.is_none() && !ctx.wants_keyboard_input() {
            let (escape, enter) = ctx.input(|input| {
                (
                    input.key_pressed(egui::Key::Escape),
                    input.key_pressed(egui::Key::Enter),
                )
            });
            if escape {
                // With nothing to cancel, Escape dismisses like the only button.
                chosen = self.cancel.or(self.default);
            } else if enter {
                chosen = self.default;
            }
        }
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ctx: &Context, events: Vec<egui::Event>, ui: impl FnOnce(&Context)) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))),
                events,
                ..Default::default()
            },
            ui,
        )
    }

    fn painted_text(output: &egui::FullOutput, text: &str) -> Option<Pos2> {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::Text(shape) if shape.galley.job.text == text => {
                    Some(shape.visual_bounding_rect().center())
                }
                _ => None,
            })
    }

    #[test]
    fn close_button_in_the_title_strip_closes_the_dialog() {
        let ctx = Context::default();
        crate::theme::install(&ctx);
        crate::settings::Settings::default().theme.apply(&ctx);
        let mut open = true;
        let mut close_at = None;
        let show = |ctx: &Context, open: &mut bool| {
            Dialog::new("Test dialog")
                .open(open)
                .collapsible(false)
                .resizable(false)
                .default_pos([100.0, 100.0])
                .show(ctx, |ui| ui.label("Body text"))
        };
        // A new window stays invisible for its first frame, while egui
        // learns its size.
        for frame in 0..3 {
            let output = run(&ctx, vec![], |ctx| {
                let shown = show(ctx, &mut open).expect("open dialog");
                close_at = Some(shown.response.rect.right_top());
            });
            if frame > 0 {
                assert!(painted_text(&output, "Test dialog").is_some());
                assert!(painted_text(&output, "Body text").is_some());
            }
        }
        let corner = close_at.unwrap();
        let pos = pos2(
            corner.x - TITLE_PAD + 4.0 - TITLE_BUTTON * 0.5,
            corner.y + TITLE_HEIGHT * 0.5,
        );
        for pressed in [true, false] {
            run(
                &ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                |ctx| {
                    show(ctx, &mut open);
                },
            );
        }
        assert!(!open);
    }

    #[test]
    fn confirm_answers_with_the_button_escape_or_enter() {
        let ctx = Context::default();
        crate::theme::install(&ctx);
        crate::settings::Settings::default().theme.apply(&ctx);
        let ask = |ctx: &Context| {
            Confirm::new("Question", Tone::Warning, "Keep going?")
                .detail("Nothing is lost either way.")
                .action("Go on", Kind::Primary)
                .default_action()
                .cancel("Stop")
                .show(ctx)
        };
        let key = |key| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut answer = None;
        for _ in 0..2 {
            let output = run(&ctx, vec![], |ctx| answer = ask(ctx));
            assert_eq!(answer, None);
            if painted_text(&output, "Keep going?").is_none() {
                continue;
            }
            assert!(painted_text(&output, "Go on").is_some());
            assert!(painted_text(&output, "Stop").is_some());
        }
        run(&ctx, vec![key(egui::Key::Escape)], |ctx| answer = ask(ctx));
        assert_eq!(answer, Some(1));
        run(&ctx, vec![key(egui::Key::Enter)], |ctx| answer = ask(ctx));
        assert_eq!(answer, Some(0));
    }

    /// The footer once took the rest of egui's default height and centred
    /// its buttons in it, so a two-line question stood 420 points tall.
    #[test]
    fn a_short_question_stays_short() {
        let ctx = Context::default();
        crate::theme::install(&ctx);
        crate::settings::Settings::default().theme.apply(&ctx);
        for _ in 0..3 {
            run(&ctx, vec![], |ctx| {
                let _ = Confirm::new("Short question", Tone::Warning, "Save first?")
                    .detail("One line of detail.")
                    .action("Save", Kind::Primary)
                    .cancel("Cancel")
                    .show(ctx);
            });
        }
        let height = ctx
            .memory(|memory| memory.area_rect(Id::new("Short question")))
            .expect("the question")
            .height();
        assert!(height < 220.0, "{height} points tall");
    }

    #[test]
    fn windows_fade_in_and_out_instead_of_popping() {
        let ctx = Context::default();
        crate::theme::install(&ctx);
        let frame_at = |ctx: &Context, time: f64, show: bool| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))),
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    if show {
                        Dialog::new("Fading").show(ctx, |ui| ui.label("Fading body"));
                    }
                    fade_closed(ctx);
                },
            )
        };
        let body_alpha = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    Shape::Text(text) if text.galley.job.text == "Fading body" => {
                        Some(text.override_text_color.unwrap_or(text.fallback_color).a())
                    }
                    _ => None,
                })
        };
        // Appearing: faint at first, solid once it has arrived.
        frame_at(&ctx, 0.0, true);
        let early = body_alpha(&frame_at(&ctx, 0.03, true)).expect("drawn while appearing");
        let late = body_alpha(&frame_at(&ctx, 1.0, true)).expect("drawn once arrived");
        assert!(early < late, "{early} should be fainter than {late}");
        // Closed: its picture stays a moment, fading, then goes.
        let fading = body_alpha(&frame_at(&ctx, 1.05, false)).expect("fading out");
        assert!(fading <= late);
        assert!(body_alpha(&frame_at(&ctx, 2.0, false)).is_none());
    }

    #[test]
    fn rolled_up_dialog_shows_only_its_title() {
        let ctx = Context::default();
        crate::theme::install(&ctx);
        let id = Id::new("roll_test");
        ctx.data_mut(|data| data.insert_temp(id.with("dialog_collapsed"), true));
        let mut inner = Some(Some(()));
        let mut output = None;
        for _ in 0..2 {
            output = Some(run(&ctx, vec![], |ctx| {
                inner = Dialog::new("Rolled")
                    .id(id)
                    .show(ctx, |ui| {
                        ui.label("Hidden body");
                    })
                    .map(|response| response.inner);
            }));
        }
        let output = output.unwrap();
        assert_eq!(inner, Some(None));
        assert!(painted_text(&output, "Rolled").is_some());
        assert!(painted_text(&output, "Hidden body").is_none());
    }

    #[test]
    fn every_badge_draws_while_arriving_and_after() {
        let ctx = Context::default();
        let badges = [
            Badge::Glyph('!'),
            Badge::Glyph('i'),
            Badge::Glyph('?'),
            Badge::Icon(Icon::Delete),
            Badge::Icon(Icon::Layer),
        ];
        for arrived in [0.0, 0.3, 1.0] {
            for urge in [0.0, 1.0] {
                let output = run(&ctx, vec![], |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        for badge in badges {
                            paint_badge(
                                ui.painter(),
                                pos2(100.0, 100.0),
                                DANGER,
                                badge,
                                arrived,
                                urge,
                            );
                        }
                    });
                });
                assert!(output.shapes.len() > badges.len());
            }
        }
    }
}
