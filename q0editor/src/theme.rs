use egui::{style::Selection, vec2, Color32, Context, Rounding, Stroke, Visuals};

const PANEL_FILL: Color32 = Color32::from_rgb(0x3C, 0x3C, 0x3C);
const WINDOW_FILL: Color32 = Color32::from_rgb(0x2F, 0x2F, 0x2F);
const STROKE_DARK: Color32 = Color32::from_rgb(0x20, 0x20, 0x20);
const STROKE_LIGHT: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
const BG_DEEP: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
const BG_MEDIUM: Color32 = Color32::from_rgb(0x46, 0x46, 0x46);
const BG_HOVER: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
const BG_ACTIVE: Color32 = Color32::from_rgb(0x6B, 0x6B, 0x6B);
const ACCENT: Color32 = Color32::from_rgb(0xC8, 0x10, 0x2E);
const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xE6, 0xE6, 0xE6);
const TEXT_DIM: Color32 = Color32::from_rgb(0x9F, 0x9F, 0x9F);

pub const STAGE_BG: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const STAGE_BORDER: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
pub const STAGE_SHADOW: Color32 = Color32::from_black_alpha(110);
pub const TIMELINE_FRAME_LINE: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
pub const TIMELINE_FRAME_FILL: Color32 = Color32::from_rgb(0x46, 0x46, 0x46);
pub const TIMELINE_FRAME_FIVE: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);
pub const TIMELINE_PLAYHEAD: Color32 = Color32::from_rgb(0xCC, 0x33, 0x33);
pub const TIMELINE_KEYFRAME: Color32 = Color32::from_rgb(0x10, 0x10, 0x10);
/// Static extension (Tween::None) — fills the cells of held frames with a
/// near-white grey so the active span pops against the dark row background,
/// matching Flash CS3's "frame extension" look.
pub const TIMELINE_EXTENSION: Color32 = Color32::from_rgb(0xCF, 0xCF, 0xCF);
/// Same as extension but slightly tinted, used for the *current* layer so
/// the user can tell which row drawing/keyframe ops are about to land in.
pub const TIMELINE_EXTENSION_ACTIVE: Color32 = Color32::from_rgb(0xE0, 0xE6, 0xEC);
/// Tween (Tween::Linear) — pale Flash-violet fill behind the cells, with
/// dark arrow drawn over it.
pub const TIMELINE_TWEEN: Color32 = Color32::from_rgb(0xC8, 0xB8, 0xE6);
pub const TIMELINE_TWEEN_ARROW: Color32 = Color32::from_rgb(0x3A, 0x1A, 0x55);
/// Empty area beyond a q0rg's frame_count — darker than the row itself to
/// signal "no frames defined here".
pub const TIMELINE_EMPTY_BEYOND: Color32 = Color32::from_rgb(0x1F, 0x1F, 0x1F);
/// Frame cell within `frame_count` that has no placement on this layer at
/// this frame. Mid-grey so it reads as "blank canvas, you could draw here"
/// vs. the much darker beyond-frame_count cells.
pub const TIMELINE_EMPTY_INSIDE: Color32 = Color32::from_rgb(0x40, 0x40, 0x40);
/// Same but for the *current* layer — slightly brighter so the row drawing
/// will land in is unmistakable.
pub const TIMELINE_EMPTY_INSIDE_ACTIVE: Color32 = Color32::from_rgb(0x4A, 0x4A, 0x4A);
/// Every 5th cell beyond `frame_count` is brightened a touch to read as a
/// chequer pattern — same idea as the every-5th frame number in the header.
pub const TIMELINE_BEYOND_FIVE: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
/// Onion-skin tint applied to past-frame renders. Lower alpha + slight blue
/// tint so the user can tell historical layers from the live current frame.
pub const ONION_PAST: Color32 = Color32::from_rgba_premultiplied(0x40, 0x60, 0x80, 0x70);
/// Onion-skin tint applied to upcoming-frame renders.
pub const ONION_FUTURE: Color32 = Color32::from_rgba_premultiplied(0x80, 0x60, 0x40, 0x70);
pub const ACCENT_COLOR: Color32 = ACCENT;

pub fn install(ctx: &Context) {
    let mut style = (*ctx.style()).clone();
    let mut visuals = Visuals::dark();

    visuals.panel_fill = PANEL_FILL;
    visuals.window_fill = WINDOW_FILL;
    visuals.window_stroke = Stroke::new(1.0_f32, STROKE_DARK);
    visuals.menu_rounding = Rounding::same(2.0);
    visuals.window_rounding = Rounding::same(2.0);
    visuals.extreme_bg_color = BG_DEEP;
    visuals.faint_bg_color = BG_MEDIUM;
    visuals.code_bg_color = BG_DEEP;

    visuals.widgets.noninteractive.bg_fill = PANEL_FILL;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL_FILL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, STROKE_DARK);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.noninteractive.rounding = Rounding::same(2.0);

    visuals.widgets.inactive.bg_fill = BG_MEDIUM;
    visuals.widgets.inactive.weak_bg_fill = BG_MEDIUM;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, STROKE_DARK);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.inactive.rounding = Rounding::same(2.0);

    visuals.widgets.hovered.bg_fill = BG_HOVER;
    visuals.widgets.hovered.weak_bg_fill = BG_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, STROKE_LIGHT);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.hovered.rounding = Rounding::same(2.0);

    visuals.widgets.active.bg_fill = BG_ACTIVE;
    visuals.widgets.active.weak_bg_fill = BG_ACTIVE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(2.0);

    visuals.widgets.open.bg_fill = BG_ACTIVE;
    visuals.widgets.open.weak_bg_fill = BG_ACTIVE;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.open.rounding = Rounding::same(2.0);

    visuals.selection = Selection {
        bg_fill: ACCENT,
        stroke: Stroke::new(1.0_f32, Color32::WHITE),
    };
    visuals.hyperlink_color = ACCENT;
    visuals.warn_fg_color = Color32::from_rgb(0xFF, 0xC8, 0x40);
    visuals.error_fg_color = Color32::from_rgb(0xFF, 0x60, 0x60);

    style.visuals = visuals;
    style.spacing.item_spacing = vec2(4.0, 3.0);
    style.spacing.button_padding = vec2(6.0, 3.0);
    style.spacing.menu_margin = egui::Margin::same(4.0);
    style.spacing.window_margin = egui::Margin::same(2.0);
    style.spacing.indent = 14.0;
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.bar_inner_margin = 1.0;

    ctx.set_style(style);
}

pub fn text_dim() -> Color32 {
    TEXT_DIM
}
