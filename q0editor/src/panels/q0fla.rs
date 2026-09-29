use egui::{Context, RichText};

use crate::app::{Action, EditorApp};

pub fn render(app: &mut EditorApp, ctx: &Context) {
    if !app.q0fla_open {
        return;
    }

    let mut open = app.q0fla_open;
    egui::Window::new("q0fla")
        .open(&mut open)
        .resizable(false)
        .default_width(390.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("Adobe Animate -> q1s").strong());
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Open imports a FLA/XFL into memory as an unsaved dirty q1s project. Convert writes a protected q1s copy without touching the source file.",
                )
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
            );
            ui.add_space(12.0);
            if ui.button("Open FLA/XFL as project...").clicked() {
                app.queue(Action::ImportFlaProject);
            }
            if ui.button("Convert FLA/XFL to .q1s...").clicked() {
                app.queue(Action::ConvertFlaToQ1s);
            }
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Current importer: document size, fps, timeline length, layers/folders, hidden/locked state and keyframes. Artwork/symbol/media import comes next.",
                )
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
            );
        });
    app.q0fla_open = open;
}
