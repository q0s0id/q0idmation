use std::path::Path;

use egui::{Align, Button, Layout, RichText, ScrollArea, Ui, Vec2};

use crate::app::{Action, EditorApp};
use crate::release_feed::{
    parse_release_markdown, short_date, MarkdownBlock, MarkdownSpan, MarkdownSpanKind,
    ReleaseFeedStatus, ReleaseNote,
};

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    ScrollArea::vertical()
        .id_source("home_page")
        .show(ui, |ui| {
            let page_width = (ui.available_width() - 48.0).clamp(300.0, 1080.0);
            let side_space = ((ui.available_width() - page_width) * 0.5).max(0.0);
            ui.add_space(32.0);
            ui.horizontal(|ui| {
                ui.add_space(side_space);
                ui.allocate_ui_with_layout(
                    Vec2::new(page_width, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 16.0;
                        header(app, ui);
                        ui.add_space(24.0);
                        if !app.autosave.recovered().is_empty() {
                            recovered_work(app, ui);
                            ui.add_space(16.0);
                        }
                        action_row(app, ui);
                        ui.add_space(28.0);
                        content_columns(app, ui);
                        ui.add_space(24.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("q0editor").strong());
                            ui.label(RichText::new(env!("CARGO_PKG_VERSION")).small());
                            ui.separator();
                            ui.label(
                                RichText::new("Vector animation, frame by frame.")
                                    .color(app.settings.theme.text_dim.to_color32()),
                            );
                        });
                        ui.add_space(24.0);
                    },
                );
            });
        });
}

fn header(app: &EditorApp, ui: &mut Ui) {
    let accent = ui.visuals().hyperlink_color;
    ui.horizontal(|ui| {
        ui.label(RichText::new("q0e").size(22.0).strong().color(accent));
        ui.add_space(8.0);
        ui.label(
            RichText::new("YOUR ANIMATION WORKSPACE")
                .size(11.0)
                .color(app.settings.theme.text_dim.to_color32()),
        );
    });
    ui.add_space(16.0);
    ui.label(RichText::new("Make your next move.").size(38.0).strong());
    ui.add_space(6.0);
    ui.label(
        RichText::new("A new idea, a blank stage. Or pick up right where you left off.")
            .size(15.0)
            .color(app.settings.theme.text_dim.to_color32()),
    );
}

/// Snapshots left behind by a session that did not exit cleanly.
fn recovered_work(app: &mut EditorApp, ui: &mut Ui) {
    use crate::autosave::DiscardTarget;

    let dim = app.settings.theme.text_dim.to_color32();
    let accent = app.settings.theme.accent.to_color32();
    let rows = app
        .autosave
        .recovered()
        .iter()
        .map(|entry| {
            (
                entry.title().to_string(),
                entry.detail(),
                entry.original_path(),
                entry.key().to_path_buf(),
            )
        })
        .collect::<Vec<_>>();
    let armed = app.autosave.discard_armed.clone();
    let mut open = None;
    let mut discard = None;
    let mut discard_all = false;
    let mut arm = None;
    crate::theme::surface(ui)
        .fill(app.settings.theme.panel.to_color32())
        .stroke(egui::Stroke::new(1.0_f32, accent))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Recovered work").size(19.0).strong());
            ui.label(
                RichText::new(
                    "q0editor did not close cleanly. These unsaved documents were kept; \
                     opening one changes nothing on disk until you save.",
                )
                .color(dim),
            );
            ui.add_space(12.0);
            for (index, (title, detail, path, key)) in rows.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    if crate::theme::primary_button(ui, "Open", accent).clicked() {
                        open = Some(index);
                    }
                    let this = DiscardTarget::One(key.clone());
                    if armed.as_ref() == Some(&this) {
                        if ui.button("Delete for good").clicked() {
                            discard = Some(index);
                        }
                    } else if ui.button("Discard").clicked() {
                        arm = Some(this);
                    }
                    ui.label(RichText::new(title).strong());
                    ui.label(RichText::new(detail).small().color(dim));
                    let origin = match path {
                        Some(path) => path.display().to_string(),
                        None => "never saved".to_string(),
                    };
                    ui.label(RichText::new(origin).small().color(dim));
                });
                ui.add_space(4.0);
            }
            if rows.len() > 1 {
                ui.add_space(4.0);
                if armed == Some(DiscardTarget::All) {
                    if ui.button("Delete all for good").clicked() {
                        discard_all = true;
                    }
                } else if ui.button("Discard all").clicked() {
                    arm = Some(DiscardTarget::All);
                }
            }
        });
    if let Some(target) = arm {
        app.autosave.discard_armed = Some(target);
    }
    if let Some(index) = open {
        app.open_recovered_document(index);
    } else if let Some(index) = discard {
        app.autosave.discard_recovered(index);
    } else if discard_all {
        app.autosave.discard_all_recovered();
    }
}

fn action_row(app: &mut EditorApp, ui: &mut Ui) {
    ui.columns(2, |columns| {
        crate::theme::surface(&columns[0])
            .fill(app.settings.theme.panel.to_color32())
            .stroke(egui::Stroke::new(
                1.0_f32,
                app.settings.theme.accent.to_color32(),
            ))
            .show(&mut columns[0], |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Start something new").size(19.0).strong());
                ui.label(
                    RichText::new("Set your stage. Bring an idea to life.")
                        .color(app.settings.theme.text_dim.to_color32()),
                );
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if crate::theme::primary_button(
                        ui,
                        "New project",
                        app.settings.theme.accent.to_color32(),
                    )
                    .clicked()
                    {
                        app.queue(Action::NewProject);
                    }
                    ui.label(
                        RichText::new("Ctrl + N")
                            .small()
                            .color(app.settings.theme.text_dim.to_color32()),
                    );
                });
            });
        crate::theme::surface(&columns[1]).show(&mut columns[1], |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Back to your craft").size(19.0).strong());
            ui.label(
                RichText::new("Open a project and keep the story moving.")
                    .color(app.settings.theme.text_dim.to_color32()),
            );
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui
                    .add_sized([144.0, 36.0], Button::new("Open project..."))
                    .clicked()
                {
                    app.queue(Action::OpenProject);
                }
                ui.label(
                    RichText::new("Ctrl + O")
                        .small()
                        .color(app.settings.theme.text_dim.to_color32()),
                );
            });
        });
    });
}

fn content_columns(app: &mut EditorApp, ui: &mut Ui) {
    app.release_feed.ensure_started(ui.ctx());
    app.release_feed.poll();
    let recent = app.settings.recent_projects.clone();
    let mut open_recent = None;
    ui.columns(2, |columns| {
        crate::theme::surface(&columns[0]).show(&mut columns[0], |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(230.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Recent projects").size(17.0).strong());
                ui.label(
                    RichText::new(format!("{:02}", recent.len()))
                        .small()
                        .color(app.settings.theme.text_dim.to_color32()),
                );
            });
            ui.add_space(12.0);
            if recent.is_empty() {
                ui.add_space(24.0);
                ui.label(
                    RichText::new("Your next story starts here.")
                        .size(16.0)
                        .strong(),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Projects you open or save will appear here, ready for your next session.",
                    )
                    .color(app.settings.theme.text_dim.to_color32()),
                );
                ui.add_space(16.0);
                if ui.button("Create your first project").clicked() {
                    app.queue(Action::NewProject);
                }
            } else {
                ScrollArea::vertical()
                    .id_source("home_recent_projects")
                    .max_height(240.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for path in &recent {
                            if recent_project_button(ui, path).clicked() {
                                open_recent = Some(path.clone());
                            }
                            ui.add_space(4.0);
                        }
                    });
            }
        });
        crate::theme::surface(&columns[1]).show(&mut columns[1], |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(230.0);
            release_feed(app, ui);
        });
    });
    if let Some(path) = open_recent {
        app.queue(Action::OpenProjectFromPath(path));
    }
}

fn release_feed(app: &mut EditorApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("What’s new").size(17.0).strong());
        ui.label(
            RichText::new("GitHub releases")
                .small()
                .color(ui.visuals().hyperlink_color),
        );
    });
    ui.add_space(10.0);

    let status = app.release_feed.status().clone();
    match status {
        ReleaseFeedStatus::Idle | ReleaseFeedStatus::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new("Loading releases from GitHub...")
                        .color(app.settings.theme.text_dim.to_color32()),
                );
            });
        }
        ReleaseFeedStatus::Ready(releases) if releases.is_empty() => {
            ui.label(
                RichText::new("No published GitHub releases yet.")
                    .color(app.settings.theme.text_dim.to_color32()),
            );
        }
        ReleaseFeedStatus::Ready(releases) => {
            ScrollArea::vertical()
                .id_source("home_release_feed")
                .auto_shrink([false, false])
                .max_height(240.0)
                .show(ui, |ui| {
                    for (index, release) in releases.iter().enumerate() {
                        if index > 0 {
                            ui.add_space(10.0);
                            ui.separator();
                            ui.add_space(10.0);
                        }
                        release_card(app, ui, release);
                    }
                });
        }
        ReleaseFeedStatus::Error(error) => {
            ui.label(RichText::new("GitHub feed is temporarily unavailable.").strong());
            ui.add_space(4.0);
            ui.label(
                RichText::new(error)
                    .small()
                    .color(app.settings.theme.text_dim.to_color32()),
            );
            ui.add_space(10.0);
            if ui.button("Retry").clicked() {
                app.release_feed.retry(ui.ctx());
            }
        }
    }
}

fn release_card(app: &EditorApp, ui: &mut Ui, release: &ReleaseNote) {
    ui.label(RichText::new(&release.title).size(15.0).strong());
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(&release.tag)
                .small()
                .color(ui.visuals().hyperlink_color),
        );
        if let Some(date) = short_date(release.published_at.as_deref()) {
            ui.label(
                RichText::new(date)
                    .small()
                    .color(app.settings.theme.text_dim.to_color32()),
            );
        }
        if release.prerelease {
            ui.label(
                RichText::new("prerelease")
                    .small()
                    .color(app.settings.theme.text_dim.to_color32()),
            );
        }
    });
    ui.add_space(6.0);
    render_release_markdown(app, ui, &release.body);
    ui.add_space(8.0);
    ui.hyperlink_to("View release on GitHub", &release.url);
}

fn render_release_markdown(app: &EditorApp, ui: &mut Ui, body: &str) {
    for block in parse_release_markdown(body) {
        match block {
            MarkdownBlock::Heading { level, spans } => {
                if level <= 2 {
                    ui.add_space(5.0);
                }
                let size = match level {
                    1 => 16.0,
                    2 => 14.5,
                    _ => 13.5,
                };
                render_markdown_spans(app, ui, &spans, size, true, false);
                ui.add_space(2.0);
            }
            MarkdownBlock::Bullet(spans) => {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(RichText::new("? ").color(ui.visuals().hyperlink_color));
                    render_markdown_spans_inline(app, ui, &spans, 13.0, false, true);
                });
                ui.add_space(2.0);
            }
            MarkdownBlock::Paragraph(spans) => {
                render_markdown_spans(app, ui, &spans, 13.0, false, true);
                ui.add_space(2.0);
            }
            MarkdownBlock::Spacer => ui.add_space(5.0),
        }
    }
}

fn render_markdown_spans(
    app: &EditorApp,
    ui: &mut Ui,
    spans: &[MarkdownSpan],
    size: f32,
    strong_all: bool,
    dim_plain: bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        render_markdown_spans_inline(app, ui, spans, size, strong_all, dim_plain);
    });
}

fn render_markdown_spans_inline(
    app: &EditorApp,
    ui: &mut Ui,
    spans: &[MarkdownSpan],
    size: f32,
    strong_all: bool,
    dim_plain: bool,
) {
    let accent = ui.visuals().hyperlink_color;
    let normal = if dim_plain {
        app.settings.theme.text_dim.to_color32()
    } else {
        ui.visuals().text_color()
    };

    for span in spans {
        let mut text = RichText::new(&span.text).size(size).color(normal);
        if strong_all || matches!(span.kind, MarkdownSpanKind::Strong) {
            text = text.strong();
        }
        match &span.kind {
            MarkdownSpanKind::Code => {
                ui.label(text.monospace().color(accent));
            }
            MarkdownSpanKind::Emphasis => {
                ui.label(text.italics());
            }
            MarkdownSpanKind::Link(url) => {
                ui.hyperlink_to(text.color(accent).underline(), url);
            }
            MarkdownSpanKind::Plain | MarkdownSpanKind::Strong => {
                ui.label(text);
            }
        }
    }
}

fn recent_project_button(ui: &mut Ui, path: &Path) -> egui::Response {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let parent = path
        .parent()
        .map(Path::display)
        .map(|display| display.to_string())
        .unwrap_or_default();
    let width = ui.available_width();
    let (row, response) = ui.allocate_exact_size(egui::vec2(width, 56.0), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    ui.painter()
        .rect(row, visuals.rounding, visuals.bg_fill, visuals.bg_stroke);
    let rect = response.rect.shrink2(egui::vec2(14.0, 9.0));
    let painter = ui.painter().with_clip_rect(rect);
    painter.text(
        rect.left_top(),
        egui::Align2::LEFT_TOP,
        file_name,
        egui::FontId::proportional(14.0),
        ui.visuals().text_color(),
    );
    painter.text(
        rect.left_bottom(),
        egui::Align2::LEFT_BOTTOM,
        parent,
        egui::FontId::proportional(11.0),
        ui.visuals().weak_text_color(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, path.display().to_string())
    });
    response.on_hover_text(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovered_work_lists_every_snapshot_of_a_crashed_session() {
        let root = crate::autosave::test_support::TestDir::new("home");
        let ctx = egui::Context::default();
        crate::autosave::test_support::leave_crashed_script(&root, "go 4");

        let mut app = EditorApp::default();
        crate::autosave::test_support::enable(&mut app, &root);
        crate::autosave::tick(&mut app, &ctx);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| recovered_work(&mut app, ui));
        });
        assert!(!output.shapes.is_empty());
        assert_eq!(
            app.autosave.recovered().len(),
            1,
            "drawing discards nothing"
        );
        assert_eq!(root.files().len(), 2);
    }
}
