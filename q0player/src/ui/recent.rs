//! Helpers for rendering the recent-files panel. The list itself lives
//! on `Settings::recent` so it persists, this module is just the view.

use std::path::{Path, PathBuf};

use egui::{ScrollArea, Ui};

pub enum RecentAction {
    Open(PathBuf),
    Forget(PathBuf),
    ClearAll,
}

pub fn render(recent: &[PathBuf], ui: &mut Ui) -> Option<RecentAction> {
    let mut action = None;

    ui.horizontal(|ui| {
        ui.heading("Recent");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Clear")
                .on_hover_text("Forget all recent files")
                .clicked()
            {
                action = Some(RecentAction::ClearAll);
            }
        });
    });
    ui.separator();

    if recent.is_empty() {
        ui.label(
            egui::RichText::new(
                "No recent files yet.\nUse File > Open... or drop a .q0s\nonto the window.",
            )
            .weak()
            .small(),
        );
        return action;
    }

    ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for path in recent {
                ui.horizontal(|ui| {
                    let label = truncate_label(&display_label(path), 36);
                    let resp = ui
                        .add(
                            egui::Label::new(egui::RichText::new(label).small())
                                .sense(egui::Sense::click())
                                .truncate(true),
                        )
                        .on_hover_text(path.display().to_string());
                    if resp.clicked() {
                        action = Some(RecentAction::Open(path.clone()));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("x")
                            .on_hover_text("Remove from recent")
                            .clicked()
                        {
                            action = Some(RecentAction::Forget(path.clone()));
                        }
                    });
                });
            }
        });

    action
}

fn display_label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut shortened: String = text.chars().take(keep).collect();
    shortened.push_str("...");
    shortened
}
