use std::path::Path;

use egui::{Align, Button, Frame, Layout, Margin, RichText, ScrollArea, Ui, Vec2};

use crate::app::{Action, EditorApp};
use crate::release_feed::{
    parse_release_markdown, short_date, MarkdownBlock, MarkdownSpan, MarkdownSpanKind,
    ReleaseFeedStatus, ReleaseNote,
};

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    ui.add_space(24.0);
    let page_width = ui.available_width().min(1120.0);
    let side_space = ((ui.available_width() - page_width) * 0.5).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(side_space);
        ui.allocate_ui_with_layout(
            Vec2::new(page_width, ui.available_height()),
            Layout::top_down(Align::Min),
            |ui| {
                header(app, ui);
                ui.add_space(22.0);
                action_row(app, ui);
                ui.add_space(24.0);
                content_columns(app, ui);
            },
        );
    });
}

fn header(app: &EditorApp, ui: &mut Ui) {
    ui.label(RichText::new("q0editor").size(38.0).strong());
    ui.label(
        RichText::new("home")
            .size(16.0)
            .color(app.settings.theme.accent.to_color32()),
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new("Create a project or continue from a recent q1s file.")
            .color(app.settings.theme.text_dim.to_color32()),
    );
}

fn action_row(app: &mut EditorApp, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_sized(
                [164.0, 42.0],
                Button::new(RichText::new("New project").strong()),
            )
            .clicked()
        {
            app.queue(Action::NewProject);
        }
        if ui
            .add_sized([164.0, 42.0], Button::new("Open project..."))
            .clicked()
        {
            app.queue(Action::OpenProject);
        }
    });
}

fn content_columns(app: &mut EditorApp, ui: &mut Ui) {
    app.release_feed.ensure_started(ui.ctx());
    app.release_feed.poll();
    let recent = app.settings.recent_projects.clone();
    let mut open_recent = None;
    ui.columns(2, |columns| {
        columns[0].set_max_width(columns[0].available_width());
        Frame::group(columns[0].style())
            .inner_margin(Margin::same(16.0))
            .show(&mut columns[0], |ui| {
                ui.set_min_height(360.0);
                ui.label(RichText::new("Recent projects").size(18.0).strong());
                ui.add_space(8.0);
                if recent.is_empty() {
                    ui.label(
                        RichText::new("No recent projects yet.")
                            .color(app.settings.theme.text_dim.to_color32()),
                    );
                    ui.label(
                        RichText::new("Saved and opened q1s files will appear here.")
                            .small()
                            .color(app.settings.theme.text_dim.to_color32()),
                    );
                } else {
                    for path in &recent {
                        if recent_project_button(ui, path).clicked() {
                            open_recent = Some(path.clone());
                        }
                        ui.add_space(4.0);
                    }
                }
            });

        Frame::group(columns[1].style())
            .inner_margin(Margin::same(16.0))
            .show(&mut columns[1], |ui| {
                ui.set_min_height(360.0);
                release_feed(app, ui);
            });
    });

    if let Some(path) = open_recent {
        app.queue(Action::OpenProjectFromPath(path));
    }
}

fn release_feed(app: &mut EditorApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Feed").size(18.0).strong());
        ui.label(
            RichText::new("GitHub releases")
                .small()
                .color(app.settings.theme.accent.to_color32()),
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
                .max_height(310.0)
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
                .color(app.settings.theme.accent.to_color32()),
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
                    ui.label(RichText::new("? ").color(app.settings.theme.accent.to_color32()));
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
    let accent = app.settings.theme.accent.to_color32();
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
    ui.add_sized(
        [ui.available_width(), 48.0],
        Button::new(format!("{file_name}\n{parent}")),
    )
    .on_hover_text(path.display().to_string())
}
