use egui::{FontFamily, FontId, RichText, TextEdit};
use q0s_format::v2::FrameScript;

use crate::app::{Action, EditorApp};

pub fn render(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(mut draft) = app.session.frame_script_editor.clone() else {
        return;
    };

    let q0rg_name = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == draft.q0rg_id)
        .map(|q0rg| q0rg.name.clone())
        .unwrap_or_else(|| format!("q0rg {}", draft.q0rg_id));
    let layer_name = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == draft.q0rg_id)
        .and_then(|q0rg| {
            q0rg.layers
                .iter()
                .find(|layer| layer.layer_id == draft.layer_id)
        })
        .map(|layer| layer.name.clone())
        .unwrap_or_else(|| format!("layer {}", draft.layer_id));

    let theme = app.settings.theme.clone();
    let font_id = FontId::new(
        app.settings.q0lang_font_size.0.clamp(8.0, 72.0),
        FontFamily::Name(super::fonts::Q0LANG_FAMILY.into()),
    );
    let analysis = super::authoring::analyze(ctx, &draft.source);
    let diagnostics = &analysis.diagnostics;
    let text_edit_id = egui::Id::new((
        "frame_script_source",
        draft.q0rg_id,
        draft.layer_id,
        draft.frame,
    ));
    super::editor::edit_shortcuts(
        ctx,
        text_edit_id,
        &mut draft.source,
        app.settings.q0lang_tab_width.max(1) as usize,
    );
    let had_existing = app
        .state
        .project
        .runtime
        .frame_scripts
        .iter()
        .any(|script| {
            script.q0rg_id == draft.q0rg_id
                && script.layer_id == draft.layer_id
                && script.frame == draft.frame
        });

    let mut open = true;
    let mut save = false;
    let mut remove = false;
    let mut close = false;
    egui::Window::new(format!(
        "frame code - {} / {} / {}",
        q0rg_name,
        layer_name,
        draft.frame + 1
    ))
    .id(egui::Id::new("frame_script_editor"))
    .open(&mut open)
    .default_width(640.0)
    .default_height(480.0)
    .resizable(true)
    .show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("q0lang frame code").strong());
            ui.separator();
            ui.label(
                RichText::new(format!("frame {}", draft.frame + 1))
                    .small()
                    .color(theme.text_dim.to_color32()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = if diagnostics.is_empty() {
                    "no issues".to_string()
                } else {
                    format!(
                        "{} issue{}",
                        diagnostics.len(),
                        if diagnostics.len() == 1 { "" } else { "s" }
                    )
                };
                ui.label(
                    RichText::new(text)
                        .small()
                        .color(if diagnostics.is_empty() {
                            theme.text_dim.to_color32()
                        } else {
                            theme.syntax_signal.to_color32()
                        }),
                );
            });
        });
        ui.separator();

        super::authoring::tools_ui(ui, text_edit_id, &mut draft.source);
        ui.separator();

        let theme_for_layouter = theme.clone();
        let font_for_layouter = font_id.clone();
        let mut layouter = move |ui: &egui::Ui, text: &str, _wrap_width: f32| {
            let job = super::syntax::layout_line(
                text,
                &theme_for_layouter,
                font_for_layouter.clone(),
                f32::INFINITY,
            );
            ui.fonts(|fonts| fonts.layout_job(job))
        };
        egui::ScrollArea::both()
            .id_source(text_edit_id.with("scroll"))
            .auto_shrink([false, false])
            .max_height((ui.available_height() - 115.0).max(180.0))
            .show(ui, |ui| {
                let output = TextEdit::multiline(&mut draft.source)
                    .id(text_edit_id)
                    .desired_width(ui.available_width().max(240.0))
                    .desired_rows(18)
                    .font(font_id.clone())
                    .layouter(&mut layouter)
                    .show(ui);
                super::authoring::scroll_to_selection(ui, &output);
            });
        let analysis = super::authoring::analyze(ctx, &draft.source);
        let diagnostics = &analysis.diagnostics;

        if !diagnostics.is_empty() {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_source("frame_script_diagnostics")
                .max_height(70.0)
                .show(ui, |ui| {
                    for diagnostic in diagnostics {
                        if ui
                            .selectable_label(
                                false,
                                RichText::new(format!(
                                    "line {}: {}",
                                    diagnostic.line, diagnostic.message
                                ))
                                .small()
                                .color(theme.syntax_signal.to_color32()),
                            )
                            .clicked()
                        {
                            super::authoring::select_line(
                                ctx,
                                text_edit_id,
                                &draft.source,
                                diagnostic.line,
                            );
                        }
                    }
                });
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("save").clicked() {
                save = true;
            }
            if ui
                .add_enabled(had_existing, egui::Button::new("remove frame code"))
                .clicked()
            {
                remove = true;
            }
            if ui.button("close").clicked() {
                close = true;
            }
            ui.label(
                RichText::new("code runs when playback enters this frame")
                    .small()
                    .color(theme.text_dim.to_color32()),
            );
        });
    });

    if save {
        app.history.snapshot(&app.state.project);
        app.state.project.runtime.frame_scripts.retain(|script| {
            script.q0rg_id != draft.q0rg_id
                || script.layer_id != draft.layer_id
                || script.frame != draft.frame
        });
        if !draft.source.trim().is_empty() {
            if let Some(layer) = app
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q0rg| q0rg.q0rg_id == draft.q0rg_id)
                .and_then(|q0rg| {
                    q0rg.layers
                        .iter_mut()
                        .find(|layer| layer.layer_id == draft.layer_id)
                })
            {
                layer.extend_timeline_through(draft.frame);
            }
            app.state.project.runtime.frame_scripts.push(FrameScript {
                q0rg_id: draft.q0rg_id,
                layer_id: draft.layer_id,
                frame: draft.frame,
                source: draft.source.clone(),
            });
            app.state
                .project
                .runtime
                .frame_scripts
                .sort_by_key(|script| (script.q0rg_id, script.frame, script.layer_id));
            app.session.status = format!("saved frame code for frame {}", draft.frame + 1);
        } else {
            app.session.status = format!("removed empty frame code from frame {}", draft.frame + 1);
        }
        app.state.mark_dirty();
        app.session.reset_q0lang_preview();
    }
    if remove {
        app.queue(Action::DeleteFrameScript(
            draft.q0rg_id,
            draft.layer_id,
            draft.frame,
        ));
        close = true;
    }

    if close || !open {
        app.session.frame_script_editor = None;
    } else {
        app.session.frame_script_editor = Some(draft);
    }
}
