//! Q0Build's look: desktop software of around 2007, glossy title bands,
//! bevelled buttons, etched group boxes and a progress bar with a sweep of
//! light, in any of its own skins or any shared q0 theme (as CanRipper
//! offers them).

use std::cell::Cell;

use egui::{
    pos2, vec2, Align2, Color32, FontId, Mesh, Painter, Rect, Response, Rounding, Sense, Stroke, Ui,
};

const fn rgb(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// The colours a skin paints with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Skin {
    pub id: &'static str,
    pub name: &'static str,
    /// Side columns, bars and the console's frame.
    pub panel: Color32,
    /// The page in the middle.
    pub sheet: Color32,
    /// Text fields and the console.
    pub field: Color32,
    /// Borders and the dark side of a bevel.
    pub edge: Color32,
    pub text: Color32,
    pub dim: Color32,
    /// Captions, links and focus.
    pub accent: Color32,
    /// The title band, top to bottom.
    pub title: [Color32; 2],
    pub title_text: Color32,
    /// Plain buttons, top to bottom.
    pub button: [Color32; 2],
    pub button_text: Color32,
    /// The chosen page and selections, top to bottom.
    pub select: [Color32; 2],
}

const BLACK_GLASS: Skin = Skin {
    id: "black-glass",
    name: "Black Glass",
    panel: rgb(0x23272E),
    sheet: rgb(0x2A2F37),
    field: rgb(0x14161A),
    edge: rgb(0x0B0C0F),
    text: rgb(0xE8EBF0),
    dim: rgb(0x97A1AE),
    accent: rgb(0x58C4FF),
    title: [rgb(0x4C7DB5), rgb(0x16304F)],
    title_text: rgb(0xFFFFFF),
    button: [rgb(0x4A525E), rgb(0x2B3038)],
    button_text: rgb(0xFFFFFF),
    select: [rgb(0x3E6EA3), rgb(0x214B78)],
};

/// Q0Build's own skins, before the shared q0 themes.
const OWN: &[Skin] = &[
    BLACK_GLASS,
    Skin {
        id: "studio-2007",
        name: "Studio 2007",
        panel: rgb(0xE3E3E3),
        sheet: rgb(0xF0F0F0),
        field: rgb(0xFFFFFF),
        edge: rgb(0x8C8C8C),
        text: rgb(0x1A1A1A),
        dim: rgb(0x666666),
        accent: rgb(0x2E6FBF),
        title: [rgb(0x6E6E6E), rgb(0x383838)],
        title_text: rgb(0xFFFFFF),
        button: [rgb(0xFDFDFD), rgb(0xD6D6D6)],
        button_text: rgb(0x1A1A1A),
        select: [rgb(0x6FA3DD), rgb(0x3A73B5)],
    },
    Skin {
        id: "aqua-graphite",
        name: "Aqua Graphite",
        panel: rgb(0xD9DDE3),
        sheet: rgb(0xEDEFF2),
        field: rgb(0xFFFFFF),
        edge: rgb(0x8E959E),
        text: rgb(0x1C1F24),
        dim: rgb(0x5E6670),
        accent: rgb(0x2F7BD1),
        title: [rgb(0xC9D0D8), rgb(0x8A949F)],
        title_text: rgb(0x1C1F24),
        button: [rgb(0xFFFFFF), rgb(0xC8D0D9)],
        button_text: rgb(0x1C1F24),
        select: [rgb(0x63A6EC), rgb(0x2C6CC0)],
    },
    Skin {
        id: "luna",
        name: "Luna",
        panel: rgb(0xECE9D8),
        sheet: rgb(0xF7F6EF),
        field: rgb(0xFFFFFF),
        edge: rgb(0x7F9DB9),
        text: rgb(0x000000),
        dim: rgb(0x6B6B6B),
        accent: rgb(0x2457C5),
        title: [rgb(0x3D86E8), rgb(0x0A3FA8)],
        title_text: rgb(0xFFFFFF),
        button: [rgb(0xFFFFFF), rgb(0xD8D4C4)],
        button_text: rgb(0x000000),
        select: [rgb(0x3D8AF7), rgb(0x1C5CD1)],
    },
    Skin {
        id: "zune-night",
        name: "Zune Night",
        panel: rgb(0x1C1C1C),
        sheet: rgb(0x242424),
        field: rgb(0x101010),
        edge: rgb(0x000000),
        text: rgb(0xF2F2F2),
        dim: rgb(0x9A9A9A),
        accent: rgb(0xFF8A33),
        title: [rgb(0x3E3E3E), rgb(0x121212)],
        title_text: rgb(0xFFFFFF),
        button: [rgb(0x454545), rgb(0x262626)],
        button_text: rgb(0xFFFFFF),
        select: [rgb(0xFF9440), rgb(0xC2550A)],
    },
];

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

pub fn luma(color: Color32) -> f32 {
    0.299 * f32::from(color.r()) + 0.587 * f32::from(color.g()) + 0.114 * f32::from(color.b())
}

/// Black or white, whichever reads on `fill`.
pub fn ink_on(fill: Color32) -> Color32 {
    if luma(fill) > 150.0 {
        Color32::from_gray(18)
    } else {
        Color32::WHITE
    }
}

/// Text on the selection gradient, read from its middle.
pub fn select_ink(skin: &Skin) -> Color32 {
    ink_on(mix(skin.select[0], skin.select[1], 0.5))
}

impl Skin {
    pub fn dark(&self) -> bool {
        luma(self.panel) < 128.0
    }

    /// A shared q0 theme in Q0Build's glass.
    fn from_theme(theme: &'static q0theme::BuiltinTheme) -> Self {
        let c = |value: q0theme::Rgb| Color32::from_rgb(value.r, value.g, value.b);
        let panel = c(theme.panel);
        let accent = c(theme.accent);
        Self {
            id: theme.id,
            name: theme.name,
            panel,
            sheet: c(theme.window),
            field: c(theme.deep_bg),
            edge: c(theme.stroke_dark),
            text: c(theme.text),
            dim: c(theme.text_dim),
            accent,
            title: [
                mix(panel, Color32::WHITE, 0.16),
                mix(panel, Color32::BLACK, 0.32),
            ],
            title_text: ink_on(mix(panel, Color32::BLACK, 0.1)),
            button: [
                mix(panel, Color32::WHITE, 0.14),
                mix(panel, Color32::BLACK, 0.12),
            ],
            button_text: c(theme.text),
            select: [accent, mix(accent, Color32::BLACK, 0.3)],
        }
    }

    pub fn all() -> &'static [Skin] {
        static ALL: std::sync::OnceLock<Vec<Skin>> = std::sync::OnceLock::new();
        ALL.get_or_init(|| {
            OWN.iter()
                .copied()
                .chain(q0theme::BUILTIN_THEMES.iter().map(Skin::from_theme))
                .collect()
        })
    }

    pub fn by_id(id: &str) -> Skin {
        Self::all()
            .iter()
            .copied()
            .find(|skin| skin.id == id)
            .unwrap_or(BLACK_GLASS)
    }

    /// One of Q0Build's own, not a shared q0 theme.
    pub fn is_own(&self) -> bool {
        OWN.iter().any(|skin| skin.id == self.id)
    }
}

thread_local! {
    static CURRENT: Cell<Skin> = const { Cell::new(BLACK_GLASS) };
}

/// The skin everything paints with now.
pub fn skin() -> Skin {
    CURRENT.with(Cell::get)
}

/// Paints with `skin` from now on.
pub fn use_skin(ctx: &egui::Context, skin: Skin) {
    CURRENT.with(|current| current.set(skin));
    install(ctx);
}

/// Done, ready, ok.
pub fn go() -> Color32 {
    if skin().dark() {
        rgb(0x7ED957)
    } else {
        rgb(0x2E8B22)
    }
}

/// Worth a look.
pub fn warn() -> Color32 {
    if skin().dark() {
        rgb(0xFFB547)
    } else {
        rgb(0xB36200)
    }
}

/// Failed or missing.
pub fn bad() -> Color32 {
    if skin().dark() {
        rgb(0xFF6161)
    } else {
        rgb(0xC62828)
    }
}

/// The egui side of the skin: fields, sliders and popups in the same glass.
pub fn install(ctx: &egui::Context) {
    let skin = skin();
    let mut style = (*ctx.style()).clone();
    let visuals = &mut style.visuals;
    *visuals = if skin.dark() {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = skin.panel;
    visuals.window_fill = skin.sheet;
    visuals.extreme_bg_color = skin.field;
    visuals.faint_bg_color = mix(skin.sheet, skin.text, 0.04);
    visuals.code_bg_color = skin.field;
    visuals.override_text_color = Some(skin.text);
    visuals.hyperlink_color = skin.accent;
    visuals.selection.bg_fill = skin.select[1];
    visuals.selection.stroke = Stroke::new(1.0_f32, select_ink(&skin));
    visuals.window_rounding = Rounding::same(3.0);
    visuals.window_stroke = Stroke::new(1.0_f32, skin.edge);
    let hovered = mix(skin.button[0], skin.accent, 0.12);
    for (widget, fill) in [
        (&mut visuals.widgets.noninteractive, skin.sheet),
        (
            &mut visuals.widgets.inactive,
            mix(skin.button[0], skin.button[1], 0.6),
        ),
        (&mut visuals.widgets.hovered, hovered),
        (&mut visuals.widgets.active, skin.select[1]),
        (
            &mut visuals.widgets.open,
            mix(skin.button[0], skin.button[1], 0.6),
        ),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.rounding = Rounding::same(2.0);
        widget.bg_stroke = Stroke::new(1.0_f32, skin.edge);
        widget.fg_stroke = Stroke::new(1.0_f32, skin.text);
    }
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, skin.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, select_ink(&skin));
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(8.0, 3.0);
    style.spacing.interact_size.y = 22.0;
    ctx.set_style(style);
}

/// Top colour to bottom colour across `rect`.
pub fn vgradient(painter: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// The glass: a soft white sheen over the upper half.
pub fn gloss(painter: &Painter, rect: Rect, strength: f32) {
    let upper = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.center().y));
    vgradient(
        painter,
        upper.shrink2(vec2(1.0, 0.0)),
        Color32::from_white_alpha((70.0 * strength) as u8),
        Color32::from_white_alpha((14.0 * strength) as u8),
    );
}

/// The band across the top: name, what it is, and the profile.
pub fn title_band(ui: &mut Ui, profile: &str, dirty: bool) {
    let skin = skin();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::hover());
    let painter = ui.painter_at(rect);
    vgradient(&painter, rect, skin.title[0], skin.title[1]);
    gloss(&painter, rect, 0.9);
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, skin.edge),
    );
    let mark = Rect::from_center_size(pos2(rect.left() + 34.0, rect.center().y), vec2(36.0, 36.0));
    glyph(&painter, mark, Glyph::Package, skin.title_text);
    let ink = skin.title_text;
    let soft = Color32::from_rgba_unmultiplied(ink.r(), ink.g(), ink.b(), 185);
    painter.text(
        pos2(rect.left() + 62.0, rect.top() + 9.0),
        Align2::LEFT_TOP,
        "Q0Build",
        FontId::proportional(25.0),
        ink,
    );
    painter.text(
        pos2(rect.left() + 64.0, rect.top() + 38.0),
        Align2::LEFT_TOP,
        "QIR · q0s movies into apps of their own",
        FontId::proportional(12.0),
        soft,
    );
    let label = if dirty {
        format!("{profile} •")
    } else {
        profile.to_string()
    };
    painter.text(
        pos2(rect.right() - 16.0, rect.top() + 14.0),
        Align2::RIGHT_TOP,
        label,
        FontId::proportional(14.0),
        ink,
    );
    painter.text(
        pos2(rect.right() - 16.0, rect.top() + 36.0),
        Align2::RIGHT_TOP,
        format!("q0idmation {}", env!("CARGO_PKG_VERSION")),
        FontId::proportional(11.0),
        soft,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    /// The one button that builds.
    Go,
    Accent,
}

/// A bevelled glass button. `enabled` false greys it and ignores clicks.
pub fn button(ui: &mut Ui, text: &str, tone: Tone, enabled: bool) -> Response {
    let skin = skin();
    let size_pt = if tone == Tone::Go { 15.0 } else { 13.0 };
    let font = FontId::proportional(size_pt);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), skin.text);
    let height = if tone == Tone::Go { 30.0 } else { 24.0 };
    let size = vec2((galley.size().x + 26.0).max(64.0), height);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter_at(rect.expand(1.0));
    let (top, bottom, ink) = match tone {
        Tone::Plain => (skin.button[0], skin.button[1], skin.button_text),
        Tone::Go => (rgb(0x6CC24A), rgb(0x2E7A1F), Color32::WHITE),
        Tone::Accent => (skin.select[0], skin.select[1], select_ink(&skin)),
    };
    let (top, bottom, ink) = if !enabled {
        let flat = mix(skin.button[0], skin.panel, 0.5);
        (flat, mix(skin.button[1], skin.panel, 0.5), skin.dim)
    } else if response.is_pointer_button_down_on() {
        (bottom, top, ink)
    } else if response.hovered() {
        (
            mix(top, Color32::WHITE, 0.12),
            mix(bottom, Color32::WHITE, 0.08),
            ink,
        )
    } else {
        (top, bottom, ink)
    };
    painter.rect_filled(rect, 3.0, bottom);
    vgradient(&painter, rect.shrink(1.0), top, bottom);
    if enabled {
        gloss(
            &painter,
            rect.shrink(1.0),
            if skin.dark() { 0.8 } else { 0.5 },
        );
    }
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, skin.edge));
    painter.rect_stroke(
        rect.shrink(1.0),
        2.0,
        Stroke::new(
            1.0_f32,
            Color32::from_white_alpha(if enabled { 40 } else { 10 }),
        ),
    );
    if luma(ink) > 128.0 {
        painter.text(
            rect.center() + vec2(0.0, 1.0),
            Align2::CENTER_CENTER,
            text,
            font.clone(),
            Color32::from_black_alpha(140),
        );
    }
    painter.text(rect.center(), Align2::CENTER_CENTER, text, font, ink);
    if response.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// An etched group box with its title set into the line, as dialogs drew
/// them in 2007.
pub fn group<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let skin = skin();
    ui.add_space(10.0);
    let frame = egui::Frame::none()
        .inner_margin(egui::Margin {
            left: 12.0,
            right: 12.0,
            top: 14.0,
            bottom: 10.0,
        })
        .outer_margin(egui::Margin::symmetric(2.0, 0.0));
    let inner = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    let rect = inner.response.rect;
    let painter = ui.painter();
    let line = Rect::from_min_max(rect.min + vec2(2.0, 0.0), rect.max - vec2(2.0, 0.0));
    let etch = Color32::from_white_alpha(if skin.dark() { 18 } else { 150 });
    painter.rect_stroke(
        line.translate(vec2(1.0, 1.0)),
        3.0,
        Stroke::new(1.0_f32, etch),
    );
    painter.rect_stroke(
        line,
        3.0,
        Stroke::new(
            1.0_f32,
            mix(skin.edge, skin.sheet, if skin.dark() { 0.0 } else { 0.35 }),
        ),
    );
    let font = FontId::proportional(12.5);
    let galley = painter.layout_no_wrap(title.to_string(), font.clone(), skin.accent);
    let label = Rect::from_min_size(
        pos2(line.left() + 10.0, line.top() - galley.size().y * 0.5),
        galley.size() + vec2(8.0, 0.0),
    );
    painter.rect_filled(label, 0.0, skin.sheet);
    painter.text(
        label.left_center() + vec2(4.0, 0.0),
        Align2::LEFT_CENTER,
        title,
        font,
        skin.accent,
    );
    inner.inner
}

/// One labelled row: the label, the control, and a line on what it does.
pub fn row<R>(ui: &mut Ui, label: &str, hint: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let skin = skin();
    let out = ui
        .horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(170.0, 22.0), Sense::hover());
            ui.painter().text(
                rect.left_center(),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(13.0),
                skin.text,
            );
            add(ui)
        })
        .inner;
    if !hint.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(178.0);
            ui.add(
                egui::Label::new(egui::RichText::new(hint).size(11.0).color(skin.dim)).wrap(true),
            );
        });
    }
    ui.add_space(2.0);
    out
}

/// A left-hand page button.
pub fn nav_item(
    ui: &mut Ui,
    glyph_kind: Glyph,
    label: &str,
    selected: bool,
    badge: Option<(&str, Color32)>,
) -> Response {
    let skin = skin();
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let painter = ui.painter_at(rect);
    let ink = if selected {
        select_ink(&skin)
    } else {
        skin.text
    };
    if selected {
        vgradient(&painter, rect, skin.select[0], skin.select[1]);
        gloss(&painter, rect, 0.6);
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, skin.edge));
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, mix(skin.panel, skin.accent, 0.12));
    }
    let icon = Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(16.0, 16.0));
    glyph(
        &painter,
        icon,
        glyph_kind,
        if selected { ink } else { skin.accent },
    );
    painter.text(
        pos2(rect.left() + 38.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.5),
        ink,
    );
    if let Some((text, color)) = badge {
        let galley =
            painter.layout_no_wrap(text.to_string(), FontId::proportional(10.5), ink_on(color));
        let pill = Rect::from_center_size(
            pos2(
                rect.right() - 10.0 - galley.size().x * 0.5 - 4.0,
                rect.center().y,
            ),
            galley.size() + vec2(8.0, 2.0),
        );
        painter.rect_filled(pill, 7.0, color);
        painter.galley(pill.min + vec2(4.0, 1.0), galley, ink_on(color));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// A section caption inside the navigation column.
pub fn nav_caption(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(10.5)
            .color(skin().dim)
            .strong(),
    );
    ui.add_space(2.0);
}

/// A glossy progress bar; while `running` a band of light sweeps along it.
pub fn progress(ui: &mut Ui, fraction: f32, text: &str, running: bool, failed: bool) {
    let skin = skin();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect_filled(rect, 2.0, skin.field);
    let filled = Rect::from_min_size(
        rect.min,
        vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()),
    );
    if filled.width() > 1.0 {
        let (top, bottom) = if failed {
            (rgb(0xE05A5A), rgb(0x7A1E1E))
        } else {
            (rgb(0x8EE35F), rgb(0x2F7D1E))
        };
        vgradient(&painter, filled.shrink(1.0), top, bottom);
        gloss(&painter, filled.shrink(1.0), 1.0);
        if running {
            let t = ui.input(|input| input.time) as f32;
            let x = filled.left() + (t * 0.6).fract() * (filled.width() + 120.0) - 60.0;
            let sweep = Rect::from_min_max(
                pos2(x - 40.0, filled.top() + 1.0),
                pos2(x + 40.0, filled.bottom() - 1.0),
            )
            .intersect(filled.shrink(1.0));
            if sweep.is_positive() {
                let mut mesh = Mesh::default();
                let clear = Color32::from_white_alpha(0);
                let shine = Color32::from_white_alpha(90);
                let mid = sweep.center().x;
                for (x, color) in [(sweep.left(), clear), (mid, shine), (sweep.right(), clear)] {
                    mesh.colored_vertex(pos2(x, sweep.top()), color);
                    mesh.colored_vertex(pos2(x, sweep.bottom()), color);
                }
                for i in 0..2_u32 {
                    let a = i * 2;
                    mesh.add_triangle(a, a + 1, a + 2);
                    mesh.add_triangle(a + 1, a + 3, a + 2);
                }
                painter.add(mesh);
            }
            ui.ctx().request_repaint();
        }
    }
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, skin.edge));
    let ink = if fraction > 0.5 {
        Color32::WHITE
    } else {
        skin.text
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(12.0),
        ink,
    );
}

/// A file path that keeps to the room it has: cut short with an ellipsis,
/// the whole path on hover. Long paths used to widen the page and paint over
/// the preview column.
pub fn path_label(ui: &mut Ui, text: &str, size: f32) {
    ui.add(egui::Label::new(egui::RichText::new(text).monospace().size(size)).truncate(true))
        .on_hover_text(text);
}

/// A coloured status pill.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), FontId::proportional(11.0), ink_on(color));
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(12.0, 4.0), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, color);
    ui.painter()
        .galley(rect.min + vec2(6.0, 2.0), galley, ink_on(color));
}

/// A small square of a skin's colours, for the theme menu.
pub fn swatch(ui: &mut Ui, skin: &Skin) {
    let (rect, _) = ui.allocate_exact_size(vec2(28.0, 14.0), Sense::hover());
    let painter = ui.painter();
    let half = Rect::from_min_max(rect.min, pos2(rect.center().x, rect.max.y));
    vgradient(painter, half, skin.title[0], skin.title[1]);
    painter.rect_filled(
        Rect::from_min_max(pos2(rect.center().x, rect.min.y), rect.max),
        0.0,
        skin.sheet,
    );
    painter.rect_filled(
        Rect::from_center_size(pos2(rect.right() - 6.0, rect.center().y), vec2(6.0, 6.0)),
        1.0,
        skin.accent,
    );
    painter.rect_stroke(rect, 1.0, Stroke::new(1.0_f32, skin.edge));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Package,
    Film,
    Tag,
    Window,
    Gear,
    Disk,
    Targets,
    Folder,
    Lens,
    Check,
    Picture,
}

/// Small vector icons, drawn rather than shipped.
pub fn glyph(painter: &Painter, rect: Rect, kind: Glyph, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let stroke = Stroke::new((s / 11.0).max(1.2), color);
    let p = |x: f32, y: f32| pos2(c.x + x * s * 0.5, c.y + y * s * 0.5);
    match kind {
        Glyph::Package => {
            // An isometric box: the app in its crate.
            let top = [p(0.0, -0.9), p(0.85, -0.45), p(0.0, 0.0), p(-0.85, -0.45)];
            let left = [p(-0.85, -0.45), p(0.0, 0.0), p(0.0, 0.95), p(-0.85, 0.5)];
            let right = [p(0.0, 0.0), p(0.85, -0.45), p(0.85, 0.5), p(0.0, 0.95)];
            painter.add(egui::Shape::convex_polygon(
                top.to_vec(),
                skin().accent.gamma_multiply(0.95),
                Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                left.to_vec(),
                rgb(0x2D7FC1),
                Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                right.to_vec(),
                rgb(0x1B5A92),
                Stroke::NONE,
            ));
            for face in [&top[..], &left[..], &right[..]] {
                painter.add(egui::Shape::closed_line(
                    face.to_vec(),
                    Stroke::new(s / 18.0, color),
                ));
            }
            painter.line_segment(
                [p(-0.42, -0.67), p(0.42, -0.23)],
                Stroke::new(s / 16.0, go()),
            );
        }
        Glyph::Film => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(s * 0.9, s * 0.7)),
                1.5,
                stroke,
            );
            for x in [-0.32, 0.0, 0.32] {
                painter.rect_filled(
                    Rect::from_center_size(p(x, -0.5), vec2(s * 0.1, s * 0.1)),
                    0.0,
                    color,
                );
                painter.rect_filled(
                    Rect::from_center_size(p(x, 0.5), vec2(s * 0.1, s * 0.1)),
                    0.0,
                    color,
                );
            }
        }
        Glyph::Tag => {
            let points = vec![
                p(-0.85, -0.5),
                p(0.35, -0.5),
                p(0.85, 0.0),
                p(0.35, 0.5),
                p(-0.85, 0.5),
            ];
            painter.add(egui::Shape::closed_line(points, stroke));
            painter.circle_filled(p(-0.5, 0.0), s * 0.08, color);
        }
        Glyph::Window => {
            let frame = Rect::from_center_size(c, vec2(s * 0.95, s * 0.8));
            painter.rect_stroke(frame, 1.5, stroke);
            painter.rect_filled(
                Rect::from_min_max(frame.min, pos2(frame.max.x, frame.min.y + s * 0.2)),
                1.5,
                color,
            );
        }
        Glyph::Gear => {
            painter.circle_stroke(c, s * 0.25, stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let (sin, cos) = a.sin_cos();
                painter.line_segment(
                    [
                        pos2(c.x + cos * s * 0.32, c.y + sin * s * 0.32),
                        pos2(c.x + cos * s * 0.46, c.y + sin * s * 0.46),
                    ],
                    stroke,
                );
            }
        }
        Glyph::Disk => {
            let r = Rect::from_center_size(c, vec2(s * 0.85, s * 0.85));
            painter.rect_stroke(r, 1.5, stroke);
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(r.left() + s * 0.2, r.top()),
                    pos2(r.right() - s * 0.2, r.top() + s * 0.3),
                ),
                0.0,
                color,
            );
            painter.rect_stroke(
                Rect::from_min_max(
                    pos2(r.left() + s * 0.15, c.y + s * 0.05),
                    pos2(r.right() - s * 0.15, r.bottom() - s * 0.08),
                ),
                0.0,
                stroke,
            );
        }
        Glyph::Targets => {
            painter.circle_stroke(c, s * 0.45, stroke);
            painter.circle_stroke(c, s * 0.25, stroke);
            painter.circle_filled(c, s * 0.08, color);
        }
        Glyph::Folder => {
            let points = vec![
                p(-0.9, -0.55),
                p(-0.3, -0.55),
                p(-0.15, -0.35),
                p(0.9, -0.35),
                p(0.9, 0.65),
                p(-0.9, 0.65),
            ];
            painter.add(egui::Shape::closed_line(points, stroke));
        }
        Glyph::Lens => {
            painter.circle_stroke(p(-0.15, -0.15), s * 0.3, stroke);
            painter.line_segment(
                [p(0.28, 0.28), p(0.85, 0.85)],
                Stroke::new(stroke.width * 1.5, color),
            );
        }
        Glyph::Check => {
            painter.add(egui::Shape::line(
                vec![p(-0.7, 0.0), p(-0.2, 0.55), p(0.75, -0.6)],
                Stroke::new(stroke.width * 1.4, color),
            ));
        }
        Glyph::Picture => {
            let r = Rect::from_center_size(c, vec2(s * 0.95, s * 0.8));
            painter.rect_stroke(r, 1.5, stroke);
            painter.add(egui::Shape::line(
                vec![
                    p(-0.85, 0.6),
                    p(-0.25, -0.05),
                    p(0.15, 0.3),
                    p(0.45, 0.05),
                    p(0.85, 0.5),
                ],
                stroke,
            ));
            painter.circle_filled(p(0.45, -0.35), s * 0.09, color);
        }
    }
}
