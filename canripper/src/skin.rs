//! CanRipper's look: the desktop software of around 2007, the Flash studio
//! of that year and the Windows of the time. Gradient title bars, bevelled
//! buttons, classic tabs and sunken status fields, in any of the skins
//! below or in any q0 theme.

use egui::{pos2, vec2, Color32, Mesh, Painter, Rect, Response, Rounding, Sense, Stroke, Ui};

use crate::icons::{draw_icon, UiIcon};

/// The colours a skin paints with.
#[derive(Debug, Clone, PartialEq)]
pub struct Skin {
    pub id: &'static str,
    pub name: &'static str,
    /// Panels, bars and buttons.
    pub face: Color32,
    /// Windows, menus and popups.
    pub window: Color32,
    /// Text fields, the tree and lists.
    pub field: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    /// Borders and the dark side of a bevel.
    pub border: Color32,
    /// Title bars run from `title[0]` at the top to `title[1]`.
    pub title: [Color32; 2],
    pub title_text: Color32,
    /// The highlight colour: hover, focus, links.
    pub accent: Color32,
    /// What is selected, and its text.
    pub select: Color32,
    pub select_text: Color32,
}

const fn rgb(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// The skins drawn for CanRipper, before the shared q0 themes.
const RETRO: &[Skin] = &[
    Skin {
        id: "studio-2007",
        name: "Studio 2007",
        face: rgb(0xE3E3E3),
        window: rgb(0xF0F0F0),
        field: rgb(0xFFFFFF),
        text: rgb(0x1A1A1A),
        text_dim: rgb(0x666666),
        border: rgb(0x8C8C8C),
        title: [rgb(0x626262), rgb(0x383838)],
        title_text: rgb(0xF2F2F2),
        accent: rgb(0x3A78D4),
        select: rgb(0x3D7DDB),
        select_text: rgb(0xFFFFFF),
    },
    Skin {
        id: "silver-2007",
        name: "Silver 2007",
        face: rgb(0xE6EAF0),
        window: rgb(0xF3F5F8),
        field: rgb(0xFFFFFF),
        text: rgb(0x1E2329),
        text_dim: rgb(0x68707C),
        border: rgb(0xA4ADB9),
        title: [rgb(0xF5F7FA), rgb(0xC5CCD6)],
        title_text: rgb(0x26303C),
        accent: rgb(0xE8962A),
        select: rgb(0xFFD983),
        select_text: rgb(0x1E2329),
    },
    Skin {
        id: "ebony-2007",
        name: "Ebony 2007",
        face: rgb(0x393B3E),
        window: rgb(0x2E3033),
        field: rgb(0x1F2022),
        text: rgb(0xEDEDED),
        text_dim: rgb(0xA3A6AA),
        border: rgb(0x151617),
        title: [rgb(0x5C6168), rgb(0x2A2C30)],
        title_text: rgb(0xF4F4F4),
        accent: rgb(0xF0A133),
        select: rgb(0xD98A1F),
        select_text: rgb(0x111111),
    },
    Skin {
        id: "luna",
        name: "Luna Blue",
        face: rgb(0xECE9D8),
        window: rgb(0xF6F5EF),
        field: rgb(0xFFFFFF),
        text: rgb(0x000000),
        text_dim: rgb(0x6D6D6D),
        border: rgb(0x7F9DB9),
        title: [rgb(0x3A8CF0), rgb(0x0842A8)],
        title_text: rgb(0xFFFFFF),
        accent: rgb(0x316AC5),
        select: rgb(0x316AC5),
        select_text: rgb(0xFFFFFF),
    },
    Skin {
        id: "classic",
        name: "Classic Grey",
        face: rgb(0xC0C0C0),
        window: rgb(0xC8C8C8),
        field: rgb(0xFFFFFF),
        text: rgb(0x000000),
        text_dim: rgb(0x555555),
        border: rgb(0x808080),
        title: [rgb(0x000080), rgb(0x1084D0)],
        title_text: rgb(0xFFFFFF),
        accent: rgb(0x000080),
        select: rgb(0x000080),
        select_text: rgb(0xFFFFFF),
    },
];

pub const DEFAULT_SKIN: &str = "studio-2007";

/// `a` moved `t` of the way to `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

fn luma(color: Color32) -> f32 {
    0.299 * f32::from(color.r()) + 0.587 * f32::from(color.g()) + 0.114 * f32::from(color.b())
}

impl Skin {
    /// A q0 theme in this chrome: its panels for faces, its accent for
    /// selection.
    pub fn from_theme(theme: &'static q0theme::BuiltinTheme) -> Self {
        let c = |value: q0theme::Rgb| Color32::from_rgb(value.r, value.g, value.b);
        let face = c(theme.panel);
        let accent = c(theme.accent);
        Self {
            id: theme.id,
            name: theme.name,
            face,
            window: c(theme.window),
            field: c(theme.deep_bg),
            text: c(theme.text),
            text_dim: c(theme.text_dim),
            border: c(theme.stroke_dark),
            title: [
                mix(face, Color32::WHITE, 0.16),
                mix(face, Color32::BLACK, 0.32),
            ],
            title_text: c(theme.text),
            accent,
            select: accent,
            select_text: if luma(accent) > 150.0 {
                Color32::from_gray(20)
            } else {
                Color32::WHITE
            },
        }
    }

    pub fn all() -> &'static [Skin] {
        static ALL: std::sync::OnceLock<Vec<Skin>> = std::sync::OnceLock::new();
        ALL.get_or_init(|| {
            RETRO
                .iter()
                .cloned()
                .chain(q0theme::BUILTIN_THEMES.iter().map(Skin::from_theme))
                .collect()
        })
    }

    pub fn by_id(id: &str) -> &'static Skin {
        let all = Self::all();
        all.iter()
            .find(|skin| skin.id == id)
            .or_else(|| all.iter().find(|skin| skin.id == DEFAULT_SKIN))
            .unwrap_or(&all[0])
    }

    pub fn is_dark(&self) -> bool {
        luma(self.face) < 110.0
    }

    /// The light edge of a raised bevel.
    pub fn light(&self) -> Color32 {
        mix(
            self.face,
            Color32::WHITE,
            if self.is_dark() { 0.18 } else { 0.75 },
        )
    }

    /// The dark edge of a raised bevel.
    pub fn shadow(&self) -> Color32 {
        mix(
            self.face,
            Color32::BLACK,
            if self.is_dark() { 0.45 } else { 0.32 },
        )
    }

    /// Pushes the skin into egui, so every stock widget follows it.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut visuals = if self.is_dark() {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.override_text_color = Some(self.text);
        visuals.panel_fill = self.face;
        visuals.window_fill = self.window;
        visuals.extreme_bg_color = self.field;
        visuals.faint_bg_color = mix(self.field, self.face, 0.45);
        visuals.code_bg_color = self.field;
        visuals.window_stroke = Stroke::new(1.0_f32, self.border);
        visuals.window_rounding = Rounding::same(3.0);
        visuals.menu_rounding = Rounding::same(2.0);
        let hard_shadow = egui::epaint::Shadow {
            offset: vec2(3.0, 3.0),
            blur: 5.0,
            spread: 0.0,
            color: Color32::from_black_alpha(if self.is_dark() { 150 } else { 70 }),
        };
        visuals.window_shadow = hard_shadow;
        visuals.popup_shadow = hard_shadow;
        visuals.hyperlink_color = self.accent;
        visuals.selection.bg_fill = self.select;
        visuals.selection.stroke = Stroke::new(1.0_f32, self.select_text);
        visuals.warn_fg_color = mix(self.text, Color32::from_rgb(220, 150, 0), 0.8);
        visuals.error_fg_color = mix(self.text, Color32::from_rgb(210, 40, 40), 0.85);
        visuals.striped = true;

        let button = mix(self.face, self.light(), 0.35);
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
        ] {
            widget.bg_fill = button;
            widget.weak_bg_fill = button;
            widget.bg_stroke = Stroke::new(1.0_f32, self.border);
            widget.fg_stroke = Stroke::new(1.0_f32, self.text);
            widget.rounding = Rounding::same(2.0);
            widget.expansion = 0.0;
        }
        visuals.widgets.noninteractive.bg_fill = self.face;
        visuals.widgets.noninteractive.weak_bg_fill = self.face;
        visuals.widgets.noninteractive.bg_stroke =
            Stroke::new(1.0_f32, mix(self.border, self.face, 0.3));
        let hover = mix(button, self.accent, 0.16);
        visuals.widgets.hovered.bg_fill = hover;
        visuals.widgets.hovered.weak_bg_fill = hover;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, self.accent);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, self.text);
        visuals.widgets.hovered.rounding = Rounding::same(2.0);
        visuals.widgets.hovered.expansion = 0.0;
        let pressed = mix(button, self.accent, 0.3);
        visuals.widgets.active.bg_fill = pressed;
        visuals.widgets.active.weak_bg_fill = pressed;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, self.accent);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, self.text);
        visuals.widgets.active.rounding = Rounding::same(2.0);
        visuals.widgets.active.expansion = 0.0;
        visuals.widgets.open = visuals.widgets.active;

        let mut style = (*ctx.style()).clone();
        style.visuals = visuals;
        style.spacing.item_spacing = vec2(5.0, 4.0);
        style.spacing.button_padding = vec2(7.0, 3.0);
        style.spacing.menu_margin = egui::Margin::same(3.0);
        style.spacing.window_margin = egui::Margin::same(8.0);
        style.spacing.indent = 16.0;
        style.spacing.interact_size.y = 22.0;
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 12.0;
        style.spacing.scroll.bar_inner_margin = 1.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
        ctx.set_style(style);
    }
}

// ------------------------------------------------------------- painting

/// A rectangle shaded from `top` to `bottom`.
pub fn gradient(painter: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(mesh);
}

/// A one-pixel bevel round `rect`: raised has its light edge at the top
/// left, sunken at the bottom right.
pub fn bevel(painter: &Painter, rect: Rect, light: Color32, dark: Color32, raised: bool) {
    let (top_left, bottom_right) = if raised { (light, dark) } else { (dark, light) };
    let r = rect.shrink(0.5);
    painter.line_segment(
        [r.left_bottom(), r.left_top()],
        Stroke::new(1.0_f32, top_left),
    );
    painter.line_segment(
        [r.left_top(), r.right_top()],
        Stroke::new(1.0_f32, top_left),
    );
    painter.line_segment(
        [r.right_top(), r.right_bottom()],
        Stroke::new(1.0_f32, bottom_right),
    );
    painter.line_segment(
        [r.right_bottom(), r.left_bottom()],
        Stroke::new(1.0_f32, bottom_right),
    );
}

/// A face shaded the way buttons and bars were: light at the top.
pub fn face_gradient(painter: &Painter, rect: Rect, skin: &Skin) {
    let (top, bottom) = if skin.is_dark() {
        (
            mix(skin.face, Color32::WHITE, 0.08),
            mix(skin.face, Color32::BLACK, 0.18),
        )
    } else {
        (
            mix(skin.face, Color32::WHITE, 0.55),
            mix(skin.face, Color32::BLACK, 0.06),
        )
    };
    gradient(painter, rect, top, bottom);
}

/// A gradient title bar the width of `ui`, with `title` in it; returns the
/// bar's rect so callers may add to its right end.
pub fn title_bar(ui: &mut Ui, skin: &Skin, icon: Option<UiIcon>, title: &str) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let painter = ui.painter();
    gradient(painter, rect, skin.title[0], skin.title[1]);
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, mix(skin.title[1], Color32::BLACK, 0.3)),
    );
    let mut x = rect.left() + 6.0;
    if let Some(icon) = icon {
        draw_icon(
            painter,
            Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 14.0)),
            icon,
            true,
        );
        x += 19.0;
    }
    painter.text(
        pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.0),
        skin.title_text,
    );
    rect
}

/// A toolbar button: an icon over a small label, flat until the pointer
/// reaches it, then raised, and pushed in while pressed.
pub fn tool_button(
    ui: &mut Ui,
    skin: &Skin,
    icon: UiIcon,
    label: &str,
    enabled: bool,
    tooltip: &str,
) -> Response {
    let font = egui::FontId::proportional(11.5);
    let text_width = ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(label.to_string(), font.clone(), skin.text)
            .size()
            .x
    });
    let size = vec2((text_width + 14.0).max(44.0), 46.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let response = response.on_hover_text(tooltip);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let hovered = enabled && response.hovered();
        let pressed = enabled && response.is_pointer_button_down_on();
        if hovered || pressed {
            let fill = if pressed {
                mix(skin.face, skin.accent, 0.28)
            } else {
                mix(skin.face, skin.accent, 0.12)
            };
            gradient(painter, rect, mix(fill, Color32::WHITE, 0.25), fill);
            bevel(painter, rect, skin.light(), skin.shadow(), !pressed);
        }
        let nudge = if pressed {
            vec2(1.0, 1.0)
        } else {
            vec2(0.0, 0.0)
        };
        draw_icon(
            painter,
            Rect::from_center_size(
                pos2(rect.center().x, rect.top() + 15.0) + nudge,
                vec2(22.0, 22.0),
            ),
            icon,
            enabled,
        );
        painter.text(
            pos2(rect.center().x, rect.bottom() - 9.0) + nudge,
            egui::Align2::CENTER_CENTER,
            label,
            font,
            if enabled {
                skin.text
            } else {
                mix(skin.text_dim, skin.face, 0.4)
            },
        );
    }
    response
}

/// A small square button with an icon, for the preview's own toolbar.
pub fn small_button(
    ui: &mut Ui,
    skin: &Skin,
    icon: UiIcon,
    on: bool,
    enabled: bool,
    tooltip: &str,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(24.0, 22.0), sense);
    let response = response.on_hover_text(tooltip);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let hovered = enabled && response.hovered();
        let pressed = (enabled && response.is_pointer_button_down_on()) || on;
        if hovered || pressed {
            let fill = mix(skin.face, skin.accent, if pressed { 0.26 } else { 0.12 });
            painter.rect_filled(rect, 0.0, fill);
            bevel(painter, rect, skin.light(), skin.shadow(), !pressed);
        }
        draw_icon(
            painter,
            Rect::from_center_size(rect.center(), vec2(15.0, 15.0)),
            icon,
            enabled,
        );
    }
    response
}

/// A groove between toolbar groups.
pub fn separator(ui: &mut Ui, skin: &Skin, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(8.0, height), Sense::hover());
    let x = rect.center().x;
    let painter = ui.painter();
    painter.line_segment(
        [pos2(x, rect.top() + 4.0), pos2(x, rect.bottom() - 4.0)],
        Stroke::new(1.0_f32, skin.shadow()),
    );
    painter.line_segment(
        [
            pos2(x + 1.0, rect.top() + 4.0),
            pos2(x + 1.0, rect.bottom() - 4.0),
        ],
        Stroke::new(1.0_f32, skin.light()),
    );
}

/// Classic tabs. Returns the index picked, if one was.
pub fn tabs(ui: &mut Ui, skin: &Skin, labels: &[&str], current: usize) -> Option<usize> {
    let mut picked = None;
    let height = 24.0;
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter().clone();
    let font = egui::FontId::proportional(13.0);
    let mut x = strip.left() + 4.0;
    let mut selected_rect = None;
    for (index, label) in labels.iter().enumerate() {
        let width = ui.fonts(|fonts| {
            fonts
                .layout_no_wrap(label.to_string(), font.clone(), skin.text)
                .size()
                .x
        }) + 22.0;
        let selected = index == current;
        let rect = if selected {
            Rect::from_min_max(
                pos2(x - 2.0, strip.top()),
                pos2(x + width + 2.0, strip.bottom() + 1.0),
            )
        } else {
            Rect::from_min_max(pos2(x, strip.top() + 3.0), pos2(x + width, strip.bottom()))
        };
        let response = ui.interact(rect, ui.id().with(("tab", index)), Sense::click());
        let fill = if selected {
            skin.window
        } else if response.hovered() {
            mix(skin.face, skin.accent, 0.12)
        } else {
            mix(
                skin.face,
                Color32::BLACK,
                if skin.is_dark() { 0.12 } else { 0.05 },
            )
        };
        let rounding = Rounding {
            nw: 3.0,
            ne: 3.0,
            sw: 0.0,
            se: 0.0,
        };
        painter.rect_filled(rect, rounding, fill);
        let edge = Stroke::new(1.0_f32, skin.border);
        let r = rect.shrink(0.5);
        painter.add(egui::Shape::line(
            vec![
                r.left_bottom(),
                pos2(r.left(), r.top() + 2.0),
                pos2(r.left() + 2.0, r.top()),
                pos2(r.right() - 2.0, r.top()),
                pos2(r.right(), r.top() + 2.0),
                r.right_bottom(),
            ],
            edge,
        ));
        if selected {
            painter.line_segment(
                [
                    pos2(r.left() + 2.0, r.top() + 1.0),
                    pos2(r.right() - 2.0, r.top() + 1.0),
                ],
                Stroke::new(2.0_f32, skin.accent),
            );
            selected_rect = Some(rect);
        }
        painter.text(
            rect.center() + vec2(0.0, if selected { 0.0 } else { 1.0 }),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if selected { skin.text } else { skin.text_dim },
        );
        if response.clicked() {
            picked = Some(index);
        }
        x += width + 2.0;
    }
    // The line the tabs stand on, open under the selected one.
    let base = strip.bottom() - 0.5;
    let line = Stroke::new(1.0_f32, skin.border);
    match selected_rect {
        Some(selected) => {
            painter.line_segment(
                [pos2(strip.left(), base), pos2(selected.left(), base)],
                line,
            );
            painter.line_segment(
                [pos2(selected.right(), base), pos2(strip.right(), base)],
                line,
            );
        }
        None => {
            painter.line_segment([pos2(strip.left(), base), pos2(strip.right(), base)], line);
        }
    }
    picked
}

/// A sunken field in the status bar.
pub fn status_field(ui: &mut Ui, skin: &Skin, text: &str, width: f32) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
    let painter = ui.painter();
    bevel(painter, rect, skin.light(), skin.shadow(), false);
    let galley = ui.fonts(|fonts| {
        fonts.layout(
            text.to_string(),
            egui::FontId::proportional(11.5),
            skin.text,
            f32::INFINITY,
        )
    });
    let clipped = painter.with_clip_rect(rect.shrink(2.0));
    clipped.galley(
        pos2(rect.left() + 5.0, rect.center().y - galley.size().y * 0.5),
        galley.clone(),
        skin.text,
    );
    if galley.size().x > width - 10.0 {
        response.on_hover_text(text);
    }
}

/// The classic tree expander: a small box with a plus or a minus.
pub fn expander(painter: &Painter, center: egui::Pos2, open: bool, skin: &Skin) {
    let rect = Rect::from_center_size(center, vec2(9.0, 9.0));
    painter.rect(
        rect,
        0.0,
        skin.field,
        Stroke::new(1.0_f32, mix(skin.border, skin.field, 0.2)),
    );
    let ink = Stroke::new(1.0_f32, skin.text);
    painter.line_segment(
        [
            pos2(rect.left() + 2.0, center.y),
            pos2(rect.right() - 2.0, center.y),
        ],
        ink,
    );
    if !open {
        painter.line_segment(
            [
                pos2(center.x, rect.top() + 2.0),
                pos2(center.x, rect.bottom() - 2.0),
            ],
            ink,
        );
    }
}

/// A dotted line, as tree connectors were drawn.
pub fn dotted(painter: &Painter, from: egui::Pos2, to: egui::Pos2, color: Color32) {
    let length = from.distance(to);
    let steps = (length / 2.0) as usize;
    let direction = (to - from) / length.max(0.001);
    for step in 0..=steps {
        let p = from + direction * (step as f32 * 2.0);
        painter.rect_filled(Rect::from_min_size(p, vec2(1.0, 1.0)), 0.0, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_skin_is_listed_once_and_the_default_is_a_retro_one() {
        let all = Skin::all();
        let mut ids: Vec<_> = all.iter().map(|skin| skin.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        assert_eq!(Skin::by_id("nonsense").id, DEFAULT_SKIN);
        assert_eq!(all.len(), RETRO.len() + q0theme::BUILTIN_THEMES.len());
    }

    #[test]
    fn q0_themes_keep_their_palette_in_the_chrome() {
        let ctx = egui::Context::default();
        for theme in q0theme::BUILTIN_THEMES {
            let skin = Skin::from_theme(theme);
            skin.apply(&ctx);
            let style = ctx.style();
            let c = |value: q0theme::Rgb| Color32::from_rgb(value.r, value.g, value.b);
            assert_eq!(
                style.visuals.panel_fill,
                c(theme.panel),
                "{} panel",
                theme.id
            );
            assert_eq!(
                style.visuals.window_fill,
                c(theme.window),
                "{} window",
                theme.id
            );
            assert_eq!(
                style.visuals.extreme_bg_color,
                c(theme.deep_bg),
                "{} field",
                theme.id
            );
            assert_eq!(
                style.visuals.selection.bg_fill,
                c(theme.accent),
                "{} accent",
                theme.id
            );
        }
    }

    #[test]
    fn bevels_read_on_light_and_dark_skins() {
        for skin in Skin::all() {
            assert!(luma(skin.light()) > luma(skin.shadow()), "{}", skin.id);
        }
    }
}
