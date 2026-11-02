//! CanRipper's window, laid out like the decompilers of its era:
//!
//!   ┌──────────────────────────────────────────────────────────────┐
//!   │ File  Edit  View  Help                                        │
//!   │ [Open][Save][Save as] | [Export][Extract all][To q1s] | ...    │
//!   ├───────────────┬──────────────────────────────────────────────┤
//!   │▓ Resources ▓▓▓│▓ selected item ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│
//!   │ [filter…]     │ ╭Preview╮╭Hex╮╭Info╮╭Code╮                    │
//!   │ ⊟ shapes      │ │ playback · zoom                            │
//!   │ ┊ ├ shape 1   │ │ checkerboard canvas                        │
//!   ├───────────────┴──────────────────────────────────────────────┤
//!   │ status                      │ format │ size │ items │ skin    │
//!   └──────────────────────────────────────────────────────────────┘

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use egui::{pos2, vec2, Color32, Rect, Sense, Stroke};
use lyon_path::math::point as lyon_point;
use lyon_tessellation::geometry_builder::{BuffersBuilder, Positions, VertexBuffers};
use lyon_tessellation::{
    FillOptions, FillRule, FillTessellator, LineCap, LineJoin, StrokeOptions, StrokeTessellator,
};
use serde::{Deserialize, Serialize};

use crate::icons::{draw_icon, icon_for_tree_node, UiIcon};
use crate::skin::{self, Skin};
use crate::{
    document::{Document, EditRef, FormatKind, PreviewRef},
    edit::{
        can_replace_with_image, code_source, replace_with_image_path, set_code_source,
        MAX_CODE_BYTES,
    },
    extract::{export_selection, extract_document},
    preview::{
        build_preview, node_bytes, preview_fps, preview_frame_count, PreviewContent, VectorPreview,
    },
    save::{can_export_q1s, can_save, export_q1s, save_document},
};

const ROW_HEIGHT: f32 = 18.0;
const INDENT: f32 = 16.0;
const MAX_RECENT: usize = 10;

#[derive(Default)]
struct PreviewCache {
    key: Option<(PreviewRef, u32)>,
    vector: Option<VectorPreview>,
    texture: Option<egui::TextureHandle>,
    text: Option<String>,
    caption: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Preview,
    Hex,
    Info,
    Code,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Preview => "Preview",
            Self::Hex => "Hex",
            Self::Info => "Info",
            Self::Code => "Code",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Zoom {
    Fit,
    Scale(f32),
}

/// What CanRipper remembers between runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    skin: String,
    recent: Vec<PathBuf>,
}

impl Settings {
    fn path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("canripper").join("settings.json"))
    }

    fn load() -> Self {
        Self::path()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, bytes);
        }
    }

    fn push_recent(&mut self, path: &Path) {
        self.recent.retain(|known| known != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }
}

/// One visible row of the resource tree.
#[derive(Debug, Clone, Copy)]
struct Row {
    id: usize,
    depth: usize,
    children: bool,
    open: bool,
    last: bool,
    /// For each depth above this row, whether a line runs on down past it.
    lines: u64,
}

/// The bytes the hex tab shows, and for which node.
struct HexView {
    node: Option<usize>,
    bytes: Vec<u8>,
    base: usize,
    label: String,
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
    status: String,
    settings: Settings,
    skin: &'static Skin,
    skin_applied: Option<&'static str>,
    expanded: HashSet<usize>,
    filter: String,
    rows: Option<Vec<Row>>,
    /// Bring this row into view on the next frame.
    reveal: Option<usize>,
    /// The tree's scroll offset and height, last frame.
    tree_view: (f32, f32),
    tab: Tab,
    playing: bool,
    play_clock: f32,
    last_tick: Instant,
    zoom: Zoom,
    pan: egui::Vec2,
    hex: Option<HexView>,
    about_open: bool,
    focus_filter: bool,
    pending_title: Option<String>,
}

impl CanRipperApp {
    pub fn new(initial: Option<PathBuf>, ctx: &egui::Context) -> Self {
        let settings = Settings::load();
        let skin = Skin::by_id(&settings.skin);
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
            status: "Open a q1s, q0s, q0v, swf or Flash projector — or drop one here.".to_string(),
            settings,
            skin,
            skin_applied: None,
            expanded: HashSet::new(),
            filter: String::new(),
            rows: None,
            reveal: None,
            tree_view: (0.0, 0.0),
            tab: Tab::Preview,
            playing: false,
            play_clock: 0.0,
            last_tick: Instant::now(),
            zoom: Zoom::Fit,
            pan: egui::Vec2::ZERO,
            hex: None,
            about_open: false,
            focus_filter: false,
            pending_title: None,
        };
        app.apply_skin(ctx);
        if let Some(path) = initial {
            app.open_path(&path);
        }
        app
    }

    fn apply_skin(&mut self, ctx: &egui::Context) {
        if self.skin_applied != Some(self.skin.id) {
            self.skin.apply(ctx);
            self.skin_applied = Some(self.skin.id);
        }
    }

    fn set_skin(&mut self, skin: &'static Skin) {
        self.skin = skin;
        self.settings.skin = skin.id.to_string();
        self.settings.save();
        self.preview = PreviewCache::default();
        self.status = format!("Skin: {}", skin.name);
    }

    // ------------------------------------------------------------ files

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
        let started = Instant::now();
        match Document::open(path) {
            Ok(document) => {
                self.status = format!(
                    "Opened {} • {} • {} in {:.0} ms",
                    path.display(),
                    document.format.label(),
                    byte_size(document.bytes.len()),
                    started.elapsed().as_secs_f64() * 1000.0
                );
                self.reset_selection_state();
                self.expanded = default_expansion(&document);
                self.filter.clear();
                self.rows = None;
                self.dirty = false;
                self.tab = Tab::Preview;
                self.pending_title = Some(file_name(path));
                self.settings.push_recent(path);
                self.settings.save();
                self.document = Some(document);
            }
            Err(error) => {
                self.status = format!("Could not open {}: {error}", path.display());
            }
        }
    }

    fn close_document(&mut self) {
        self.document = None;
        self.reset_selection_state();
        self.rows = None;
        self.dirty = false;
        self.pending_title = Some(String::new());
        self.status = "Closed.".to_string();
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
        let texture = self.preview.texture.take();
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
                // Frames of one animation reuse one texture.
                let texture = match texture {
                    Some(mut texture) => {
                        texture.set(image, egui::TextureOptions::LINEAR);
                        texture
                    }
                    None => {
                        ctx.load_texture("canripper-preview", image, egui::TextureOptions::LINEAR)
                    }
                };
                self.preview.texture = Some(texture);
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
            self.status = format!("Code change could not be applied: {error}");
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
                self.status = format!("Could not load code: {error}");
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
        self.playing = false;
        self.zoom = Zoom::Fit;
        self.pan = egui::Vec2::ZERO;
        self.hex = None;
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
            self.status = format!("Save blocked: {error}");
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
                    self.expanded = default_expansion(&document);
                    self.document = Some(document);
                    self.reset_selection_state();
                    self.rows = None;
                    self.dirty = false;
                    self.status = format!("Saved {}", path.display());
                }
                Err(error) => {
                    self.status = format!("Saved, but reading it back failed: {error}");
                }
            },
            Err(error) => self.status = format!("Save failed: {error}"),
        }
    }

    fn save_current(&mut self) {
        let Some(path) = self.document.as_ref().map(|document| document.path.clone()) else {
            self.status = "Open a file first.".to_string();
            return;
        };
        if self
            .document
            .as_ref()
            .is_some_and(|document| !can_save(document))
        {
            self.status = "This document is read-only.".to_string();
            return;
        }
        self.save_to_path(path);
    }

    fn save_as(&mut self) {
        if self
            .document
            .as_ref()
            .is_none_or(|document| !can_save(document))
        {
            self.status = "This document is read-only.".to_string();
            return;
        }
        if let Some(path) = self.native_save_path() {
            self.save_to_path(path);
        } else {
            self.status = "Save as cancelled.".to_string();
        }
    }

    fn export_to_q1s(&mut self) {
        if let Err(error) = self.commit_code_edit() {
            self.status = format!("Export blocked: {error}");
            return;
        }
        let Some(document) = self.document.as_ref() else {
            self.status = "Open a file first.".to_string();
            return;
        };
        if !can_export_q1s(document) {
            self.status = "This document cannot become a q1s without losing something.".to_string();
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
            self.status = "q1s export cancelled.".to_string();
            return;
        };
        path.set_extension("q1s");
        match export_q1s(document, &path) {
            Ok(_) => self.status = format!("Exported q1s to {}", path.display()),
            Err(error) => self.status = format!("q1s export failed: {error}"),
        }
    }

    fn selected_preview_ref(&self) -> Option<PreviewRef> {
        self.document
            .as_ref()
            .and_then(|document| self.selected_node.and_then(|id| document.node_preview(id)))
            .cloned()
    }

    fn selected_label(&self) -> Option<String> {
        self.document.as_ref().and_then(|document| {
            self.selected_node
                .and_then(|id| document.node(id))
                .map(|node| node.label.clone())
        })
    }

    fn export_selected(&mut self) {
        let (Some(document), Some(reference)) =
            (self.document.as_ref(), self.selected_preview_ref())
        else {
            self.status = "Select something to export first.".to_string();
            return;
        };
        let label = self.selected_label().unwrap_or_default();
        let exported = match export_selection(document, &reference, self.preview_frame, &label) {
            Ok(exported) => exported,
            Err(error) => {
                self.status = format!("Export failed: {error}");
                return;
            }
        };
        let mut dialog = rfd::FileDialog::new().set_file_name(&exported.name);
        if let Some(parent) = document.path.parent() {
            dialog = dialog.set_directory(parent);
        }
        let Some(path) = dialog.save_file() else {
            self.status = "Export cancelled.".to_string();
            return;
        };
        match std::fs::write(&path, &exported.bytes) {
            Ok(()) => {
                self.status = format!(
                    "Exported {} ({})",
                    path.display(),
                    byte_size(exported.bytes.len())
                )
            }
            Err(error) => self.status = format!("Export failed: {error}"),
        }
    }

    fn replace_selected_with_image(&mut self) {
        if let Err(error) = self.commit_code_edit() {
            self.status = format!("Replace blocked: {error}");
            return;
        }
        let Some(reference) = self.selected_preview_ref() else {
            self.status = "Select a vector or bitmap asset first.".to_string();
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("image", &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_file()
        else {
            self.status = "Replace cancelled.".to_string();
            return;
        };
        let Some(document) = self.document.as_mut() else {
            return;
        };
        match replace_with_image_path(document, &reference, &path) {
            Ok(()) => {
                self.dirty = true;
                self.reset_selection_state();
                self.rows = None;
                self.status = format!("Replaced graphics with {}", path.display());
            }
            Err(error) => self.status = format!("Replace failed: {error}"),
        }
    }

    fn extract_all(&mut self) {
        let Some(document) = &self.document else {
            self.status = "Open a file first.".to_string();
            return;
        };
        let parent = document.path.parent().unwrap_or_else(|| Path::new("."));
        let Some(output) = rfd::FileDialog::new()
            .set_directory(parent)
            .set_title("Choose where CanRipper extracts to")
            .pick_folder()
        else {
            self.status = "Extract cancelled.".to_string();
            return;
        };
        let started = Instant::now();
        match extract_document(document, &output) {
            Ok(report) => {
                self.status = format!(
                    "Extracted {} files to {} in {:.1} s",
                    report.files_written,
                    report.output.display(),
                    started.elapsed().as_secs_f32()
                );
            }
            Err(error) => {
                self.status = format!("Extract failed: {error}");
            }
        }
    }

    // ------------------------------------------------------------- tree

    fn rows(&mut self) -> &[Row] {
        self.ensure_rows();
        self.rows.as_deref().unwrap_or_default()
    }

    fn ensure_rows(&mut self) {
        if self.rows.is_none() {
            let rows = self
                .document
                .as_ref()
                .map(|document| build_rows(document, &self.expanded, &self.filter))
                .unwrap_or_default();
            self.rows = Some(rows);
        }
    }

    fn select(&mut self, id: usize) {
        if self.selected_node != Some(id) {
            self.selected_node = Some(id);
            if let Some(index) = self.rows().iter().position(|row| row.id == id) {
                self.reveal = Some(index);
            }
        }
    }

    fn toggle(&mut self, id: usize) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.rows = None;
    }

    fn set_all_expanded(&mut self, open: bool) {
        self.expanded.clear();
        if open {
            if let Some(document) = &self.document {
                fn walk(nodes: &[crate::document::TreeNode], out: &mut HashSet<usize>) {
                    for node in nodes {
                        // Folders of tens of thousands stay shut.
                        if !node.children.is_empty() && node.children.len() <= 5_000 {
                            out.insert(node.id);
                            walk(&node.children, out);
                        }
                    }
                }
                walk(&document.roots, &mut self.expanded);
            }
        }
        self.rows = None;
    }

    /// Up, down, left and right in the tree, as Windows trees work.
    fn tree_keys(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || self.document.is_none() {
            return;
        }
        let keys = ctx.input(|input| {
            [
                egui::Key::ArrowUp,
                egui::Key::ArrowDown,
                egui::Key::ArrowLeft,
                egui::Key::ArrowRight,
                egui::Key::Home,
                egui::Key::End,
                egui::Key::PageUp,
                egui::Key::PageDown,
            ]
            .into_iter()
            .filter(|key| input.key_pressed(*key))
            .collect::<Vec<_>>()
        });
        if keys.is_empty() {
            return;
        }
        let page = ((self.tree_view.1 / ROW_HEIGHT) as usize).max(1);
        for key in keys {
            self.ensure_rows();
            let rows = self.rows.as_deref().unwrap_or_default();
            if rows.is_empty() {
                return;
            }
            let current = self
                .selected_node
                .and_then(|id| rows.iter().position(|row| row.id == id));
            let last = rows.len() - 1;
            let target = match (key, current) {
                (egui::Key::ArrowUp, Some(i)) => Some(i.saturating_sub(1)),
                (egui::Key::ArrowDown, Some(i)) => Some((i + 1).min(last)),
                (egui::Key::PageUp, Some(i)) => Some(i.saturating_sub(page)),
                (egui::Key::PageDown, Some(i)) => Some((i + page).min(last)),
                (egui::Key::Home, _) | (_, None) => Some(0),
                (egui::Key::End, _) => Some(last),
                (egui::Key::ArrowRight, Some(i)) => {
                    let row = rows[i];
                    if row.children && !row.open {
                        self.toggle(row.id);
                        None
                    } else if row.children {
                        Some((i + 1).min(last))
                    } else {
                        None
                    }
                }
                (egui::Key::ArrowLeft, Some(i)) => {
                    let row = rows[i];
                    if row.children && row.open {
                        self.toggle(row.id);
                        None
                    } else {
                        // Up to the parent: the nearest row above, one level up.
                        rows[..i]
                            .iter()
                            .rposition(|above| above.depth + 1 == row.depth)
                    }
                }
                _ => None,
            };
            if let Some(index) = target {
                let id = self.rows()[index].id;
                self.selected_node = Some(id);
                self.reveal = Some(index);
            }
        }
    }

    // ------------------------------------------------------------ chrome

    fn menu_bar(&mut self, ctx: &egui::Context) {
        let skin = self.skin;
        egui::TopBottomPanel::top("menu")
            .frame(egui::Frame::none().fill(skin.face).inner_margin(egui::Margin::symmetric(4.0, 1.0)))
            .show(ctx, |ui| {
                skin::face_gradient(ui.painter(), ui.max_rect().expand2(vec2(4.0, 1.0)), skin);
                egui::menu::bar(ui, |ui| {
                    ui.menu_button("File", |ui| self.file_menu(ui));
                    ui.menu_button("Edit", |ui| {
                        let has_selection = self.selected_node.is_some();
                        if menu_item(ui, "Export selection…", "Ctrl+E", self.selected_preview_ref().is_some()) {
                            ui.close_menu();
                            self.export_selected();
                        }
                        let replace = self.replace_enabled();
                        if menu_item(ui, "Replace with image…", "", replace) {
                            ui.close_menu();
                            self.replace_selected_with_image();
                        }
                        ui.separator();
                        if menu_item(ui, "Copy details", "", has_selection || self.document.is_some()) {
                            ui.close_menu();
                            let detail = self.detail_text();
                            ui.output_mut(|output| output.copied_text = detail);
                            self.status = "Details copied.".to_string();
                        }
                        if menu_item(ui, "Find in tree…", "Ctrl+F", self.document.is_some()) {
                            ui.close_menu();
                            self.focus_filter = true;
                        }
                    });
                    ui.menu_button("View", |ui| {
                        for tab in [Tab::Preview, Tab::Hex, Tab::Info, Tab::Code] {
                            if ui.radio(self.tab == tab, tab.label()).clicked() {
                                ui.close_menu();
                                self.tab = tab;
                            }
                        }
                        ui.separator();
                        if menu_item(ui, "Expand all", "", self.document.is_some()) {
                            ui.close_menu();
                            self.set_all_expanded(true);
                        }
                        if menu_item(ui, "Collapse all", "", self.document.is_some()) {
                            ui.close_menu();
                            self.set_all_expanded(false);
                        }
                        ui.separator();
                        ui.menu_button("Skin", |ui| {
                            for candidate in Skin::all() {
                                if ui.radio(self.skin.id == candidate.id, candidate.name).clicked() {
                                    ui.close_menu();
                                    self.set_skin(candidate);
                                }
                            }
                        });
                    });
                    ui.menu_button("Help", |ui| {
                        if menu_item(ui, "Keyboard shortcuts", "", true) {
                            ui.close_menu();
                            self.status = "Ctrl+O open • Ctrl+S save • Ctrl+E export selection • Ctrl+F find • Ctrl+1–4 tabs • arrows walk the tree • Space plays • , and . step frames".to_string();
                        }
                        if menu_item(ui, "About CanRipper", "", true) {
                            ui.close_menu();
                            self.about_open = true;
                        }
                    });
                });
            });
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        if menu_item(ui, "Open…", "Ctrl+O", true) {
            ui.close_menu();
            self.open_dialog();
        }
        ui.menu_button("Open recent", |ui| {
            if self.settings.recent.is_empty() {
                ui.weak("(nothing yet)");
            }
            let recent = self.settings.recent.clone();
            for path in recent {
                if ui
                    .button(file_name(&path))
                    .on_hover_text(path.display().to_string())
                    .clicked()
                {
                    ui.close_menu();
                    self.open_path(&path);
                }
            }
            if !self.settings.recent.is_empty() {
                ui.separator();
                if ui.button("Clear list").clicked() {
                    ui.close_menu();
                    self.settings.recent.clear();
                    self.settings.save();
                }
            }
        });
        ui.separator();
        let save = self.document.as_ref().is_some_and(can_save);
        if menu_item(ui, "Save", "Ctrl+S", save) {
            ui.close_menu();
            self.save_current();
        }
        if menu_item(ui, "Save as…", "Ctrl+Shift+S", save) {
            ui.close_menu();
            self.save_as();
        }
        if menu_item(
            ui,
            "Export to q1s…",
            "",
            self.document.as_ref().is_some_and(can_export_q1s),
        ) {
            ui.close_menu();
            self.export_to_q1s();
        }
        ui.separator();
        if menu_item(
            ui,
            "Export selection…",
            "Ctrl+E",
            self.selected_preview_ref().is_some(),
        ) {
            ui.close_menu();
            self.export_selected();
        }
        if menu_item(ui, "Extract all…", "Ctrl+Shift+E", self.document.is_some()) {
            ui.close_menu();
            self.extract_all();
        }
        ui.separator();
        if menu_item(ui, "Close", "", self.document.is_some()) {
            ui.close_menu();
            self.close_document();
        }
        if menu_item(ui, "Exit", "Alt+F4", true) {
            ui.close_menu();
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn replace_enabled(&self) -> bool {
        self.document
            .as_ref()
            .zip(self.selected_preview_ref().as_ref())
            .is_some_and(|(document, reference)| can_replace_with_image(document, reference))
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        let skin = self.skin;
        let save = self.document.as_ref().is_some_and(can_save);
        let q1s = self.document.as_ref().is_some_and(can_export_q1s);
        let selection = self.selected_preview_ref().is_some();
        let replace = self.replace_enabled();
        let open = self.document.is_some();
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::none()
                    .fill(skin.face)
                    .inner_margin(egui::Margin::symmetric(6.0, 3.0)),
            )
            .show(ctx, |ui| {
                let bar = ui.max_rect().expand2(vec2(6.0, 3.0));
                skin::face_gradient(ui.painter(), bar, skin);
                ui.painter().line_segment(
                    [
                        bar.left_bottom() - vec2(0.0, 0.5),
                        bar.right_bottom() - vec2(0.0, 0.5),
                    ],
                    Stroke::new(1.0_f32, skin.shadow()),
                );
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;
                    if skin::tool_button(
                        ui,
                        skin,
                        UiIcon::Open,
                        "Open",
                        true,
                        "Open a file (Ctrl+O)",
                    )
                    .clicked()
                    {
                        self.open_dialog();
                    }
                    if skin::tool_button(ui, skin, UiIcon::Save, "Save", save, "Save (Ctrl+S)")
                        .clicked()
                    {
                        self.save_current();
                    }
                    if skin::tool_button(
                        ui,
                        skin,
                        UiIcon::SaveAs,
                        "Save as",
                        save,
                        "Save as… (Ctrl+Shift+S)",
                    )
                    .clicked()
                    {
                        self.save_as();
                    }
                    skin::separator(ui, skin, 46.0);
                    if skin::tool_button(
                        ui,
                        skin,
                        UiIcon::Export,
                        "Export",
                        selection,
                        "Export the selection as PNG, SVG, MP3, WAV or text (Ctrl+E)",
                    )
                    .clicked()
                    {
                        self.export_selected();
                    }
                    if skin::tool_button(
                        ui,
                        skin,
                        UiIcon::Extract,
                        "Extract all",
                        open,
                        "Extract every resource into folders (Ctrl+Shift+E)",
                    )
                    .clicked()
                    {
                        self.extract_all();
                    }
                    if skin::tool_button(
                        ui,
                        skin,
                        UiIcon::ToQ1s,
                        "To q1s",
                        q1s,
                        "Export as an editable q1s project",
                    )
                    .clicked()
                    {
                        self.export_to_q1s();
                    }
                    skin::separator(ui, skin, 46.0);
                    if skin::tool_button(
                        ui,
                        skin,
                        UiIcon::ReplaceImage,
                        "Replace",
                        replace,
                        "Replace the selected vector or bitmap with an image",
                    )
                    .clicked()
                    {
                        self.replace_selected_with_image();
                    }
                    skin::separator(ui, skin, 46.0);
                    if skin::tool_button(ui, skin, UiIcon::About, "About", true, "About CanRipper")
                        .clicked()
                    {
                        self.about_open = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        egui::ComboBox::from_id_source("canripper-skin")
                            .selected_text(self.skin.name)
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for candidate in Skin::all() {
                                    if ui
                                        .selectable_label(
                                            self.skin.id == candidate.id,
                                            candidate.name,
                                        )
                                        .clicked()
                                    {
                                        self.set_skin(candidate);
                                    }
                                }
                            });
                        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                        draw_icon(ui.painter(), rect, UiIcon::Theme, true);
                    });
                });
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        let skin = self.skin;
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::none()
                    .fill(skin.face)
                    .inner_margin(egui::Margin::symmetric(3.0, 2.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let (format, size, items) = match &self.document {
                        Some(document) => (
                            document.format.label().to_string(),
                            byte_size(document.bytes.len()),
                            format!("{} rows", self.rows.as_ref().map_or(0, Vec::len)),
                        ),
                        None => ("no file".to_string(), "-".to_string(), "-".to_string()),
                    };
                    let selection = self
                        .selected_node
                        .map_or("nothing selected".to_string(), |id| format!("node {id}"));
                    let fixed = 120.0 + 80.0 + 90.0 + 110.0 + 120.0 + 10.0;
                    let mut status = self.status.clone();
                    if self.dirty {
                        status = format!("● unsaved • {status}");
                    }
                    skin::status_field(
                        ui,
                        skin,
                        &status,
                        (ui.available_width() - fixed).max(120.0),
                    );
                    skin::status_field(ui, skin, &format, 120.0);
                    skin::status_field(ui, skin, &size, 80.0);
                    skin::status_field(ui, skin, &items, 90.0);
                    skin::status_field(ui, skin, &selection, 110.0);
                    skin::status_field(ui, skin, skin.name, 120.0);
                });
            });
    }

    fn resources_panel(&mut self, ctx: &egui::Context) {
        let skin = self.skin;
        egui::SidePanel::left("resources")
            .resizable(true)
            .default_width(320.0)
            .min_width(200.0)
            .frame(
                egui::Frame::none()
                    .fill(skin.face)
                    .inner_margin(egui::Margin::same(3.0)),
            )
            .show(ctx, |ui| {
                skin::title_bar(ui, skin, Some(UiIcon::FolderOpen), "Resources");
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    draw_icon(ui.painter(), rect, UiIcon::Search, true);
                    // Sized to the panel exactly: anything wider would make
                    // the panel grow to fit it, frame after frame.
                    let width =
                        (ui.available_width() - 24.0 - ui.spacing().item_spacing.x - 2.0).max(40.0);
                    let field =
                        egui::TextEdit::singleline(&mut self.filter).hint_text("Filter by name…");
                    let response = ui.add_sized([width, 20.0], field);
                    if std::mem::take(&mut self.focus_filter) {
                        response.request_focus();
                    }
                    if response.changed() {
                        self.rows = None;
                    }
                    if skin::small_button(
                        ui,
                        skin,
                        UiIcon::Clear,
                        false,
                        !self.filter.is_empty(),
                        "Clear the filter",
                    )
                    .clicked()
                    {
                        self.filter.clear();
                        self.rows = None;
                    }
                });
                ui.add_space(3.0);
                let well = ui.available_rect_before_wrap();
                ui.painter().rect_filled(well, 0.0, skin.field);
                skin::bevel(ui.painter(), well, skin.light(), skin.shadow(), false);
                let inner = well.shrink(2.0);
                let mut child = ui.child_ui(inner, egui::Layout::top_down(egui::Align::Min));
                child.set_clip_rect(inner);
                self.tree(&mut child);
                ui.allocate_rect(well, Sense::hover());
            });
    }

    fn tree(&mut self, ui: &mut egui::Ui) {
        let skin = self.skin;
        if self.document.is_none() {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("  Nothing open.").color(skin.text_dim));
            return;
        }
        let row_count = self.rows().len();
        if row_count == 0 {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("  Nothing matches the filter.").color(skin.text_dim));
            return;
        }
        let mut scroll = egui::ScrollArea::vertical()
            .id_source("canripper-tree")
            .auto_shrink([false, false]);
        if let Some(index) = self.reveal.take() {
            let (offset, height) = self.tree_view;
            let top = index as f32 * ROW_HEIGHT;
            if top < offset {
                scroll = scroll.vertical_scroll_offset(top);
            } else if top + ROW_HEIGHT > offset + height && height > 0.0 {
                scroll = scroll.vertical_scroll_offset(top + ROW_HEIGHT - height);
            }
        }
        let mut clicked: Option<usize> = None;
        let mut toggled: Option<usize> = None;
        let rows = self.rows.clone().unwrap_or_default();
        let document = self.document.as_ref().expect("checked above");
        let selected = self.selected_node;
        let output = scroll.show_rows(ui, ROW_HEIGHT, row_count, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                let Some(node) = document.node(row.id) else {
                    continue;
                };
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let painter = ui.painter();
                let line = skin::mix(skin.border, skin.field, 0.25);
                let column = |depth: usize| rect.left() + 4.0 + depth as f32 * INDENT + 6.0;
                for depth in 0..row.depth.min(63) {
                    if row.lines & (1 << depth) != 0 {
                        let x = column(depth);
                        skin::dotted(painter, pos2(x, rect.top()), pos2(x, rect.bottom()), line);
                    }
                }
                let x = column(row.depth);
                let mid = rect.center().y;
                skin::dotted(
                    painter,
                    pos2(x, rect.top()),
                    pos2(x, if row.last { mid } else { rect.bottom() }),
                    line,
                );
                skin::dotted(painter, pos2(x, mid), pos2(x + 9.0, mid), line);
                let expander = Rect::from_center_size(pos2(x, mid), vec2(12.0, 12.0));
                if row.children {
                    skin::expander(painter, pos2(x, mid), row.open, skin);
                }
                let icon = Rect::from_min_size(pos2(x + 10.0, mid - 8.0), vec2(16.0, 16.0));
                draw_icon(
                    painter,
                    icon,
                    icon_for_tree_node(node, Some(document), row.open),
                    true,
                );
                let is_selected = selected == Some(row.id);
                let text_left = icon.right() + 4.0;
                let galley = ui.fonts(|fonts| {
                    fonts.layout_no_wrap(
                        node.label.clone(),
                        egui::FontId::proportional(13.0),
                        if is_selected {
                            skin.select_text
                        } else {
                            skin.text
                        },
                    )
                });
                let label_rect = Rect::from_min_size(
                    pos2(text_left - 2.0, rect.top() + 1.0),
                    vec2(galley.size().x + 4.0, ROW_HEIGHT - 2.0),
                );
                if is_selected {
                    painter.rect_filled(label_rect, 0.0, skin.select);
                } else if response.hovered() {
                    painter.rect_filled(label_rect, 0.0, skin::mix(skin.field, skin.select, 0.18));
                }
                let color = if is_selected {
                    skin.select_text
                } else {
                    skin.text
                };
                painter.galley(pos2(text_left, mid - galley.size().y * 0.5), galley, color);
                if response.clicked() {
                    let on_expander = row.children
                        && response
                            .interact_pointer_pos()
                            .is_some_and(|pos| expander.contains(pos));
                    if on_expander {
                        toggled = Some(row.id);
                    } else {
                        clicked = Some(row.id);
                    }
                }
                if response.double_clicked() && row.children {
                    toggled = Some(row.id);
                }
            }
        });
        self.tree_view = (output.state.offset.y, output.inner_rect.height());
        if let Some(id) = toggled {
            self.toggle(id);
        }
        if let Some(id) = clicked {
            self.select(id);
        }
    }

    fn detail_text(&self) -> String {
        self.document
            .as_ref()
            .map(|document| {
                self.selected_node
                    .and_then(|id| document.node_detail(id))
                    .unwrap_or(&document.summary)
                    .to_string()
            })
            .unwrap_or_default()
    }

    fn tabs_available(&self, preview: bool) -> Vec<Tab> {
        let mut tabs = Vec::new();
        if preview {
            tabs.push(Tab::Preview);
        }
        if self.document.is_some() {
            tabs.push(Tab::Hex);
            tabs.push(Tab::Info);
        }
        if self.edit_ref.is_some() {
            tabs.push(Tab::Code);
        }
        tabs
    }

    fn central(&mut self, ctx: &egui::Context, preview_ref: Option<PreviewRef>, frame_count: u32) {
        let skin = self.skin;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(skin.face)
                    .inner_margin(egui::Margin::same(3.0)),
            )
            .show(ctx, |ui| {
                let Some(document) = &self.document else {
                    self.welcome(ui);
                    return;
                };
                let title = self
                    .selected_node
                    .and_then(|id| document.node(id))
                    .map(|node| node.label.clone())
                    .unwrap_or_else(|| file_name(&document.path));
                let icon = self
                    .selected_node
                    .and_then(|id| document.node(id))
                    .map_or(UiIcon::Document, |node| {
                        icon_for_tree_node(node, Some(document), true)
                    });
                skin::title_bar(ui, skin, Some(icon), &title);
                ui.add_space(4.0);

                // A tab that this item lacks is shown as the first it has,
                // and comes back for the next item that has it.
                let tabs = self.tabs_available(preview_ref.is_some());
                let mut shown = if tabs.contains(&self.tab) {
                    self.tab
                } else {
                    tabs.first().copied().unwrap_or(Tab::Info)
                };
                let labels: Vec<&str> = tabs.iter().map(|tab| tab.label()).collect();
                let current = tabs.iter().position(|tab| *tab == shown).unwrap_or(0);
                if let Some(picked) = skin::tabs(ui, skin, &labels, current) {
                    self.tab = tabs[picked];
                    shown = self.tab;
                }
                let page = ui.available_rect_before_wrap();
                ui.painter().rect_filled(page, 0.0, skin.window);
                let edge = Stroke::new(1.0_f32, skin.border);
                ui.painter()
                    .line_segment([page.left_top(), page.left_bottom()], edge);
                ui.painter()
                    .line_segment([page.right_top(), page.right_bottom()], edge);
                ui.painter()
                    .line_segment([page.left_bottom(), page.right_bottom()], edge);
                let inner = page.shrink(6.0);
                let mut child = ui.child_ui(inner, egui::Layout::top_down(egui::Align::Min));
                child.set_clip_rect(inner);
                match shown {
                    Tab::Preview => self.preview_tab(&mut child, frame_count),
                    Tab::Hex => self.hex_tab(&mut child),
                    Tab::Info => self.info_tab(&mut child),
                    Tab::Code => self.code_tab(&mut child),
                }
                ui.allocate_rect(page, Sense::hover());
            });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let skin = self.skin;
        skin::title_bar(ui, skin, Some(UiIcon::Document), "Start");
        let page = ui.available_rect_before_wrap();
        ui.painter().rect_filled(page, 0.0, skin.window);
        skin::bevel(ui.painter(), page, skin.light(), skin.shadow(), false);
        let mut child = ui.child_ui(page.shrink(24.0), egui::Layout::top_down(egui::Align::Min));
        child.add_space(16.0);
        child.label(
            egui::RichText::new("CanRipper")
                .size(30.0)
                .strong()
                .color(skin.text),
        );
        child.label(
            egui::RichText::new("Opens q0idmation projects, movies and media, and Flash movies and projectors; shows what is inside; takes it back out.")
                .color(skin.text_dim),
        );
        child.add_space(18.0);
        child.horizontal(|ui| {
            if skin::tool_button(ui, skin, UiIcon::Open, "Open…", true, "Open a file").clicked() {
                self.open_dialog();
            }
        });
        child.add_space(14.0);
        if !self.settings.recent.is_empty() {
            child.label(
                egui::RichText::new("Recent files")
                    .strong()
                    .color(skin.text),
            );
            child.add_space(4.0);
            let recent = self.settings.recent.clone();
            for path in recent {
                child.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    draw_icon(ui.painter(), rect, UiIcon::Document, true);
                    if ui
                        .link(file_name(&path))
                        .on_hover_text(path.display().to_string())
                        .clicked()
                    {
                        self.open_path(&path);
                    }
                    ui.label(
                        egui::RichText::new(
                            path.parent()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default(),
                        )
                        .small()
                        .color(skin.text_dim),
                    );
                });
            }
        }
        child.add_space(14.0);
        child.label(
            egui::RichText::new("Drag a file onto this window to open it.").color(skin.text_dim),
        );
        ui.allocate_rect(page, Sense::hover());
    }

    fn preview_tab(&mut self, ui: &mut egui::Ui, frame_count: u32) {
        let skin = self.skin;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let animated = frame_count > 1;
            if skin::small_button(ui, skin, UiIcon::First, false, animated, "First frame").clicked()
            {
                self.preview_frame = 0;
            }
            if skin::small_button(
                ui,
                skin,
                UiIcon::Previous,
                false,
                animated,
                "Previous frame (,)",
            )
            .clicked()
            {
                self.preview_frame = self.preview_frame.saturating_sub(1);
            }
            let play_icon = if self.playing {
                UiIcon::Pause
            } else {
                UiIcon::Play
            };
            if skin::small_button(
                ui,
                skin,
                play_icon,
                self.playing,
                animated,
                "Play / pause (Space)",
            )
            .clicked()
            {
                self.playing = !self.playing;
                self.play_clock = 0.0;
            }
            if skin::small_button(ui, skin, UiIcon::Next, false, animated, "Next frame (.)")
                .clicked()
            {
                self.preview_frame = (self.preview_frame + 1).min(frame_count.saturating_sub(1));
            }
            if skin::small_button(ui, skin, UiIcon::Last, false, animated, "Last frame").clicked() {
                self.preview_frame = frame_count.saturating_sub(1);
            }
            if animated {
                ui.add_space(4.0);
                ui.spacing_mut().slider_width = (ui.available_width() - 330.0).clamp(80.0, 600.0);
                let mut frame = self.preview_frame;
                if ui
                    .add(egui::Slider::new(&mut frame, 0..=frame_count - 1).show_value(false))
                    .changed()
                {
                    self.preview_frame = frame;
                    self.playing = false;
                }
                ui.monospace(format!("{:>5} / {frame_count}", self.preview_frame + 1));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let scale = match self.zoom {
                    Zoom::Fit => "fit".to_string(),
                    Zoom::Scale(scale) => format!("{:.0}%", scale * 100.0),
                };
                ui.label(egui::RichText::new(scale).monospace().color(skin.text_dim));
                if skin::small_button(ui, skin, UiIcon::ZoomIn, false, true, "Zoom in").clicked() {
                    self.zoom = Zoom::Scale((self.current_scale() * 1.25).min(64.0));
                }
                if skin::small_button(ui, skin, UiIcon::ZoomOut, false, true, "Zoom out").clicked()
                {
                    self.zoom = Zoom::Scale((self.current_scale() / 1.25).max(0.02));
                }
                if skin::small_button(
                    ui,
                    skin,
                    UiIcon::ZoomActual,
                    self.zoom == Zoom::Scale(1.0),
                    true,
                    "Actual size",
                )
                .clicked()
                {
                    self.zoom = Zoom::Scale(1.0);
                    self.pan = egui::Vec2::ZERO;
                }
                if skin::small_button(
                    ui,
                    skin,
                    UiIcon::ZoomFit,
                    self.zoom == Zoom::Fit,
                    true,
                    "Fit to the window",
                )
                .clicked()
                {
                    self.zoom = Zoom::Fit;
                    self.pan = egui::Vec2::ZERO;
                }
            });
        });
        ui.add_space(3.0);
        let caption_height = 18.0;
        let canvas = Rect::from_min_max(
            ui.cursor().min,
            pos2(
                ui.max_rect().right(),
                ui.max_rect().bottom() - caption_height,
            ),
        );
        if canvas.height() < 20.0 {
            return;
        }
        let response = ui.allocate_rect(canvas, Sense::click_and_drag());
        let painter = ui.painter_at(canvas);
        skin::bevel(&painter, canvas, skin.light(), skin.shadow(), false);
        let inner = canvas.shrink(1.0);

        if let Some(error) = &self.preview.error {
            painter.rect_filled(inner, 0.0, skin.field);
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                error,
                egui::FontId::proportional(13.0),
                ui.visuals().error_fg_color,
            );
        } else if let Some(text) = self.preview.text.clone() {
            painter.rect_filled(inner, 0.0, skin.field);
            let mut child =
                ui.child_ui(inner.shrink(4.0), egui::Layout::top_down(egui::Align::Min));
            egui::ScrollArea::both()
                .id_source("canripper-preview-text")
                .auto_shrink([false, false])
                .show(&mut child, |ui| {
                    let mut shown = text;
                    ui.add(
                        egui::TextEdit::multiline(&mut shown)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .frame(false),
                    );
                });
        } else {
            paint_checker(&painter, inner, skin);
            let content = if let Some(vector) = &self.preview.vector {
                let (min_x, min_y, max_x, max_y) = vector.bounds;
                Some(vec2(
                    (max_x - min_x).abs().max(1.0),
                    (max_y - min_y).abs().max(1.0),
                ))
            } else {
                self.preview
                    .texture
                    .as_ref()
                    .map(|texture| texture.size_vec2())
            };
            if let Some(size) = content {
                let fit = ((inner.width() - 24.0).max(1.0) / size.x)
                    .min((inner.height() - 24.0).max(1.0) / size.y)
                    .max(0.001);
                let fit = if self.preview.texture.is_some() {
                    fit.min(4.0)
                } else {
                    fit
                };
                let scale = match self.zoom {
                    Zoom::Fit => fit,
                    Zoom::Scale(scale) => scale,
                };
                // Wheel zooms about the pointer; dragging moves the picture.
                if response.hovered() {
                    let scroll = ui.input(|input| input.raw_scroll_delta.y);
                    if scroll != 0.0 {
                        let next = (scale * (1.0015_f32).powf(scroll)).clamp(0.02, 64.0);
                        if let Some(pointer) = response.hover_pos() {
                            let from_center = pointer - inner.center() - self.pan;
                            self.pan += from_center - from_center * (next / scale);
                        }
                        self.zoom = Zoom::Scale(next);
                    }
                }
                if response.dragged() {
                    self.pan += response.drag_delta();
                }
                if response.double_clicked() {
                    self.zoom = Zoom::Fit;
                    self.pan = egui::Vec2::ZERO;
                }
                let shown = Rect::from_center_size(inner.center() + self.pan, size * scale);
                if let Some(vector) = &self.preview.vector {
                    paint_vector(&painter, vector, shown, scale);
                } else if let Some(texture) = &self.preview.texture {
                    painter.image(
                        texture.id(),
                        shown,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            } else {
                painter.text(
                    inner.center(),
                    egui::Align2::CENTER_CENTER,
                    "Nothing to preview.",
                    egui::FontId::proportional(13.0),
                    skin.text_dim,
                );
            }
        }
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(&self.preview.caption)
                .small()
                .color(skin.text_dim),
        );
    }

    fn current_scale(&self) -> f32 {
        match self.zoom {
            Zoom::Scale(scale) => scale,
            Zoom::Fit => 1.0,
        }
    }

    fn hex_tab(&mut self, ui: &mut egui::Ui) {
        let skin = self.skin;
        let Some(document) = &self.document else {
            return;
        };
        if self
            .hex
            .as_ref()
            .is_none_or(|hex| hex.node != self.selected_node)
        {
            let reference = self.selected_node.and_then(|id| document.node_preview(id));
            let (bytes, base, label) = node_bytes(document, reference);
            self.hex = Some(HexView {
                node: self.selected_node,
                bytes: bytes.into_owned(),
                base,
                label,
            });
        }
        let Some(hex) = &self.hex else {
            return;
        };
        ui.label(
            egui::RichText::new(format!(
                "{} • {} • starts at 0x{:x}",
                hex.label,
                byte_size(hex.bytes.len()),
                hex.base
            ))
            .color(skin.text_dim),
        );
        ui.add_space(2.0);
        let font = egui::FontId::monospace(12.5);
        let line = ui.fonts(|fonts| fonts.row_height(&font)) + 2.0;
        ui.label(
            egui::RichText::new(format!(
                "{:<10}00 01 02 03 04 05 06 07  08 09 0a 0b 0c 0d 0e 0f   ASCII",
                "offset"
            ))
            .font(font.clone())
            .color(skin.accent),
        );
        let rows = hex.bytes.len().div_ceil(16);
        let well = ui.available_rect_before_wrap();
        ui.painter().rect_filled(well, 0.0, skin.field);
        skin::bevel(ui.painter(), well, skin.light(), skin.shadow(), false);
        let mut child = ui.child_ui(well.shrink(3.0), egui::Layout::top_down(egui::Align::Min));
        egui::ScrollArea::vertical()
            .id_source("canripper-hex")
            .auto_shrink([false, false])
            .show_rows(&mut child, line, rows, |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for row in range {
                    let start = row * 16;
                    let chunk = &hex.bytes[start..(start + 16).min(hex.bytes.len())];
                    let mut text = format!("{:08x}  ", hex.base + start);
                    for index in 0..16 {
                        match chunk.get(index) {
                            Some(byte) => text.push_str(&format!("{byte:02x} ")),
                            None => text.push_str("   "),
                        }
                        if index == 7 {
                            text.push(' ');
                        }
                    }
                    text.push_str("  ");
                    for byte in chunk {
                        text.push(if byte.is_ascii_graphic() || *byte == b' ' {
                            char::from(*byte)
                        } else {
                            '.'
                        });
                    }
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), line), Sense::hover());
                    if row % 2 == 1 {
                        ui.painter()
                            .rect_filled(rect, 0.0, skin::mix(skin.field, skin.face, 0.35));
                    }
                    ui.painter().text(
                        pos2(rect.left() + 2.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        text,
                        font.clone(),
                        skin.text,
                    );
                }
            });
        ui.allocate_rect(well, Sense::hover());
    }

    fn info_tab(&mut self, ui: &mut egui::Ui) {
        let skin = self.skin;
        let detail = self.detail_text();
        let mut rows = Vec::new();
        let mut rest = String::new();
        let mut in_rest = false;
        for line in detail.lines() {
            if !in_rest {
                if let Some((key, value)) = line.split_once(": ") {
                    if !key.is_empty() && key.len() < 40 {
                        rows.push((key.to_string(), value.to_string()));
                        continue;
                    }
                }
                if line.trim().is_empty() && !rows.is_empty() {
                    in_rest = true;
                    continue;
                }
                if rows.is_empty() {
                    rows.push(("".to_string(), line.to_string()));
                    continue;
                }
                in_rest = true;
            }
            rest.push_str(line);
            rest.push('\n');
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Properties").strong().color(skin.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Copy").clicked() {
                    ui.output_mut(|output| output.copied_text = detail.clone());
                    self.status = "Details copied.".to_string();
                }
            });
        });
        ui.add_space(3.0);
        egui::ScrollArea::vertical()
            .id_source("canripper-info")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("canripper-properties")
                    .num_columns(2)
                    .striped(true)
                    .spacing([14.0, 3.0])
                    .min_col_width(110.0)
                    .show(ui, |ui| {
                        for (key, value) in &rows {
                            ui.label(egui::RichText::new(key).color(skin.text_dim));
                            ui.label(egui::RichText::new(value).color(skin.text));
                            ui.end_row();
                        }
                    });
                if !rest.trim().is_empty() {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("More").strong().color(skin.text));
                    let mut shown = rest.clone();
                    ui.add(
                        egui::TextEdit::multiline(&mut shown)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    );
                }
            });
    }

    fn code_tab(&mut self, ui: &mut egui::Ui) {
        let skin = self.skin;
        ui.horizontal(|ui| {
            let bytes = self.edit_buffer.len();
            ui.label(egui::RichText::new("q0lang").strong().color(skin.text));
            if bytes > MAX_CODE_BYTES {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("{bytes} bytes • too large"),
                );
            } else {
                ui.label(egui::RichText::new(format!("{bytes} bytes")).color(skin.text_dim));
            }
            if self.edit_buffer_dirty {
                ui.label(egui::RichText::new("• changed").color(skin.accent));
            }
        });
        ui.add_space(3.0);
        let hint_height = 18.0;
        let height = (ui.available_height() - hint_height).max(60.0);
        egui::ScrollArea::vertical()
            .id_source("canripper-code")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let response = ui.add(
                    egui::TextEdit::multiline(&mut self.edit_buffer)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(24),
                );
                if response.changed() {
                    self.edit_buffer_dirty = true;
                    self.dirty = true;
                    self.status = "Code changed • save to write it".to_string();
                }
            });
        ui.label(
            egui::RichText::new("Changes go into the file, through nested q0s containers, when you pick something else or save.")
                .small()
                .color(skin.text_dim),
        );
    }

    fn about(&mut self, ctx: &egui::Context) {
        let skin = self.skin;
        let mut open = self.about_open;
        egui::Window::new("About CanRipper")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                    draw_icon(ui.painter(), rect, UiIcon::Extract, true);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("CanRipper").size(20.0).strong().color(skin.text));
                        ui.label(egui::RichText::new(format!("version {}", env!("CARGO_PKG_VERSION"))).color(skin.text_dim));
                    });
                });
                ui.add_space(8.0);
                ui.label("Opens q1s projects, q0s movies and q0v media, and Flash SWF movies and projectors. Shows shapes, sprites, frames, images, sounds, texts, fonts and scripts, and takes them back out.");
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Part of q0idmation.").color(skin.text_dim));
            });
        self.about_open = open;
    }

    fn shortcuts(&mut self, ctx: &egui::Context, frame_count: u32) {
        let (open, save, save_as, export, extract, find, space, back, forward) =
            ctx.input(|input| {
                let command = input.modifiers.command;
                let shift = input.modifiers.shift;
                (
                    command && input.key_pressed(egui::Key::O),
                    command && !shift && input.key_pressed(egui::Key::S),
                    command && shift && input.key_pressed(egui::Key::S),
                    command && !shift && input.key_pressed(egui::Key::E),
                    command && shift && input.key_pressed(egui::Key::E),
                    command && input.key_pressed(egui::Key::F),
                    input.key_pressed(egui::Key::Space),
                    input.key_pressed(egui::Key::Comma),
                    input.key_pressed(egui::Key::Period),
                )
            });
        if open {
            self.open_dialog();
        }
        if save_as {
            self.save_as();
        } else if save {
            self.save_current();
        }
        if export {
            self.export_selected();
        }
        if extract {
            self.extract_all();
        }
        if find {
            self.focus_filter = true;
        }
        let picked = ctx.input(|input| {
            [
                (egui::Key::Num1, Tab::Preview),
                (egui::Key::Num2, Tab::Hex),
                (egui::Key::Num3, Tab::Info),
                (egui::Key::Num4, Tab::Code),
            ]
            .into_iter()
            .find(|(key, _)| input.modifiers.command && input.key_pressed(*key))
            .map(|(_, tab)| tab)
        });
        if let Some(tab) = picked {
            self.tab = tab;
        }
        if !ctx.wants_keyboard_input() && frame_count > 1 {
            if space {
                self.playing = !self.playing;
                self.play_clock = 0.0;
            }
            if back {
                self.playing = false;
                self.preview_frame = self.preview_frame.saturating_sub(1);
            }
            if forward {
                self.playing = false;
                self.preview_frame = (self.preview_frame + 1).min(frame_count - 1);
            }
        }
    }
}

impl eframe::App for CanRipperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_skin(ctx);
        if let Some(title) = self.pending_title.take() {
            let title = if title.is_empty() {
                "CanRipper".to_string()
            } else {
                format!("{title} - CanRipper")
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
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

        self.tree_keys(ctx);
        self.sync_editor_to_selection();
        if self.preview_node != self.selected_node {
            self.preview_node = self.selected_node;
            self.preview_frame = 0;
            self.playing = false;
            self.zoom = Zoom::Fit;
            self.pan = egui::Vec2::ZERO;
            self.preview = PreviewCache {
                texture: self.preview.texture.take(),
                ..PreviewCache::default()
            };
        }
        let preview_ref = self.selected_preview_ref();
        let (frame_count, fps) = self
            .document
            .as_ref()
            .zip(preview_ref.as_ref())
            .map(|(document, reference)| {
                (
                    preview_frame_count(document, reference).max(1),
                    preview_fps(document, reference),
                )
            })
            .unwrap_or((1, 24.0));
        self.shortcuts(ctx, frame_count);

        // Playback runs on its own clock, a frame at the movie's rate.
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f32().min(0.25);
        self.last_tick = now;
        if self.playing && frame_count > 1 && self.tab == Tab::Preview {
            self.play_clock += elapsed * fps;
            let steps = self.play_clock.floor();
            if steps >= 1.0 {
                self.play_clock -= steps;
                self.preview_frame = (self.preview_frame + steps as u32) % frame_count;
            }
            ctx.request_repaint();
        } else {
            self.playing &= frame_count > 1;
        }
        self.preview_frame = self.preview_frame.min(frame_count - 1);
        if self.tab == Tab::Preview {
            self.refresh_preview(ctx, preview_ref.clone());
        }

        self.menu_bar(ctx);
        self.toolbar(ctx);
        self.status_bar(ctx);
        self.resources_panel(ctx);
        self.central(ctx, preview_ref, frame_count);
        if self.about_open {
            self.about(ctx);
        }

        let dragging = ctx.input(|input| !input.raw.hovered_files.is_empty());
        if dragging {
            let screen = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop"),
            ));
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(90));
            painter.rect_stroke(
                screen.shrink(12.0),
                4.0,
                Stroke::new(3.0_f32, self.skin.accent),
            );
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                "Drop to open",
                egui::FontId::proportional(26.0),
                Color32::WHITE,
            );
        }
    }
}

/// Which folders start open: the small ones at the top.
fn default_expansion(document: &Document) -> HashSet<usize> {
    document
        .roots
        .iter()
        .filter(|node| !node.children.is_empty() && node.children.len() <= 64)
        .map(|node| node.id)
        .collect()
}

/// The rows the tree shows: open folders' children, or with a filter, the
/// nodes whose names match and the folders that lead to them.
fn build_rows(document: &Document, expanded: &HashSet<usize>, filter: &str) -> Vec<Row> {
    use crate::document::TreeNode;
    let needle = filter.trim().to_lowercase();
    let keep = (!needle.is_empty()).then(|| {
        fn mark(node: &TreeNode, needle: &str, keep: &mut HashSet<usize>) -> bool {
            let mut any = false;
            for child in &node.children {
                any |= mark(child, needle, keep);
            }
            if any || node.label.to_lowercase().contains(needle) {
                keep.insert(node.id);
                true
            } else {
                false
            }
        }
        let mut keep = HashSet::new();
        for root in &document.roots {
            mark(root, &needle, &mut keep);
        }
        keep
    });
    fn walk(
        nodes: &[TreeNode],
        depth: usize,
        lines: u64,
        expanded: &HashSet<usize>,
        keep: Option<&HashSet<usize>>,
        rows: &mut Vec<Row>,
    ) {
        let shown: Vec<&TreeNode> = nodes
            .iter()
            .filter(|node| keep.is_none_or(|keep| keep.contains(&node.id)))
            .collect();
        let count = shown.len();
        for (index, node) in shown.into_iter().enumerate() {
            let last = index + 1 == count;
            let children = !node.children.is_empty();
            let open = children && (keep.is_some() || expanded.contains(&node.id));
            rows.push(Row {
                id: node.id,
                depth,
                children,
                open,
                last,
                lines,
            });
            if open {
                let below = if !last && depth < 63 {
                    lines | (1 << depth)
                } else {
                    lines
                };
                walk(&node.children, depth + 1, below, expanded, keep, rows);
            }
        }
    }
    let mut rows = Vec::new();
    walk(&document.roots, 0, 0, expanded, keep.as_ref(), &mut rows);
    rows
}

fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str, enabled: bool) -> bool {
    let button = egui::Button::new(label).shortcut_text(shortcut);
    ui.add_enabled(enabled, button).clicked()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn byte_size(bytes: usize) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= K * K * K {
        format!("{:.2} GB", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.1} MB", b / (K * K))
    } else if b >= K {
        format!("{:.1} KB", b / K)
    } else {
        format!("{bytes} B")
    }
}

/// The grey and white squares behind see-through pixels.
fn paint_checker(painter: &egui::Painter, rect: Rect, skin: &Skin) {
    let (light, dark) = if skin.is_dark() {
        (Color32::from_gray(70), Color32::from_gray(52))
    } else {
        (Color32::from_gray(255), Color32::from_gray(222))
    };
    painter.rect_filled(rect, 0.0, light);
    let cell = 10.0;
    let cols = (rect.width() / cell).ceil() as usize;
    let rows = (rect.height() / cell).ceil() as usize;
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 == 0 {
                let min = pos2(rect.left() + x as f32 * cell, rect.top() + y as f32 * cell);
                let max = pos2(
                    (min.x + cell).min(rect.right()),
                    (min.y + cell).min(rect.bottom()),
                );
                painter.rect_filled(Rect::from_min_max(min, max), 0.0, dark);
            }
        }
    }
}

/// Draws a vector preview so that its bounds fill `shown`.
fn paint_vector(painter: &egui::Painter, vector: &VectorPreview, shown: Rect, scale: f32) {
    let (min_x, min_y, _, _) = vector.bounds;
    let to_screen = |point: q0s_format::v2::Vec2| {
        pos2(
            shown.left() + (point.x - min_x) * scale,
            shown.top() + (point.y - min_y) * scale,
        )
    };
    if let Some(background) = vector.background {
        painter.rect_filled(shown, 0.0, rgba_color(background));
    }
    for layer in &vector.layers {
        if let Some(fill) = layer.fill {
            let path = build_lyon_path(&layer.paths, &to_screen, true);
            let mut buffers: VertexBuffers<lyon_path::math::Point, u32> = VertexBuffers::new();
            let options = FillOptions::default()
                .with_fill_rule(FillRule::NonZero)
                .with_tolerance(0.15);
            if FillTessellator::new()
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
        if let Some(stroke) = layer.stroke {
            let path = build_lyon_path(&layer.paths, &to_screen, false);
            let mut buffers: VertexBuffers<lyon_path::math::Point, u32> = VertexBuffers::new();
            let cap = match stroke.cap {
                q0s_format::geom::CapShape::Round => LineCap::Round,
                q0s_format::geom::CapShape::Butt => LineCap::Butt,
                q0s_format::geom::CapShape::Square => LineCap::Square,
            };
            let join = match stroke.join {
                q0s_format::geom::JoinShape::Miter => LineJoin::Miter,
                q0s_format::geom::JoinShape::Round => LineJoin::Round,
                q0s_format::geom::JoinShape::Bevel => LineJoin::Bevel,
            };
            let options = StrokeOptions::default()
                .with_line_width((stroke.width * scale).max(0.6))
                .with_line_join(join)
                .with_miter_limit(q0s_format::geom::MITER_LIMIT)
                .with_start_cap(cap)
                .with_end_cap(cap)
                .with_tolerance(0.15);
            if StrokeTessellator::new()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        id: usize,
        label: &str,
        children: Vec<crate::document::TreeNode>,
    ) -> crate::document::TreeNode {
        crate::document::TreeNode {
            id,
            label: label.to_string(),
            detail: String::new(),
            preview: None,
            edit: None,
            children,
        }
    }

    #[test]
    fn the_tree_shows_open_folders_and_filters_to_matches() {
        let roots = vec![
            node(
                3,
                "shapes (2)",
                vec![
                    node(1, "DefineShape (1)", vec![]),
                    node(2, "DefineShape (2) “hero”", vec![]),
                ],
            ),
            node(5, "sounds (1)", vec![node(4, "DefineSound (9)", vec![])]),
        ];
        let mut document = Document::from_bytes(
            PathBuf::from("x.q1s"),
            std::fs::read("../q0s-format/testdata/empty.q1s").expect("fixture"),
        )
        .expect("open");
        document.roots = roots;
        document.rebuild_lookup();
        let mut expanded = HashSet::new();
        let rows = build_rows(&document, &expanded, "");
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![3, 5]
        );
        expanded.insert(3);
        let rows = build_rows(&document, &expanded, "");
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![3, 1, 2, 5]
        );
        assert!(
            rows[1].lines & 1 != 0,
            "the line to the next folder runs past the children"
        );
        let rows = build_rows(&document, &HashSet::new(), "HERO");
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![3, 2]
        );
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
            .as_chunks::<3>()
            .0
            .iter()
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
