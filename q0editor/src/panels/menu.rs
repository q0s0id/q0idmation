use egui::{Key, Modifiers, Ui};
use std::path::{Path, PathBuf};

use crate::app::{Action, EditorApp};

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    egui::menu::bar(ui, |ui| {
        ui.menu_button("File", |ui| {
            if app.has_active_project() && !app.home_visible() {
                if ui.button("Home").clicked() {
                    app.queue(Action::ShowHome);
                    ui.close_menu();
                }
                ui.separator();
            }
            if shortcut_button(ui, "New", "Ctrl+N").clicked() {
                app.queue(Action::NewProject);
                ui.close_menu();
            }
            if shortcut_button(ui, "Open...", "Ctrl+O").clicked() {
                app.queue(Action::OpenProject);
                ui.close_menu();
            }
            let recent = app.settings.recent_projects.clone();
            let labels = recent_menu_labels(&recent);
            ui.add_enabled_ui(!recent.is_empty(), |ui| {
                ui.menu_button("Open Recent", |ui| {
                    for (path, label) in recent.into_iter().zip(labels) {
                        if ui
                            .button(label)
                            .on_hover_text(path.display().to_string())
                            .clicked()
                        {
                            app.queue(Action::OpenProjectFromPath(path));
                            ui.close_menu();
                        }
                    }
                });
            });
            ui.separator();
            if ui
                .add_enabled(
                    app.has_active_project() && !app.home_visible(),
                    egui::Button::new("Import Media to Library..."),
                )
                .clicked()
            {
                app.queue(Action::ImportMedia);
                ui.close_menu();
            }
            ui.separator();
            if ui
                .add_enabled(
                    app.has_active_project() && !app.home_visible(),
                    shortcut_button_widget("Save", "Ctrl+S"),
                )
                .clicked()
            {
                app.queue(Action::SaveProject);
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    app.has_active_project() && !app.home_visible(),
                    shortcut_button_widget("Save As...", "Ctrl+Shift+S"),
                )
                .clicked()
            {
                app.queue(Action::SaveProjectAs);
                ui.close_menu();
            }
            ui.separator();
            if ui
                .add_enabled(
                    app.has_active_project() && !app.home_visible(),
                    shortcut_button_widget("Export...", "Ctrl+E"),
                )
                .clicked()
            {
                app.queue(Action::OpenQ0Enc);
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Exit").clicked() {
                app.queue(Action::Exit);
                ui.close_menu();
            }
        });

        if !app.home_visible() {
            ui.menu_button("Edit", |ui| {
                let can_undo = app.history.can_undo();
                let can_redo = app.history.can_redo();
                if ui
                    .add_enabled(can_undo, shortcut_button_widget("Undo", "Ctrl+Z"))
                    .clicked()
                {
                    app.queue(Action::Undo);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(can_redo, shortcut_button_widget("Redo", "Ctrl+Y"))
                    .clicked()
                {
                    app.queue(Action::Redo);
                    ui.close_menu();
                }
                ui.separator();
                let can_clip = app.session.timeline_selection.is_some()
                    || app.session.timeline_layer_selection.is_some()
                    || crate::selection_edit::selection_can_clip(&app.session.selection);
                let can_delete = app.session.timeline_selection.is_some()
                    || selection_can_be_deleted(&app.session.selection);
                let has_clip = app.session.clipboard.is_some();
                if ui
                    .add_enabled(can_clip, shortcut_button_widget("Cut", "Ctrl+X"))
                    .clicked()
                {
                    app.queue(Action::CutSelection);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(can_clip, shortcut_button_widget("Copy", "Ctrl+C"))
                    .clicked()
                {
                    app.queue(Action::CopySelection);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(has_clip, shortcut_button_widget("Paste", "Ctrl+V"))
                    .clicked()
                {
                    app.queue(Action::Paste);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(can_clip, shortcut_button_widget("Duplicate", "Ctrl+D"))
                    .clicked()
                {
                    app.queue(Action::DuplicateSelection);
                    ui.close_menu();
                }
                ui.separator();
                if ui
                    .add_enabled(can_delete, shortcut_button_widget("Delete", "Del"))
                    .clicked()
                {
                    app.queue(Action::DeleteSelection);
                    ui.close_menu();
                }
            });

            ui.menu_button("Modify", |ui| {
                let can_convert = crate::selection_edit::selection_can_clip(&app.session.selection);
                if ui
                    .add_enabled(
                        can_convert,
                        shortcut_button_widget("Convert to Symbol", "Ctrl+G"),
                    )
                    .clicked()
                {
                    app.queue(Action::ConvertSelectionToQ0rg);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        app.can_break_apart_selection(),
                        shortcut_button_widget("Break Apart", "Ctrl+B"),
                    )
                    .clicked()
                {
                    app.queue(Action::BreakApartSelection);
                    ui.close_menu();
                }
            });

            ui.menu_button("View", |ui| {
                if shortcut_button(ui, "Zoom In", "Ctrl+=").clicked() {
                    app.queue(Action::ZoomIn);
                    ui.close_menu();
                }
                if shortcut_button(ui, "Zoom Out", "Ctrl+-").clicked() {
                    app.queue(Action::ZoomOut);
                    ui.close_menu();
                }
                if shortcut_button(ui, "Reset Zoom", "Ctrl+0").clicked() {
                    app.queue(Action::ZoomReset);
                    ui.close_menu();
                }
                ui.separator();
                if shortcut_button(ui, "Script Editor (q0lang)...", "F9").clicked() {
                    app.queue(Action::OpenQ0langEditor(None));
                    ui.close_menu();
                }
                ui.separator();
                ui.label(
                    egui::RichText::new("Mouse wheel: zoom, MMB or hold H/Space + drag: pan")
                        .color(app.settings.theme.text_dim.to_color32())
                        .small(),
                );
            });
        }

        ui.menu_button("Help", |ui| {
            if ui.button("Credits...").clicked() {
                app.queue(Action::ToggleCredits);
                ui.close_menu();
            }
            ui.separator();
            ui.label(
                egui::RichText::new(format!("q0editor {}", env!("CARGO_PKG_VERSION")))
                    .color(app.settings.theme.text_dim.to_color32()),
            );
            ui.label(
                egui::RichText::new(
                    "V/A/P/B/N/R/O/K/I tools, Space play/pause, arrows step frames",
                )
                .color(app.settings.theme.text_dim.to_color32()),
            );
        });

        ui.separator();
        let status = truncate_label(&app.session.status, 72);
        let status_color = if status_is_error(&app.session.status) {
            app.settings.theme.accent.to_color32()
        } else {
            app.settings.theme.text_dim.to_color32()
        };
        let status_response = ui
            .label(egui::RichText::new(status).color(status_color))
            .on_hover_text(format!(
                "{}\nRight-click to copy",
                app.session.status.as_str()
            ));
        status_response.context_menu(|ui| {
            if ui.button("Copy status").clicked() {
                ui.output_mut(|output| output.copied_text = app.session.status.clone());
                ui.close_menu();
            }
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let settings_resp = ui
                .selectable_label(app.session.show_settings, "Settings")
                .on_hover_text("Open or close settings");
            if settings_resp.clicked() {
                app.queue(Action::ToggleSettings);
            }
            let credits_resp = ui
                .selectable_label(app.session.show_credits, "Credits")
                .on_hover_text("Open or close contributor credits");
            if credits_resp.clicked() {
                app.queue(Action::ToggleCredits);
            }
            if app.has_active_project() && !app.home_visible() {
                let q0enc_resp = ui
                    .selectable_label(app.q0enc.open, "q0enc")
                    .on_hover_text("Open or close the export workshop");
                if q0enc_resp.clicked() {
                    app.queue(Action::ToggleQ0Enc);
                }
            }
            let home_resp = ui
                .selectable_label(app.home_visible(), "Home")
                .on_hover_text("Open the q0editor home page");
            if home_resp.clicked() {
                app.queue(if app.home_visible() && app.has_active_project() {
                    Action::ResumeProject
                } else {
                    Action::ShowHome
                });
            }
        });
    });
}

fn shortcut_button(ui: &mut Ui, label: &str, accel: &str) -> egui::Response {
    ui.add(shortcut_button_widget(label, accel))
}

fn shortcut_button_widget<'a>(label: &'a str, accel: &'a str) -> impl egui::Widget + 'a {
    move |ui: &mut Ui| {
        ui.horizontal(|ui| {
            let resp = ui.button(label);
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(accel)
                    .color(crate::theme::text_dim())
                    .small(),
            );
            resp
        })
        .inner
    }
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

fn recent_menu_labels(recent: &[PathBuf]) -> Vec<String> {
    let names: Vec<String> = recent
        .iter()
        .map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        })
        .collect();
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for name in &names {
        *counts.entry(name.to_lowercase()).or_default() += 1;
    }

    recent
        .iter()
        .zip(names)
        .map(|(path, name)| {
            if counts.get(&name.to_lowercase()).copied().unwrap_or(0) > 1 {
                let parent = path
                    .parent()
                    .and_then(Path::file_name)
                    .map(|part| part.to_string_lossy().into_owned())
                    .filter(|part| !part.is_empty())
                    .unwrap_or_else(|| "root".to_string());
                truncate_label(&format!("{name} ({parent})"), 64)
            } else {
                truncate_label(&name, 64)
            }
        })
        .collect()
}

pub(crate) fn selection_can_be_deleted(selection: &crate::state::Selection) -> bool {
    match selection {
        crate::state::Selection::Asset(_)
        | crate::state::Selection::Q0rg(_)
        | crate::state::Selection::Placement { .. }
        | crate::state::Selection::Path { .. }
        | crate::state::Selection::PathPoints { .. } => true,
        crate::state::Selection::Paths(items) => !items.is_empty(),
        crate::state::Selection::Multi(items) => !items.is_empty(),
        crate::state::Selection::RawArea {
            placements,
            objects,
            ..
        } => !placements.is_empty() || !objects.is_empty(),
        crate::state::Selection::None => false,
    }
}

fn status_is_error(status: &str) -> bool {
    let status = status.to_ascii_lowercase();
    [
        "failed",
        "error",
        "unsupported",
        "too large",
        "unavailable",
        "confirmation required",
    ]
    .iter()
    .any(|needle| status.contains(needle))
}

pub fn handle_home_shortcuts(app: &mut EditorApp, ctx: &egui::Context) {
    ctx.input_mut(|input| {
        if input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab) {
            app.queue(Action::CycleDocumentTab(-1));
        } else if input.consume_key(Modifiers::COMMAND, Key::Tab) {
            app.queue(Action::CycleDocumentTab(1));
        }
        if input.consume_key(Modifiers::COMMAND, Key::N) {
            app.queue(Action::NewProject);
        }
        if input.consume_key(Modifiers::COMMAND, Key::O) {
            app.queue(Action::OpenProject);
        }
    });
}

pub fn handle_global_shortcuts(app: &mut EditorApp, ctx: &egui::Context) {
    let typing = ctx.wants_keyboard_input();
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab) {
            app.queue(Action::CycleDocumentTab(-1));
        } else if i.consume_key(Modifiers::COMMAND, Key::Tab) {
            app.queue(Action::CycleDocumentTab(1));
        }
        if i.consume_key(Modifiers::COMMAND, Key::N) {
            app.queue(Action::NewProject);
        }
        if i.consume_key(Modifiers::COMMAND, Key::O) {
            app.queue(Action::OpenProject);
        }
        if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
            app.queue(Action::SaveProjectAs);
        } else if i.consume_key(Modifiers::COMMAND, Key::S) {
            app.queue(Action::SaveProject);
        }
        if !typing {
            if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y)
            {
                app.queue(Action::Redo);
            } else if i.consume_key(Modifiers::COMMAND, Key::Z) {
                app.queue(Action::Undo);
            }
            if i.consume_key(Modifiers::COMMAND, Key::G) {
                app.queue(Action::ConvertSelectionToQ0rg);
            }
            if i.consume_key(Modifiers::COMMAND, Key::B) {
                app.queue(Action::BreakApartSelection);
            }
        }
        if i.consume_key(Modifiers::COMMAND, Key::E) {
            app.queue(Action::OpenQ0Enc);
        }
        if !typing {
            if i.consume_key(Modifiers::COMMAND, Key::X) {
                app.queue(Action::CutSelection);
            }
            if i.consume_key(Modifiers::COMMAND, Key::C) {
                app.queue(Action::CopySelection);
            }
            if i.consume_key(Modifiers::COMMAND, Key::V) {
                app.queue(Action::Paste);
            }
            if i.consume_key(Modifiers::COMMAND, Key::D) {
                app.queue(Action::DuplicateSelection);
            }
        }
        // Zoom: Ctrl+= and Ctrl++ (some keyboards send Plus, others Equals).
        if i.consume_key(Modifiers::COMMAND, Key::Plus)
            || i.consume_key(Modifiers::COMMAND, Key::Equals)
        {
            app.queue(Action::ZoomIn);
        }
        if i.consume_key(Modifiers::COMMAND, Key::Minus) {
            app.queue(Action::ZoomOut);
        }
        if i.consume_key(Modifiers::COMMAND, Key::Num0) {
            app.queue(Action::ZoomReset);
        }
        if !typing
            && (i.consume_key(Modifiers::NONE, Key::Delete)
                || i.consume_key(Modifiers::NONE, Key::Backspace))
        {
            app.queue(Action::DeleteSelection);
        }
        // Space is reserved for the temporary hand tool over the stage.
        if !typing && i.consume_key(Modifiers::NONE, Key::Enter) {
            app.queue(Action::TogglePlay);
        }
        if !typing && i.consume_key(Modifiers::NONE, Key::Home) {
            app.queue(Action::FirstFrame);
        }
        if !typing && i.consume_key(Modifiers::NONE, Key::End) {
            app.queue(Action::LastFrame);
        }
        if !typing && i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
            app.queue(Action::PreviousFrame);
        }
        if !typing && i.consume_key(Modifiers::NONE, Key::ArrowRight) {
            app.queue(Action::NextFrame);
        }
        if !typing
            && matches!(app.session.tool_state, crate::state::ToolState::Idle)
            && i.consume_key(Modifiers::NONE, Key::Escape)
        {
            app.session.selection = crate::state::Selection::None;
            app.session.timeline_selection = None;
            app.session.status = "selection cleared".to_string();
        }
        if i.consume_key(Modifiers::SHIFT, Key::F5) {
            app.queue(Action::RemoveFrame);
        } else if i.consume_key(Modifiers::NONE, Key::F5) {
            app.queue(Action::InsertFrame);
        }
        if i.consume_key(Modifiers::SHIFT, Key::F6) {
            app.queue(Action::ClearKeyframe);
        } else if i.consume_key(Modifiers::NONE, Key::F6) {
            app.queue(Action::InsertKeyframe);
        }
        if i.consume_key(Modifiers::NONE, Key::F7) {
            app.queue(Action::InsertBlankKeyframe);
        }
        // F9 вЂ” open the q0lang script editor for the current q0rg.
        // Active even while typing (TextEdit doesn't claim F9), so it
        // works whether the user is in a panel input or focused on the
        // canvas.
        if i.consume_key(Modifiers::NONE, Key::F9) {
            app.queue(Action::OpenQ0langEditor(None));
        }
        if i.consume_key(Modifiers::COMMAND | Modifiers::ALT, Key::T) {
            app.queue(Action::CreateMotionTween);
        }
        // Tool letter shortcuts only when no widget is capturing keyboard.
        if !typing {
            for tool in crate::state::Tool::ALL {
                let key = match tool.glyph() {
                    "V" => Key::V,
                    "H" => Key::H,
                    "A" => Key::A,
                    "P" => Key::P,
                    "Y" => Key::Y,
                    "B" => Key::B,
                    "E" => Key::E,
                    "N" => Key::N,
                    "R" => Key::R,
                    "O" => Key::O,
                    "K" => Key::K,
                    "I" => Key::I,
                    _ => continue,
                };
                if i.consume_key(Modifiers::NONE, key) {
                    app.queue(Action::SelectTool(tool));
                }
            }
        }
    });
}
