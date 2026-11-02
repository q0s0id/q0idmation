//! Shows a plugin page written in q0ex page markup (see [`q0ex::page`]).

use egui::{Color32, RichText, Ui};
use q0ex::page::{parse_page, Align, PageBlock, Span};
use q0ex::PluginDoc;

fn rich(ui: &Ui, span: &Span, base: f32) -> RichText {
    let mut text = RichText::new(&span.text).size(base * span.size);
    if span.bold {
        text = text.strong();
    }
    if span.italic {
        text = text.italics();
    }
    if span.underline {
        text = text.underline();
    }
    if span.strike {
        text = text.strikethrough();
    }
    if span.code {
        text = text
            .monospace()
            .background_color(ui.visuals().extreme_bg_color);
    }
    if let Some([r, g, b, a]) = span.color {
        text = text.color(Color32::from_rgba_unmultiplied(r, g, b, a));
    }
    if span.link.is_some() {
        text = text.color(ui.visuals().hyperlink_color).underline();
    }
    text
}

fn spans_line(ui: &mut Ui, spans: &[Span], base: f32) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for span in spans {
            let text = rich(ui, span, base);
            match &span.link {
                Some(url) => {
                    let response = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
                    if response.clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                    response.on_hover_text(url);
                }
                None => {
                    ui.label(text);
                }
            }
        }
    });
}

fn aligned(ui: &mut Ui, align: Align, add: impl FnOnce(&mut Ui)) {
    match align {
        Align::Left => add(ui),
        Align::Center => {
            ui.vertical_centered(add);
        }
        Align::Right => {
            ui.with_layout(egui::Layout::top_down(egui::Align::Max), add);
        }
    }
}

/// Draws `markup`; pictures come from `doc`'s blobs.
pub(super) fn show(ui: &mut Ui, markup: &str, doc: Option<&PluginDoc>, id: &str) {
    let base = 14.0;
    for block in parse_page(markup) {
        match block {
            PageBlock::Heading(level, spans, align) => {
                let size = match level {
                    1 => 24.0,
                    2 => 19.0,
                    _ => 16.0,
                };
                ui.add_space(if level == 1 { 4.0 } else { 8.0 });
                let spans = spans
                    .into_iter()
                    .map(|span| Span { bold: true, ..span })
                    .collect::<Vec<_>>();
                aligned(ui, align, |ui| spans_line(ui, &spans, size));
                ui.add_space(2.0);
            }
            PageBlock::Paragraph(spans, align) => {
                aligned(ui, align, |ui| spans_line(ui, &spans, base));
                ui.add_space(4.0);
            }
            PageBlock::Bullet(spans, depth) => {
                ui.horizontal(|ui| {
                    ui.add_space(8.0 + f32::from(depth) * 16.0);
                    ui.label(RichText::new(if depth == 0 { "•" } else { "◦" }).size(base));
                    spans_line(ui, &spans, base);
                });
            }
            PageBlock::Numbered(number, spans) => {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("{number}.")).size(base).strong());
                    spans_line(ui, &spans, base);
                });
            }
            PageBlock::Quote(spans) => {
                let accent = ui.visuals().hyperlink_color;
                egui::Frame::none()
                    .fill(ui.visuals().faint_bg_color)
                    .stroke(egui::Stroke::new(1.0_f32, accent.gamma_multiply(0.5)))
                    .rounding(8.0)
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                    .show(ui, |ui| spans_line(ui, &spans, base));
                ui.add_space(4.0);
            }
            PageBlock::Code(code) => {
                egui::Frame::none()
                    .fill(ui.visuals().extreme_bg_color)
                    .rounding(6.0)
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(code).monospace().size(12.5)).wrap(true),
                        );
                    });
                ui.add_space(4.0);
            }
            PageBlock::Rule => {
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
            }
            PageBlock::Image { blob, caption } => {
                let texture = doc
                    .and_then(|doc| doc.blob(&blob))
                    .and_then(|blob| blob.bytes())
                    .and_then(|bytes| {
                        super::cached_texture(ui.ctx(), &format!("plugin-page-{id}-{blob}"), &bytes)
                    });
                ui.vertical_centered(|ui| {
                    match texture {
                        Some(texture) => {
                            let size = texture.size_vec2();
                            let width = size.x.min(ui.available_width());
                            ui.image((
                                texture.id(),
                                egui::vec2(width, width * size.y / size.x.max(1.0)),
                            ));
                        }
                        None => {
                            ui.label(
                                RichText::new(format!("(missing picture {blob})"))
                                    .italics()
                                    .color(ui.visuals().weak_text_color()),
                            );
                        }
                    }
                    if !caption.is_empty() {
                        ui.label(
                            RichText::new(caption)
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                    }
                });
                ui.add_space(4.0);
            }
        }
    }
}
