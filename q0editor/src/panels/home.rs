use std::path::Path;

use egui::{Align, Button, Frame, Layout, Margin, RichText, Ui, Vec2};

use crate::app::{Action, EditorApp};

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    ui.add_space(24.0);
    let page_width = ui.available_width().min(1120.0);
    let side_space = ((ui.available_width() - page_width) * 0.5).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(side_space);
        ui.allocate_ui_with_layout(
            Vec2::new(page_width, ui.available_height()),
            Layout::top_down(Align::Min),
            |ui| {
                header(app, ui);
                ui.add_space(22.0);
                action_row(app, ui);
                ui.add_space(24.0);
                content_columns(app, ui);
            },
        );
    });
}

fn header(app: &EditorApp, ui: &mut Ui) {
    ui.label(RichText::new("q0editor").size(38.0).strong());
    ui.label(
        RichText::new("home")
            .size(16.0)
            .color(app.settings.theme.accent.to_color32()),
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new("Create a project or continue from a recent q1s file.")
            .color(app.settings.theme.text_dim.to_color32()),
    );
}

fn action_row(app: &mut EditorApp, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_sized(
                [164.0, 42.0],
                Button::new(RichText::new("New project").strong()),
            )
            .clicked()
        {
            app.queue(Action::NewProject);
        }
        if ui
            .add_sized([164.0, 42.0], Button::new("Open project..."))
            .clicked()
        {
            app.queue(Action::OpenProject);
        }
    });
}

fn content_columns(app: &mut EditorApp, ui: &mut Ui) {
    let recent = app.settings.recent_projects.clone();
    let mut open_recent = None;
    ui.columns(2, |columns| {
        columns[0].set_max_width(columns[0].available_width());
        Frame::group(columns[0].style())
            .inner_margin(Margin::same(16.0))
            .show(&mut columns[0], |ui| {
                ui.set_min_height(360.0);
                ui.label(RichText::new("Recent projects").size(18.0).strong());
                ui.add_space(8.0);
                if recent.is_empty() {
                    ui.label(
                        RichText::new("No recent projects yet.")
                            .color(app.settings.theme.text_dim.to_color32()),
                    );
                    ui.label(
                        RichText::new("Saved and opened q1s files will appear here.")
                            .small()
                            .color(app.settings.theme.text_dim.to_color32()),
                    );
                } else {
                    for path in &recent {
                        if recent_project_button(ui, path).clicked() {
                            open_recent = Some(path.clone());
                        }
                        ui.add_space(4.0);
                    }
                }
            });

        Frame::group(columns[1].style())
            .inner_margin(Margin::same(16.0))
            .show(&mut columns[1], |ui| {
                ui.set_min_height(360.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Feed").size(18.0).strong());
                    ui.label(
                        RichText::new("reserved")
                            .small()
                            .color(app.settings.theme.accent.to_color32()),
                    );
                });
                ui.add_space(16.0);
                ui.label(
                    RichText::new("Project news will live here.")
                        .size(16.0)
                        .color(app.settings.theme.text_dim.to_color32()),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Changelogs, release notes and repository updates will appear after GitHub integration and the first public commits.",
                    )
                    .color(app.settings.theme.text_dim.to_color32()),
                );
                ui.add_space(18.0);
                ui.separator();
                ui.add_space(12.0);
                for label in ["changelog", "releases", "development updates"] {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("○")
                                .color(app.settings.theme.accent.to_color32()),
                        );
                        ui.label(
                            RichText::new(label)
                                .color(app.settings.theme.text_dim.to_color32()),
                        );
                    });
                    ui.add_space(5.0);
                }
            });
    });

    if let Some(path) = open_recent {
        app.queue(Action::OpenProjectFromPath(path));
    }
}

fn recent_project_button(ui: &mut Ui, path: &Path) -> egui::Response {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let parent = path
        .parent()
        .map(Path::display)
        .map(|display| display.to_string())
        .unwrap_or_default();
    ui.add_sized(
        [ui.available_width(), 48.0],
        Button::new(format!("{file_name}\n{parent}")),
    )
    .on_hover_text(path.display().to_string())
}
