use egui::{Key, Ui};
use std::path::{Path, PathBuf};

use crate::app::{Action, EditorApp};
use crate::hotkeys::{HotkeySettings, ShortcutChord, ShortcutCommand, ShortcutKey};

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
            if shortcut_button(
                ui,
                "New",
                app.settings.hotkeys.display(ShortcutCommand::NewProject),
            )
            .clicked()
            {
                app.queue(Action::NewProject);
                ui.close_menu();
            }
            if shortcut_button(
                ui,
                "Open...",
                app.settings.hotkeys.display(ShortcutCommand::OpenProject),
            )
            .clicked()
            {
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
                    shortcut_button_widget(
                        "Save",
                        app.settings.hotkeys.display(ShortcutCommand::SaveProject),
                    ),
                )
                .clicked()
            {
                app.queue(Action::SaveProject);
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    app.has_active_document() && !app.home_visible(),
                    shortcut_button_widget(
                        "Save As...",
                        app.settings.hotkeys.display(ShortcutCommand::SaveProjectAs),
                    ),
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
                    shortcut_button_widget(
                        "Export...",
                        app.settings.hotkeys.display(ShortcutCommand::Export),
                    ),
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
                    .add_enabled(
                        can_undo,
                        shortcut_button_widget(
                            "Undo",
                            app.settings.hotkeys.display(ShortcutCommand::Undo),
                        ),
                    )
                    .clicked()
                {
                    app.queue(Action::Undo);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        can_redo,
                        shortcut_button_widget(
                            "Redo",
                            app.settings.hotkeys.display(ShortcutCommand::Redo),
                        ),
                    )
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
                let has_clip = app.has_clipboard();
                if ui
                    .add_enabled(
                        can_clip,
                        shortcut_button_widget(
                            "Cut",
                            app.settings.hotkeys.display(ShortcutCommand::Cut),
                        ),
                    )
                    .clicked()
                {
                    app.queue(Action::CutSelection);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        can_clip,
                        shortcut_button_widget(
                            "Copy",
                            app.settings.hotkeys.display(ShortcutCommand::Copy),
                        ),
                    )
                    .clicked()
                {
                    app.queue(Action::CopySelection);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        has_clip,
                        shortcut_button_widget(
                            "Paste",
                            app.settings.hotkeys.display(ShortcutCommand::Paste),
                        ),
                    )
                    .clicked()
                {
                    app.queue(Action::Paste);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        has_clip,
                        shortcut_button_widget(
                            "Paste in Place",
                            app.settings.hotkeys.display(ShortcutCommand::PasteInPlace),
                        ),
                    )
                    .clicked()
                {
                    app.queue(Action::PasteInPlace);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        can_clip,
                        shortcut_button_widget(
                            "Duplicate",
                            app.settings.hotkeys.display(ShortcutCommand::Duplicate),
                        ),
                    )
                    .clicked()
                {
                    app.queue(Action::DuplicateSelection);
                    ui.close_menu();
                }
                ui.separator();
                if ui
                    .add_enabled(
                        can_delete,
                        shortcut_button_widget(
                            "Delete",
                            app.settings.hotkeys.display(ShortcutCommand::Delete),
                        ),
                    )
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
                        shortcut_button_widget(
                            "Convert to Symbol",
                            app.settings
                                .hotkeys
                                .display(ShortcutCommand::ConvertToSymbol),
                        ),
                    )
                    .clicked()
                {
                    app.queue(Action::ConvertSelectionToQ0rg);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        app.can_break_apart_selection(),
                        shortcut_button_widget(
                            "Break Apart",
                            app.settings.hotkeys.display(ShortcutCommand::BreakApart),
                        ),
                    )
                    .clicked()
                {
                    app.queue(Action::BreakApartSelection);
                    ui.close_menu();
                }
            });

            ui.menu_button("View", |ui| {
                if shortcut_button(
                    ui,
                    "Zoom In",
                    app.settings.hotkeys.display(ShortcutCommand::ZoomIn),
                )
                .clicked()
                {
                    app.queue(Action::ZoomIn);
                    ui.close_menu();
                }
                if shortcut_button(
                    ui,
                    "Zoom Out",
                    app.settings.hotkeys.display(ShortcutCommand::ZoomOut),
                )
                .clicked()
                {
                    app.queue(Action::ZoomOut);
                    ui.close_menu();
                }
                if shortcut_button(
                    ui,
                    "Reset Zoom",
                    app.settings.hotkeys.display(ShortcutCommand::ZoomReset),
                )
                .clicked()
                {
                    app.queue(Action::ZoomReset);
                    ui.close_menu();
                }
                ui.separator();
                if ui
                    .selectable_label(app.session.outline_mode, "outline mode")
                    .clicked()
                {
                    app.queue(Action::ToggleOutlineMode);
                    ui.close_menu();
                }
                if shortcut_button(
                    ui,
                    "Show / Hide Timeline",
                    app.settings
                        .hotkeys
                        .display(ShortcutCommand::ToggleTimeline),
                )
                .clicked()
                {
                    app.queue(Action::ToggleTimeline);
                    ui.close_menu();
                }
                if ui
                    .selectable_label(app.session.show_animation_preview, "Animation Preview")
                    .clicked()
                {
                    app.session.show_animation_preview = !app.session.show_animation_preview;
                    app.session.animation_preview_frame = app.session.current_frame;
                    app.session.animation_preview_tick = std::time::Instant::now();
                    ui.close_menu();
                }
                if shortcut_button(
                    ui,
                    "Script Editor (q0lang)...",
                    app.settings.hotkeys.display(ShortcutCommand::ScriptEditor),
                )
                .clicked()
                {
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
                    "V/A/P/B/N/R/O/K/I tools; arrows move selection (Shift: fine), or step frames without selection",
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

fn shortcut_button(ui: &mut Ui, label: &str, accel: String) -> egui::Response {
    ui.add(shortcut_button_widget(label, accel))
}

fn shortcut_button_widget<'a>(label: &'a str, accel: String) -> impl egui::Widget + 'a {
    egui::Button::new(label).shortcut_text(if accel == "Unassigned" {
        String::new()
    } else {
        accel
    })
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

/// egui-winit converts native Ctrl+X/C/V into Cut/Copy/Paste events and returns
/// before emitting Event::Key for the pressed key. Handle that real native
/// path explicitly, resolving the user's current mapping from the chord.
fn consume_native_clipboard_shortcuts(
    input: &mut egui::InputState,
    settings: &HotkeySettings,
) -> (Vec<ShortcutCommand>, bool) {
    let modifiers = input.modifiers;
    let mut commands = Vec::new();
    let mut consumed_native_event = false;
    input.events.retain(|event| {
        let key = match event {
            egui::Event::Cut => Some(Key::X),
            egui::Event::Copy => Some(Key::C),
            egui::Event::Paste(_) => Some(Key::V),
            _ => None,
        };
        if let Some(key) = key {
            consumed_native_event = true;
            if let Some(chord) = ShortcutChord::from_modifiers(key, modifiers) {
                if let Some(command) = settings.command_for(chord) {
                    commands.push(command);
                }
            }
            false
        } else {
            true
        }
    });
    (commands, consumed_native_event)
}

fn consume_shortcut(
    input: &mut egui::InputState,
    settings: &HotkeySettings,
    command: ShortcutCommand,
    typing: bool,
    skip_native_clipboard_keys: bool,
) -> bool {
    settings.shortcuts(command).into_iter().any(|shortcut| {
        if skip_native_clipboard_keys
            && matches!(
                shortcut.key,
                ShortcutKey::X | ShortcutKey::C | ShortcutKey::V
            )
        {
            return false;
        }
        let function_key = matches!(
            shortcut.key,
            ShortcutKey::F1
                | ShortcutKey::F2
                | ShortcutKey::F3
                | ShortcutKey::F4
                | ShortcutKey::F5
                | ShortcutKey::F6
                | ShortcutKey::F7
                | ShortcutKey::F8
                | ShortcutKey::F9
                | ShortcutKey::F10
                | ShortcutKey::F11
                | ShortcutKey::F12
        );
        if typing && !shortcut.command && !shortcut.alt && !function_key {
            return false;
        }
        let pressed = input.modifiers;
        let plus_key_allows_shift =
            matches!(shortcut.key, ShortcutKey::Plus | ShortcutKey::Equals) && !shortcut.shift;
        if pressed.command != shortcut.command
            || pressed.alt != shortcut.alt
            || (pressed.shift != shortcut.shift && !(plus_key_allows_shift && pressed.shift))
        {
            return false;
        }
        input.consume_key(shortcut.modifiers(), shortcut.key.to_egui())
    })
}

fn action_for_shortcut(command: ShortcutCommand) -> Option<Action> {
    use ShortcutCommand as C;
    Some(match command {
        C::NextTab => Action::CycleDocumentTab(1),
        C::PreviousTab => Action::CycleDocumentTab(-1),
        C::NewProject => Action::NewProject,
        C::OpenProject => Action::OpenProject,
        C::SaveProject => Action::SaveProject,
        C::SaveProjectAs => Action::SaveProjectAs,
        C::Export => Action::OpenQ0Enc,
        C::Undo => Action::Undo,
        C::Redo => Action::Redo,
        C::Cut => Action::CutSelection,
        C::Copy => Action::CopySelection,
        C::Paste => Action::Paste,
        C::PasteInPlace => Action::PasteInPlace,
        C::Duplicate => Action::DuplicateSelection,
        C::Delete => Action::DeleteSelection,
        C::ConvertToSymbol => Action::ConvertSelectionToQ0rg,
        C::BreakApart => Action::BreakApartSelection,
        C::FlipHorizontal => Action::FlipHorizontal,
        C::FlipVertical => Action::FlipVertical,
        C::BringForward => Action::BringForward,
        C::SendBackward => Action::SendBackward,
        C::ZoomIn => Action::ZoomIn,
        C::ZoomOut => Action::ZoomOut,
        C::ZoomReset => Action::ZoomReset,
        C::ScriptEditor => Action::OpenQ0langEditor(None),
        C::PlaybackToggle => Action::TogglePlay,
        C::ToggleTimeline => Action::ToggleTimeline,
        C::FirstFrame => Action::FirstFrame,
        C::LastFrame => Action::LastFrame,
        C::PreviousFrame => Action::PreviousFrame,
        C::NextFrame => Action::NextFrame,
        C::InsertFrame => Action::InsertFrame,
        C::RemoveFrame => Action::RemoveSelectedTimelineFrames,
        C::InsertKeyframe => Action::InsertKeyframe,
        C::ClearKeyframe => Action::ClearKeyframe,
        C::InsertBlankKeyframe => Action::InsertBlankKeyframe,
        C::MotionTween => Action::CreateMotionTween,
        C::ToolSelect => Action::SelectTool(crate::state::Tool::Select),
        C::ToolLasso => Action::SelectTool(crate::state::Tool::Lasso),
        C::ToolSubselect => Action::SelectTool(crate::state::Tool::Subselect),
        C::ToolHand => Action::SelectTool(crate::state::Tool::Hand),
        C::ToolPen => Action::SelectTool(crate::state::Tool::Pen),
        C::ToolPencil => Action::SelectTool(crate::state::Tool::Pencil),
        C::ToolBrush => Action::SelectTool(crate::state::Tool::Brush),
        C::ToolEraser => Action::SelectTool(crate::state::Tool::Eraser),
        C::ToolLine => Action::SelectTool(crate::state::Tool::Line),
        C::ToolRectangle => Action::SelectTool(crate::state::Tool::Rectangle),
        C::ToolOval => Action::SelectTool(crate::state::Tool::Oval),
        C::ToolBucket => Action::SelectTool(crate::state::Tool::Bucket),
        C::ToolEyedropper => Action::SelectTool(crate::state::Tool::Eyedropper),
        C::ToolText => Action::SelectTool(crate::state::Tool::Text),
        C::ToolRig => Action::SelectTool(crate::state::Tool::Rig),
        C::PerformanceSnapshot | C::ClearSelection => return None,
    })
}

fn dispatch_shortcut(
    app: &mut EditorApp,
    ctx: &egui::Context,
    command: ShortcutCommand,
    can_clip: bool,
    came_from_native_clipboard: bool,
) {
    match command {
        ShortcutCommand::PerformanceSnapshot => app.copy_performance_snapshot(ctx),
        ShortcutCommand::ClearSelection => {
            if matches!(app.session.tool_state, crate::state::ToolState::Idle) {
                app.session.selection = crate::state::Selection::None;
                app.session.timeline_selection = None;
                app.session.status = "selection cleared".to_string();
            }
        }
        _ => {
            if let Some(action) = action_for_shortcut(command) {
                if came_from_native_clipboard
                    && can_clip
                    && matches!(action, Action::CopySelection | Action::CutSelection)
                {
                    // egui-winit only emits Event::Paste when the system clipboard
                    // contains non-empty text. Keep a harmless marker there so an
                    // internal artwork copy/cut can always be pasted with Ctrl+V.
                    ctx.output_mut(|output| output.copied_text = "q0editor artwork".to_string());
                }
                app.queue(action);
            }
        }
    }
}

pub fn handle_home_shortcuts(app: &mut EditorApp, ctx: &egui::Context) {
    if app.session.shortcut_capture.is_some() {
        return;
    }
    let typing = ctx.wants_keyboard_input();
    let settings = &app.settings.hotkeys;
    let mut commands = Vec::new();
    ctx.input_mut(|input| {
        for command in [
            ShortcutCommand::PerformanceSnapshot,
            ShortcutCommand::NextTab,
            ShortcutCommand::PreviousTab,
            ShortcutCommand::NewProject,
            ShortcutCommand::OpenProject,
        ] {
            if consume_shortcut(input, settings, command, typing, false) {
                commands.push(command);
            }
        }
    });
    for command in commands {
        dispatch_shortcut(app, ctx, command, false, false);
    }
}

pub fn handle_global_shortcuts(app: &mut EditorApp, ctx: &egui::Context) {
    let typing = ctx.wants_keyboard_input();
    let code_document = app.active_document_is_q0lang();
    let can_clip = app.session.timeline_selection.is_some()
        || app.session.timeline_layer_selection.is_some()
        || crate::selection_edit::selection_can_clip(&app.session.selection);
    if app.session.shortcut_capture.is_some() {
        return;
    }
    let settings = &app.settings.hotkeys;
    let mut commands = Vec::new();
    let mut native_commands = Vec::new();
    let mut nudge = q0s_format::v2::Vec2::new(0.0, 0.0);
    let stage_selection = !typing
        && !code_document
        && !matches!(
            app.session.selection,
            crate::state::Selection::None
                | crate::state::Selection::Asset(_)
                | crate::state::Selection::Q0rg(_)
        )
        && app.session.timeline_selection.is_none()
        && app.session.timeline_layer_selection.is_none()
        && matches!(app.session.tool_state, crate::state::ToolState::Idle);
    ctx.input_mut(|i| {
        if stage_selection && !i.modifiers.command && !i.modifiers.alt {
            let step = if i.modifiers.shift { 0.1 } else { 1.0 };
            let modifiers = i.modifiers;
            for (key, x, y) in [
                (Key::ArrowLeft, -1.0, 0.0),
                (Key::ArrowRight, 1.0, 0.0),
                (Key::ArrowUp, 0.0, -1.0),
                (Key::ArrowDown, 0.0, 1.0),
            ] {
                if i.consume_key(modifiers, key) {
                    nudge.x += x * step;
                    nudge.y += y * step;
                }
            }
        }
        let (native, had_native_clipboard_event) = if !typing && !code_document {
            consume_native_clipboard_shortcuts(i, settings)
        } else {
            (Vec::new(), false)
        };
        commands.extend(native.iter().copied());
        // Retain which commands came through the native clipboard event so
        // internal artwork copy/cut can repopulate egui's text clipboard.
        native_commands = native;
        let mut test = |command| {
            if consume_shortcut(i, settings, command, typing, had_native_clipboard_event) {
                commands.push(command);
            }
        };

        test(ShortcutCommand::PerformanceSnapshot);
        test(ShortcutCommand::NextTab);
        test(ShortcutCommand::PreviousTab);
        test(ShortcutCommand::NewProject);
        test(ShortcutCommand::OpenProject);
        test(ShortcutCommand::SaveProjectAs);
        test(ShortcutCommand::SaveProject);
        if code_document {
            return;
        }

        if !typing {
            test(ShortcutCommand::Undo);
            test(ShortcutCommand::Redo);
            test(ShortcutCommand::ConvertToSymbol);
            test(ShortcutCommand::BreakApart);
            test(ShortcutCommand::FlipHorizontal);
            test(ShortcutCommand::FlipVertical);
            test(ShortcutCommand::BringForward);
            test(ShortcutCommand::SendBackward);
            test(ShortcutCommand::Cut);
            test(ShortcutCommand::Copy);
            test(ShortcutCommand::Paste);
            test(ShortcutCommand::PasteInPlace);
            test(ShortcutCommand::Duplicate);
            test(ShortcutCommand::Delete);
        }
        test(ShortcutCommand::Export);
        test(ShortcutCommand::ZoomIn);
        test(ShortcutCommand::ZoomOut);
        test(ShortcutCommand::ZoomReset);
        if !typing {
            test(ShortcutCommand::PlaybackToggle);
            test(ShortcutCommand::ToggleTimeline);
            test(ShortcutCommand::FirstFrame);
            test(ShortcutCommand::LastFrame);
            test(ShortcutCommand::PreviousFrame);
            test(ShortcutCommand::NextFrame);
            test(ShortcutCommand::ClearSelection);
        }
        test(ShortcutCommand::InsertFrame);
        test(ShortcutCommand::RemoveFrame);
        test(ShortcutCommand::InsertKeyframe);
        test(ShortcutCommand::ClearKeyframe);
        test(ShortcutCommand::InsertBlankKeyframe);
        test(ShortcutCommand::ScriptEditor);
        test(ShortcutCommand::MotionTween);
        if !typing {
            for command in [
                ShortcutCommand::ToolSelect,
                ShortcutCommand::ToolLasso,
                ShortcutCommand::ToolSubselect,
                ShortcutCommand::ToolHand,
                ShortcutCommand::ToolPen,
                ShortcutCommand::ToolPencil,
                ShortcutCommand::ToolBrush,
                ShortcutCommand::ToolEraser,
                ShortcutCommand::ToolLine,
                ShortcutCommand::ToolRectangle,
                ShortcutCommand::ToolOval,
                ShortcutCommand::ToolBucket,
                ShortcutCommand::ToolEyedropper,
                ShortcutCommand::ToolText,
                ShortcutCommand::ToolRig,
            ] {
                test(command);
            }
        }
    });
    if nudge.x != 0.0 || nudge.y != 0.0 {
        crate::tools::nudge_selection(app, nudge);
    }
    for command in commands {
        dispatch_shortcut(
            app,
            ctx,
            command,
            can_clip,
            native_commands.contains(&command),
        );
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;
    use egui::Modifiers;
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
