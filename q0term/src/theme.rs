use egui::{Color32, FontFamily, FontId, Rounding, Stroke, Visuals};

pub const BG: Color32 = Color32::from_rgb(8, 8, 8);
pub const RED: Color32 = Color32::from_rgb(220, 32, 32);
pub const RED_DIM: Color32 = Color32::from_rgb(140, 28, 28);
pub const RED_BRIGHT: Color32 = Color32::from_rgb(255, 72, 72);
pub const RED_ERROR: Color32 = Color32::from_rgb(255, 96, 96);
pub const PROMPT_USER: Color32 = Color32::from_rgb(200, 200, 200);
pub const PROMPT_HOST: Color32 = Color32::from_rgb(255, 72, 72);
pub const PROMPT_PATH: Color32 = Color32::from_rgb(200, 64, 64);
pub const DIM: Color32 = Color32::from_rgb(100, 36, 36);

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = Visuals {
        dark_mode: true,
        override_text_color: Some(RED),
        window_fill: BG,
        panel_fill: BG,
        extreme_bg_color: BG,
        faint_bg_color: BG,
        code_bg_color: BG,
        warn_fg_color: RED_ERROR,
        error_fg_color: RED_ERROR,
        window_rounding: Rounding::same(0.0),
        window_stroke: Stroke::new(1.0_f32, RED_DIM),
        selection: egui::style::Selection {
            bg_fill: Color32::from_rgba_unmultiplied(220, 32, 32, 48),
            stroke: Stroke::new(1.0_f32, RED_DIM),
        },
        widgets: egui::style::Widgets {
            inactive: egui::style::WidgetVisuals {
                bg_fill: BG,
                weak_bg_fill: BG,
                bg_stroke: Stroke::NONE,
                fg_stroke: Stroke::new(1.0_f32, RED),
                rounding: Rounding::same(0.0),
                expansion: 0.0,
            },
            hovered: egui::style::WidgetVisuals {
                bg_fill: BG,
                weak_bg_fill: BG,
                bg_stroke: Stroke::NONE,
                fg_stroke: Stroke::new(1.0_f32, RED_BRIGHT),
                rounding: Rounding::same(0.0),
                expansion: 0.0,
            },
            active: egui::style::WidgetVisuals {
                bg_fill: BG,
                weak_bg_fill: BG,
                bg_stroke: Stroke::NONE,
                fg_stroke: Stroke::new(1.0_f32, RED_BRIGHT),
                rounding: Rounding::same(0.0),
                expansion: 0.0,
            },
            open: egui::style::WidgetVisuals {
                bg_fill: BG,
                weak_bg_fill: BG,
                bg_stroke: Stroke::NONE,
                fg_stroke: Stroke::new(1.0_f32, RED_BRIGHT),
                rounding: Rounding::same(0.0),
                expansion: 0.0,
            },
            noninteractive: egui::style::WidgetVisuals {
                bg_fill: BG,
                weak_bg_fill: BG,
                bg_stroke: Stroke::NONE,
                fg_stroke: Stroke::new(1.0_f32, RED),
                rounding: Rounding::same(0.0),
                expansion: 0.0,
            },
        },
        ..Visuals::dark()
    };
    style.spacing.item_spacing = egui::vec2(0.0, 2.0);
    style.spacing.indent = 0.0;
    ctx.set_style(style);
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
