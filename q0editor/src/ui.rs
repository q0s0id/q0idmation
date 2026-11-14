//! Small, font-independent controls shared by the editor's working panels.
use egui::epaint::CubicBezierShape;
use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};

pub(crate) const ICON_CONTROL_SIZE: f32 = 26.0;

/// Logical screen size the editor's layout is designed never to go below.
pub(crate) const MIN_SCREEN_SIZE: egui::Vec2 = vec2(900.0, 560.0);

/// Makes a floating window usable on a screen smaller than the layout was
/// designed for. egui already stops a window growing past the screen; without
/// scrolling, whatever does not fit would simply be cut off at that edge.
pub(crate) trait FitToScreen {
    fn fit_to_screen(self, ctx: &egui::Context) -> Self;
}

impl FitToScreen for egui::Window<'_> {
    fn fit_to_screen(self, ctx: &egui::Context) -> Self {
        let screen_rect = ctx.screen_rect();
        let screen = screen_rect.size();
        let window = self.constrain_to(screen_rect);
        if screen.x >= MIN_SCREEN_SIZE.x && screen.y >= MIN_SCREEN_SIZE.y {
            window
        } else {
            window.scroll2([true, true])
        }
    }
}

/// Shrinks the whole interface when the device's screen is too small for the
/// editor at the system's own scale.
#[cfg(target_os = "android")]
pub(crate) fn apply_compact_zoom(ctx: &egui::Context, screen: Option<Rect>) {
    let (Some(screen), Some(native_scale)) = (screen, ctx.native_pixels_per_point()) else {
        return;
    };
    let native_points = screen.size() * ctx.pixels_per_point() / native_scale;
    // Whole percents only: the size computed above shifts by rounding error
    // from frame to frame, and every new factor costs a full relayout.
    let zoom = (compact_zoom_factor(native_points) * 100.0).floor() / 100.0;
    ctx.set_zoom_factor(zoom);
}

/// Interface zoom for a screen measuring `native_points` at the system's own
/// scale, where 1.0 leaves the interface at its normal size.
#[cfg(target_os = "android")]
fn compact_zoom_factor(native_points: egui::Vec2) -> f32 {
    // The touch layout keeps the canvas large by itself, so phones only get a
    // little denser and tablets keep the normal size. The long side barely
    // changes with the system bars, so turning the device keeps text the same.
    const FULL_SIZE_LONG_SIDE: f32 = 1000.0;
    (native_points.max_elem() / FULL_SIZE_LONG_SIDE).clamp(0.75, 1.0)
}

pub(crate) fn hidden_palette_note_trigger(ui: &Ui, label: &Response) {
    // A deliberately obscure gesture on an existing, otherwise inert label.
    // The counter and reveal live only in this UI session, never in a project.
    let chord = ui.input(|input| {
        let modifiers = input.modifiers;
        modifiers.ctrl && modifiers.alt && modifiers.shift
    });
    if !chord {
        return;
    }
    if label.interact(Sense::click()).secondary_clicked() {
        ui.ctx().data_mut(|data| {
            let id = egui::Id::new("palette_border_note_knocks");
            let knocks = data.get_temp::<u8>(id).unwrap_or(0) + 1;
            if knocks >= 7 {
                data.insert_temp(id, 0_u8);
                data.insert_temp(egui::Id::new("palette_border_note_open"), true);
            } else {
                data.insert_temp(id, knocks);
            }
        });
    }
}

pub(crate) fn hidden_palette_note(ctx: &egui::Context, settings_open: bool, accent: Color32) {
    let id = egui::Id::new("palette_border_note_open");
    if !settings_open {
        ctx.data_mut(|data| {
            data.remove::<bool>(id);
            data.remove::<u8>(egui::Id::new("palette_border_note_knocks"));
        });
        return;
    }
    let mut open = ctx.data(|data| data.get_temp::<bool>(id).unwrap_or(false));
    if !open {
        return;
    }
    egui::Window::new(" ")
        .id(egui::Id::new("palette_border_note_inline_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(300.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .fit_to_screen(ctx)
        .show(ctx, |ui| {
            ui.add_space(12.0);
            let text_color = ui.visuals().text_color();
            let text = ui.painter().layout_no_wrap(
                "I love you, Walibite".to_string(),
                egui::FontId::proportional(22.0),
                text_color,
            );
            let width = text.size().x + 8.0 + 18.0;
            let (row, _) = ui.allocate_exact_size(
                vec2(ui.available_width().max(width), text.size().y.max(24.0)),
                Sense::hover(),
            );
            let text_pos = pos2(
                row.center().x - width * 0.5,
                row.center().y - text.size().y * 0.5,
            );
            let heart = Rect::from_min_size(
                pos2(text_pos.x + text.size().x + 8.0, row.center().y - 9.0),
                vec2(18.0, 18.0),
            );
            ui.painter().galley(text_pos, text, text_color);
            paint_note_heart(ui.painter(), heart, accent);
            ui.add_space(12.0);
        });
    ctx.data_mut(|data| data.insert_temp(id, open));
}

fn paint_note_heart(painter: &Painter, rect: Rect, color: Color32) {
    // Rounded notch and tip keep the silhouette smooth and avoid acute stroke
    // joins. The small heart remains vector geometry at every display scale.
    const CURVES: [[(f32, f32); 4]; 7] = [
        [(10.0, 4.1), (9.75, 4.1), (9.5, 4.0), (9.35, 3.7)],
        [(9.35, 3.7), (6.7, 0.1), (1.0, 0.6), (1.0, 5.3)],
        [(1.0, 5.3), (1.0, 9.0), (5.8, 12.7), (9.1, 15.6)],
        [(9.1, 15.6), (9.6, 16.1), (10.4, 16.1), (10.9, 15.6)],
        [(10.9, 15.6), (14.2, 12.7), (19.0, 9.0), (19.0, 5.3)],
        [(19.0, 5.3), (19.0, 0.6), (13.3, 0.1), (10.65, 3.7)],
        [(10.65, 3.7), (10.5, 4.0), (10.25, 4.1), (10.0, 4.1)],
    ];
    let scale = rect.width() / 22.0;
    let point = |(x, y): (f32, f32)| rect.center() + vec2(x - 10.0, y - 8.0) * scale;
    let mut contour = Vec::new();
    for controls in CURVES {
        let curve = egui::epaint::CubicBezierShape::from_points_stroke(
            controls.map(point),
            false,
            Color32::TRANSPARENT,
            Stroke::NONE,
        );
        let points = curve.flatten(Some(0.03));
        contour.extend(points.into_iter().skip(usize::from(!contour.is_empty())));
    }
    contour.pop(); // The final point repeats the first one.
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(point((10.0, 8.0)), color);
    for position in &contour {
        mesh.colored_vertex(*position, color);
    }
    let count = contour.len() as u32;
    for index in 0..count {
        mesh.add_triangle(0, index + 1, (index + 1) % count + 1);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::closed_line(contour, Stroke::new(0.65_f32, color)));
}

/// Keep a dock's assigned size independent of its contents' minimum size.
/// egui panels otherwise persist an overflowing child's size as the next panel
/// size, which can cause a small overflow to grow the panel on every frame.
pub(crate) fn panel_contents<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let available = ui.available_rect_before_wrap();
    let rect = Rect::from_min_size(available.min, available.size().max(egui::Vec2::ZERO));
    let mut content = ui.child_ui_with_id_source(rect, *ui.layout(), "panel_contents");
    content.set_clip_rect(ui.clip_rect().intersect(rect));
    let result = add_contents(&mut content);
    ui.advance_cursor_after_rect(rect);
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
    Add,
    Close,
    Check,
    Rename,
    Delete,
    Import,
    Home,
    Grip,
    Search,
    Camera,
    Play,
    Pause,
    First,
    Previous,
    Next,
    Last,
    Onion,
    Layer,
    Folder,
    Mask,
    Symbol,
    Frame,
    Keyframe,
    BlankKeyframe,
    RemoveFrames,
    RemoveColumns,
    Tween,
    ShapeTween,
    Up,
    Down,
    Indent,
    Outdent,
    Fit,
    Back,
    Menu,
    Record,
    Undo,
    Redo,
    Properties,
    Library,
    Tools,
    Timeline,
    /// Two chevrons pointing left: put everything back.
    Expand,
    /// A parent square with two children hanging from it: layer parenting.
    Parenting,
}

pub(crate) fn icon_button(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    icon_button_enabled(ui, true, icon, label)
}

pub(crate) fn shortcut_button<'a>(
    hotkeys: &crate::hotkeys::HotkeySettings,
    command: crate::hotkeys::ShortcutCommand,
    label: &'a str,
) -> egui::Button<'a> {
    let shortcut = if hotkeys.shortcuts(command).is_empty() {
        String::new()
    } else {
        hotkeys.display(command)
    };
    egui::Button::new(label).shortcut_text(shortcut)
}

pub(crate) fn blend(a: Color32, b: Color32, amount: f32) -> Color32 {
    let channel =
        |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

/// The theme's chrome gradients as colours, handed from `Theme::apply` to
/// every widget through the egui context.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Chrome {
    pub enabled: bool,
    pub bar: [Color32; 2],
    pub panel: [Color32; 2],
    pub hover: [Color32; 2],
    pub selected: [Color32; 2],
    pub accent: Color32,
    pub text: Color32,
}

impl Chrome {
    /// The stops when gradients are on, otherwise `flat` top to bottom.
    pub(crate) fn pick(&self, stops: [Color32; 2], flat: Color32) -> [Color32; 2] {
        if self.enabled {
            stops
        } else {
            [flat, flat]
        }
    }
}

const CHROME_ID: &str = "q0editor_chrome_gradients";

pub(crate) fn set_chrome(ctx: &egui::Context, chrome: Chrome) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(CHROME_ID), chrome));
}

pub(crate) fn chrome(ctx: &egui::Context) -> Chrome {
    ctx.data(|data| data.get_temp(egui::Id::new(CHROME_ID)))
        .unwrap_or_default()
}

/// Blend of two premultiplied colours, alpha included.
fn mix_premultiplied(a: Color32, b: Color32, t: f32) -> Color32 {
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

/// A rounded rectangle fading from `top` to `bottom`, with egui's own soft
/// edges: the outline is tessellated as usual and then recoloured by height.
pub(crate) fn gradient_shape(
    ctx: &egui::Context,
    rect: Rect,
    rounding: f32,
    [top, bottom]: [Color32; 2],
) -> Shape {
    if top == bottom || rect.height() <= 0.0 || rect.width() <= 0.0 {
        return Shape::rect_filled(rect, rounding, top);
    }
    let mut outline = Vec::new();
    egui::epaint::tessellator::path::rounded_rectangle(
        &mut outline,
        rect,
        egui::Rounding::same(rounding),
    );
    let mut tessellator = egui::epaint::Tessellator::new(
        ctx.pixels_per_point(),
        Default::default(),
        [1, 1],
        Vec::new(),
    );
    let mut mesh = egui::epaint::Mesh::default();
    tessellator.tessellate_shape(
        Shape::convex_polygon(outline, Color32::WHITE, Stroke::NONE),
        &mut mesh,
    );
    for vertex in &mut mesh.vertices {
        let t = ((vertex.pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let coverage = f32::from(vertex.color.a()) / 255.0;
        vertex.color = mix_premultiplied(top, bottom, t).gamma_multiply(coverage);
    }
    Shape::mesh(mesh)
}

pub(crate) fn paint_gradient(painter: &Painter, rect: Rect, rounding: f32, stops: [Color32; 2]) {
    painter.add(gradient_shape(painter.ctx(), rect, rounding, stops));
}

/// Fills a bar (the menu bar, the document tabs) with the theme's bar
/// gradient. Call it first thing inside a top or bottom panel with egui's
/// default frame: the panel is its content plus that frame's margin (the
/// clip rect is not the panel, it reaches over the panels above).
pub(crate) fn paint_bar(ui: &Ui) {
    let chrome = chrome(ui.ctx());
    if chrome.enabled {
        let margin = egui::Frame::side_top_panel(ui.style()).inner_margin;
        let content = ui.max_rect();
        let panel = Rect::from_min_max(
            content.min - vec2(margin.left, margin.top),
            content.max + vec2(margin.right, margin.bottom),
        );
        paint_gradient(ui.painter(), panel, 0.0, chrome.bar);
    }
}

/// Height of the dock's tab strips; `workspace` sets the dock style from it.
pub(crate) const TAB_BAR_HEIGHT: f32 = 26.0;

const TAB_BARS_ID: &str = "q0editor_tab_bars";

/// Called first thing inside a docked panel: paints the panel gradient
/// behind its contents (unless `body` is false) and notes where its tab
/// strip is, for `paint_tab_bars`.
pub(crate) fn panel_backdrop(ui: &Ui, body: bool) {
    let chrome = chrome(ui.ctx());
    if !chrome.enabled {
        return;
    }
    let rect = ui.clip_rect();
    if body {
        paint_gradient(ui.painter(), rect, 0.0, chrome.panel);
    }
    let bar = Rect::from_min_max(
        pos2(rect.left(), rect.top() - TAB_BAR_HEIGHT),
        pos2(rect.right(), rect.top()),
    );
    let layer = ui.layer_id();
    ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<(egui::LayerId, Rect)>>(egui::Id::new(TAB_BARS_ID))
            .push((layer, bar));
    });
}

/// Fills `slot`, reserved on `ui`'s layer before the dock drew, with this
/// frame's tab strips, so they sit under the tabs.
pub(crate) fn paint_tab_bars(ui: &Ui, slot: egui::layers::ShapeIdx) {
    let bars: Vec<(egui::LayerId, Rect)> = ui
        .ctx()
        .data_mut(|data| data.remove_temp(egui::Id::new(TAB_BARS_ID)))
        .unwrap_or_default();
    let chrome = chrome(ui.ctx());
    if !chrome.enabled {
        return;
    }
    let shapes = bars
        .into_iter()
        .filter(|(layer, _)| *layer == ui.layer_id())
        .map(|(_, rect)| gradient_shape(ui.ctx(), rect, 0.0, chrome.bar))
        .collect();
    ui.painter().set(slot, Shape::Vec(shapes));
}

/// The soft gradient a button wears: the selection, or the hover at
/// `strength` (0…1, eased in and out by the caller).
fn paint_button_surface(ui: &Ui, rect: Rect, rounding: f32, selected: bool, strength: f32) {
    let chrome = chrome(ui.ctx());
    if selected {
        let stops = chrome.pick(chrome.selected, ui.visuals().selection.bg_fill);
        paint_gradient(ui.painter(), rect, rounding, stops);
        // A hairline of light along the top edge, and the accent around it.
        let accent = if chrome.accent == Color32::TRANSPARENT {
            ui.visuals().selection.bg_fill
        } else {
            chrome.accent
        };
        ui.painter().rect_stroke(
            rect,
            rounding,
            Stroke::new(1.0_f32, accent.gamma_multiply(0.7)),
        );
        ui.painter().line_segment(
            [
                pos2(rect.left() + rounding, rect.top() + 1.0),
                pos2(rect.right() - rounding, rect.top() + 1.0),
            ],
            Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
        );
    } else if strength > 0.0 {
        let [top, bottom] = chrome.pick(chrome.hover, ui.visuals().widgets.hovered.weak_bg_fill);
        paint_gradient(
            ui.painter(),
            rect,
            rounding,
            [
                top.gamma_multiply(strength),
                bottom.gamma_multiply(strength),
            ],
        );
    }
}

/// 0…1: how lit a control is, eased so the pointer passing over a row of
/// buttons leaves a short glow instead of a blink.
pub(crate) fn hover_strength(ui: &Ui, response: &Response) -> f32 {
    let lit = response.hovered() || response.has_focus() || response.is_pointer_button_down_on();
    ui.ctx()
        .animate_bool_with_time(response.id.with("lit"), lit, 0.12)
}

/// A tool or icon button's surface: selection gradient, or the eased hover.
pub(crate) fn paint_control(
    ui: &Ui,
    response: &Response,
    rect: Rect,
    rounding: f32,
    selected: bool,
) {
    let strength = hover_strength(ui, response);
    paint_button_surface(ui, rect, rounding, selected, strength);
}

pub(crate) fn icon_button_enabled(ui: &mut Ui, enabled: bool, icon: Icon, label: &str) -> Response {
    ui.add_enabled_ui(enabled, |ui| icon_control(ui, icon, label, false))
        .inner
}

pub(crate) fn icon_toggle(ui: &mut Ui, icon: Icon, label: &str, selected: bool) -> Response {
    icon_control(ui, icon, label, selected)
}

/// Side of an icon control meant for a fingertip rather than a mouse.
pub(crate) const TOUCH_CONTROL_SIZE: f32 = 40.0;

pub(crate) fn touch_icon_button(ui: &mut Ui, enabled: bool, icon: Icon, label: &str) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        icon_control_sized(ui, icon, label, false, TOUCH_CONTROL_SIZE)
    })
    .inner
}

pub(crate) fn touch_icon_toggle(ui: &mut Ui, icon: Icon, label: &str, selected: bool) -> Response {
    icon_control_sized(ui, icon, label, selected, TOUCH_CONTROL_SIZE)
}

pub(crate) fn sized_icon_toggle(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    size: f32,
) -> Response {
    icon_control_sized(ui, icon, label, selected, size)
}

pub(crate) fn sized_icon_button(ui: &mut Ui, icon: Icon, label: &str, size: f32) -> Response {
    icon_control_sized(ui, icon, label, false, size)
}

fn icon_control(ui: &mut Ui, icon: Icon, label: &str, selected: bool) -> Response {
    icon_control_sized(ui, icon, label, selected, ICON_CONTROL_SIZE)
}

fn icon_control_sized(ui: &mut Ui, icon: Icon, label: &str, selected: bool, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, selected, label));
    if ui.is_rect_visible(rect) {
        let rounding = (size * 0.24).round();
        let body = rect.shrink(1.0);
        paint_control(ui, &response, body, rounding, selected);
        let color = if selected {
            ui.visuals().selection.stroke.color
        } else if response.hovered() {
            ui.visuals().widgets.hovered.fg_stroke.color
        } else {
            ui.style().interact(&response).fg_stroke.color
        };
        let glyph = size * 16.0 / ICON_CONTROL_SIZE;
        // A pressed button sinks by half a point.
        let sink = if response.is_pointer_button_down_on() {
            vec2(0.0, 0.5)
        } else {
            egui::Vec2::ZERO
        };
        paint_icon(
            ui.painter(),
            Rect::from_center_size(rect.center() + sink, vec2(glyph, glyph)),
            icon,
            color,
        );
        if response.has_focus() {
            ui.painter()
                .rect_stroke(body, rounding, ui.visuals().selection.stroke);
        }
    }
    response
        .on_hover_text(label)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(crate) fn search_field(ui: &mut Ui, text: &mut String, hint: &str) -> Response {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 24.0), Sense::hover());
        paint_icon(
            ui.painter(),
            rect.shrink2(vec2(0.0, 4.0)),
            Icon::Search,
            ui.visuals().weak_text_color(),
        );
        let has_text = !text.is_empty();
        let clear_width = if has_text {
            ICON_CONTROL_SIZE + ui.spacing().item_spacing.x
        } else {
            0.0
        };
        // TextEdit's desired_width excludes its margins. Reserve those as well
        // as the trailing button and gap so the entire row fits its viewport.
        let margin = egui::Margin::symmetric(4.0, 2.0);
        let width = (ui.available_width() - clear_width - margin.sum().x).max(24.0);
        let mut response = ui.add(
            egui::TextEdit::singleline(text)
                .margin(margin)
                .hint_text(hint)
                .desired_width(width),
        );
        if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            text.clear();
            response.mark_changed();
            response.surrender_focus();
        }
        if has_text && icon_button(ui, Icon::Close, "clear search").clicked() {
            text.clear();
            response.mark_changed();
            response.request_focus();
        }
        response
    })
    .inner
}

pub(crate) fn name_button(ui: &mut Ui, text: &str, selected: bool, width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width.max(1.0), 26.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, selected, text)
    });
    if ui.is_rect_visible(rect) {
        if selected || response.hovered() || response.has_focus() {
            let fill = if selected {
                blend(
                    ui.visuals().panel_fill,
                    ui.visuals().selection.bg_fill,
                    0.22,
                )
            } else {
                ui.style().interact(&response).weak_bg_fill
            };
            ui.painter().rect_filled(rect, 4.0, fill);
        }
        if selected {
            ui.painter().rect_filled(
                Rect::from_min_max(rect.left_bottom() - vec2(0.0, 2.0), rect.right_bottom()),
                1.0,
                ui.visuals().selection.bg_fill,
            );
        }
        clipped_label(
            ui.painter(),
            rect.shrink2(vec2(6.0, 1.0)),
            text,
            egui::FontId::proportional(13.0),
            ui.visuals().text_color(),
        );
    }
    response
        .on_hover_text(text)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Allocate the entire grid, but interact with and paint only visible rows.
/// The source rectangle remains available even before it has scrolled on screen.
pub(crate) fn preview_card_grid(
    ui: &mut Ui,
    count: usize,
    source_index: Option<usize>,
    mut paint_card: impl FnMut(&mut Ui, usize, Rect),
) -> Option<Rect> {
    if count == 0 {
        return None;
    }
    let width = ui.available_width().max(48.0);
    let gap = ui.spacing().item_spacing.x.max(4.0);
    let columns = (((width + gap) / (64.0 + gap)).floor() as usize).max(1);
    let size = ((width - gap * columns.saturating_sub(1) as f32) / columns as f32).max(48.0);
    let rows = count.div_ceil(columns);
    let stride = size + gap;
    let height = rows as f32 * stride - gap;
    let (grid, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let card_rect = |index: usize| {
        Rect::from_min_size(
            grid.min
                + vec2(
                    (index % columns) as f32 * stride,
                    (index / columns) as f32 * stride,
                ),
            vec2(size, size),
        )
    };
    let source_rect = source_index.filter(|index| *index < count).map(card_rect);
    let visible = grid.intersect(ui.clip_rect());
    if visible.is_positive() {
        let first_row =
            (((visible.top() - grid.top()) / stride).floor().max(0.0) as usize).min(rows);
        let last_row =
            (((visible.bottom() - grid.top()) / stride).floor().max(0.0) as usize + 1).min(rows);
        for index in first_row * columns..(last_row * columns).min(count) {
            let rect = card_rect(index);
            if ui.is_rect_visible(rect) {
                // A child UI gives each visible cell a stable ID without
                // advancing the parent cursor beyond the allocated grid.
                let mut card_ui = ui.child_ui_with_id_source(rect, *ui.layout(), index);
                paint_card(&mut card_ui, index, rect);
            }
        }
    }
    source_rect
}

pub(crate) fn preview_card_at(
    ui: &mut Ui,
    label: &str,
    selected: bool,
    rect: Rect,
) -> (Response, Rect) {
    let response = ui.interact(rect, ui.id().with("preview_card"), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, selected, label)
    });
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        let fill = if selected {
            ui.visuals().selection.bg_fill
        } else {
            visuals.weak_bg_fill
        };
        let stroke = if selected {
            ui.visuals().selection.stroke
        } else {
            visuals.bg_stroke
        };
        let color = if selected {
            ui.visuals().selection.stroke.color
        } else {
            visuals.text_color()
        };
        let painter = ui.painter_at(rect);
        painter.rect(rect.shrink(0.5), 4.0, fill, stroke);
        clipped_label(
            &painter,
            Rect::from_min_max(
                rect.min + vec2(5.0, 2.0),
                pos2(rect.right() - 5.0, rect.top() + 18.0),
            ),
            label,
            egui::FontId::proportional(11.0),
            color,
        );
    }
    let preview = Rect::from_min_max(rect.min + vec2(3.0, 19.0), rect.max - vec2(3.0, 3.0));
    (
        response
            .on_hover_text(label)
            .on_hover_cursor(egui::CursorIcon::PointingHand),
        preview,
    )
}

/// Pixel-based ellipsis: long names use all available room without covering controls.
pub(crate) fn clipped_label(
    painter: &Painter,
    rect: Rect,
    text: &str,
    font: egui::FontId,
    color: Color32,
) {
    if rect.width() <= 2.0 {
        return;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap.max_width = rect.width();
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = painter.layout_job(job);
    painter.with_clip_rect(rect).galley(
        pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
}

/// The editor's icons: one colour in two strengths, an outline over a soft
/// fill of the same colour, with round line ends. They are drawn on a unit
/// square, so they stay sharp at any size.
pub(crate) fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let p = |x: f32, y: f32| {
        pos2(
            rect.left() + x * rect.width(),
            rect.top() + y * rect.height(),
        )
    };
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(p(x0, y0), p(x1, y1));
    let unit = rect.width().min(rect.height());
    let width = (unit * 0.095).clamp(1.25, 2.6);
    let stroke = Stroke::new(width, color);
    let thin = Stroke::new(width * 0.75, color);
    let tint = color.gamma_multiply(0.30);
    let cap = |at: Pos2| {
        painter.circle_filled(at, width * 0.5, color);
    };
    // An open stroke with round ends.
    let path = |points: &[(f32, f32)]| {
        let points: Vec<Pos2> = points.iter().map(|&(x, y)| p(x, y)).collect();
        if let (Some(first), Some(last)) = (points.first(), points.last()) {
            cap(*first);
            cap(*last);
        }
        painter.add(Shape::line(points, stroke));
    };
    let line = |a: (f32, f32), b: (f32, f32)| path(&[a, b]);
    // A closed outline over a soft fill: the body of most icons.
    let body = |points: &[(f32, f32)]| {
        painter.add(Shape::convex_polygon(
            points.iter().map(|&(x, y)| p(x, y)).collect(),
            tint,
            stroke,
        ));
    };
    let solid = |points: &[(f32, f32)]| {
        painter.add(Shape::convex_polygon(
            points.iter().map(|&(x, y)| p(x, y)).collect(),
            color,
            Stroke::NONE,
        ));
    };
    let dot = |x: f32, y: f32, radius: f32| painter.circle_filled(p(x, y), unit * radius, color);
    let ring = |x: f32, y: f32, radius: f32| painter.circle(p(x, y), unit * radius, tint, stroke);
    let card = |rect: Rect| painter.rect(rect, unit * 0.08, tint, stroke);
    // The small "+" in the lower right of the "new ..." icons.
    let plus = || {
        line((0.78, 0.63), (0.78, 0.93));
        line((0.63, 0.78), (0.93, 0.78));
    };
    match icon {
        Icon::Add => {
            line((0.5, 0.2), (0.5, 0.8));
            line((0.2, 0.5), (0.8, 0.5));
        }
        Icon::Close => {
            line((0.28, 0.28), (0.72, 0.72));
            line((0.72, 0.28), (0.28, 0.72));
        }
        Icon::Check => path(&[(0.18, 0.52), (0.41, 0.74), (0.82, 0.28)]),
        Icon::Rename => {
            body(&[(0.30, 0.56), (0.62, 0.24), (0.80, 0.42), (0.48, 0.74)]);
            solid(&[(0.27, 0.59), (0.45, 0.77), (0.15, 0.87)]);
            line((0.55, 0.31), (0.73, 0.49));
        }
        Icon::Delete => {
            line((0.16, 0.27), (0.84, 0.27));
            path(&[(0.39, 0.26), (0.42, 0.15), (0.58, 0.15), (0.61, 0.26)]);
            body(&[(0.25, 0.35), (0.75, 0.35), (0.70, 0.86), (0.30, 0.86)]);
            line((0.43, 0.48), (0.43, 0.73));
            line((0.57, 0.48), (0.57, 0.73));
        }
        Icon::Import => {
            painter.rect_filled(r(0.14, 0.66, 0.86, 0.84), 0.0, tint);
            path(&[(0.14, 0.58), (0.14, 0.84), (0.86, 0.84), (0.86, 0.58)]);
            line((0.5, 0.14), (0.5, 0.62));
            path(&[(0.32, 0.45), (0.5, 0.63), (0.68, 0.45)]);
        }
        Icon::Home => {
            body(&[
                (0.16, 0.46),
                (0.5, 0.16),
                (0.84, 0.46),
                (0.84, 0.86),
                (0.16, 0.86),
            ]);
            painter.rect_filled(r(0.42, 0.60, 0.58, 0.86), unit * 0.03, color);
        }
        Icon::Grip => {
            for x in [0.36, 0.64] {
                for y in [0.26, 0.5, 0.74] {
                    dot(x, y, 0.075);
                }
            }
        }
        Icon::Search => {
            ring(0.43, 0.43, 0.25);
            let heavy = Stroke::new(width * 1.5, color);
            painter.line_segment([p(0.62, 0.62), p(0.84, 0.84)], heavy);
            painter.circle_filled(p(0.84, 0.84), width * 0.75, color);
        }
        Icon::Camera => {
            card(r(0.08, 0.28, 0.64, 0.74));
            solid(&[(0.68, 0.44), (0.92, 0.30), (0.92, 0.72), (0.68, 0.58)]);
            dot(0.22, 0.40, 0.05);
        }
        Icon::Play => solid(&[(0.30, 0.17), (0.84, 0.5), (0.30, 0.83)]),
        Icon::Pause => {
            painter.rect_filled(r(0.26, 0.18, 0.42, 0.82), unit * 0.05, color);
            painter.rect_filled(r(0.58, 0.18, 0.74, 0.82), unit * 0.05, color);
        }
        Icon::Previous | Icon::Back => path(&[(0.62, 0.20), (0.34, 0.5), (0.62, 0.80)]),
        Icon::Next => path(&[(0.38, 0.20), (0.66, 0.5), (0.38, 0.80)]),
        Icon::First => {
            line((0.24, 0.20), (0.24, 0.80));
            solid(&[(0.78, 0.20), (0.36, 0.5), (0.78, 0.80)]);
        }
        Icon::Last => {
            line((0.76, 0.20), (0.76, 0.80));
            solid(&[(0.22, 0.20), (0.64, 0.5), (0.22, 0.80)]);
        }
        Icon::Onion => {
            painter.circle(
                p(0.37, 0.5),
                unit * 0.27,
                tint,
                Stroke::new(width * 0.8, color.gamma_multiply(0.5)),
            );
            painter.circle_stroke(p(0.63, 0.5), unit * 0.27, stroke);
        }
        Icon::Layer => {
            body(&[(0.10, 0.34), (0.46, 0.16), (0.82, 0.34), (0.46, 0.52)]);
            path(&[(0.10, 0.50), (0.46, 0.68), (0.58, 0.62)]);
            plus();
        }
        Icon::Folder => {
            painter.add(Shape::convex_polygon(
                [
                    (0.10, 0.34),
                    (0.88, 0.34),
                    (0.88, 0.56),
                    (0.62, 0.80),
                    (0.10, 0.80),
                ]
                .iter()
                .map(|&(x, y)| p(x, y))
                .collect(),
                tint,
                Stroke::NONE,
            ));
            path(&[
                (0.60, 0.80),
                (0.10, 0.80),
                (0.10, 0.22),
                (0.36, 0.22),
                (0.45, 0.34),
                (0.88, 0.34),
                (0.88, 0.58),
            ]);
            plus();
        }
        Icon::Mask => {
            ring(0.36, 0.40, 0.24);
            painter.rect_stroke(r(0.34, 0.24, 0.74, 0.62), unit * 0.06, stroke);
            plus();
        }
        Icon::Symbol => {
            body(&[(0.42, 0.10), (0.74, 0.42), (0.42, 0.74), (0.10, 0.42)]);
            plus();
        }
        Icon::Frame | Icon::Keyframe | Icon::BlankKeyframe => {
            card(r(0.10, 0.18, 0.66, 0.66));
            match icon {
                Icon::Keyframe => {
                    dot(0.38, 0.42, 0.10);
                }
                Icon::BlankKeyframe => {
                    painter.circle_stroke(p(0.38, 0.42), unit * 0.10, stroke);
                }
                _ => line((0.27, 0.42), (0.49, 0.42)),
            }
            plus();
        }
        Icon::RemoveFrames | Icon::RemoveColumns => {
            card(r(0.10, 0.16, 0.90, 0.62));
            if icon == Icon::RemoveColumns {
                painter.line_segment([p(0.37, 0.16), p(0.37, 0.62)], thin);
                painter.line_segment([p(0.63, 0.16), p(0.63, 0.62)], thin);
            }
            line((0.32, 0.83), (0.68, 0.83));
        }
        Icon::Tween => {
            dot(0.15, 0.5, 0.10);
            painter.circle_stroke(p(0.85, 0.5), unit * 0.10, stroke);
            line((0.31, 0.5), (0.66, 0.5));
            path(&[(0.54, 0.37), (0.67, 0.5), (0.54, 0.63)]);
        }
        Icon::ShapeTween => {
            painter.rect_filled(r(0.05, 0.38, 0.27, 0.62), unit * 0.03, color);
            painter.circle(p(0.84, 0.5), unit * 0.12, tint, stroke);
            line((0.36, 0.5), (0.61, 0.5));
            path(&[(0.50, 0.38), (0.62, 0.5), (0.50, 0.62)]);
        }
        Icon::Up => {
            line((0.5, 0.82), (0.5, 0.20));
            path(&[(0.27, 0.43), (0.5, 0.20), (0.73, 0.43)]);
        }
        Icon::Down => {
            line((0.5, 0.18), (0.5, 0.80));
            path(&[(0.27, 0.57), (0.5, 0.80), (0.73, 0.57)]);
        }
        Icon::Indent => {
            line((0.14, 0.18), (0.14, 0.82));
            line((0.30, 0.5), (0.84, 0.5));
            path(&[(0.64, 0.30), (0.84, 0.5), (0.64, 0.70)]);
        }
        Icon::Outdent => {
            line((0.86, 0.18), (0.86, 0.82));
            line((0.16, 0.5), (0.70, 0.5));
            path(&[(0.36, 0.30), (0.16, 0.5), (0.36, 0.70)]);
        }
        Icon::Fit => {
            painter.rect_filled(r(0.32, 0.32, 0.68, 0.68), unit * 0.06, tint);
            path(&[(0.14, 0.36), (0.14, 0.14), (0.36, 0.14)]);
            path(&[(0.64, 0.14), (0.86, 0.14), (0.86, 0.36)]);
            path(&[(0.86, 0.64), (0.86, 0.86), (0.64, 0.86)]);
            path(&[(0.36, 0.86), (0.14, 0.86), (0.14, 0.64)]);
        }
        Icon::Menu => {
            for y in [0.28, 0.5, 0.72] {
                line((0.2, y), (0.8, y));
            }
        }
        Icon::Record => {
            painter.circle_filled(p(0.5, 0.5), unit * 0.36, tint);
            dot(0.5, 0.5, 0.22);
        }
        Icon::Undo | Icon::Redo => {
            // A hook bending back to its arrowhead; redo is its mirror.
            let x = |x: f32| if icon == Icon::Undo { x } else { 1.0 - x };
            painter.add(CubicBezierShape::from_points_stroke(
                [
                    p(x(0.80), 0.82),
                    p(x(0.82), 0.46),
                    p(x(0.62), 0.38),
                    p(x(0.26), 0.38),
                ],
                false,
                Color32::TRANSPARENT,
                stroke,
            ));
            cap(p(x(0.80), 0.82));
            path(&[(x(0.40), 0.21), (x(0.23), 0.38), (x(0.40), 0.55)]);
        }
        Icon::Properties => {
            for (y, knob) in [(0.26, 0.34), (0.5, 0.66), (0.74, 0.42)] {
                line((0.14, y), (0.86, y));
                painter.circle(p(knob, y), unit * 0.10, color, Stroke::NONE);
            }
        }
        Icon::Library => {
            // Three books, the last leaning on the others.
            body(&[(0.12, 0.20), (0.30, 0.20), (0.30, 0.84), (0.12, 0.84)]);
            body(&[(0.36, 0.14), (0.54, 0.14), (0.54, 0.84), (0.36, 0.84)]);
            body(&[(0.60, 0.30), (0.76, 0.24), (0.92, 0.80), (0.76, 0.86)]);
            painter.line_segment([p(0.12, 0.34), p(0.30, 0.34)], thin);
            painter.line_segment([p(0.36, 0.28), p(0.54, 0.28)], thin);
        }
        Icon::Tools => {
            body(&[(0.26, 0.10), (0.26, 0.76), (0.74, 0.56)]);
            let tail = Stroke::new(unit * 0.12, color);
            painter.line_segment([p(0.46, 0.64), p(0.58, 0.88)], tail);
            painter.circle_filled(p(0.58, 0.88), unit * 0.06, color);
        }
        Icon::Timeline => {
            card(r(0.06, 0.30, 0.94, 0.70));
            for x in [0.28, 0.5, 0.72] {
                painter.line_segment([p(x, 0.30), p(x, 0.70)], thin);
            }
            dot(0.17, 0.5, 0.07);
            painter.line_segment([p(0.62, 0.16), p(0.62, 0.88)], stroke);
            solid(&[(0.52, 0.08), (0.72, 0.08), (0.62, 0.20)]);
        }
        Icon::Expand => {
            path(&[(0.48, 0.22), (0.22, 0.5), (0.48, 0.78)]);
            path(&[(0.78, 0.22), (0.52, 0.5), (0.78, 0.78)]);
        }
        Icon::Parenting => {
            painter.rect(r(0.12, 0.10, 0.40, 0.34), unit * 0.05, tint, stroke);
            painter.rect(r(0.60, 0.40, 0.88, 0.62), unit * 0.05, tint, stroke);
            painter.rect(r(0.60, 0.70, 0.88, 0.92), unit * 0.05, tint, stroke);
            path(&[(0.26, 0.36), (0.26, 0.81), (0.58, 0.81)]);
            line((0.26, 0.51), (0.58, 0.51));
        }
    }
}

/// The theme editor's gradient section: every stop of every surface, a
/// preview of each, and a way back to the theme's own.
pub(crate) fn gradient_editor(ui: &mut Ui, theme: &mut crate::settings::Theme) {
    use crate::settings::{ColorRgb, GradientStops};
    let pinned = theme.gradients.is_some();
    let mut gradients = theme.gradients();
    let before = gradients;
    let dim = theme.text_dim.to_color32();
    ui.horizontal(|ui| {
        ui.checkbox(&mut gradients.enabled, "Paint gradients");
        ui.label(
            egui::RichText::new(if pinned {
                "your own stops"
            } else {
                "following the colours above"
            })
            .small()
            .color(dim),
        );
    });
    let edit = |ui: &mut Ui, color: &mut ColorRgb| {
        let mut c = [color.r, color.g, color.b];
        if crate::color_picker::color_edit_button_srgb(ui, &mut c).changed() {
            *color = ColorRgb::new(c[0], c[1], c[2]);
        }
    };
    ui.add_enabled_ui(gradients.enabled, |ui| {
        egui::Grid::new("editor_theme_gradients")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                let rows: [(&str, &str, &mut GradientStops); 4] = [
                    (
                        "Bars",
                        "The menu bar, the document tabs and every panel's tab strip.",
                        &mut gradients.bar,
                    ),
                    (
                        "Panels",
                        "Behind each panel's contents. The stage keeps its canvas colour.",
                        &mut gradients.panel,
                    ),
                    (
                        "Hover",
                        "A tool or icon button under the pointer.",
                        &mut gradients.hover,
                    ),
                    (
                        "Selected",
                        "The chosen tool and the toggles that are on.",
                        &mut gradients.selected,
                    ),
                ];
                for (label, hint, stops) in rows {
                    ui.label(label).on_hover_text(hint);
                    ui.horizontal(|ui| {
                        edit(ui, &mut stops.top);
                        let (preview, _) = ui.allocate_exact_size(vec2(72.0, 20.0), Sense::hover());
                        paint_gradient(ui.painter(), preview, 4.0, stops.to_color32());
                        ui.painter().rect_stroke(
                            preview,
                            4.0,
                            Stroke::new(
                                1.0_f32,
                                ui.visuals().widgets.noninteractive.bg_stroke.color,
                            ),
                        );
                        edit(ui, &mut stops.bottom);
                        if ui
                            .small_button("⇅")
                            .on_hover_text("Swap the top and bottom colours")
                            .clicked()
                        {
                            std::mem::swap(&mut stops.top, &mut stops.bottom);
                        }
                    });
                    ui.end_row();
                }
            });
    });
    let mut follow = false;
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(pinned, egui::Button::new("Follow the colours"))
            .on_hover_text("Forget these stops: the gradients follow the base colours again")
            .clicked()
        {
            follow = true;
        }
        fn surfaces(gradients: &mut crate::settings::ThemeGradients) -> [&mut GradientStops; 4] {
            [
                &mut gradients.bar,
                &mut gradients.panel,
                &mut gradients.hover,
                &mut gradients.selected,
            ]
        }
        if ui
            .button("Softer")
            .on_hover_text("Bring each pair of stops closer together")
            .clicked()
        {
            for stops in surfaces(&mut gradients) {
                let (top, bottom) = (stops.top, stops.bottom);
                stops.top = crate::settings::blend(top, bottom, 0.25);
                stops.bottom = crate::settings::blend(bottom, top, 0.25);
            }
        }
        if ui
            .button("Stronger")
            .on_hover_text("Push each pair of stops further apart")
            .clicked()
        {
            for stops in surfaces(&mut gradients) {
                let middle = crate::settings::blend(stops.top, stops.bottom, 0.5);
                stops.top = crate::settings::blend(middle, stops.top, 1.35);
                stops.bottom = crate::settings::blend(middle, stops.bottom, 1.35);
            }
        }
    });
    if follow {
        theme.gradients = None;
    } else if gradients != before {
        theme.gradients = Some(gradients);
    }
}
