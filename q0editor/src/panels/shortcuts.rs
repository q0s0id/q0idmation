use egui::{Context, Key, RichText, Ui};

use crate::app::EditorApp;
use crate::hotkeys::{ShortcutCapture, ShortcutChord, ShortcutCommand, ShortcutKey};
use crate::ui::{FitToScreen, Icon};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Timeline,
    Tools,
    Edit,
    Project,
    View,
}

impl Category {
    const ALL: [Self; 5] = [
        Self::Timeline,
        Self::Tools,
        Self::Edit,
        Self::Project,
        Self::View,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Timeline => "Timeline",
            Self::Tools => "Tools",
            Self::Edit => "Edit",
            Self::Project => "Project",
            Self::View => "View",
        }
    }

    fn of(command: ShortcutCommand) -> Self {
        use ShortcutCommand as C;
        match command {
            C::PlaybackToggle
            | C::ToggleTimeline
            | C::FirstFrame
            | C::LastFrame
            | C::PreviousFrame
            | C::NextFrame
            | C::InsertFrame
            | C::RemoveFrame
            | C::InsertKeyframe
            | C::ClearKeyframe
            | C::InsertBlankKeyframe
            | C::MotionTween
            | C::ShapeTween => Self::Timeline,
            C::ToolSelect
            | C::ToolLasso
            | C::ToolSubselect
            | C::ToolHand
            | C::ToolPen
            | C::ToolPencil
            | C::ToolBrush
            | C::ToolEraser
            | C::ToolKnife
            | C::ToolLine
            | C::ToolRectangle
            | C::ToolOval
            | C::ToolBucket
            | C::ToolEyedropper
            | C::ToolText
            | C::ToolRig
            | C::ToggleSnap => Self::Tools,
            C::Undo
            | C::Redo
            | C::Cut
            | C::Copy
            | C::Paste
            | C::PasteInPlace
            | C::Duplicate
            | C::Delete
            | C::ConvertToSymbol
            | C::BreakApart
            | C::FlipHorizontal
            | C::FlipVertical
            | C::BringForward
            | C::SendBackward
            | C::ClearSelection => Self::Edit,
            C::NextTab
            | C::PreviousTab
            | C::NewProject
            | C::OpenProject
            | C::SaveProject
            | C::SaveProjectAs
            | C::Export
            | C::FastPreview
            | C::FastPreviewScene => Self::Project,
            C::ZoomIn
            | C::ZoomOut
            | C::ZoomReset
            | C::ScriptEditor
            | C::OpenChat
            | C::StageOnly
            | C::Fullscreen
            | C::PerformanceSnapshot => Self::View,
        }
    }
}

#[derive(Clone, Default)]
struct Filter {
    search: String,
    category: Option<Category>,
}

pub(crate) fn render(app: &mut EditorApp, ctx: &Context) -> bool {
    let mut changed = false;
    let filter_id = egui::Id::new("shortcut_filter");
    let mut filter = ctx
        .data(|d| d.get_temp::<Filter>(filter_id))
        .unwrap_or_default();
    let mut open = true;
    egui::Window::new("Keyboard shortcuts")
        .open(&mut open)
        .default_size([640.0, 510.0])
        .default_pos(ctx.screen_rect().center() - egui::vec2(320.0, 255.0))
        .collapsible(false)
        .fit_to_screen(ctx)
        .show(ctx, |ui| {
            ui.add_enabled_ui(app.session.shortcut_capture.is_none(), |ui| {
                crate::ui::search_field(ui, &mut filter.search, "Search commands or keys");
            });
            ui.horizontal_wrapped(|ui| {
                if ui
                    .selectable_label(filter.category.is_none(), "All")
                    .clicked()
                {
                    filter.category = None;
                }
                for category in Category::ALL {
                    if ui
                        .selectable_label(filter.category == Some(category), category.label())
                        .clicked()
                    {
                        filter.category = Some(category);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !app.settings.hotkeys.overrides.is_empty(),
                            egui::Button::new("Reset all"),
                        )
                        .on_hover_text("Restore every default shortcut")
                        .clicked()
                    {
                        app.settings.hotkeys.reset();
                        app.session.shortcut_capture = None;
                        app.session.shortcut_feedback = Some("Default shortcuts restored".into());
                        changed = true;
                    }
                });
            });
            if let Some(capture) = app.session.shortcut_capture {
                ui.horizontal_wrapped(|ui| {
                    let prompt =
                        app.session.shortcut_feedback.clone().unwrap_or_else(|| {
                            format!("Press keys for {}", capture.command.label())
                        });
                    ui.colored_label(app.settings.theme.accent.to_color32(), prompt);
                    if ui.small_button("Cancel").on_hover_text("Esc").clicked() {
                        app.session.shortcut_capture = None;
                        app.session.shortcut_feedback = None;
                    }
                });
            } else if let Some(feedback) = &app.session.shortcut_feedback {
                ui.weak(feedback);
            }
            ui.separator();
            let query = filter.search.trim().to_lowercase();
            egui::ScrollArea::vertical()
                .id_source((
                    "shortcut_commands",
                    filter.category.map(|category| category as u8),
                    &query,
                ))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut any = false;
                    for category in Category::ALL {
                        if filter.category.is_some_and(|selected| selected != category) {
                            continue;
                        }
                        let mut first = true;
                        for &command in ShortcutCommand::ALL {
                            if Category::of(command) != category {
                                continue;
                            }
                            let chords = app.settings.hotkeys.shortcuts(command);
                            if !query.is_empty()
                                && !command.label().to_lowercase().contains(&query)
                                && !chords
                                    .iter()
                                    .any(|chord| chord.display().to_lowercase().contains(&query))
                            {
                                continue;
                            }
                            if first && filter.category.is_none() {
                                if any {
                                    ui.add_space(10.0);
                                }
                                ui.weak(RichText::new(category.label()).strong());
                            }
                            first = false;
                            any = true;
                            changed |= ui
                                .push_id(command as u8, |ui| command_row(app, ui, command, &chords))
                                .inner;
                        }
                    }
                    if !any {
                        ui.weak("No matching shortcuts");
                    }
                });
        });
    app.session.show_shortcuts = open;
    if open {
        ctx.data_mut(|d| d.insert_temp(filter_id, filter));
    } else {
        app.session.shortcut_capture = None;
        app.session.shortcut_feedback = None;
        ctx.data_mut(|d| d.remove::<Filter>(filter_id));
    }
    changed
}

fn command_row(
    app: &mut EditorApp,
    ui: &mut Ui,
    command: ShortcutCommand,
    chords: &[ShortcutChord],
) -> bool {
    let mut changed = false;
    let name_width = (ui.available_width() * 0.44).clamp(102.0, 270.0);
    ui.horizontal_top(|ui| {
        ui.add_sized(
            [name_width, 22.0],
            egui::Label::new(command.label()).truncate(true),
        )
        .on_hover_text(command.label());
        ui.vertical(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                if chords.is_empty() {
                    ui.weak("Unassigned");
                }
                for &chord in chords {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        let label = chord.display();
                        let editing = app.session.shortcut_capture
                            == Some(ShortcutCapture {
                                command,
                                replace: Some(chord),
                            });
                        if ui
                            .add(
                                egui::Button::new(RichText::new(&label).monospace())
                                    .small()
                                    .selected(editing),
                            )
                            .on_hover_text(format!("Change {label}"))
                            .clicked()
                        {
                            start_capture(app, ui, command, Some(chord));
                        }
                        if crate::ui::sized_icon_button(
                            ui,
                            Icon::Close,
                            &format!("Remove {label}"),
                            17.0,
                        )
                        .clicked()
                        {
                            app.settings.hotkeys.remove(command, chord);
                            app.session.shortcut_capture = None;
                            app.session.shortcut_feedback =
                                Some(format!("Removed {label} from {}", command.label()));
                            changed = true;
                        }
                    });
                }
                if crate::ui::sized_icon_button(ui, Icon::Add, "Add shortcut", 20.0).clicked() {
                    start_capture(app, ui, command, None);
                }
                if !app.settings.hotkeys.is_default(command)
                    && ui
                        .small_button("Reset")
                        .on_hover_text("Restore this command’s default shortcuts")
                        .clicked()
                {
                    app.settings.hotkeys.reset_command(command);
                    app.session.shortcut_capture = None;
                    app.session.shortcut_feedback = Some(format!(
                        "{} · {}",
                        command.label(),
                        app.settings.hotkeys.display(command)
                    ));
                    changed = true;
                }
            });
        });
    });
    ui.separator();
    changed
}

fn start_capture(
    app: &mut EditorApp,
    ui: &Ui,
    command: ShortcutCommand,
    replace: Option<ShortcutChord>,
) {
    app.session.shortcut_capture = Some(ShortcutCapture { command, replace });
    app.session.shortcut_feedback = None;
    ui.memory_mut(|m| {
        if let Some(focused) = m.focused() {
            m.surrender_focus(focused);
        }
    });
}

pub(crate) fn capture_key(app: &mut EditorApp, ctx: &Context) -> bool {
    let Some(capture) = app.session.shortcut_capture else {
        return false;
    };
    let pressed = ctx.input_mut(|input| {
        let pressed = input.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => Some((*key, *modifiers)),
            egui::Event::Cut => Some((Key::X, input.modifiers)),
            egui::Event::Copy => Some((Key::C, input.modifiers)),
            egui::Event::Paste(_) => Some((Key::V, input.modifiers)),
            _ => None,
        });
        if pressed.is_some() {
            input.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Cut
                        | egui::Event::Copy
                        | egui::Event::Paste(_)
                )
            });
        }
        pressed
    });
    let Some((key, modifiers)) = pressed else {
        return false;
    };
    let Some(chord) = ShortcutChord::from_modifiers(key, modifiers) else {
        app.session.shortcut_feedback = Some("That key cannot be assigned".into());
        return false;
    };
    app.session.shortcut_capture = None;
    if chord == ShortcutChord::key(ShortcutKey::Escape) {
        app.session.shortcut_feedback = None;
        return false;
    }
    if let Some(old) = capture.replace {
        if old == chord {
            return false;
        }
        app.settings.hotkeys.remove(capture.command, old);
    }
    let moved = app.settings.hotkeys.add(capture.command, chord);
    let mut feedback = format!("{} · {}", capture.command.label(), chord.display());
    if !moved.is_empty() {
        let from = moved
            .iter()
            .map(|item| item.label())
            .collect::<Vec<_>>()
            .join(", ");
        feedback.push_str(&format!(" · moved from {from}"));
    }
    app.session.shortcut_feedback = Some(feedback);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_window_shows_and_rebinds_frame_keys_at_minimum_size() {
        let ctx = Context::default();
        let mut app = EditorApp::default();
        app.session.show_shortcuts = true;
        crate::theme::install(&ctx);
        app.settings.theme.apply(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::ui::MIN_SCREEN_SIZE);
        let frame = |app: &mut EditorApp, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    capture_key(app, ctx);
                    render(app, ctx);
                },
            )
        };
        let visible_label = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().find_map(|painted| {
                let egui::Shape::Text(text) = &painted.shape else {
                    return None;
                };
                let point = text.visual_bounding_rect().center();
                (text.galley.job.text == label
                    && painted.clip_rect.contains(point)
                    && screen.contains(point))
                .then_some(point)
            })
        };

        frame(&mut app, vec![]);
        let output = frame(&mut app, vec![]);
        for label in ["Previous frame", "Next frame", ">", "Reset all"] {
            assert!(visible_label(&output, label).is_some(), "missing {label:?}");
        }
        let pos = visible_label(&output, "<").expect("previous frame keycap");
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(
            app.session.shortcut_capture,
            Some(ShortcutCapture {
                command: ShortcutCommand::PreviousFrame,
                replace: Some(ShortcutChord::shift(ShortcutKey::Comma)),
            })
        );
        frame(
            &mut app,
            vec![egui::Event::Key {
                key: Key::F2,
                physical_key: Some(Key::F2),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(
            app.settings
                .hotkeys
                .shortcuts(ShortcutCommand::PreviousFrame),
            vec![
                ShortcutChord::key(ShortcutKey::Comma),
                ShortcutChord::key(ShortcutKey::F2),
            ]
        );
        assert!(app.session.shortcut_capture.is_none());

        ctx.data_mut(|d| {
            d.insert_temp(
                egui::Id::new("shortcut_filter"),
                Filter {
                    search: "next frame".into(),
                    category: None,
                },
            )
        });
        let output = frame(&mut app, vec![]);
        assert!(visible_label(&output, "Next frame").is_some());
        assert!(visible_label(&output, "Previous frame").is_none());
    }
}
