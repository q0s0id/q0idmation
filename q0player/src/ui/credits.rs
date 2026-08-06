use super::theme::Theme;

pub fn show(ui: &mut egui::Ui, theme: &Theme, elapsed_seconds: f32) {
    q0credits::show(
        ui,
        &q0credits::Palette {
            accent: theme.accent.to_color32(),
            panel: theme.panel.to_color32(),
            window: theme.window.to_color32(),
            deep_bg: theme.deep_bg.to_color32(),
            text: theme.text.to_color32(),
            text_dim: theme.text_dim.to_color32(),
        },
        elapsed_seconds,
    );
}
