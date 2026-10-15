use crate::ui::{icon_button, icon_toggle, Icon};
use egui::Context;

use crate::app::{Action, EditorApp};

pub fn render(app: &mut EditorApp, ctx: &Context) {
    egui::TopBottomPanel::top("document_tabs")
        .resizable(false)
        .exact_height(42.0)
        .show(ctx, |ui| {
            ui.add_space(3.0);
            strip(app, ui);
        });
}

/// Home, new document, and a tab for every open document, in one row.
pub(crate) fn strip(app: &mut EditorApp, ui: &mut egui::Ui) {
    let summaries = app.document_tab_summaries();
    let mut switch_to = None;
    let mut close = None;
    let mut reorder_to = None;
    let mut show_home = false;
    let mut create = false;

    ui.horizontal(|ui| {
        if icon_toggle(ui, Icon::Home, "Home", app.home_visible()).clicked() {
            show_home = true;
        }
        if icon_button(
            ui,
            Icon::Add,
            &app.settings
                .hotkeys
                .hint(crate::hotkeys::ShortcutCommand::NewProject, "New project"),
        )
        .clicked()
        {
            create = true;
        }
        ui.separator();
        egui::ScrollArea::horizontal()
            .id_source("project_tab_scroll")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (tab_index, tab) in summaries.iter().enumerate() {
                        egui::Frame::none()
                            .fill(if tab.active {
                                crate::ui::blend(
                                    ui.visuals().panel_fill,
                                    ui.visuals().selection.bg_fill,
                                    0.18,
                                )
                            } else {
                                ui.visuals().panel_fill
                            })
                            .rounding(6.0)
                            .inner_margin(egui::Margin::symmetric(8.0, 2.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (grip_rect, drag_handle) = ui.allocate_exact_size(
                                        egui::vec2(12.0, 24.0),
                                        egui::Sense::drag(),
                                    );
                                    crate::ui::paint_icon(
                                        ui.painter(),
                                        grip_rect.shrink2(egui::vec2(0.0, 4.0)),
                                        Icon::Grip,
                                        ui.visuals().weak_text_color(),
                                    );
                                    let drag_handle =
                                        drag_handle.on_hover_text("Drag to reorder tabs");
                                    drag_handle.dnd_set_drag_payload(tab.id);
                                    if tab.dirty {
                                        let (dot_rect, _) = ui.allocate_exact_size(
                                            egui::vec2(6.0, 26.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().circle_filled(
                                            dot_rect.center(),
                                            2.5,
                                            ui.visuals().text_color(),
                                        );
                                    }
                                    let mut hover = tab
                                        .path
                                        .as_ref()
                                        .map(|path| path.display().to_string())
                                        .unwrap_or_else(|| "Unsaved project".to_string());
                                    if tab.dirty {
                                        hover.push_str("\nUnsaved changes");
                                    }
                                    let name_width = ui
                                        .painter()
                                        .layout_no_wrap(
                                            tab.title.clone(),
                                            egui::FontId::proportional(13.0),
                                            ui.visuals().text_color(),
                                        )
                                        .size()
                                        .x
                                        .clamp(65.0, 180.0)
                                        + 12.0;
                                    let tab_label = crate::ui::name_button(
                                        ui, &tab.title, tab.active, name_width,
                                    )
                                    .on_hover_text(hover);
                                    if tab_label.clicked() {
                                        switch_to = Some(tab.id);
                                    }
                                    if let Some(source_id) = tab_label.dnd_release_payload::<u64>()
                                    {
                                        reorder_to = Some((*source_id, tab_index));
                                    }
                                    if icon_button(ui, Icon::Close, "Close document tab").clicked()
                                    {
                                        close = Some(tab.id);
                                    }
                                });
                            });
                    }
                });
            });
    });

    if show_home {
        app.queue(Action::ShowHome);
    }
    if let Some(tab_id) = switch_to {
        app.queue(Action::SwitchDocumentTab(tab_id));
    }
    if let Some((tab_id, target_index)) = reorder_to {
        app.queue(Action::MoveDocumentTab(tab_id, target_index));
    }
    if let Some(tab_id) = close {
        app.queue(Action::CloseDocumentTab(tab_id));
    }
    if create {
        app.queue(Action::NewProject);
    }
}
