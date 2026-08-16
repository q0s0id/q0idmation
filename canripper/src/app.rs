use std::path::{Path, PathBuf};

use lyon_path::math::point as lyon_point;
use lyon_tessellation::geometry_builder::{BuffersBuilder, Positions, VertexBuffers};
use lyon_tessellation::{
    FillOptions, FillRule, FillTessellator, LineCap, LineJoin, StrokeOptions, StrokeTessellator,
};

use crate::icons::{self, toolbar_button, UiIcon};

use canripper::{
    document::{Document, EditRef, FormatKind, PreviewRef, TreeNode},
    edit::{
        can_replace_with_image, code_source, replace_with_image_path, set_code_source,
        MAX_CODE_BYTES,
    },
    extract::extract_document,
    preview::{build_preview, preview_frame_count, PreviewContent, VectorPreview},
    save::{can_export_q1s, can_save, export_q1s, save_document},
};

#[derive(Default)]
struct PreviewCache {
    key: Option<(PreviewRef, u32)>,
    vector: Option<VectorPreview>,
    texture: Option<egui::TextureHandle>,
    text: Option<String>,
    caption: String,
    error: Option<String>,
}

pub struct CanRipperApp {
    document: Option<Document>,
    selected_node: Option<usize>,
    preview_node: Option<usize>,
    preview_frame: u32,
    preview: PreviewCache,
    edit_node: Option<usize>,
    edit_ref: Option<EditRef>,
    edit_buffer: String,
    edit_buffer_dirty: bool,
    dirty: bool,
    theme_index: usize,
    status: String,
}

impl CanRipperApp {
    pub fn default_theme_index() -> usize {
        q0theme::BUILTIN_THEMES
            .iter()
            .position(|theme| theme.id == q0theme::Q0S_SIGNATURE_ID)
            .unwrap_or(0)
    }

    pub fn default_theme() -> &'static q0theme::BuiltinTheme {
        &q0theme::BUILTIN_THEMES[Self::default_theme_index()]
    }

    fn theme(&self) -> &'static q0theme::BuiltinTheme {
        q0theme::BUILTIN_THEMES
            .get(self.theme_index)
            .unwrap_or_else(|| Self::default_theme())
    }

    fn accent(&self) -> egui::Color32 {
        let value = self.theme().accent;
        egui::Color32::from_rgb(value.r, value.g, value.b)
    }

    fn text_dim(&self) -> egui::Color32 {
        let value = self.theme().text_dim;
        egui::Color32::from_rgb(value.r, value.g, value.b)
    }
    pub fn new(initial: Option<PathBuf>) -> Self {
        let mut app = Self {
            document: None,
            selected_node: None,
            preview_node: None,
            preview_frame: 0,
            preview: PreviewCache::default(),
            edit_node: None,
            edit_ref: None,
            edit_buffer: String::new(),
            edit_buffer_dirty: false,
            dirty: false,
            theme_index: Self::default_theme_index(),
            status: "drop a q1s, q0s, q0v, swf, or flash projector exe here".to_string(),
        };
        if let Some(path) = initial {
            app.open_path(&path);
        }
        app
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("CanRipper supported", &["q1s", "q0s", "q0v", "swf", "exe"])
            .add_filter("q0idmation", &["q1s", "q0s", "q0v"])
            .add_filter("Flash", &["swf", "exe"])
            .pick_file()
        {
            self.open_path(&path);
        }
    }

    fn open_path(&mut self, path: &Path) {
        match Document::open(path) {
            Ok(document) => {
                self.status = format!(
                    "opened {} • {} • {} bytes",
                    path.display(),
                    document.format.label(),
                    document.bytes.len()
                );
                self.selected_node = None;
                self.preview_node = None;
                self.preview_frame = 0;
                self.preview = PreviewCache::default();
                self.edit_node = None;
                self.edit_ref = None;
                self.edit_buffer.clear();
                self.edit_buffer_dirty = false;
                self.dirty = false;
                self.document = Some(document);
            }
            Err(error) => {
                self.status = format!("could not open {}: {error}", path.display());
            }
        }
    }

    fn refresh_preview(&mut self, ctx: &egui::Context, reference: Option<PreviewRef>) {
        let Some(reference) = reference else {
            self.preview = PreviewCache::default();
            return;
        };
        let key = (reference.clone(), self.preview_frame);
        if self.preview.key.as_ref() == Some(&key) {
            return;
        }
        let result = self
            .document
            .as_ref()
            .ok_or_else(|| "no document open".to_string())
            .and_then(|document| build_preview(document, &reference, self.preview_frame));

        self.preview = PreviewCache {
            key: Some(key),
            ..PreviewCache::default()
        };
        match result {
            Ok(PreviewContent::Vector(vector)) => {
                self.preview.caption = vector.caption.clone();
                self.preview.vector = Some(vector);
            }
            Ok(PreviewContent::Image {
                width,
                height,
                rgba,
                caption,
            }) => {
                if width == 0
                    || height == 0
                    || rgba.len() != width.saturating_mul(height).saturating_mul(4)
                {
                    self.preview.error =
                        Some("preview renderer returned invalid image dimensions".to_string());
                    return;
                }
                let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
                self.preview.texture = Some(ctx.load_texture(
                    "canripper-preview",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
                self.preview.caption = caption;
            }
            Ok(PreviewContent::Text { text, caption }) => {
                self.preview.text = Some(text);
                self.preview.caption = caption;
            }
            Err(error) => {
                self.preview.error = Some(error);
            }
        }
    }
    fn commit_code_edit(&mut self) -> Result<(), String> {
        if !self.edit_buffer_dirty {
            return Ok(());
        }
        let reference = self
            .edit_ref
            .clone()
            .ok_or_else(|| "code editor lost its target".to_string())?;
        let document = self
            .document
            .as_mut()
            .ok_or_else(|| "no document open".to_string())?;
        set_code_source(document, &reference, &self.edit_buffer)?;
        self.edit_buffer_dirty = false;
        Ok(())
    }

    fn sync_editor_to_selection(&mut self) {
        if self.edit_node == self.selected_node {
            return;
        }
        if let Err(error) = self.commit_code_edit() {
            self.status = format!("code change could not be applied: {error}");
            self.selected_node = self.edit_node;
            return;
        }
        self.edit_node = self.selected_node;
        self.edit_ref = self
            .document
            .as_ref()
            .and_then(|document| self.selected_node.and_then(|id| document.node_edit(id)))
            .cloned();
        self.edit_buffer = match self
            .document
            .as_ref()
            .zip(self.edit_ref.as_ref())
            .map(|(document, reference)| code_source(document, reference))
        {
            Some(Ok(source)) => source,
            Some(Err(error)) => {
                self.status = format!("could not load code: {error}");
                String::new()
            }
            None => String::new(),
        };
        self.edit_buffer_dirty = false;
    }

    fn reset_selection_state(&mut self) {
        self.selected_node = None;
        self.preview_node = None;
        self.preview_frame = 0;
        self.preview = PreviewCache::default();
        self.edit_node = None;
        self.edit_ref = None;
        self.edit_buffer.clear();
        self.edit_buffer_dirty = false;
    }

    fn native_save_path(&self) -> Option<PathBuf> {
        let document = self.document.as_ref()?;
        let extension = match document.format {
            FormatKind::Q1s => "q1s",
            FormatKind::Q0s => "q0s",
            _ => return None,
        };
        let default_name = document
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(if extension == "q1s" {
                "project.q1s"
            } else {
                "movie.q0s"
            });
        let mut dialog = rfd::FileDialog::new()
            .add_filter(
                if extension == "q1s" {
                    "q1s project"
                } else {
                    "q0s movie"
                },
                &[extension],
            )
            .set_file_name(default_name);
        if let Some(parent) = document.path.parent() {
            dialog = dialog.set_directory(parent);
        }
        dialog.save_file().map(|mut path| {
            path.set_extension(extension);
            path
        })
    }

    fn save_to_path(&mut self, path: PathBuf) {
        if let Err(error) = self.commit_code_edit() {
            self.status = format!("save blocked: {error}");
            return;
        }
        let result = self
            .document
            .as_ref()
            .ok_or_else(|| "open a file first".to_string())
            .and_then(|document| save_document(document, &path));
        match result {
            Ok(_) => match Document::open(&path) {
                Ok(document) => {
                    self.document = Some(document);
                    self.reset_selection_state();
                    self.dirty = false;
                    self.status = format!("saved {}", path.display());
                }
                Err(error) => {
                    self.status = format!("saved, but reread failed: {error}");
                }
            },
            Err(error) => self.status = format!("save failed: {error}"),
        }
    }

    fn save_current(&mut self) {
        let Some(path) = self.document.as_ref().map(|document| document.path.clone()) else {
            self.status = "open a file first".to_string();
            return;
        };
        self.save_to_path(path);
    }

    fn save_as(&mut self) {
        if self
            .document
            .as_ref()
            .is_none_or(|document| !can_save(document))
        {
            self.status = "this document is read-only".to_string();
            return;
        }
        if let Some(path) = self.native_save_path() {
            self.save_to_path(path);
        } else {
            self.status = "save as cancelled".to_string();
        }
    }

    fn export_to_q1s(&mut self) {
        if let Err(error) = self.commit_code_edit() {
            self.status = format!("export blocked: {error}");
            return;
        }
        let Some(document) = self.document.as_ref() else {
            self.status = "open a file first".to_string();
            return;
        };
        if !can_export_q1s(document) {
            self.status = "this document cannot be exported to q1s losslessly".to_string();
            return;
        }
        let default_name = document
            .path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(|name| format!("{name}.q1s"))
            .unwrap_or_else(|| "project.q1s".to_string());
        let mut dialog = rfd::FileDialog::new()
            .add_filter("q1s project", &["q1s"])
            .set_file_name(default_name);
        if let Some(parent) = document.path.parent() {
            dialog = dialog.set_directory(parent);
        }
        let Some(mut path) = dialog.save_file() else {
            self.status = "q1s export cancelled".to_string();
            return;
        };
        path.set_extension("q1s");
        match export_q1s(document, &path) {
            Ok(_) => self.status = format!("exported q1s to {}", path.display()),
            Err(error) => self.status = format!("q1s export failed: {error}"),
        }
    }

    fn replace_selected_with_image(&mut self) {
        if let Err(error) = self.commit_code_edit() {
            self.status = format!("replace blocked: {error}");
            return;
        }
        let Some(reference) = self
            .document
            .as_ref()
            .and_then(|document| self.selected_node.and_then(|id| document.node_preview(id)))
            .cloned()
        else {
            self.status = "select a vector or bitmap asset first".to_string();
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("image", &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_file()
        else {
            self.status = "replace image cancelled".to_string();
            return;
        };
        let Some(document) = self.document.as_mut() else {
            return;
        };
        match replace_with_image_path(document, &reference, &path) {
            Ok(()) => {
                self.dirty = true;
                self.reset_selection_state();
                self.status = format!("replaced graphics with {}", path.display());
            }
            Err(error) => self.status = format!("replace failed: {error}"),
        }
    }
    fn extract_all(&mut self) {
        let Some(document) = &self.document else {
            self.status = "open a file first".to_string();
            return;
        };
        let parent = document.path.parent().unwrap_or_else(|| Path::new("."));
        let Some(output) = rfd::FileDialog::new()
            .set_directory(parent)
            .set_title("choose CanRipper extraction directory")
            .pick_folder()
        else {
            self.status = "extract cancelled".to_string();
            return;
        };
        match extract_document(document, &output) {
            Ok(report) => {
                self.status = format!(
                    "extracted {} files to {}",
                    report.files_written,
                    report.output.display()
                );
            }
            Err(error) => {
                self.status = format!("extract failed: {error}");
            }
        }
    }
}

impl eframe::App for CanRipperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .find_map(|file| file.path.clone())
        });
        if let Some(path) = dropped {
            self.open_path(&path);
        }

        let (save_shortcut, save_as_shortcut) = ctx.input(|input| {
            (
                input.modifiers.command
                    && !input.modifiers.shift
                    && input.key_pressed(egui::Key::S),
                input.modifiers.command && input.modifiers.shift && input.key_pressed(egui::Key::S),
            )
        });
        if save_as_shortcut {
            self.save_as();
        } else if save_shortcut {
            self.save_current();
        }

        let save_enabled = self.document.as_ref().is_some_and(can_save);
        let export_enabled = self.document.as_ref().is_some_and(can_export_q1s);
        let selected_preview = self
            .document
            .as_ref()
            .and_then(|document| self.selected_node.and_then(|id| document.node_preview(id)))
            .cloned();
        let replace_enabled = self
            .document
            .as_ref()
            .zip(selected_preview.as_ref())
            .is_some_and(|(document, reference)| can_replace_with_image(document, reference));
        let document_meta = self
            .document
            .as_ref()
            .map(|document| (document.format.label(), document.path.display().to_string()));

        let theme_before = self.theme_index;
        let accent = self.accent();
        let text_dim = self.text_dim();
        let theme_name = self.theme().name;
        let extract_enabled = self.document.is_some();

        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::none()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(egui::Margin::symmetric(8.0, 6.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("canripper")
                            .strong()
                            .size(17.0)
                            .color(ctx.style().visuals.text_color()),
                    );
                    if self.dirty {
                        ui.label(egui::RichText::new("●").color(accent).size(11.0))
                            .on_hover_text("document has unsaved changes");
                    }
                    ui.add_space(4.0);
                    ui.separator();

                    if toolbar_button(ui, UiIcon::Open, true, "open", accent).clicked() {
                        self.open_dialog();
                    }
                    if toolbar_button(ui, UiIcon::Save, save_enabled, "save  •  ctrl+s", accent)
                        .clicked()
                    {
                        self.save_current();
                    }
                    if toolbar_button(
                        ui,
                        UiIcon::SaveAs,
                        save_enabled,
                        "save as  •  ctrl+shift+s",
                        accent,
                    )
                    .clicked()
                    {
                        self.save_as();
                    }
                    ui.separator();
                    if toolbar_button(ui, UiIcon::Export, export_enabled, "export to q1s", accent)
                        .clicked()
                    {
                        self.export_to_q1s();
                    }
                    if toolbar_button(
                        ui,
                        UiIcon::ReplaceImage,
                        replace_enabled,
                        "replace selected vector/bitmap with image",
                        accent,
                    )
                    .clicked()
                    {
                        self.replace_selected_with_image();
                    }
                    if toolbar_button(
                        ui,
                        UiIcon::Extract,
                        extract_enabled,
                        "extract all resources",
                        accent,
                    )
                    .clicked()
                    {
                        self.extract_all();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::ComboBox::from_id_source("canripper-theme")
                            .selected_text(theme_name)
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for (index, theme) in q0theme::BUILTIN_THEMES.iter().enumerate() {
                                    let selected = self.theme_index == index;
                                    if ui.selectable_label(selected, theme.name).clicked() {
                                        self.theme_index = index;
                                    }
                                }
                            });
                        icons::tiny_icon(ui, UiIcon::Theme, accent);
                    });
                });

                if let Some((format, path)) = &document_meta {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        let badge_fill = blend_color(ctx.style().visuals.window_fill, accent, 0.14);
                        egui::Frame::none()
                            .fill(badge_fill)
                            .rounding(4.0)
                            .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(*format).strong().color(accent));
                            });
                        ui.add_space(2.0);
                        ui.label(egui::RichText::new(path).small().color(text_dim));
                    });
                }
            });

        if self.theme_index != theme_before {
            let theme = self.theme();
            apply_theme(ctx, theme);
            self.preview = PreviewCache::default();
            self.status = format!("theme: {}", theme.name);
            ctx.request_repaint();
        }
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::none()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("●").color(accent).size(8.0));
                    ui.label(egui::RichText::new(&self.status).small().color(text_dim));
                });
            });

        egui::SidePanel::left("tree")
            .resizable(true)
            .default_width(310.0)
            .min_width(190.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    icons::tiny_icon(ui, UiIcon::Folder, accent);
                    ui.label(egui::RichText::new("structure").strong().size(15.0));
                });
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(2.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(document) = &self.document {
                        for node in &document.roots {
                            show_tree_node(ui, node, &mut self.selected_node, accent);
                        }
                    } else {
                        ui.weak("nothing open");
                    }
                });
            });

        self.sync_editor_to_selection();

        if self.preview_node != self.selected_node {
            self.preview_node = self.selected_node;
            self.preview_frame = 0;
            self.preview = PreviewCache::default();
        }
        let preview_ref = self
            .document
            .as_ref()
            .and_then(|document| self.selected_node.and_then(|id| document.node_preview(id)))
            .cloned();
        let frame_count = self
            .document
            .as_ref()
            .zip(preview_ref.as_ref())
            .map(|(document, reference)| preview_frame_count(document, reference))
            .unwrap_or(1)
            .max(1);
        self.preview_frame = self.preview_frame.min(frame_count - 1);
        self.refresh_preview(ctx, preview_ref.clone());

        let document_name = self.document.as_ref().map(|document| {
            document
                .path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("document")
                .to_string()
        });
        let detail = self.document.as_ref().map(|document| {
            self.selected_node
                .and_then(|id| document.node_detail(id))
                .unwrap_or(&document.summary)
                .to_string()
        });
        let raw_header = self
            .document
            .as_ref()
            .map(|document| hex_preview(&document.bytes, 256));

        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(document_name) = document_name.as_deref() else {
                ui.vertical_centered(|ui| {
                    ui.add_space(90.0);
                    ui.heading("canripper");
                    ui.label("q0idmation + flash container inspector/editor");
                    ui.add_space(12.0);
                    ui.weak("open a file or drag it into this window");
                });
                return;
            };

            ui.label(
                egui::RichText::new(document_name)
                    .strong()
                    .size(18.0)
                    .color(ui.visuals().text_color()),
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if preview_ref.is_some() {
                        section_frame(ui, accent).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                icons::tiny_icon(ui, UiIcon::Document, accent);
                                ui.label(egui::RichText::new("preview").strong().size(14.0));
                                if frame_count > 1 {
                                    ui.separator();
                                    let changed = ui
                                        .add(
                                            egui::Slider::new(
                                                &mut self.preview_frame,
                                                0..=frame_count - 1,
                                            )
                                            .show_value(false),
                                        )
                                        .changed();
                                    ui.monospace(format!(
                                        "{}/{}",
                                        self.preview_frame + 1,
                                        frame_count
                                    ));
                                    if changed {
                                        self.preview.key = None;
                                        ctx.request_repaint();
                                    }
                                }
                            });
                            ui.add_space(7.0);
                            show_preview_content(ui, &mut self.preview);
                        });
                        ui.add_space(8.0);
                    }

                    if self.edit_ref.is_some() {
                        section_frame(ui, accent).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                icons::tiny_icon(ui, UiIcon::Code, accent);
                                ui.label(egui::RichText::new("code editor").strong().size(14.0));
                                ui.separator();
                                let bytes = self.edit_buffer.len();
                                if bytes > MAX_CODE_BYTES {
                                    ui.colored_label(
                                        ui.visuals().error_fg_color,
                                        format!("{bytes} bytes • too large"),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new(format!("{bytes} bytes"))
                                            .small()
                                            .color(
                                                ui.visuals()
                                                    .widgets
                                                    .noninteractive
                                                    .fg_stroke
                                                    .color,
                                            ),
                                    );
                                }
                            });
                            ui.add_space(6.0);
                            let response = ui.add(
                                egui::TextEdit::multiline(&mut self.edit_buffer)
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(20),
                            );
                            if response.changed() {
                                self.edit_buffer_dirty = true;
                                self.dirty = true;
                                self.status =
                                    "code modified • save to write changes".to_string();
                            }
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "changes are repacked through nested q0s containers on selection change/save",
                                )
                                .small()
                                .color(
                                    ui.visuals()
                                        .widgets
                                        .noninteractive
                                        .fg_stroke
                                        .color,
                                ),
                            );
                        });
                        ui.add_space(8.0);
                    }

                    section_frame(ui, accent).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            icons::tiny_icon(ui, UiIcon::Binary, accent);
                            ui.label(egui::RichText::new("details").strong().size(14.0));
                        });
                        ui.add_space(6.0);
                        let mut detail = detail.clone().unwrap_or_default();
                        ui.add(
                            egui::TextEdit::multiline(&mut detail)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                        ui.add_space(6.0);
                        egui::CollapsingHeader::new("raw header • first 256 bytes")
                            .default_open(false)
                            .show(ui, |ui| {
                                ui.monospace(raw_header.clone().unwrap_or_default());
                            });
                    });
                });        });
    }
}

fn section_frame(ui: &egui::Ui, accent: egui::Color32) -> egui::Frame {
    let fill = blend_color(ui.visuals().window_fill, ui.visuals().panel_fill, 0.34);
    let border = blend_color(ui.visuals().window_stroke.color, accent, 0.22);
    egui::Frame::none()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0_f32, border))
        .rounding(6.0)
        .inner_margin(egui::Margin::same(10.0))
}
fn show_preview_content(ui: &mut egui::Ui, preview: &mut PreviewCache) {
    if !preview.caption.is_empty() {
        ui.weak(&preview.caption);
        ui.add_space(4.0);
    }
    if let Some(error) = &preview.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
        return;
    }
    if let Some(vector) = &preview.vector {
        show_vector_preview(ui, vector);
        return;
    }
    if let Some(texture) = &preview.texture {
        let native = texture.size_vec2();
        let max_width = ui.available_width().max(1.0);
        let max_height = 460.0_f32.min(ui.available_height().max(120.0));
        let scale = (max_width / native.x)
            .min(max_height / native.y)
            .clamp(0.01, 4.0);
        let size = native * scale;
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        paint_checker(ui.painter(), rect);
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        return;
    }
    if let Some(text) = &mut preview.text {
        ui.add(
            egui::TextEdit::multiline(text)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY)
                .desired_rows(16)
                .interactive(false),
        );
    } else {
        ui.weak("no preview");
    }
}

fn show_vector_preview(ui: &mut egui::Ui, vector: &VectorPreview) {
    let (min_x, min_y, max_x, max_y) = vector.bounds;
    let width = (max_x - min_x).abs().max(1.0);
    let height = (max_y - min_y).abs().max(1.0);
    let available_width = ui.available_width().max(120.0);
    let max_height = 460.0_f32;
    let padding = 18.0_f32;
    let scale = ((available_width - padding * 2.0).max(1.0) / width)
        .min((max_height - padding * 2.0).max(1.0) / height)
        .max(0.01);
    let size = egui::vec2(
        (width * scale + padding * 2.0).min(available_width),
        (height * scale + padding * 2.0).min(max_height),
    );
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    paint_checker(ui.painter(), rect);
    paint_vector_geometry(ui.painter(), rect, vector);
}

fn paint_vector_geometry(painter: &egui::Painter, rect: egui::Rect, vector: &VectorPreview) {
    let (min_x, min_y, max_x, max_y) = vector.bounds;
    let width = (max_x - min_x).abs().max(1.0);
    let height = (max_y - min_y).abs().max(1.0);
    let padding = 18.0_f32;
    let scale = ((rect.width() - padding * 2.0).max(1.0) / width)
        .min((rect.height() - padding * 2.0).max(1.0) / height)
        .max(0.01);
    let content_width = width * scale;
    let content_height = height * scale;
    let origin = egui::pos2(
        rect.center().x - content_width * 0.5 - min_x * scale,
        rect.center().y - content_height * 0.5 - min_y * scale,
    );
    let to_screen = |point: q0s_format::v2::Vec2| {
        egui::pos2(origin.x + point.x * scale, origin.y + point.y * scale)
    };

    if let Some(fill) = vector.fill {
        let path = build_lyon_path(&vector.paths, &to_screen, true);
        let mut buffers: VertexBuffers<lyon_path::math::Point, u32> = VertexBuffers::new();
        let mut tessellator = FillTessellator::new();
        let options = FillOptions::default()
            .with_fill_rule(FillRule::NonZero)
            .with_tolerance(0.05);
        if tessellator
            .tessellate_path(
                &path,
                &options,
                &mut BuffersBuilder::new(&mut buffers, Positions),
            )
            .is_ok()
        {
            painter.add(egui::Shape::Mesh(lyon_mesh(buffers, rgba_color(fill))));
        }
    }

    if let Some(stroke) = vector.stroke {
        let path = build_lyon_path(&vector.paths, &to_screen, false);
        let mut buffers: VertexBuffers<lyon_path::math::Point, u32> = VertexBuffers::new();
        let cap = match stroke.cap {
            q0s_format::geom::CapShape::Round => LineCap::Round,
            q0s_format::geom::CapShape::Butt => LineCap::Butt,
        };
        let options = StrokeOptions::default()
            .with_line_width((stroke.width * scale).max(0.5))
            .with_line_join(LineJoin::Round)
            .with_start_cap(cap)
            .with_end_cap(cap)
            .with_tolerance(0.05);
        let mut tessellator = StrokeTessellator::new();
        if tessellator
            .tessellate_path(
                &path,
                &options,
                &mut BuffersBuilder::new(&mut buffers, Positions),
            )
            .is_ok()
        {
            painter.add(egui::Shape::Mesh(lyon_mesh(
                buffers,
                rgba_color(stroke.color),
            )));
        }
    }
}

fn build_lyon_path(
    paths: &[q0s_format::v2::Path],
    to_screen: &impl Fn(q0s_format::v2::Vec2) -> egui::Pos2,
    closed_only: bool,
) -> lyon_path::Path {
    let mut builder = lyon_path::Path::builder();
    for path in paths {
        if path.anchors.is_empty() || (closed_only && !path.closed) {
            continue;
        }
        let first = to_screen(path.anchors[0].point);
        builder.begin(lyon_point(first.x, first.y));
        let segment_count = if path.closed {
            path.anchors.len()
        } else {
            path.anchors.len().saturating_sub(1)
        };
        for index in 0..segment_count {
            let a = &path.anchors[index];
            let b = &path.anchors[(index + 1) % path.anchors.len()];
            let end = to_screen(b.point);
            if a.out_handle.is_none() && b.in_handle.is_none() {
                builder.line_to(lyon_point(end.x, end.y));
            } else {
                let c1 = to_screen(a.out_handle.unwrap_or(a.point));
                let c2 = to_screen(b.in_handle.unwrap_or(b.point));
                builder.cubic_bezier_to(
                    lyon_point(c1.x, c1.y),
                    lyon_point(c2.x, c2.y),
                    lyon_point(end.x, end.y),
                );
            }
        }
        builder.end(path.closed);
    }
    builder.build()
}

fn lyon_mesh(
    buffers: VertexBuffers<lyon_path::math::Point, u32>,
    color: egui::Color32,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    mesh.vertices.reserve(buffers.vertices.len());
    for point in buffers.vertices {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui::pos2(point.x, point.y),
            uv: egui::Pos2::ZERO,
            color,
        });
    }
    mesh.indices = buffers.indices;
    mesh
}

fn rgba_color(color: q0s_format::v2::Rgba) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}
fn paint_checker(painter: &egui::Painter, rect: egui::Rect) {
    let visuals = &painter.ctx().style().visuals;
    let base = blend_color(visuals.extreme_bg_color, visuals.panel_fill, 0.34);
    let alternate = blend_color(base, visuals.widgets.noninteractive.fg_stroke.color, 0.08);
    painter.rect_filled(rect, 4.0, base);
    let cell = 16.0;
    let cols = (rect.width() / cell).ceil() as usize;
    let rows = (rect.height() / cell).ceil() as usize;
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 == 0 {
                let min = egui::pos2(rect.left() + x as f32 * cell, rect.top() + y as f32 * cell);
                let max = egui::pos2(
                    (min.x + cell).min(rect.right()),
                    (min.y + cell).min(rect.bottom()),
                );
                painter.rect_filled(egui::Rect::from_min_max(min, max), 0.0, alternate);
            }
        }
    }
}
fn show_tree_node(
    ui: &mut egui::Ui,
    node: &TreeNode,
    selected: &mut Option<usize>,
    accent: egui::Color32,
) {
    if node.children.is_empty() {
        if icons::tree_row(ui, node, *selected == Some(node.id), accent).clicked() {
            *selected = Some(node.id);
        }
        return;
    }

    let id = ui.make_persistent_id(("canripper-tree", node.id));
    let state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
    let header = state.show_header(ui, |ui| {
        icons::tree_row(ui, node, *selected == Some(node.id), accent)
    });
    let (_, header_response, _) = header.body(|ui| {
        for child in &node.children {
            show_tree_node(ui, child, selected, accent);
        }
    });
    if header_response.inner.clicked() {
        *selected = Some(node.id);
    }
}
fn hex_preview(bytes: &[u8], max: usize) -> String {
    let mut out = String::new();
    for (row, chunk) in bytes[..bytes.len().min(max)].chunks(16).enumerate() {
        use std::fmt::Write as _;
        let _ = write!(out, "{:08x}  ", row * 16);
        for index in 0..16 {
            if let Some(byte) = chunk.get(index) {
                let _ = write!(out, "{byte:02x} ");
            } else {
                out.push_str("   ");
            }
            if index == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        for byte in chunk {
            out.push(if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte)
            } else {
                '.'
            });
        }
        out.push('\n');
    }
    if bytes.len() > max {
        out.push_str("…\n");
    }
    out
}

pub fn apply_theme(ctx: &egui::Context, theme: &q0theme::BuiltinTheme) {
    use egui::{vec2, Rounding, Stroke};

    let rgb = |value: q0theme::Rgb| egui::Color32::from_rgb(value.r, value.g, value.b);
    let accent = rgb(theme.accent);
    let panel = rgb(theme.panel);
    let window = rgb(theme.window);
    let deep = rgb(theme.deep_bg);
    let text = rgb(theme.text);
    let stroke_dark = rgb(theme.stroke_dark);
    let dark = u16::from(theme.text.r) + u16::from(theme.text.g) + u16::from(theme.text.b)
        > u16::from(theme.window.r) + u16::from(theme.window.g) + u16::from(theme.window.b);
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    visuals.panel_fill = panel;
    visuals.window_fill = window;
    visuals.extreme_bg_color = deep;
    visuals.faint_bg_color = blend_color(window, panel, 0.48);
    visuals.code_bg_color = deep;
    visuals.window_stroke = Stroke::new(1.0_f32, stroke_dark);
    visuals.window_rounding = Rounding::same(5.0);
    visuals.menu_rounding = Rounding::same(4.0);
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: egui::vec2(0.0, 4.0),
        blur: 16.0,
        spread: 0.0,
        color: egui::Color32::from_black_alpha(if dark { 145 } else { 45 }),
    };
    visuals.hyperlink_color = accent;
    visuals.selection.bg_fill = accent;
    visuals.selection.stroke = Stroke::new(1.0_f32, text);
    visuals.warn_fg_color = blend_color(text, egui::Color32::from_rgb(255, 190, 55), 0.72);
    visuals.error_fg_color = blend_color(text, egui::Color32::from_rgb(255, 80, 92), 0.78);

    let inactive_bg = blend_color(window, panel, 0.62);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
    ] {
        widget.bg_fill = inactive_bg;
        widget.weak_bg_fill = inactive_bg;
        widget.bg_stroke = Stroke::new(1.0_f32, stroke_dark);
        widget.fg_stroke = Stroke::new(1.0_f32, text);
        widget.rounding = Rounding::same(4.0);
    }

    visuals.widgets.hovered.bg_fill = blend_color(panel, accent, 0.18);
    visuals.widgets.hovered.weak_bg_fill = blend_color(panel, accent, 0.14);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, accent.gamma_multiply(0.82));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, text);
    visuals.widgets.hovered.rounding = Rounding::same(4.0);

    visuals.widgets.active.bg_fill = blend_color(panel, accent, 0.31);
    visuals.widgets.active.weak_bg_fill = blend_color(panel, accent, 0.26);
    visuals.widgets.active.bg_stroke = Stroke::new(1.2_f32, accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, text);
    visuals.widgets.active.rounding = Rounding::same(4.0);
    visuals.widgets.open = visuals.widgets.active;

    let mut style = (*ctx.style()).clone();
    style.visuals = visuals;
    style.spacing.item_spacing = vec2(6.0, 5.0);
    style.spacing.button_padding = vec2(7.0, 4.0);
    style.spacing.menu_margin = egui::Margin::same(6.0);
    style.spacing.window_margin = egui::Margin::same(8.0);
    style.spacing.indent = 16.0;
    style.spacing.interact_size.y = 27.0;
    style.spacing.scroll.bar_width = 9.0;
    style.spacing.scroll.bar_inner_margin = 2.0;
    ctx.set_style(style);
}

fn blend_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let mix = |x: u8, y: u8| {
        (f32::from(x) + (f32::from(y) - f32::from(x)) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    egui::Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_canripper_theme_is_q0s_signature() {
        assert_eq!(CanRipperApp::default_theme().id, q0theme::Q0S_SIGNATURE_ID);
    }

    #[test]
    fn every_shared_builtin_theme_applies_its_palette_to_canripper_chrome() {
        let ctx = egui::Context::default();
        for theme in q0theme::BUILTIN_THEMES {
            apply_theme(&ctx, theme);
            let style = ctx.style();
            let rgb = |value: q0theme::Rgb| egui::Color32::from_rgb(value.r, value.g, value.b);
            assert_eq!(
                style.visuals.panel_fill,
                rgb(theme.panel),
                "{} panel",
                theme.id
            );
            assert_eq!(
                style.visuals.window_fill,
                rgb(theme.window),
                "{} window",
                theme.id
            );
            assert_eq!(
                style.visuals.extreme_bg_color,
                rgb(theme.deep_bg),
                "{} deep bg",
                theme.id
            );
            assert_eq!(
                style.visuals.code_bg_color,
                rgb(theme.deep_bg),
                "{} code bg",
                theme.id
            );
            assert_eq!(
                style.visuals.selection.bg_fill,
                rgb(theme.accent),
                "{} accent",
                theme.id
            );
        }
    }

    #[test]
    fn vector_preview_keeps_cubic_bezier_segments() {
        let source = q0s_format::v2::Path {
            anchors: vec![
                q0s_format::v2::Anchor {
                    point: q0s_format::v2::Vec2::new(0.0, 0.0),
                    in_handle: None,
                    out_handle: Some(q0s_format::v2::Vec2::new(20.0, 0.0)),
                },
                q0s_format::v2::Anchor {
                    point: q0s_format::v2::Vec2::new(40.0, 20.0),
                    in_handle: Some(q0s_format::v2::Vec2::new(20.0, 20.0)),
                    out_handle: None,
                },
            ],
            closed: false,
        };
        let path = build_lyon_path(&[source], &|point| egui::pos2(point.x, point.y), false);
        assert!(path
            .iter()
            .any(|event| matches!(event, lyon_path::Event::Cubic { .. })));
    }

    #[test]
    fn vector_preview_nonzero_fill_preserves_a_hole() {
        fn square(points: &[(f32, f32)]) -> q0s_format::v2::Path {
            q0s_format::v2::Path {
                anchors: points
                    .iter()
                    .map(|(x, y)| q0s_format::v2::Anchor {
                        point: q0s_format::v2::Vec2::new(*x, *y),
                        in_handle: None,
                        out_handle: None,
                    })
                    .collect(),
                closed: true,
            }
        }
        let outer = square(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]);
        let hole = square(&[(25.0, 25.0), (25.0, 75.0), (75.0, 75.0), (75.0, 25.0)]);
        let path = build_lyon_path(&[outer, hole], &|point| egui::pos2(point.x, point.y), true);
        let mut buffers: VertexBuffers<lyon_path::math::Point, u32> = VertexBuffers::new();
        FillTessellator::new()
            .tessellate_path(
                &path,
                &FillOptions::default().with_fill_rule(FillRule::NonZero),
                &mut BuffersBuilder::new(&mut buffers, Positions),
            )
            .expect("tessellate vector preview with hole");
        let area: f32 = buffers
            .indices
            .chunks_exact(3)
            .map(|triangle| {
                let a = buffers.vertices[triangle[0] as usize];
                let b = buffers.vertices[triangle[1] as usize];
                let c = buffers.vertices[triangle[2] as usize];
                ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() * 0.5
            })
            .sum();
        assert!((area - 7_500.0).abs() < 1.0, "hole area drifted: {area}");
    }
}
