//! q0lang script editor window.
//!
//! Replaces the 4-row TextEdit that used to live in the Properties panel.
//! Three jobs:
//!   * a real text-editing surface (line numbers, current-line highlight,
//!     auto-indent, tab-as-spaces),
//!   * syntax highlighting via `super::syntax::layout_line`,
//!   * a font picker (default `Consolas`, anything from
//!     `super::fonts::available_fonts()`),
//!   * everything reads colours from `Settings::theme` so swapping presets
//!     in the Settings dialog instantly recolours the script editor too.
//!
//! Auto-indent is implemented as a pre-pass: we sniff `Enter` *before* the
//! TextEdit sees it, compute the new line's indent (matching the current
//! line, plus tab_width if the line ends in `{`), and rewrite the buffer
//! + cursor ourselves. This keeps the editor layout stable in egui 0.27; the
//!   widget doesn't expose an `on_enter` hook.

use egui::text::{CCursor, CCursorRange};
use egui::widgets::text_edit::TextEditState;
use egui::{Context, FontFamily, FontId, Key, Modifiers, Sense, Stroke, TextEdit, Vec2};

use crate::app::EditorApp;
use crate::settings::PersistFontSize;

const EDITOR_ANALYSIS_INSTRUCTION_BUDGET: usize = 4_096;

/// Render the script editor window.  No-op when `session.show_q0lang_editor`
/// is false, so the caller can unconditionally invoke it from `App::update`.
pub fn render(app: &mut EditorApp, ctx: &Context) {
    if !app.session.show_q0lang_editor {
        return;
    }
    let q0rg_id = match app.session.q0lang_target {
        Some(id) => id,
        None => app.session.current_q0rg_id,
    };
    // Pull the q0rg name + script text out so we can build the title and
    // pass `&mut script` to the TextEdit. Bail with a small "missing"
    // window if the target was deleted while open.
    let (q0rg_name, mut owned_script) = match app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
    {
        Some(q) => (q.name.clone(), q.script.clone()),
        None => {
            // Target gone — close ourselves and bail.
            app.session.show_q0lang_editor = false;
            return;
        }
    };

    let mut close_requested = false;
    let mut open = true;
    let display_q0rg_name = truncate_label(&q0rg_name, 40);
    let title = format!("q0lang - {display_q0rg_name}");
    let theme = app.settings.theme.clone();
    let font_id = FontId::new(
        app.settings.q0lang_font_size.0.clamp(8.0, 72.0),
        FontFamily::Name(super::fonts::Q0LANG_FAMILY.into()),
    );
    let tab_width = app.settings.q0lang_tab_width.max(1) as usize;
    let show_lines = app.settings.q0lang_show_line_numbers;
    let parsed = super::parser::parse(&owned_script);
    let mut runtime = super::runtime::Runtime::new();
    let runtime_report =
        runtime.execute_program_with_budget(&parsed, EDITOR_ANALYSIS_INSTRUCTION_BUDGET);
    let diagnostics = runtime_report.diagnostics.clone();
    let statement_count = parsed.statements.len();

    let mut new_font_name: Option<String> = None;
    let mut new_font_size: Option<f32> = None;
    let mut new_tab_width: Option<u8> = None;
    let mut new_show_lines: Option<bool> = None;

    // The TextEdit gets a stable id so we can reach into its state from
    // the auto-indent / smart-tab pre-passes below.
    let text_edit_id = egui::Id::new(("q0lang_text_edit", q0rg_id));

    // -----------------------------------------------------------------
    // Auto-indent + smart-tab pre-pass.  Only run when the editor is
    // focused; otherwise the user might be typing in a regular text
    // box (the font picker, etc.) and we'd hijack their keys.
    // -----------------------------------------------------------------
    let focused = ctx.memory(|m| m.has_focus(text_edit_id));
    if focused {
        // Enter (no shift) → auto-indent insert.
        let want_enter = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        // Shift+Enter falls through to the TextEdit and inserts a plain
        // newline — we don't consume it so the widget's default kicks in.
        let want_tab = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab));
        let want_shift_tab = ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Tab));

        if want_enter {
            apply_auto_indent(ctx, text_edit_id, &mut owned_script, tab_width);
        } else if want_tab {
            insert_tab(ctx, text_edit_id, &mut owned_script, tab_width);
        } else if want_shift_tab {
            outdent(ctx, text_edit_id, &mut owned_script, tab_width);
        }
    }

    // Captured from the TextEdit's Response inside the Window closure so
    // we can decide outside whether to (a) snapshot history (only on
    // `gained_focus`, not on every keystroke — that would explode the
    // undo stack) and (b) request a repaint after the buffer changes.
    let mut text_gained_focus = false;

    egui::Window::new(title)
        .id(egui::Id::new("q0lang_editor_window"))
        .open(&mut open)
        .default_size([920.0, 620.0])
        .min_width(560.0)
        .min_height(360.0)
        .resizable(true)
        .collapsible(true)
        .show(ctx, |ui| {
            // ------ top toolbar: font picker, size, tab width, gutter toggle ------
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Font")
                        .color(theme.text_dim.to_color32())
                        .small(),
                );
                let current_font = app.settings.q0lang_font_name.clone();
                egui::ComboBox::from_id_source("q0lang_font_picker")
                    .width(160.0)
                    .selected_text(&current_font)
                    .show_ui(ui, |ui| {
                        for name in super::fonts::available_fonts(&current_font) {
                            if ui
                                .selectable_label(name.eq_ignore_ascii_case(&current_font), &name)
                                .clicked()
                                && !name.eq_ignore_ascii_case(&current_font)
                            {
                                new_font_name = Some(name);
                            }
                        }
                    });

                ui.separator();
                ui.label(
                    egui::RichText::new("Size")
                        .color(theme.text_dim.to_color32())
                        .small(),
                );
                let mut size = app.settings.q0lang_font_size.0;
                if ui
                    .add(
                        egui::DragValue::new(&mut size)
                            .speed(0.5)
                            .clamp_range(8.0..=72.0),
                    )
                    .changed()
                {
                    new_font_size = Some(size);
                }

                ui.separator();
                ui.label(
                    egui::RichText::new("Tab")
                        .color(theme.text_dim.to_color32())
                        .small(),
                );
                let mut tw = app.settings.q0lang_tab_width as i32;
                if ui
                    .add(egui::DragValue::new(&mut tw).clamp_range(1..=12))
                    .changed()
                {
                    new_tab_width = Some(tw.clamp(1, 12) as u8);
                }

                ui.separator();
                let mut sl = show_lines;
                if ui.checkbox(&mut sl, "Line numbers").changed() {
                    new_show_lines = Some(sl);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Close")
                        .on_hover_text("Hide the editor (the script stays in the project)")
                        .clicked()
                    {
                        close_requested = true;
                    }
                });
            });
            ui.separator();

            // ------ main body: gutter + text edit, both inside one ScrollArea
            // so vertical / horizontal scroll stays in lockstep.
            let line_count = owned_script.lines().count().max(1)
                + if owned_script.ends_with('\n') { 1 } else { 0 };
            // Layouter for the TextEdit — closure borrows `theme` and `font_id`.
            let theme_for_layouter = theme.clone();
            let font_for_layouter = font_id.clone();
            let mut layouter = move |ui: &egui::Ui, text: &str, wrap_width: f32| {
                let job = super::syntax::layout_line(
                    text,
                    &theme_for_layouter,
                    font_for_layouter.clone(),
                    wrap_width,
                );
                ui.fonts(|f| f.layout_job(job))
            };

            // Approximate row height — used to draw the line-number column
            // so each number sits next to its row.  We pull it from the
            // font metrics on the current ctx.
            let row_height = ctx.fonts(|f| f.row_height(&font_id));

            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        if show_lines {
                            // ----- Gutter: per-row line numbers, right-aligned.
                            let max_label = line_count.to_string().len().max(2);
                            let gutter_char_w = ctx.fonts(|f| f.glyph_width(&font_id, '0'));
                            let gutter_width = gutter_char_w * (max_label as f32) + 14.0;
                            let total_height = row_height * line_count as f32 + 4.0;
                            let (rect, _resp) = ui.allocate_exact_size(
                                Vec2::new(gutter_width, total_height),
                                Sense::hover(),
                            );
                            let painter = ui.painter_at(rect);
                            painter.rect_filled(rect, 0.0, theme.syntax_gutter_bg.to_color32());
                            let right_pad = 6.0;
                            for n in 1..=line_count {
                                let y = rect.top() + 2.0 + (n - 1) as f32 * row_height;
                                let pos = egui::pos2(rect.right() - right_pad, y);
                                painter.text(
                                    pos,
                                    egui::Align2::RIGHT_TOP,
                                    n.to_string(),
                                    font_id.clone(),
                                    theme.syntax_gutter_fg.to_color32(),
                                );
                            }
                            // Faint vertical separator between gutter and text.
                            painter.line_segment(
                                [
                                    egui::pos2(rect.right() - 0.5, rect.top()),
                                    egui::pos2(rect.right() - 0.5, rect.bottom()),
                                ],
                                Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
                            );
                            ui.add_space(4.0);
                        }

                        // ----- The actual TextEdit (custom layouter).
                        let avail = ui.available_width().max(200.0);
                        let edit = TextEdit::multiline(&mut owned_script)
                            .id(text_edit_id)
                            .desired_rows(line_count.max(20))
                            .desired_width(avail)
                            .frame(false)
                            .layouter(&mut layouter);
                        let resp = ui.add(edit);
                        if resp.gained_focus() {
                            text_gained_focus = true;
                        }
                        if resp.changed() {
                            // Force a repaint so the gutter line count tracks
                            // the buffer immediately after typing.
                            ctx.request_repaint();
                        }
                    });
                });

            ui.separator();
            if !diagnostics.is_empty() {
                egui::CollapsingHeader::new(format!("Issues ({})", diagnostics.len()))
                    .default_open(true)
                    .show(ui, |ui| {
                        for d in &diagnostics {
                            ui.label(
                                egui::RichText::new(format!("line {}: {}", d.line, d.message))
                                    .color(theme.syntax_signal.to_color32())
                                    .small(),
                            );
                        }
                    });
                ui.separator();
            }
            egui::CollapsingHeader::new("Script summary")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} action{}, {} variable{}, {} event listener{}",
                            runtime_report.actions.len(),
                            if runtime_report.actions.len() == 1 {
                                ""
                            } else {
                                "s"
                            },
                            runtime.vars.len(),
                            if runtime.vars.len() == 1 { "" } else { "s" },
                            runtime.listeners.len(),
                            if runtime.listeners.len() == 1 {
                                ""
                            } else {
                                "s"
                            },
                        ))
                        .color(theme.text_dim.to_color32())
                        .small(),
                    );
                    for action in &runtime_report.actions {
                        ui.label(
                            egui::RichText::new(runtime_action_label(action))
                                .color(theme.syntax_signal.to_color32())
                                .small(),
                        );
                    }
                });
            ui.separator();
            egui::CollapsingHeader::new("Built-in libraries")
                .default_open(false)
                .show(ui, |ui| {
                    for lib in super::parser::BUILTIN_LIBRARIES {
                        ui.label(egui::RichText::new(lib.name).strong());
                        ui.label(
                            egui::RichText::new(lib.symbols.join("  "))
                                .color(theme.text_dim.to_color32())
                                .small(),
                        );
                        ui.add_space(4.0);
                    }
                });
            ui.separator();
            // ------ status bar: line / col / file stats + auto-indent hint.
            ui.horizontal(|ui| {
                let cursor_range =
                    TextEditState::load(ctx, text_edit_id).and_then(|s| s.cursor.char_range());
                let (ln, col) = match cursor_range {
                    Some(r) => char_to_line_col(&owned_script, r.primary.index),
                    None => (1, 1),
                };
                let chars = owned_script.chars().count();
                let lines = owned_script.lines().count().max(1);
                ui.label(
                    egui::RichText::new(format!(
                        "Ln {ln}, Col {col} | {lines} line{}, {chars} char{} | q0lang: {}",
                        if lines == 1 { "" } else { "s" },
                        if chars == 1 { "" } else { "s" },
                        if diagnostics.is_empty() {
                            format!(
                                "ok ({statement_count} statement{})",
                                if statement_count == 1 { "" } else { "s" }
                            )
                        } else {
                            format!(
                                "{} issue{}",
                                diagnostics.len(),
                                if diagnostics.len() == 1 { "" } else { "s" }
                            )
                        },
                    ))
                    .color(theme.text_dim.to_color32())
                    .small(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(
                            "Enter: auto-indent | Shift+Enter: plain newline | Tab: spaces",
                        )
                        .color(theme.text_dim.to_color32())
                        .small(),
                    );
                });
            });
        });

    // ------------------------------------------------------------
    // Apply edits back into the project & settings, *outside* the
    // window closure so we're not holding any borrows on `app`.
    // ------------------------------------------------------------
    // History rule: snapshot ONCE per editing session, on the first
    // keystroke after the TextEdit gains focus. Subsequent keystrokes
    // mutate `q.script` without snapshotting — undo collapses to "the
    // text the way it was when the user clicked into the editor".
    let needs_write = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .map(|q| q.script != owned_script)
        .unwrap_or(false);
    // Re-arm the snapshot trigger whenever the editor gets focus — that
    // marks the start of a fresh edit session (user clicked back in
    // after navigating away).
    if text_gained_focus {
        app.session.q0lang_edit_armed = true;
    }
    if needs_write {
        if app.session.q0lang_edit_armed {
            let before = app.state.project.clone();
            app.history.snapshot(&before);
            app.session.q0lang_edit_armed = false;
        }
        if let Some(q) = app
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
        {
            q.script = owned_script;
            app.state.mark_dirty();
            app.session.reset_q0lang_preview();
        }
    }

    if let Some(name) = new_font_name {
        let installed = super::fonts::install(ctx, &name);
        app.settings.q0lang_font_name = name.clone();
        app.settings.save();
        app.session.status = if installed {
            format!("font: {name}")
        } else {
            format!("font {name} not found; using default")
        };
    }
    if let Some(size) = new_font_size {
        app.settings.q0lang_font_size = PersistFontSize(size);
        app.settings.save();
    }
    if let Some(tw) = new_tab_width {
        app.settings.q0lang_tab_width = tw;
        app.settings.save();
    }
    if let Some(sl) = new_show_lines {
        app.settings.q0lang_show_line_numbers = sl;
        app.settings.save();
    }

    if !open || close_requested {
        app.session.show_q0lang_editor = false;
    }
}

/// Full-document q0lang editor used by `.q0l` / `.q0lang` tabs.
/// It deliberately shares the same lexer/parser/theme as embedded q0rg scripts,
/// so standalone files never drift into a second dialect.
pub fn render_standalone(app: &mut EditorApp, ui: &mut egui::Ui) {
    let theme = app.settings.theme.clone();
    let font_id = FontId::new(
        app.settings.q0lang_font_size.0.clamp(8.0, 72.0),
        FontFamily::Name(super::fonts::Q0LANG_FAMILY.into()),
    );
    let tab_width = app.settings.q0lang_tab_width.max(1) as usize;
    let show_lines = app.settings.q0lang_show_line_numbers;
    let mut status: Option<String> = None;

    let Some(document) = app.active_q0lang_document_mut() else {
        ui.centered_and_justified(|ui| ui.label("no q0lang document"));
        return;
    };
    let text_edit_id = egui::Id::new(("q0lang_document_text", document.path.clone()));
    let focused = ui.ctx().memory(|memory| memory.has_focus(text_edit_id));

    if focused {
        let (find, replace, go_line, duplicate, toggle_comment, move_up, move_down) =
            ui.ctx().input_mut(|input| {
                (
                    input.consume_key(Modifiers::COMMAND, Key::F),
                    input.consume_key(Modifiers::COMMAND, Key::H),
                    input.consume_key(Modifiers::COMMAND, Key::G),
                    input.consume_key(Modifiers::COMMAND, Key::D),
                    input.consume_key(Modifiers::COMMAND, Key::Slash),
                    input.consume_key(Modifiers::ALT, Key::ArrowUp),
                    input.consume_key(Modifiers::ALT, Key::ArrowDown),
                )
            });
        if find {
            document.show_find = true;
            document.show_replace = false;
        }
        if replace {
            document.show_find = true;
            document.show_replace = true;
        }
        if go_line {
            document.show_find = true;
            document.search.clear();
            status = Some("go to line: type a line number in find and press enter".to_string());
        }
        if duplicate && duplicate_current_line(ui.ctx(), text_edit_id, &mut document.text) {
            document.dirty = true;
        }
        if toggle_comment
            && toggle_comment_selected_lines(ui.ctx(), text_edit_id, &mut document.text)
        {
            document.dirty = true;
        }
        if move_up && move_current_line(ui.ctx(), text_edit_id, &mut document.text, -1) {
            document.dirty = true;
        }
        if move_down && move_current_line(ui.ctx(), text_edit_id, &mut document.text, 1) {
            document.dirty = true;
        }

        let want_enter = ui
            .ctx()
            .input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
        let want_tab = ui
            .ctx()
            .input_mut(|input| input.consume_key(Modifiers::NONE, Key::Tab));
        let want_shift_tab = ui
            .ctx()
            .input_mut(|input| input.consume_key(Modifiers::SHIFT, Key::Tab));
        if want_enter {
            apply_auto_indent(ui.ctx(), text_edit_id, &mut document.text, tab_width);
            document.dirty = true;
        } else if want_tab {
            insert_tab(ui.ctx(), text_edit_id, &mut document.text, tab_width);
            document.dirty = true;
        } else if want_shift_tab {
            let before = document.text.clone();
            outdent(ui.ctx(), text_edit_id, &mut document.text, tab_width);
            document.dirty |= before != document.text;
        }
    }

    let parsed = super::parser::parse(&document.text);
    let diagnostics = parsed.diagnostics.clone();
    let path_text = document.path.display().to_string();

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("q0lang").strong());
        ui.separator();
        ui.label(
            egui::RichText::new(path_text)
                .color(theme.text_dim.to_color32())
                .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let state = if diagnostics.is_empty() {
                "no issues".to_string()
            } else {
                format!(
                    "{} issue{}",
                    diagnostics.len(),
                    if diagnostics.len() == 1 { "" } else { "s" }
                )
            };
            ui.label(
                egui::RichText::new(state)
                    .color(if diagnostics.is_empty() {
                        theme.text_dim.to_color32()
                    } else {
                        theme.syntax_signal.to_color32()
                    })
                    .small(),
            );
        });
    });

    if document.show_find {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("find");
            let find_response = ui.add_sized(
                [220.0, 22.0],
                TextEdit::singleline(&mut document.search).hint_text("text or line number"),
            );
            if find_response.gained_focus() {
                find_response.request_focus();
            }
            let enter =
                find_response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            if ui.small_button("prev").clicked()
                && select_match(
                    ui.ctx(),
                    text_edit_id,
                    &document.text,
                    &document.search,
                    false,
                )
            {
                status = Some(format!("found `{}`", document.search));
            }
            if ui.small_button("next").clicked() || enter {
                if let Ok(line) = document.search.trim().parse::<usize>() {
                    if select_line(ui.ctx(), text_edit_id, &document.text, line) {
                        status = Some(format!("line {line}"));
                    }
                } else if select_match(
                    ui.ctx(),
                    text_edit_id,
                    &document.text,
                    &document.search,
                    true,
                ) {
                    status = Some(format!("found `{}`", document.search));
                }
            }
            if document.show_replace {
                ui.separator();
                ui.label("replace");
                ui.add_sized([180.0, 22.0], TextEdit::singleline(&mut document.replace));
                if ui.small_button("all").clicked() && !document.search.is_empty() {
                    let count = document.text.matches(&document.search).count();
                    if count > 0 {
                        document.text = document.text.replace(&document.search, &document.replace);
                        document.dirty = true;
                    }
                    status = Some(format!(
                        "replaced {count} match{}",
                        if count == 1 { "" } else { "es" }
                    ));
                }
            }
            if ui.small_button("x").clicked() {
                document.show_find = false;
                document.show_replace = false;
            }
        });
    }

    ui.separator();
    let line_count =
        document.text.lines().count().max(1) + usize::from(document.text.ends_with('\n'));
    let theme_for_layouter = theme.clone();
    let font_for_layouter = font_id.clone();
    let mut layouter = move |ui: &egui::Ui, text: &str, wrap_width: f32| {
        let job = super::syntax::layout_line(
            text,
            &theme_for_layouter,
            font_for_layouter.clone(),
            wrap_width,
        );
        ui.fonts(|fonts| fonts.layout_job(job))
    };
    let row_height = ui.ctx().fonts(|fonts| fonts.row_height(&font_id));

    egui::ScrollArea::both()
        .id_source(("q0lang_document_scroll", &document.path))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                if show_lines {
                    let max_label = line_count.to_string().len().max(2);
                    let gutter_char_w = ui.ctx().fonts(|fonts| fonts.glyph_width(&font_id, '0'));
                    let gutter_width = gutter_char_w * max_label as f32 + 14.0;
                    let total_height = row_height * line_count as f32 + 4.0;
                    let (rect, _) = ui
                        .allocate_exact_size(Vec2::new(gutter_width, total_height), Sense::hover());
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, 0.0, theme.syntax_gutter_bg.to_color32());
                    for n in 1..=line_count {
                        painter.text(
                            egui::pos2(
                                rect.right() - 6.0,
                                rect.top() + 2.0 + (n - 1) as f32 * row_height,
                            ),
                            egui::Align2::RIGHT_TOP,
                            n.to_string(),
                            font_id.clone(),
                            theme.syntax_gutter_fg.to_color32(),
                        );
                    }
                    painter.line_segment(
                        [
                            egui::pos2(rect.right() - 0.5, rect.top()),
                            egui::pos2(rect.right() - 0.5, rect.bottom()),
                        ],
                        Stroke::new(1.0_f32, theme.stroke_dark.to_color32()),
                    );
                    ui.add_space(4.0);
                }

                let available = ui.available_width().max(300.0);
                let response = ui.add(
                    TextEdit::multiline(&mut document.text)
                        .id(text_edit_id)
                        .desired_rows(line_count.max(32))
                        .desired_width(available)
                        .frame(false)
                        .layouter(&mut layouter),
                );
                if response.changed() {
                    document.dirty = true;
                    ui.ctx().request_repaint();
                }
            });
        });

    if focused {
        if let Some(prefix) = completion_prefix(ui.ctx(), text_edit_id, &document.text) {
            if prefix.len() >= 2 {
                let suggestions = completion_candidates(&prefix);
                if !suggestions.is_empty() {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new("complete")
                                .small()
                                .color(theme.text_dim.to_color32()),
                        );
                        for suggestion in suggestions.into_iter().take(8) {
                            if ui.small_button(&suggestion).clicked()
                                && replace_completion_prefix(
                                    ui.ctx(),
                                    text_edit_id,
                                    &mut document.text,
                                    &prefix,
                                    &suggestion,
                                )
                            {
                                document.dirty = true;
                            }
                        }
                    });
                }
            }
        }
    }

    if !diagnostics.is_empty() {
        ui.separator();
        egui::CollapsingHeader::new(format!("issues ({})", diagnostics.len()))
            .default_open(true)
            .show(ui, |ui| {
                for diagnostic in &diagnostics {
                    let label = format!("line {}: {}", diagnostic.line, diagnostic.message);
                    if ui
                        .selectable_label(false, egui::RichText::new(label).small())
                        .clicked()
                    {
                        select_line(ui.ctx(), text_edit_id, &document.text, diagnostic.line);
                    }
                }
            });
    }

    ui.separator();
    let cursor =
        TextEditState::load(ui.ctx(), text_edit_id).and_then(|state| state.cursor.char_range());
    let (line, column) = cursor
        .map(|range| char_to_line_col(&document.text, range.primary.index))
        .unwrap_or((1, 1));
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "ln {line}, col {column}   utf-8   {} lines   {} chars{}",
                line_count,
                document.text.chars().count(),
                if document.dirty { "   modified" } else { "" }
            ))
            .small()
            .color(theme.text_dim.to_color32()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(
                    "ctrl+f find  ctrl+h replace  ctrl+/ comment  ctrl+d duplicate  alt+?/? move",
                )
                .small()
                .color(theme.text_dim.to_color32()),
            );
        });
    });

    if let Some(status) = status {
        app.session.status = status;
    }
}

fn current_cursor_index(ctx: &Context, id: egui::Id, fallback: usize) -> usize {
    TextEditState::load(ctx, id)
        .and_then(|state| state.cursor.char_range())
        .map(|range| range.primary.index)
        .unwrap_or(fallback)
}

fn set_cursor_range(ctx: &Context, id: egui::Id, start: usize, end: usize) {
    if let Some(mut state) = TextEditState::load(ctx, id) {
        state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(start),
            CCursor::new(end),
        )));
        state.store(ctx, id);
        ctx.memory_mut(|memory| memory.request_focus(id));
    }
}

fn select_line(ctx: &Context, id: egui::Id, text: &str, line: usize) -> bool {
    if line == 0 {
        return false;
    }
    let mut current_line = 1usize;
    let mut start = 0usize;
    let mut end = text.chars().count();
    for (index, ch) in text.chars().enumerate() {
        if current_line == line && start == 0 && line != 1 {
            start = index;
        }
        if ch == '\n' {
            if current_line == line {
                end = index;
                break;
            }
            current_line += 1;
            if current_line == line {
                start = index + 1;
            }
        }
    }
    if current_line < line {
        return false;
    }
    set_cursor_range(ctx, id, start, end);
    true
}

fn select_match(ctx: &Context, id: egui::Id, text: &str, needle: &str, forward: bool) -> bool {
    if needle.is_empty() {
        return false;
    }
    let total_chars = text.chars().count();
    let cursor = current_cursor_index(ctx, id, total_chars).min(total_chars);
    let cursor_byte = char_to_byte(text, cursor);
    let found_byte = if forward {
        text[cursor_byte..]
            .find(needle)
            .map(|offset| cursor_byte + offset)
            .or_else(|| text[..cursor_byte].find(needle))
    } else {
        text[..cursor_byte].rfind(needle).or_else(|| {
            text[cursor_byte..]
                .rfind(needle)
                .map(|offset| cursor_byte + offset)
        })
    };
    let Some(found_byte) = found_byte else {
        return false;
    };
    let start = byte_to_char(text, found_byte);
    let end = start + needle.chars().count();
    set_cursor_range(ctx, id, start, end);
    true
}

fn line_bounds_chars(text: &str, cursor: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|ch| *ch == '\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let end = chars[cursor..]
        .iter()
        .position(|ch| *ch == '\n')
        .map(|offset| cursor + offset)
        .unwrap_or(chars.len());
    (start, end)
}

fn duplicate_current_line(ctx: &Context, id: egui::Id, text: &mut String) -> bool {
    let cursor = current_cursor_index(ctx, id, text.chars().count());
    let (start, end) = line_bounds_chars(text, cursor);
    let start_byte = char_to_byte(text, start);
    let end_byte = char_to_byte(text, end);
    let line = text[start_byte..end_byte].to_string();
    let insert = if end_byte < text.len() {
        format!("{line}\n")
    } else {
        format!("\n{line}")
    };
    let insert_byte = if end_byte < text.len() {
        start_byte
    } else {
        end_byte
    };
    text.insert_str(insert_byte, &insert);
    let shift = line.chars().count() + 1;
    set_cursor_range(ctx, id, cursor + shift, cursor + shift);
    true
}

fn toggle_comment_selected_lines(ctx: &Context, id: egui::Id, text: &mut String) -> bool {
    let Some(mut state) = TextEditState::load(ctx, id) else {
        return false;
    };
    let Some(range) = state.cursor.char_range() else {
        return false;
    };
    let min = range.primary.index.min(range.secondary.index);
    let max = range.primary.index.max(range.secondary.index);
    let (start, _) = line_bounds_chars(text, min);
    let (_, end) = line_bounds_chars(text, max);
    let start_byte = char_to_byte(text, start);
    let end_byte = char_to_byte(text, end);
    let block = text[start_byte..end_byte].to_string();
    let uncomment = block
        .lines()
        .filter(|line| !line.trim().is_empty())
        .all(|line| line.trim_start().starts_with('#'));
    let mut rebuilt = String::new();
    for (index, line) in block.split('\n').enumerate() {
        if index > 0 {
            rebuilt.push('\n');
        }
        if uncomment {
            let indent = line.len() - line.trim_start().len();
            rebuilt.push_str(&line[..indent]);
            let rest = &line[indent..];
            rebuilt.push_str(
                rest.strip_prefix("# ")
                    .or_else(|| rest.strip_prefix('#'))
                    .unwrap_or(rest),
            );
        } else if line.trim().is_empty() {
            rebuilt.push_str(line);
        } else {
            let indent = line.len() - line.trim_start().len();
            rebuilt.push_str(&line[..indent]);
            rebuilt.push_str("# ");
            rebuilt.push_str(&line[indent..]);
        }
    }
    text.replace_range(start_byte..end_byte, &rebuilt);
    let new_end = start + rebuilt.chars().count();
    state.cursor.set_char_range(Some(CCursorRange::two(
        CCursor::new(start),
        CCursor::new(new_end),
    )));
    state.store(ctx, id);
    true
}

fn move_current_line(ctx: &Context, id: egui::Id, text: &mut String, direction: i8) -> bool {
    let cursor = current_cursor_index(ctx, id, text.chars().count());
    let mut lines: Vec<String> = text.split('\n').map(ToString::to_string).collect();
    if lines.len() < 2 {
        return false;
    }
    let line_index = text
        .chars()
        .take(cursor)
        .filter(|ch| *ch == '\n')
        .count()
        .min(lines.len() - 1);
    let target = if direction < 0 {
        let Some(target) = line_index.checked_sub(1) else {
            return false;
        };
        target
    } else {
        if line_index + 1 >= lines.len() {
            return false;
        }
        line_index + 1
    };
    lines.swap(line_index, target);
    *text = lines.join("\n");
    let new_start: usize = lines[..target]
        .iter()
        .map(|line| line.chars().count() + 1)
        .sum();
    set_cursor_range(ctx, id, new_start, new_start);
    true
}

fn completion_prefix(ctx: &Context, id: egui::Id, text: &str) -> Option<String> {
    let cursor = current_cursor_index(ctx, id, text.chars().count());
    let before: String = text.chars().take(cursor).collect();
    let prefix: String = before
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '.')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    (!prefix.is_empty()).then_some(prefix)
}

fn completion_candidates(prefix: &str) -> Vec<String> {
    let mut candidates = vec![
        "import".to_string(),
        "listen".to_string(),
        "xlisten".to_string(),
        "pb".to_string(),
        "pr".to_string(),
        "func".to_string(),
        "if".to_string(),
        "else".to_string(),
        "while".to_string(),
        "return".to_string(),
        "true".to_string(),
        "false".to_string(),
        "null".to_string(),
        "and".to_string(),
        "or".to_string(),
        "not".to_string(),
        "do!".to_string(),
        "gorun!".to_string(),
        "gostop!".to_string(),
    ];
    for library in super::parser::BUILTIN_LIBRARIES {
        candidates.push(library.name.to_string());
        candidates.extend(library.symbols.iter().map(|symbol| (*symbol).to_string()));
    }
    candidates.sort();
    candidates.dedup();
    candidates
        .into_iter()
        .filter(|candidate| candidate.starts_with(prefix) && candidate != prefix)
        .collect()
}

fn replace_completion_prefix(
    ctx: &Context,
    id: egui::Id,
    text: &mut String,
    prefix: &str,
    suggestion: &str,
) -> bool {
    let cursor = current_cursor_index(ctx, id, text.chars().count());
    let prefix_chars = prefix.chars().count();
    if cursor < prefix_chars {
        return false;
    }
    let start = cursor - prefix_chars;
    let start_byte = char_to_byte(text, start);
    let end_byte = char_to_byte(text, cursor);
    text.replace_range(start_byte..end_byte, suggestion);
    let new_cursor = start + suggestion.chars().count();
    set_cursor_range(ctx, id, new_cursor, new_cursor);
    true
}

// ---------------- Auto-indent / tab helpers ----------------

/// On Enter: insert "\n" + indent matching the current line, plus an
/// extra `tab_width` spaces if the line ends with `{`.
fn apply_auto_indent(ctx: &Context, id: egui::Id, text: &mut String, tab_width: usize) {
    let mut state = match TextEditState::load(ctx, id) {
        Some(s) => s,
        None => return,
    };
    let cursor_range = state.cursor.char_range();
    let cur = cursor_range
        .map(|r| r.primary.index)
        .unwrap_or_else(|| text.chars().count());
    let byte_pos = char_to_byte(text, cur);

    // Walk back from byte_pos to start of line (or string).
    let line_start = text[..byte_pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let current_line = &text[line_start..byte_pos];
    let indent: String = current_line
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    // Trim trailing whitespace from `current_line` to look at last
    // non-whitespace char.
    let trimmed_end = current_line.trim_end();
    let extra = if trimmed_end.ends_with('{') || trimmed_end.ends_with('(') {
        " ".repeat(tab_width)
    } else {
        String::new()
    };
    let to_insert = format!("\n{indent}{extra}");
    text.insert_str(byte_pos, &to_insert);

    let new_cursor_chars = cur + to_insert.chars().count();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(new_cursor_chars))));
    state.store(ctx, id);
    ctx.request_repaint();
}

/// Tab → insert `tab_width` spaces (or however many fit before the next
/// tab-stop column, measured from the start of the line).
fn insert_tab(ctx: &Context, id: egui::Id, text: &mut String, tab_width: usize) {
    let mut state = match TextEditState::load(ctx, id) {
        Some(s) => s,
        None => return,
    };
    let cursor_range = state.cursor.char_range();
    let cur = cursor_range
        .map(|r| r.primary.index)
        .unwrap_or_else(|| text.chars().count());
    let byte_pos = char_to_byte(text, cur);
    let line_start = text[..byte_pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col_chars = text[line_start..byte_pos].chars().count();
    let spaces_to_next_stop = tab_width - (col_chars % tab_width);
    let pad: String = std::iter::repeat_n(' ', spaces_to_next_stop).collect();
    text.insert_str(byte_pos, &pad);
    let new_cursor_chars = cur + pad.chars().count();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(new_cursor_chars))));
    state.store(ctx, id);
    ctx.request_repaint();
}

/// Shift+Tab → if the cursor sits in leading whitespace, remove up to
/// `tab_width` spaces from the start of the current line.
fn outdent(ctx: &Context, id: egui::Id, text: &mut String, tab_width: usize) {
    let mut state = match TextEditState::load(ctx, id) {
        Some(s) => s,
        None => return,
    };
    let cursor_range = state.cursor.char_range();
    let cur = cursor_range
        .map(|r| r.primary.index)
        .unwrap_or_else(|| text.chars().count());
    let byte_pos = char_to_byte(text, cur);
    let line_start = text[..byte_pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    // How many leading spaces?  Count from line_start, capped at tab_width.
    let leading: usize = text[line_start..]
        .chars()
        .take_while(|c| *c == ' ')
        .count()
        .min(tab_width);
    if leading == 0 {
        return;
    }
    // Remove `leading` spaces from line_start.
    text.replace_range(line_start..line_start + leading, "");
    // Cursor: if we were past line_start + leading, shift it left by `leading`.
    let line_start_chars = byte_to_char(text, line_start);
    let new_cursor = if cur >= line_start_chars + leading {
        cur - leading
    } else {
        line_start_chars
    };
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(new_cursor))));
    state.store(ctx, id);
    ctx.request_repaint();
}

// ---------------- Char/byte index helpers ----------------

/// Convert char-index to byte-index in a UTF-8 string. Saturates at end.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

/// Inverse of `char_to_byte`: count chars up to `byte_idx`.
fn byte_to_char(s: &str, byte_idx: usize) -> usize {
    s[..byte_idx.min(s.len())].chars().count()
}

/// Convert a char offset to (line, column), both 1-indexed, using
/// '\n' as the only line break.
fn char_to_line_col(s: &str, char_idx: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (consumed, c) in s.chars().enumerate() {
        if consumed == char_idx {
            return (line, col);
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
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

fn runtime_action_label(action: &super::runtime::RuntimeAction) -> String {
    use super::runtime::RuntimeAction;

    match action {
        RuntimeAction::GoRun(target) => format!("gorun! {}", timeline_target_label(target)),
        RuntimeAction::GoStop(target) => format!("gostop! {}", timeline_target_label(target)),
        RuntimeAction::SceneSwitch { alias } => format!("q0scene.switch! {:?}", alias),
        RuntimeAction::ShellCommand { command, args } => {
            let args = args
                .iter()
                .map(|arg| arg.display_lossy())
                .collect::<Vec<_>>()
                .join(", ");
            format!("q0shell.{command}! {args}")
        }
        RuntimeAction::RigSetPosition { control, x, y } => {
            format!("q0rig.position! {:?}, {x}, {y}", control)
        }
        RuntimeAction::RigSetValue { control, value } => {
            format!("q0rig.value! {:?}, {value}", control)
        }
        RuntimeAction::RigReset { control } => format!("q0rig.reset! {:?}", control),
        RuntimeAction::RigSetPose { pose, weight } => {
            format!("q0rig.pose! {:?}, {weight}", pose)
        }
        RuntimeAction::RigResetPose { pose } => format!("q0rig.pose_reset! {:?}", pose),
    }
}

fn timeline_target_label(target: &super::runtime::TimelineTarget) -> String {
    use super::runtime::TimelineTarget;

    match target {
        TimelineTarget::Frame(frame) => frame.to_string(),
        TimelineTarget::Label(label) => label.clone(),
    }
}
