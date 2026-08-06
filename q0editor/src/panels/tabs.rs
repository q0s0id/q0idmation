use egui::{Context, RichText};

use crate::app::{Action, EditorApp};

pub fn render(app: &mut EditorApp, ctx: &Context) {
    let summaries = app.project_tab_summaries();
    let mut switch_to = None;
    let mut close = None;
    let mut show_home = false;
    let mut create = false;

    egui::TopBottomPanel::top("document_tabs")
        .resizable(false)
        .exact_height(32.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(app.home_visible(), RichText::new("Home").strong())
                    .clicked()
                {
                    show_home = true;
                }
                if ui.small_button("+").on_hover_text("New project").clicked() {
                    create = true;
                }
                ui.separator();
                egui::ScrollArea::horizontal()
                    .id_source("project_tab_scroll")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for tab in &summaries {
                                ui.group(|ui| {
                                    ui.horizontal(|ui| {
                                        let label = if tab.dirty {
                                            format!("{} *", tab.title)
                                        } else {
                                            tab.title.clone()
                                        };
                                        let mut hover = tab
                                            .path
                                            .as_ref()
                                            .map(|path| path.display().to_string())
                                            .unwrap_or_else(|| "Unsaved project".to_string());
                                        if tab.dirty {
                                            hover.push_str("\nUnsaved changes");
                                        }
                                        if ui
                                            .selectable_label(tab.active, label)
                                            .on_hover_text(hover)
                                            .clicked()
                                        {
                                            switch_to = Some(tab.id);
                                        }
                                        if ui
                                            .small_button("x")
                                            .on_hover_text("Close project tab")
                                            .clicked()
                                        {
                                            close = Some(tab.id);
                                        }
                                    });
                                });
                            }
                        });
                    });
            });
        });

    if show_home {
        app.queue(Action::ShowHome);
    }
    if let Some(tab_id) = switch_to {
        app.queue(Action::SwitchProjectTab(tab_id));
    }
    if let Some(tab_id) = close {
        app.queue(Action::CloseProjectTab(tab_id));
    }
    if create {
        app.queue(Action::NewProject);
    }
}
