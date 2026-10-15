//! Small, font-independent controls shared by the editor's working panels.
use egui::{pos2, vec2, Color32, Painter, Rect, Response, Sense, Shape, Stroke, Ui};

const ICON_CONTROL_SIZE: f32 = 26.0;

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
        let screen = ctx.screen_rect().size();
        if screen.x >= MIN_SCREEN_SIZE.x && screen.y >= MIN_SCREEN_SIZE.y {
            self
        } else {
            self.scroll2([true, true])
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
    Undo,
    Redo,
    Properties,
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
        let visuals = ui.style().interact_selectable(&response, selected);
        let fill = if selected {
            ui.visuals().selection.bg_fill
        } else {
            visuals.weak_bg_fill
        };
        ui.painter()
            .rect(rect.shrink(0.5), 4.0, fill, visuals.bg_stroke);
        let color = if selected {
            ui.visuals().selection.stroke.color
        } else {
            visuals.fg_stroke.color
        };
        let glyph = size * 16.0 / ICON_CONTROL_SIZE;
        paint_icon(
            ui.painter(),
            Rect::from_center_size(rect.center(), vec2(glyph, glyph)),
            icon,
            color,
        );
        if response.has_focus() {
            ui.painter()
                .rect_stroke(rect.shrink(0.5), 4.0, ui.visuals().selection.stroke);
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

pub(crate) fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let p = |x: f32, y: f32| {
        pos2(
            rect.left() + x * rect.width(),
            rect.top() + y * rect.height(),
        )
    };
    let r = |x0, y0, x1, y1| Rect::from_min_max(p(x0, y0), p(x1, y1));
    let stroke = Stroke::new(1.45_f32, color);
    let line = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([p(a.0, a.1), p(b.0, b.1)], stroke);
    };
    let path = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|(x, y)| p(*x, *y)).collect(),
            stroke,
        ));
    };
    let polygon = |points: &[(f32, f32)]| {
        painter.add(Shape::convex_polygon(
            points.iter().map(|(x, y)| p(*x, *y)).collect(),
            color,
            Stroke::NONE,
        ));
    };
    let plus = || {
        line((0.72, 0.60), (0.72, 0.96));
        line((0.54, 0.78), (0.90, 0.78));
    };
    match icon {
        Icon::Add => {
            line((0.5, 0.15), (0.5, 0.85));
            line((0.15, 0.5), (0.85, 0.5));
        }
        Icon::Close => {
            line((0.22, 0.22), (0.78, 0.78));
            line((0.78, 0.22), (0.22, 0.78));
        }
        Icon::Check => path(&[(0.13, 0.50), (0.39, 0.76), (0.88, 0.23)]),
        Icon::Rename => {
            path(&[
                (0.13, 0.68),
                (0.66, 0.15),
                (0.85, 0.34),
                (0.32, 0.87),
                (0.10, 0.90),
                (0.13, 0.68),
            ]);
            line((0.55, 0.26), (0.74, 0.45));
        }
        Icon::Delete => {
            line((0.14, 0.25), (0.86, 0.25));
            path(&[(0.27, 0.27), (0.31, 0.88), (0.69, 0.88), (0.73, 0.27)]);
            path(&[(0.36, 0.24), (0.36, 0.10), (0.64, 0.10), (0.64, 0.24)]);
            line((0.43, 0.40), (0.43, 0.73));
            line((0.57, 0.40), (0.57, 0.73));
        }
        Icon::Import => {
            path(&[(0.16, 0.60), (0.16, 0.87), (0.84, 0.87), (0.84, 0.60)]);
            line((0.5, 0.10), (0.5, 0.65));
            path(&[(0.29, 0.43), (0.5, 0.65), (0.71, 0.43)]);
        }
        Icon::Home => {
            path(&[(0.10, 0.43), (0.50, 0.12), (0.90, 0.43)]);
            path(&[(0.23, 0.38), (0.23, 0.86), (0.77, 0.86), (0.77, 0.38)]);
            path(&[(0.42, 0.85), (0.42, 0.58), (0.59, 0.58), (0.59, 0.85)]);
        }
        Icon::Grip => {
            for x in [0.35, 0.65] {
                for y in [0.24, 0.5, 0.76] {
                    painter.circle_filled(p(x, y), 1.2, color);
                }
            }
        }
        Icon::Search => {
            painter.circle_stroke(p(0.40, 0.40), rect.width() * 0.27, stroke);
            line((0.60, 0.60), (0.88, 0.88));
        }
        Icon::Camera => {
            painter.rect_stroke(r(0.06, 0.24, 0.65, 0.76), 1.5, stroke);
            polygon(&[(0.68, 0.36), (0.95, 0.22), (0.95, 0.78), (0.68, 0.64)]);
        }
        Icon::Play => polygon(&[(0.26, 0.13), (0.85, 0.5), (0.26, 0.87)]),
        Icon::Pause => {
            painter.rect_filled(r(0.24, 0.16, 0.40, 0.84), 1.0, color);
            painter.rect_filled(r(0.60, 0.16, 0.76, 0.84), 1.0, color);
        }
        Icon::Previous | Icon::Back => path(&[(0.68, 0.18), (0.30, 0.50), (0.68, 0.82)]),
        Icon::Next => path(&[(0.32, 0.18), (0.70, 0.50), (0.32, 0.82)]),
        Icon::First => {
            line((0.20, 0.16), (0.20, 0.84));
            path(&[(0.76, 0.19), (0.38, 0.50), (0.76, 0.81)]);
        }
        Icon::Last => {
            line((0.80, 0.16), (0.80, 0.84));
            path(&[(0.24, 0.19), (0.62, 0.50), (0.24, 0.81)]);
        }
        Icon::Onion => {
            painter.circle_stroke(
                p(0.35, 0.5),
                rect.width() * 0.26,
                Stroke::new(1.1_f32, color.gamma_multiply(0.5)),
            );
            painter.circle_stroke(p(0.64, 0.5), rect.width() * 0.26, stroke);
        }
        Icon::Layer => {
            path(&[
                (0.14, 0.25),
                (0.50, 0.10),
                (0.86, 0.25),
                (0.50, 0.42),
                (0.14, 0.25),
            ]);
            path(&[(0.14, 0.42), (0.50, 0.59), (0.78, 0.47)]);
            plus();
        }
        Icon::Folder => {
            path(&[
                (0.08, 0.75),
                (0.08, 0.20),
                (0.36, 0.20),
                (0.47, 0.35),
                (0.89, 0.35),
                (0.89, 0.55),
            ]);
            line((0.08, 0.75), (0.41, 0.75));
            plus();
        }
        Icon::Mask => {
            painter.circle_stroke(p(0.35, 0.39), rect.width() * 0.26, stroke);
            painter.rect_stroke(r(0.32, 0.25, 0.77, 0.70), 1.0, stroke);
            plus();
        }
        Icon::Symbol => {
            path(&[
                (0.44, 0.09),
                (0.79, 0.39),
                (0.44, 0.69),
                (0.09, 0.39),
                (0.44, 0.09),
            ]);
            plus();
        }
        Icon::Frame | Icon::Keyframe | Icon::BlankKeyframe => {
            painter.rect_stroke(r(0.12, 0.16, 0.77, 0.64), 1.0, stroke);
            if icon == Icon::Keyframe {
                painter.circle_filled(p(0.36, 0.40), 2.0, color);
            } else if icon == Icon::BlankKeyframe {
                painter.circle_stroke(p(0.36, 0.40), 2.0, stroke);
            } else {
                line((0.30, 0.46), (0.56, 0.46));
            }
            plus();
        }
        Icon::RemoveFrames | Icon::RemoveColumns => {
            painter.rect_stroke(r(0.12, 0.15, 0.88, 0.65), 1.0, stroke);
            if icon == Icon::RemoveColumns {
                line((0.38, 0.15), (0.38, 0.65));
                line((0.63, 0.15), (0.63, 0.65));
            }
            line((0.30, 0.86), (0.72, 0.86));
        }
        Icon::Tween => {
            painter.circle_filled(p(0.13, 0.50), 1.9, color);
            painter.circle_stroke(p(0.88, 0.50), 1.9, stroke);
            line((0.28, 0.50), (0.70, 0.50));
            path(&[(0.53, 0.30), (0.73, 0.50), (0.53, 0.70)]);
        }
        Icon::ShapeTween => {
            // A square becoming a circle.
            painter.rect_filled(r(0.02, 0.36, 0.26, 0.64), 0.0, color);
            painter.circle_stroke(p(0.86, 0.50), 2.4, stroke);
            line((0.36, 0.50), (0.66, 0.50));
            path(&[(0.52, 0.33), (0.69, 0.50), (0.52, 0.67)]);
        }
        Icon::Up => {
            line((0.50, 0.85), (0.50, 0.15));
            path(&[(0.25, 0.40), (0.50, 0.15), (0.75, 0.40)]);
        }
        Icon::Down => {
            line((0.50, 0.15), (0.50, 0.85));
            path(&[(0.25, 0.60), (0.50, 0.85), (0.75, 0.60)]);
        }
        Icon::Indent => {
            line((0.10, 0.16), (0.10, 0.84));
            line((0.24, 0.50), (0.87, 0.50));
            path(&[(0.63, 0.26), (0.87, 0.50), (0.63, 0.74)]);
        }
        Icon::Outdent => {
            line((0.90, 0.16), (0.90, 0.84));
            line((0.13, 0.50), (0.76, 0.50));
            path(&[(0.37, 0.26), (0.13, 0.50), (0.37, 0.74)]);
        }
        Icon::Fit => {
            path(&[(0.12, 0.36), (0.12, 0.12), (0.36, 0.12)]);
            path(&[(0.64, 0.12), (0.88, 0.12), (0.88, 0.36)]);
            path(&[(0.88, 0.64), (0.88, 0.88), (0.64, 0.88)]);
            path(&[(0.36, 0.88), (0.12, 0.88), (0.12, 0.64)]);
        }
        Icon::Menu => {
            for y in [0.26, 0.5, 0.74] {
                line((0.16, y), (0.84, y));
            }
        }
        Icon::Undo => {
            path(&[
                (0.84, 0.82),
                (0.84, 0.58),
                (0.74, 0.43),
                (0.58, 0.37),
                (0.18, 0.37),
            ]);
            path(&[(0.36, 0.19), (0.18, 0.37), (0.36, 0.55)]);
        }
        Icon::Redo => {
            path(&[
                (0.16, 0.82),
                (0.16, 0.58),
                (0.26, 0.43),
                (0.42, 0.37),
                (0.82, 0.37),
            ]);
            path(&[(0.64, 0.19), (0.82, 0.37), (0.64, 0.55)]);
        }
        Icon::Properties => {
            for (y, knob) in [(0.24, 0.34), (0.5, 0.66), (0.76, 0.44)] {
                line((0.12, y), (0.88, y));
                painter.circle_filled(p(knob, y), rect.width() * 0.09, color);
            }
        }
    }
}
