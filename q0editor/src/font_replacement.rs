use std::collections::{BTreeMap, BTreeSet};

use crate::app::EditorApp;
use crate::ui::FitToScreen;

pub(crate) enum DialogAction {
    Apply(BTreeMap<String, String>),
    Cancel,
}

pub(crate) struct FontReplacementDialog {
    rows: Vec<q0fla::MissingFont>,
    queries: Vec<String>,
    use_one: bool,
    common: String,
    common_query: String,
    pub error: Option<String>,
}

impl FontReplacementDialog {
    pub fn new(rows: Vec<q0fla::MissingFont>) -> Self {
        Self {
            common: rows
                .first()
                .map(|row| row.replacement.clone())
                .unwrap_or_default(),
            queries: vec![String::new(); rows.len()],
            rows,
            use_one: false,
            common_query: String::new(),
            error: None,
        }
    }

    fn choices(&self) -> BTreeMap<String, String> {
        self.rows
            .iter()
            .map(|row| {
                (
                    row.name.clone(),
                    if self.use_one {
                        self.common.clone()
                    } else {
                        row.replacement.clone()
                    },
                )
            })
            .collect()
    }

    pub fn show(&mut self, ctx: &egui::Context, importing: bool) -> Option<DialogAction> {
        let mut action = None;
        let mut open = true;
        egui::Window::new("Replace missing fonts")
            .id(egui::Id::new(("missing_fonts", importing)))
            .open(&mut open)
            .collapsible(false)
            .default_width(540.0)
            .min_width(420.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .fit_to_screen(ctx)
            .show(ctx, |ui| {
                ui.label("These fonts are unavailable. Choose replacements - your text will be preserved.");
                ui.add_space(10.0);
                ui.checkbox(&mut self.use_one, "Use one font for all");
                if self.use_one {
                    ui.horizontal(|ui| {
                        ui.label("Replace all with");
                        font_picker(ui, "all", &mut self.common, &mut self.common_query);
                    });
                }
                ui.add_space(8.0);
                ui.separator();
                egui::ScrollArea::vertical().id_source("missing_font_rows")
                    .max_height((ctx.screen_rect().height() - 260.0).clamp(100.0, 360.0))
                    .show(ui, |ui| {
                        for (index, row) in self.rows.iter_mut().enumerate() {
                            ui.push_id(index, |ui| {
                                ui.add_space(8.0);
                                ui.label(egui::RichText::new(format!("Replace '{}' with:", row.name)).strong());
                                ui.horizontal(|ui| {
                                    if self.use_one {
                                        ui.label(format!("Replacement: {}", self.common));
                                    } else {
                                        font_picker(ui, "replacement", &mut row.replacement, &mut self.queries[index]);
                                    }
                                    ui.weak(format!("Text objects: {}", row.text_count));
                                });
                                ui.add_space(8.0);
                                ui.separator();
                            });
                        }
                    });
                if let Some(error) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let valid = self.choices().values().all(|name| q0fla::fonts::find(name).is_some());
                    if ui.add_enabled(valid, egui::Button::new(if importing { "Replace and continue" } else { "Replace" })).clicked() {
                        action = Some(DialogAction::Apply(self.choices()));
                    }
                    if ui.button(if importing { "Cancel import" } else { "Keep original appearance" }).clicked() {
                        action = Some(DialogAction::Cancel);
                    }
                });
            });
        if !open {
            action = Some(DialogAction::Cancel);
        }
        action
    }
}

fn font_picker(ui: &mut egui::Ui, id: &str, selected: &mut String, query: &mut String) {
    egui::ComboBox::from_id_source(id)
        .width(275.0)
        .selected_text(selected.as_str())
        .show_ui(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(query)
                    .hint_text("Search fonts...")
                    .desired_width(260.0),
            );
            let query_lower = query.to_lowercase();
            let fonts: Vec<_> = q0fla::fonts::installed()
                .iter()
                .filter(|font| font.name.to_lowercase().contains(&query_lower))
                .collect();
            egui::ScrollArea::vertical().max_height(210.0).show_rows(
                ui,
                22.0,
                fonts.len(),
                |ui, rows| {
                    for row in rows {
                        let name = &fonts[row].name;
                        if ui.selectable_label(selected == name, name).clicked() {
                            *selected = name.clone();
                            query.clear();
                            ui.close_menu();
                        }
                    }
                },
            );
            if fonts.is_empty() {
                ui.weak("No fonts found");
            }
        });
}

#[derive(Default)]
pub(crate) struct ProjectFontAudit {
    revision: Option<u64>,
    ignored: BTreeSet<String>,
    dialog: Option<FontReplacementDialog>,
}

pub(crate) fn render_project_dialog(app: &mut EditorApp, ctx: &egui::Context) {
    if app.active_project_tab_id().is_none()
        || app.home_visible()
        || app.session.text_editor.is_some()
    {
        return;
    }
    if crate::text_fonts::installed().is_none() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    }
    let mut audit = std::mem::take(&mut app.session.font_audit);
    if audit.revision != Some(app.state.revision()) {
        audit.revision = Some(app.state.revision());
        let rows: Vec<_> = crate::text_tool::missing_fonts(&app.state.project)
            .into_iter()
            .filter(|row| !audit.ignored.contains(&row.name.to_lowercase()))
            .collect();
        audit.dialog = (!rows.is_empty()).then(|| FontReplacementDialog::new(rows));
    }
    if let Some(dialog) = &mut audit.dialog {
        match dialog.show(ctx, false) {
            Some(DialogAction::Apply(choices)) => {
                match crate::text_tool::replace_missing_fonts(app, &choices) {
                    Ok(()) => {
                        audit.dialog = None;
                        audit.revision = Some(app.state.revision());
                    }
                    Err(error) => dialog.error = Some(error),
                }
            }
            Some(DialogAction::Cancel) => {
                audit
                    .ignored
                    .extend(dialog.rows.iter().map(|row| row.name.to_lowercase()));
                audit.dialog = None;
            }
            None => {}
        }
    }
    app.session.font_audit = audit;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_font_does_not_destroy_individual_choices() {
        let mut dialog = FontReplacementDialog::new(vec![
            q0fla::MissingFont {
                name: "missing one".into(),
                replacement: "Arial".into(),
                text_count: 2,
            },
            q0fla::MissingFont {
                name: "missing two".into(),
                replacement: "Tahoma".into(),
                text_count: 1,
            },
        ]);
        dialog.common = "Segoe UI".into();
        dialog.use_one = true;
        assert!(dialog.choices().values().all(|name| name == "Segoe UI"));
        dialog.use_one = false;
        assert_eq!(dialog.choices()["missing one"], "Arial");
        assert_eq!(dialog.choices()["missing two"], "Tahoma");
    }
}
