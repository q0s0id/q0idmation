//! Helpers for rendering the recent-files panel. The list itself lives
//! on `Settings::recent` so it persists, this module is just the view.

use std::path::{Path, PathBuf};

use egui::{ScrollArea, Ui};

use super::chrome;

pub enum RecentAction {
    Open(PathBuf),
    Forget(PathBuf),
    ClearAll,
}

/// The recent movies as cards, `current` marked as the one playing.
pub fn render(recent: &[PathBuf], current: Option<&Path>, ui: &mut Ui) -> Option<RecentAction> {
    let mut action = None;

    chrome::panel_header(ui, "Recent", |ui| {
        if !recent.is_empty()
            && ui
                .small_button("Clear")
                .on_hover_text("Forget all recent files")
                .clicked()
        {
            action = Some(RecentAction::ClearAll);
        }
    });

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
            ui.spacing_mut().item_spacing.y = 2.0;
            for path in recent {
                let kind = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("")
                    .to_uppercase();
                let (response, card) = chrome::recent_card(
                    ui,
                    &display_label(path),
                    &folder_label(path),
                    &kind,
                    current == Some(path.as_path()),
                    super::platform::TOUCH,
                );
                response.on_hover_text(path.display().to_string());
                if card.forget {
                    action = Some(RecentAction::Forget(path.clone()));
                } else if card.open {
                    action = Some(RecentAction::Open(path.clone()));
                }
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

/// The folder a movie is in, by name.
fn folder_label(path: &Path) -> String {
    path.parent()
        .and_then(|parent| parent.file_name().or(Some(parent.as_os_str())))
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_name_the_movie_and_its_folder() {
        let path = Path::new("movies").join("fight").join("scene 2.q0s");
        assert_eq!(display_label(&path), "scene 2.q0s");
        assert_eq!(folder_label(&path), "fight");
        assert_eq!(folder_label(Path::new("alone.q0s")), "");
    }
}
