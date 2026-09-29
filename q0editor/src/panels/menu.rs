use egui::{Key, Modifiers, Ui};
use std::path::{Path, PathBuf};

use crate::app::{Action, EditorApp};

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    egui::menu::bar(ui, |ui| {
        ui.menu_button("File", |ui| {
            if app.has_active_document() && !app.home_visible() {
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
                    app.has_active_document() && !app.home_visible(),
                    shortcut_button_widget("Save", "Ctrl+S"),
                )
                .clicked()
            {
                app.queue(Action::SaveProject);
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    app.has_active_document() && !app.home_visible(),
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

        if !app.home_visible() && app.has_active_project() {
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
                    .add_enabled(
                        has_clip,
                        shortcut_button_widget("Paste in Place", "Ctrl+Shift+V"),
                    )
                    .clicked()
                {
                    app.queue(Action::PasteInPlace);
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
                if ui
                    .selectable_label(app.project_tree_open(), "Project...")
                    .clicked()
                {
                    app.queue(Action::ToggleProjectTree);
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
                let project_resp = ui
                    .selectable_label(app.project_tree_open(), "Project")
                    .on_hover_text("Open or close the project dependency tree");
                if project_resp.clicked() {
                    app.queue(Action::ToggleProjectTree);
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

            // q0enc/q0fla are tools, not navigation. Keep them together and
            // slightly left of the main Home/Project/Credits/Settings group.
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            if app.has_active_project() && !app.home_visible() {
                let q0enc_resp = ui
                    .selectable_label(app.q0enc.open, "q0enc")
                    .on_hover_text("Open or close the export workshop");
                if q0enc_resp.clicked() {
                    app.queue(Action::ToggleQ0Enc);
                }
            }
            let q0fla_resp = ui
                .selectable_label(app.q0fla_open, "q0fla")
                .on_hover_text("Open or close the FLA/XFL import workshop");
            if q0fla_resp.clicked() {
                app.queue(Action::ToggleQ0Fla);
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
        | crate::state::Selection::PathPoints { .. }
        | crate::state::Selection::SubselectPoints { .. }
        | crate::state::Selection::StrokeSegment(_) => true,
        crate::state::Selection::Paths(items) => !items.is_empty(),
        crate::state::Selection::RawParts {
            fills,
            strokes,
            stroke_segments,
            objects,
        } => {
            !fills.is_empty()
                || !strokes.is_empty()
                || !stroke_segments.is_empty()
                || !objects.is_empty()
        }
        crate::state::Selection::Mixed { paths, objects } => {
            !paths.is_empty() || !objects.is_empty()
        }
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

fn consume_performance_snapshot_shortcut(input: &mut egui::InputState) -> bool {
    input.consume_key(Modifiers::NONE, Key::F4)
}

/// egui-winit converts native Ctrl+X/C/V into Cut/Copy/Paste events and returns
/// before emitting Event::Key for the pressed key. Handle that real native
/// path explicitly; the key-based fallback below is retained for other backends
/// and for synthetic callers.
fn consume_native_clipboard_shortcut(input: &mut egui::InputState) -> Option<Action> {
    let paste_in_place = input.modifiers.shift;
    let mut shortcut = None;
    input.events.retain(|event| {
        let action = match event {
            egui::Event::Cut => Some(Action::CutSelection),
            egui::Event::Copy => Some(Action::CopySelection),
            egui::Event::Paste(_) => Some(if paste_in_place {
                Action::PasteInPlace
            } else {
                Action::Paste
            }),
            _ => None,
        };
        if let Some(action) = action {
            if shortcut.is_none() {
                shortcut = Some(action);
            }
            false
        } else {
            true
        }
    });
    shortcut
}

pub fn handle_home_shortcuts(app: &mut EditorApp, ctx: &egui::Context) {
    let mut copy_perf = false;
    ctx.input_mut(|input| {
        copy_perf |= consume_performance_snapshot_shortcut(input);
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
    if copy_perf {
        app.copy_performance_snapshot(ctx);
    }
}

pub fn handle_global_shortcuts(app: &mut EditorApp, ctx: &egui::Context) {
    let typing = ctx.wants_keyboard_input();
    let code_document = app.active_document_is_q0lang();
    let can_clip = app.session.timeline_selection.is_some()
        || app.session.timeline_layer_selection.is_some()
        || crate::selection_edit::selection_can_clip(&app.session.selection);
    let mut copy_perf = false;
    let mut arm_native_artwork_clipboard = false;
    ctx.input_mut(|i| {
        copy_perf |= consume_performance_snapshot_shortcut(i);
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
        if code_document {
            return;
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
            if let Some(action) = consume_native_clipboard_shortcut(i) {
                if can_clip && matches!(action, Action::CopySelection | Action::CutSelection) {
                    // egui-winit only emits Event::Paste when the system clipboard
                    // contains non-empty text. Keep a harmless marker there so an
                    // internal artwork copy/cut can always be pasted with Ctrl+V.
                    arm_native_artwork_clipboard = true;
                }
                app.queue(action);
            } else {
                if i.consume_key(Modifiers::COMMAND, Key::X) {
                    app.queue(Action::CutSelection);
                }
                if i.consume_key(Modifiers::COMMAND, Key::C) {
                    app.queue(Action::CopySelection);
                }
                if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::V) {
                    app.queue(Action::PasteInPlace);
                } else if i.consume_key(Modifiers::COMMAND, Key::V) {
                    app.queue(Action::Paste);
                }
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
                    "Q" => Key::Q,
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
                    "T" => Key::T,
                    _ => continue,
                };
                if i.consume_key(Modifiers::NONE, key) {
                    app.queue(Action::SelectTool(tool));
                }
            }
        }
    });
    if copy_perf {
        app.copy_performance_snapshot(ctx);
    }
    if arm_native_artwork_clipboard {
        ctx.output_mut(|output| output.copied_text = "q0editor artwork".to_string());
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;
    use std::time::Duration;

    fn raw_clipboard_square(min: f32, max: f32) -> crate::state::ClipboardPayload {
        use q0s_format::v2::{Anchor, Path, Rgba, Vec2, VectorAsset};
        crate::state::ClipboardPayload {
            raw_vectors: vec![crate::state::RawVectorClipboard {
                vector: VectorAsset {
                    asset_id: 0,
                    paths: vec![Path {
                        anchors: vec![
                            Anchor {
                                point: Vec2::new(min, min),
                                in_handle: None,
                                out_handle: None,
                            },
                            Anchor {
                                point: Vec2::new(max, min),
                                in_handle: None,
                                out_handle: None,
                            },
                            Anchor {
                                point: Vec2::new(max, max),
                                in_handle: None,
                                out_handle: None,
                            },
                            Anchor {
                                point: Vec2::new(min, max),
                                in_handle: None,
                                out_handle: None,
                            },
                        ],
                        closed: true,
                    }],
                    fill: Some(Rgba {
                        r: 20,
                        g: 30,
                        b: 40,
                        a: 255,
                    }),
                    stroke: None,
                },
                appearance: None,
            }],
            artwork_center: Some(Vec2::new((min + max) * 0.5, (min + max) * 0.5)),
            ..Default::default()
        }
    }

    fn vector_bounds_for_shortcut_test(app: &EditorApp) -> (f32, f32, f32, f32) {
        let vector = app
            .state
            .project
            .assets
            .iter()
            .filter_map(|asset| match asset {
                q0s_format::v2::Asset::Vector(vector) => Some(vector),
                _ => None,
            })
            .next_back()
            .expect("pasted vector");
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for anchor in vector.paths.iter().flat_map(|path| &path.anchors) {
            min_x = min_x.min(anchor.point.x);
            min_y = min_y.min(anchor.point.y);
            max_x = max_x.max(anchor.point.x);
            max_y = max_y.max(anchor.point.y);
        }
        (min_x, min_y, max_x, max_y)
    }

    fn run_paste_shortcut(app: &mut EditorApp, modifiers: Modifiers) {
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                modifiers,
                // This is the event egui-winit actually emits for native Ctrl+V.
                // It deliberately does NOT emit Event::Key(V) on key press.
                events: vec![egui::Event::Paste("system clipboard text".to_string())],
                ..Default::default()
            },
            |ctx| handle_global_shortcuts(app, ctx),
        );
        app.flush_pending_actions();
    }

    fn seed_native_copy_square(app: &mut EditorApp) {
        use q0s_format::v2::{
            Anchor, Asset, Path, Placement, Rgba, Target, Transform2D, Tween, Vec2, VectorAsset,
        };
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 1,
            paths: vec![Path {
                anchors: vec![
                    Anchor {
                        point: Vec2::new(10.0, 10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(30.0, 10.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(30.0, 30.0),
                        in_handle: None,
                        out_handle: None,
                    },
                    Anchor {
                        point: Vec2::new(10.0, 30.0),
                        in_handle: None,
                        out_handle: None,
                    },
                ],
                closed: true,
            }],
            fill: Some(Rgba {
                r: 20,
                g: 30,
                b: 40,
                a: 255,
            }),
            stroke: None,
        }));
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                instance_id: 0,
                frame: 0,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            });
        app.session.selection = crate::state::Selection::Path {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
            path_idx: 0,
        };
    }

    #[test]
    fn native_copy_event_fills_internal_clipboard_and_arms_native_paste() {
        let mut app = EditorApp::default();
        seed_native_copy_square(&mut app);
        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                modifiers: Modifiers::COMMAND,
                events: vec![egui::Event::Copy],
                ..Default::default()
            },
            |ctx| handle_global_shortcuts(&mut app, ctx),
        );
        app.flush_pending_actions();

        let clipboard = app
            .session
            .clipboard
            .as_ref()
            .expect("internal artwork clipboard");
        assert_eq!(clipboard.raw_vectors.len(), 1);
        assert_eq!(output.platform_output.copied_text, "q0editor artwork");
    }

    #[test]
    fn ctrl_v_centres_artwork_clipboard_on_stage_cursor() {
        let mut app = EditorApp::default();
        app.session.clipboard = Some(raw_clipboard_square(10.0, 30.0));
        app.session.stage_cursor = Some(q0s_format::v2::Vec2::new(300.0, 200.0));

        run_paste_shortcut(&mut app, Modifiers::COMMAND);

        assert_eq!(
            vector_bounds_for_shortcut_test(&app),
            (290.0, 190.0, 310.0, 210.0)
        );
        assert_eq!(app.session.status, "pasted at cursor");
    }

    #[test]
    fn ctrl_shift_v_pastes_artwork_in_original_coordinates() {
        let mut app = EditorApp::default();
        app.session.clipboard = Some(raw_clipboard_square(10.0, 30.0));
        app.session.stage_cursor = Some(q0s_format::v2::Vec2::new(300.0, 200.0));

        run_paste_shortcut(&mut app, Modifiers::COMMAND | Modifiers::SHIFT);

        assert_eq!(
            vector_bounds_for_shortcut_test(&app),
            (10.0, 10.0, 30.0, 30.0)
        );
        assert_eq!(app.session.status, "pasted in place");
    }

    #[test]
    fn f4_copies_performance_snapshot() {
        let mut app = EditorApp::default();
        app.perf.begin_frame();
        app.perf
            .record(crate::perf::PerfSpan::Stage, Duration::from_millis(37));
        app.perf.finish_frame();

        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: Key::F4,
                    physical_key: Some(Key::F4),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| handle_global_shortcuts(&mut app, ctx),
        );

        assert!(output
            .platform_output
            .copied_text
            .contains("q0editor performance snapshot"));
        assert!(output.platform_output.copied_text.contains("stage.total"));
        assert!(app.session.status.contains("performance log copied"));
    }
}
