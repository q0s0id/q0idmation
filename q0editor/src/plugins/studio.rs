//! The Plugin Studio, where plugins are made.
//!
//! Simple mode builds a plugin from blocks and forms: its code as stacks
//! of blocks, its card, page, settings and pictures as fields. Pro mode is
//! the whole `.qx` text with colours, line numbers, suggestions and a
//! reference of every word. Both edit the same file; switching keeps it.

use std::path::PathBuf;

use egui::{vec2, Color32, Context, FontId, RichText, Ui};
use q0ex::doc::{join_blobs, split_blobs, template, Blob};
use q0ex::{PluginDoc, SettingDecl, SettingKind};

use super::api::{self, Cat};
use super::blocks::BlockEditor;
use crate::app::EditorApp;

/// Plugins to start from, shipped with the editor.
pub(crate) const EXAMPLES: [(&str, &str); 4] = [
    (
        "Hello",
        include_str!("../../../docs/q0ex/examples/hello.qx"),
    ),
    (
        "Spiral",
        include_str!("../../../docs/q0ex/examples/spiral.qx"),
    ),
    (
        "Ruler",
        include_str!("../../../docs/q0ex/examples/ruler.qx"),
    ),
    (
        "Scatter",
        include_str!("../../../docs/q0ex/examples/scatter.qx"),
    ),
];

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    #[default]
    Simple,
    Pro,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Blocks,
    Card,
    Page,
    Settings,
    Pictures,
}

/// The plugin open in the studio.
struct Draft {
    /// The id it had when opened: which installed plugin it saves over.
    saved_id: Option<String>,
    /// The text without its pictures.
    text: String,
    blobs: Vec<Blob>,
    doc: Option<PluginDoc>,
    error: Option<q0ex::ParseError>,
    dirty: bool,
}

impl Draft {
    fn new(source: &str, saved_id: Option<String>) -> Self {
        let (text, blobs) = split_blobs(source);
        let mut draft = Self {
            saved_id,
            text,
            blobs,
            doc: None,
            error: None,
            dirty: false,
        };
        draft.reparse();
        draft
    }

    fn full_source(&self) -> String {
        join_blobs(&self.text, &self.blobs)
    }

    /// Reads the text again after it was typed in.
    fn reparse(&mut self) {
        match PluginDoc::parse(&self.full_source()) {
            Ok(doc) => {
                self.doc = Some(doc);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
            }
        }
    }

    /// Writes the text again after a form or the blocks changed the plugin.
    fn regenerate(&mut self) {
        if let Some(doc) = &self.doc {
            let (text, blobs) = split_blobs(&doc.source());
            self.text = text;
            self.blobs = blobs;
            self.dirty = true;
        }
    }

    fn name(&self) -> String {
        self.doc
            .as_ref()
            .map(|doc| doc.meta().name)
            .unwrap_or_else(|| "Untitled plugin".into())
    }
}

#[derive(Default)]
pub(crate) struct Studio {
    pub open: bool,
    /// Start a new plugin when the window next shows.
    pub new_requested: bool,
    pending_open: Option<String>,
    draft: Option<Draft>,
    mode: Mode,
    tab: Tab,
    blocks: BlockEditor,
    reference_filter: String,
    /// Suggestions shown under the cursor last frame, for Tab to take.
    suggestions: Vec<&'static str>,
    suggestion_prefix: String,
    /// A picture picked from a file, waiting to be added.
    pub picked_image: Option<PathBuf>,
}

impl Studio {
    /// The tab's name.
    pub fn tab_title(&self) -> String {
        match &self.draft {
            Some(draft) => format!("Studio · {}", draft.name()),
            None => "Plugin Studio".into(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.draft.as_ref().is_some_and(|draft| draft.dirty)
    }

    pub fn open_plugin(&mut self, id: &str) {
        self.open = true;
        self.pending_open = Some(id.to_string());
    }
}

/// Adds a picture to the plugin being made.
pub(crate) fn add_picture(app: &mut EditorApp, path: PathBuf) {
    app.plugins.studio.picked_image = Some(path);
    app.open_plugin_studio_tab();
}

fn unique_blob_name(blobs: &[Blob], wanted: &str) -> String {
    let base = q0ex::doc::slug(wanted).replace('-', "_");
    let mut name = base.clone();
    let mut n = 2;
    while blobs.iter().any(|blob| blob.name == name) {
        name = format!("{base}{n}");
        n += 1;
    }
    name
}

/// Opens the studio's tab when something asked for the studio.
pub(crate) fn render(app: &mut EditorApp, _ctx: &Context) {
    if app.plugins.studio.open {
        app.plugins.studio.open = false;
        app.open_plugin_studio_tab();
    }
}

/// The studio, filling its document tab.
pub(crate) fn render_tab(app: &mut EditorApp, ui: &mut Ui) {
    if let Some(id) = app.plugins.studio.pending_open.take() {
        if let Some(index) = app.plugins.index_of(&id) {
            let source = app.plugins.list[index].source.clone();
            app.plugins.studio.draft = Some(Draft::new(&source, Some(id)));
        }
    }
    if app.plugins.studio.new_requested {
        app.plugins.studio.new_requested = false;
        let author = whoami_name();
        let mut name = "My plugin".to_string();
        let mut n = 2;
        while app
            .plugins
            .list
            .iter()
            .any(|plugin| plugin.meta.name == name)
        {
            name = format!("My plugin {n}");
            n += 1;
        }
        let mut draft = Draft::new(&template(&name, &author), None);
        draft.dirty = true;
        app.plugins.studio.draft = Some(draft);
        app.plugins.studio.tab = Tab::Blocks;
    }
    if let Some(path) = app.plugins.studio.picked_image.take() {
        picture_from_file(app, &path);
    }
    if app.plugins.studio.draft.is_none() {
        picker(app, ui);
    } else {
        editor(app, ui);
    }
}

/// Saves the plugin being made, for ctrl+s and the unsaved-changes
/// question. False when there was nothing that could be saved.
pub(crate) fn save_from_tab(app: &mut EditorApp) -> bool {
    let Some(ctx) = app.plugins.ctx.clone() else {
        return false;
    };
    let can_save = app
        .plugins
        .studio
        .draft
        .as_ref()
        .is_some_and(|draft| draft.error.is_none() && draft.doc.is_some());
    if !can_save {
        app.session.status = "the plugin has a mistake; fix it in Pro mode before saving".into();
        return false;
    }
    save_draft(app, &ctx).is_some()
}

fn whoami_name() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "me".into())
}

fn picture_from_file(app: &mut EditorApp, path: &std::path::Path) {
    let Some(draft) = app.plugins.studio.draft.as_mut() else {
        return;
    };
    match std::fs::read(path) {
        Ok(bytes) => {
            if bytes.len() > 4 * 1024 * 1024 {
                app.session.status = "that picture is over 4 MB; plugins stay small".into();
                return;
            }
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("picture");
            let kind = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("png")
                .to_ascii_lowercase();
            let name = unique_blob_name(&draft.blobs, stem);
            draft.blobs.push(Blob::new(&name, &kind, &bytes));
            draft.dirty = true;
            draft.reparse();
            // The first picture becomes the thumbnail.
            if let Some(doc) = draft.doc.as_mut() {
                let mut meta = doc.meta();
                if meta.thumbnail.is_empty() && draft.blobs.len() == 1 {
                    meta.thumbnail = name.clone();
                    doc.set_meta(&meta);
                    draft.regenerate();
                }
            }
            app.session.status = format!("added the picture {name}");
        }
        Err(error) => app.session.status = format!("could not read the picture: {error}"),
    }
}

/// The list to pick a plugin from when nothing is open.
fn picker(app: &mut EditorApp, ui: &mut Ui) {
    let accent = app.settings.theme.accent.to_color32();
    ui.add_space(10.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("Make something").size(22.0).strong());
        ui.label("Start a new plugin, or open one you have to change it.");
        ui.add_space(8.0);
        if crate::theme::primary_button(ui, "New plugin", accent).clicked() {
            app.plugins.studio.new_requested = true;
        }
    });
    ui.add_space(14.0);
    ui.label(RichText::new("Start from an example").strong());
    let mut example = None;
    ui.horizontal_wrapped(|ui| {
        for (name, source) in EXAMPLES {
            let summary = PluginDoc::parse(source)
                .map(|doc| doc.meta().summary)
                .unwrap_or_default();
            if ui.button(name).on_hover_text(summary).clicked() {
                example = Some(source);
            }
        }
    });
    if let Some(source) = example {
        let mut draft = Draft::new(source, None);
        draft.dirty = true;
        app.plugins.studio.draft = Some(draft);
        return;
    }
    ui.add_space(10.0);
    ui.separator();
    ui.label(RichText::new("Your plugins").strong());
    let mut open = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for plugin in &app.plugins.list {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&plugin.meta.name).strong());
                ui.label(
                    RichText::new(&plugin.meta.summary)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Open").clicked() {
                        open = Some(plugin.meta.id.clone());
                    }
                });
            });
        }
    });
    if let Some(id) = open {
        app.plugins.studio.open_plugin(&id);
    }
}

fn editor(app: &mut EditorApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let accent = app.settings.theme.accent.to_color32();
    // The bar: mode, run, save, export, close.
    ui.horizontal(|ui| {
        let studio = &mut app.plugins.studio;
        for (mode, label, hint) in [
            (Mode::Simple, "Simple", "Blocks and forms"),
            (Mode::Pro, "Pro", "The whole file as q0ex text"),
        ] {
            if ui
                .selectable_label(studio.mode == mode, RichText::new(label).strong())
                .on_hover_text(hint)
                .clicked()
            {
                studio.mode = mode;
            }
        }
        ui.separator();
        let runnable = studio
            .draft
            .as_ref()
            .is_some_and(|draft| draft.error.is_none());
        let run = ui.add_enabled(
            runnable,
            egui::Button::new(
                RichText::new("▶ Run")
                    .strong()
                    .color(crate::theme::on_accent(accent)),
            )
            .fill(accent),
        );
        let run = run.on_hover_text("Save, then turn the plugin on (or restart it)");
        let save = ui.add_enabled(runnable, egui::Button::new("Save"));
        #[cfg(not(target_os = "android"))]
        let export = ui.add_enabled(runnable, egui::Button::new("Export .qx..."));
        if ui
            .button("Put away")
            .on_hover_text("Back to the list of plugins; unsaved changes are dropped")
            .clicked()
        {
            app.plugins.studio.draft = None;
            return;
        }
        if run.clicked() || save.clicked() {
            if let Some(id) = save_draft(app, &ctx) {
                if run.clicked() {
                    if let Some(index) = app.plugins.index_of(&id) {
                        super::stop(app, &ctx, index, None);
                        super::enable(app, &ctx, index);
                        let name = app.plugins.list[index].meta.name.clone();
                        app.session.status = format!("{name} is running");
                    }
                }
            }
        }
        #[cfg(not(target_os = "android"))]
        if export.clicked() {
            export_draft(app);
        }
    });
    if let Some(error) = app
        .plugins
        .studio
        .draft
        .as_ref()
        .and_then(|draft| draft.error.clone())
    {
        egui::Frame::none()
            .fill(Color32::from_rgb(235, 87, 87).gamma_multiply(0.14))
            .rounding(6.0)
            .inner_margin(egui::Margin::symmetric(10.0, 6.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!("Line {}: {}", error.line, error.message))
                        .color(Color32::from_rgb(235, 87, 87)),
                );
                if app.plugins.studio.mode == Mode::Simple {
                    ui.label(RichText::new("Fix it in Pro mode to use the blocks again.").small());
                }
            });
    }
    ui.separator();
    match app.plugins.studio.mode {
        Mode::Simple => simple(app, ui),
        Mode::Pro => pro(app, ui),
    }
}

/// Saves the plugin being made into the plugin folder. Gives back its id.
fn save_draft(app: &mut EditorApp, ctx: &Context) -> Option<String> {
    let draft = app.plugins.studio.draft.as_mut()?;
    let doc = draft.doc.as_ref()?;
    let id = doc.meta().id;
    let source = draft.full_source();
    // A changed id is a new plugin; the old one stays as it was.
    let old_id = draft.saved_id.clone();
    match super::save_plugin_file(app, ctx, &id, &source) {
        Ok(()) => {
            if let Some(draft) = app.plugins.studio.draft.as_mut() {
                draft.saved_id = Some(id.clone());
                draft.dirty = false;
            }
            if old_id.as_deref().is_some_and(|old| old != id) {
                app.session.status = format!("saved as a new plugin, {id}");
            } else {
                app.session.status = "plugin saved".into();
            }
            Some(id)
        }
        Err(error) => {
            app.session.status = format!("could not save the plugin: {error}");
            None
        }
    }
}

#[cfg(not(target_os = "android"))]
fn export_draft(app: &mut EditorApp) {
    let Some(draft) = app.plugins.studio.draft.as_ref() else {
        return;
    };
    let Some(doc) = &draft.doc else {
        return;
    };
    let name = super::file_name_for(&doc.meta().id);
    if let Some(path) = rfd::FileDialog::new()
        .set_file_name(&name)
        .add_filter("q0ex plugin", &q0ex::EXTENSIONS)
        .save_file()
    {
        match std::fs::write(&path, draft.full_source()) {
            Ok(()) => app.session.status = format!("exported {}", path.display()),
            Err(error) => app.session.status = format!("could not export: {error}"),
        }
    }
}

// ------------------------------------------------------------- simple

fn simple(app: &mut EditorApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        let studio = &mut app.plugins.studio;
        for (tab, label) in [
            (Tab::Blocks, "Blocks"),
            (Tab::Card, "Card"),
            (Tab::Page, "Page"),
            (Tab::Settings, "Settings"),
            (Tab::Pictures, "Pictures"),
        ] {
            if ui.selectable_label(studio.tab == tab, label).clicked() {
                studio.tab = tab;
            }
        }
    });
    ui.add_space(4.0);
    let usable = app
        .plugins
        .studio
        .draft
        .as_ref()
        .is_some_and(|draft| draft.error.is_none() && draft.doc.is_some());
    if !usable {
        ui.label("This plugin's text has a mistake; Pro mode shows where.");
        return;
    }
    match app.plugins.studio.tab {
        Tab::Blocks => {
            let theme = app.settings.theme.clone();
            let studio = &mut app.plugins.studio;
            let draft = studio.draft.as_mut().expect("checked above");
            let doc = draft.doc.as_mut().expect("checked above");
            if studio.blocks.show(ui, &mut doc.program, &theme) {
                draft.regenerate();
            }
        }
        Tab::Card => card_tab(app, ui),
        Tab::Page => page_tab(app, ui),
        Tab::Settings => settings_tab(app, ui),
        Tab::Pictures => pictures_tab(app, ui),
    }
}

fn card_tab(app: &mut EditorApp, ui: &mut Ui) {
    let draft = app.plugins.studio.draft.as_mut().expect("a plugin is open");
    let doc = draft.doc.as_mut().expect("it reads");
    let mut meta = doc.meta();
    let before = meta.clone();
    let blob_names = draft
        .blobs
        .iter()
        .map(|blob| blob.name.clone())
        .collect::<Vec<_>>();
    egui::Grid::new("plugin_card_form")
        .num_columns(2)
        .spacing([14.0, 8.0])
        .show(ui, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut meta.name);
            ui.end_row();
            ui.label("Id")
                .on_hover_text("Unique, like yourname.pluginname. Changing it makes a new plugin.");
            ui.text_edit_singleline(&mut meta.id);
            ui.end_row();
            ui.label("Version");
            ui.text_edit_singleline(&mut meta.version);
            ui.end_row();
            ui.label("Author");
            ui.text_edit_singleline(&mut meta.author);
            ui.end_row();
            ui.label("Summary");
            ui.add(egui::TextEdit::singleline(&mut meta.summary).desired_width(360.0));
            ui.end_row();
            ui.label("Colour");
            let [r, g, b, _] =
                q0ex::stdlib::parse_color(&meta.color).unwrap_or([157, 78, 255, 255]);
            let mut color = Color32::from_rgb(r, g, b);
            if ui.color_edit_button_srgba(&mut color).changed() {
                meta.color = q0ex::stdlib::color_text([color.r(), color.g(), color.b(), 255]);
            }
            ui.end_row();
            ui.label("Picture");
            egui::ComboBox::from_id_source("plugin_card_thumb")
                .selected_text(if meta.thumbnail.is_empty() {
                    "none"
                } else {
                    &meta.thumbnail
                })
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(meta.thumbnail.is_empty(), "none")
                        .clicked()
                    {
                        meta.thumbnail.clear();
                    }
                    for name in &blob_names {
                        if ui.selectable_label(meta.thumbnail == *name, name).clicked() {
                            meta.thumbnail = name.clone();
                        }
                    }
                });
            ui.end_row();
        });
    if blob_names.is_empty() {
        ui.label(
            RichText::new("Add a picture under Pictures to give the plugin a thumbnail.")
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    }
    if meta != before {
        doc.set_meta(&meta);
        draft.regenerate();
    }
}

/// Inserts markup at the page editor's cursor, around the selection.
fn insert_markup(ctx: &Context, id: egui::Id, text: &mut String, before: &str, after: &str) {
    let state = egui::TextEdit::load_state(ctx, id);
    let range = state
        .as_ref()
        .and_then(|state| state.cursor.char_range())
        .map(|range| {
            let (a, b) = (range.primary.index, range.secondary.index);
            (a.min(b), a.max(b))
        })
        .unwrap_or((text.chars().count(), text.chars().count()));
    let byte = |chars: usize| {
        text.char_indices()
            .nth(chars)
            .map_or(text.len(), |(at, _)| at)
    };
    let (start, end) = (byte(range.0), byte(range.1));
    let selected = text[start..end].to_string();
    let inserted = format!("{before}{selected}{after}");
    text.replace_range(start..end, &inserted);
    if let Some(mut state) = state {
        let cursor = range.0 + before.chars().count() + selected.chars().count();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(cursor),
            )));
        state.store(ctx, id);
    }
}

fn page_tab(app: &mut EditorApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let draft = app.plugins.studio.draft.as_mut().expect("a plugin is open");
    let doc = draft.doc.as_mut().expect("it reads");
    let mut page = doc.page();
    let before = page.clone();
    let editor_id = egui::Id::new("plugin_page_editor");
    ui.horizontal_wrapped(|ui| {
        let mut wrap = |label: &str, hint: &str, a: &str, b: &str, ui: &mut Ui| {
            if ui.button(label).on_hover_text(hint).clicked() {
                insert_markup(&ctx, editor_id, &mut page, a, b);
            }
        };
        wrap("H1", "A big heading", "\n# ", "", ui);
        wrap("H2", "A smaller heading", "\n## ", "", ui);
        wrap("B", "Bold", "**", "**", ui);
        wrap("I", "Italic", "*", "*", ui);
        wrap("U", "Underline", "__", "__", ui);
        wrap("Colour", "Coloured text", "{#ff4488}", "{/}", ui);
        wrap("Big", "Bigger text", "{big}", "{/}", ui);
        wrap("•", "A bullet", "\n- ", "", ui);
        wrap("Note", "A callout", "\n> ", "", ui);
        wrap("Centre", "A centred line", "->", "<-", ui);
        wrap("---", "A divider", "\n---\n", "", ui);
        wrap("Link", "A link", "[", "](https://)", ui);
        let names = draft
            .blobs
            .iter()
            .map(|blob| blob.name.clone())
            .collect::<Vec<_>>();
        if !names.is_empty() {
            ui.menu_button("Picture", |ui| {
                for name in names {
                    if ui.button(&name).clicked() {
                        insert_markup(&ctx, editor_id, &mut page, &format!("\n![]({name})\n"), "");
                        ui.close_menu();
                    }
                }
            });
        }
    });
    ui.add_space(4.0);
    let preview_doc = doc.clone();
    let id = doc.meta().id;
    ui.columns(2, |columns| {
        egui::ScrollArea::vertical()
            .id_source("plugin_page_text")
            .show(&mut columns[0], |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut page)
                        .id(editor_id)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(24)
                        .desired_width(f32::INFINITY),
                );
            });
        egui::Frame::none()
            .fill(columns[1].visuals().faint_bg_color)
            .rounding(8.0)
            .inner_margin(egui::Margin::same(10.0))
            .show(&mut columns[1], |ui| {
                egui::ScrollArea::vertical()
                    .id_source("plugin_page_preview")
                    .show(ui, |ui| {
                        super::page::show(ui, &page, Some(&preview_doc), &id)
                    });
            });
    });
    if page != before {
        doc.set_page(&page);
        draft.regenerate();
    }
}

fn settings_tab(app: &mut EditorApp, ui: &mut Ui) {
    let draft = app.plugins.studio.draft.as_mut().expect("a plugin is open");
    let doc = draft.doc.as_mut().expect("it reads");
    let mut decls = doc.settings();
    let before = decls.clone();
    let mut remove = None;
    let mut swap = None;
    ui.label(
        RichText::new(
            "What people can set on the plugin's page. Code reads a setting as config.name.",
        )
        .small()
        .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_source("plugin_settings_form")
        .max_height(ui.available_height() - 40.0)
        .show(ui, |ui| {
            let count = decls.len();
            for (index, decl) in decls.iter_mut().enumerate() {
                egui::Frame::none()
                    .fill(ui.visuals().faint_bg_color)
                    .rounding(8.0)
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            egui::ComboBox::from_id_source(("setting-kind", index))
                                .selected_text(decl.kind.word())
                                .width(80.0)
                                .show_ui(ui, |ui| {
                                    for kind in SettingKind::ALL {
                                        ui.selectable_value(&mut decl.kind, kind, kind.word());
                                    }
                                });
                            if decl.kind.has_key() {
                                ui.label("name");
                                ui.add(
                                    egui::TextEdit::singleline(&mut decl.key).desired_width(90.0),
                                );
                            }
                            ui.label("label");
                            ui.add(
                                egui::TextEdit::singleline(&mut decl.label).desired_width(150.0),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
                                        remove = Some(index);
                                    }
                                    if index + 1 < count && ui.small_button("↓").clicked() {
                                        swap = Some(index);
                                    }
                                    if index > 0 && ui.small_button("↑").clicked() {
                                        swap = Some(index - 1);
                                    }
                                },
                            );
                        });
                        setting_details(ui, decl, index);
                    });
                ui.add_space(4.0);
            }
        });
    ui.horizontal(|ui| {
        ui.menu_button("+ Add setting", |ui| {
            for kind in SettingKind::ALL {
                if ui.button(kind.word()).clicked() {
                    let n = decls.len() + 1;
                    decls.push(SettingDecl {
                        kind,
                        key: if kind.has_key() {
                            format!("setting{n}")
                        } else {
                            String::new()
                        },
                        label: match kind {
                            SettingKind::Heading => "A section".into(),
                            SettingKind::Note => "Something to know.".into(),
                            _ => format!("Setting {n}"),
                        },
                        default: None,
                        min: matches!(kind, SettingKind::Slider).then_some(0.0),
                        max: matches!(kind, SettingKind::Slider).then_some(10.0),
                        step: None,
                        options: if kind == SettingKind::Choice {
                            vec![
                                q0ex::Expr::new(q0ex::ExprKind::Str("one".into()), 0),
                                q0ex::Expr::new(q0ex::ExprKind::Str("two".into()), 0),
                            ]
                        } else {
                            Vec::new()
                        },
                        help: None,
                        line: 0,
                    });
                    ui.close_menu();
                }
            }
        });
    });
    if let Some(index) = remove {
        decls.remove(index);
    }
    if let Some(index) = swap {
        decls.swap(index, index + 1);
    }
    for decl in &mut decls {
        decl.key = decl
            .key
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if decl.kind.has_key() && decl.key.is_empty() {
            decl.key = "setting".into();
        }
        if !decl.kind.has_key() {
            decl.key.clear();
        }
    }
    if decls != before {
        doc.set_settings(decls);
        draft.regenerate();
    }
}

/// The parts a kind of setting has beyond its name and label.
fn setting_details(ui: &mut Ui, decl: &mut SettingDecl, index: usize) {
    let expr_field =
        |ui: &mut Ui, label: &str, expr: &mut Option<q0ex::Expr>, id: (usize, &str)| {
            let mut text = expr
                .as_ref()
                .map(q0ex::printer::print_expr)
                .unwrap_or_default();
            ui.label(label);
            let response = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .id_source(id)
                    .desired_width(110.0),
            );
            if response.changed() {
                *expr = if text.trim().is_empty() {
                    None
                } else {
                    q0ex::parser::parse_expr(&text)
                        .ok()
                        .or_else(|| expr.clone())
                };
            }
        };
    ui.horizontal_wrapped(|ui| {
        match decl.kind {
            SettingKind::Slider | SettingKind::Number => {
                expr_field(ui, "starts at", &mut decl.default, (index, "default"));
                let mut low = decl.min.unwrap_or(0.0);
                let mut high = decl.max.unwrap_or(10.0);
                let ranged = decl.kind == SettingKind::Slider || decl.min.is_some();
                let mut use_range = ranged;
                if decl.kind == SettingKind::Number {
                    ui.checkbox(&mut use_range, "limits");
                }
                if use_range {
                    ui.label("from");
                    ui.add(egui::DragValue::new(&mut low).speed(0.1));
                    ui.label("to");
                    ui.add(egui::DragValue::new(&mut high).speed(0.1));
                    decl.min = Some(low);
                    decl.max = Some(high);
                } else {
                    decl.min = None;
                    decl.max = None;
                }
                let mut stepped = decl.step.is_some();
                ui.checkbox(&mut stepped, "step");
                if stepped {
                    let mut step = decl.step.unwrap_or(1.0);
                    ui.add(
                        egui::DragValue::new(&mut step)
                            .speed(0.05)
                            .clamp_range(0.0001..=1e6),
                    );
                    decl.step = Some(step);
                } else {
                    decl.step = None;
                }
            }
            SettingKind::Toggle => {
                let mut on = matches!(
                    decl.default.as_ref().map(|expr| &expr.kind),
                    Some(q0ex::ExprKind::Bool(true))
                );
                if ui.checkbox(&mut on, "starts on").changed() {
                    decl.default = Some(q0ex::Expr::new(q0ex::ExprKind::Bool(on), 0));
                }
            }
            SettingKind::Text | SettingKind::Color => {
                let mut text = match decl.default.as_ref().map(|expr| &expr.kind) {
                    Some(q0ex::ExprKind::Str(text)) => text.clone(),
                    _ => String::new(),
                };
                ui.label("starts as");
                if decl.kind == SettingKind::Color {
                    let [r, g, b, a] =
                        q0ex::stdlib::parse_color(&text).unwrap_or([255, 255, 255, 255]);
                    let mut color = Color32::from_rgba_unmultiplied(r, g, b, a);
                    if ui.color_edit_button_srgba(&mut color).changed() {
                        let [r, g, b, a] = color.to_srgba_unmultiplied();
                        text = q0ex::stdlib::color_text([r, g, b, a]);
                        decl.default = Some(q0ex::Expr::new(q0ex::ExprKind::Str(text), 0));
                    }
                } else if ui
                    .add(egui::TextEdit::singleline(&mut text).desired_width(140.0))
                    .changed()
                {
                    decl.default = Some(q0ex::Expr::new(q0ex::ExprKind::Str(text), 0));
                }
            }
            SettingKind::Choice => {
                let mut options = decl
                    .options
                    .iter()
                    .map(|option| match &option.kind {
                        q0ex::ExprKind::Str(text) => text.clone(),
                        _ => q0ex::printer::print_expr(option),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                ui.label("options");
                if ui
                    .add(egui::TextEdit::singleline(&mut options).desired_width(200.0))
                    .on_hover_text("Separated by commas")
                    .changed()
                {
                    decl.options = options
                        .split(',')
                        .map(str::trim)
                        .filter(|option| !option.is_empty())
                        .map(|option| q0ex::Expr::new(q0ex::ExprKind::Str(option.to_string()), 0))
                        .collect();
                    if decl.options.is_empty() {
                        decl.options
                            .push(q0ex::Expr::new(q0ex::ExprKind::Str("one".into()), 0));
                    }
                }
                let current = match decl.default.as_ref().map(|expr| &expr.kind) {
                    Some(q0ex::ExprKind::Str(text)) => text.clone(),
                    _ => String::new(),
                };
                egui::ComboBox::from_id_source(("choice-default", index))
                    .selected_text(if current.is_empty() {
                        "first"
                    } else {
                        &current
                    })
                    .show_ui(ui, |ui| {
                        for option in decl.options.clone() {
                            let text = match &option.kind {
                                q0ex::ExprKind::Str(text) => text.clone(),
                                _ => continue,
                            };
                            if ui.selectable_label(current == text, &text).clicked() {
                                decl.default = Some(option.clone());
                            }
                        }
                    });
            }
            _ => {}
        }
        if decl.kind != SettingKind::Note {
            let mut help = decl.help.clone().unwrap_or_default();
            ui.label(if decl.kind == SettingKind::Heading {
                "under it"
            } else {
                "hint"
            });
            if ui
                .add(egui::TextEdit::singleline(&mut help).desired_width(180.0))
                .changed()
            {
                decl.help = (!help.trim().is_empty()).then_some(help);
            }
        }
    });
}

fn pictures_tab(app: &mut EditorApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    if ui.button("+ Add picture...").clicked() {
        app.queue(crate::app::Action::PluginStudioPickPicture);
    }
    ui.add_space(6.0);
    let draft = app.plugins.studio.draft.as_mut().expect("a plugin is open");
    let thumbnail = draft
        .doc
        .as_ref()
        .map(|doc| doc.meta().thumbnail)
        .unwrap_or_default();
    let mut remove = None;
    let mut make_thumb = None;
    egui::ScrollArea::vertical().id_source("plugin_pictures").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (index, blob) in draft.blobs.iter().enumerate() {
                egui::Frame::none()
                    .fill(ui.visuals().faint_bg_color)
                    .rounding(8.0)
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.set_width(150.0);
                        ui.vertical(|ui| {
                            let texture = blob
                                .bytes()
                                .and_then(|bytes| super::cached_texture(&ctx, &format!("studio-{}", blob.name), &bytes));
                            match texture {
                                Some(texture) => {
                                    let size = texture.size_vec2();
                                    let scale = (130.0 / size.x.max(size.y)).min(1.0);
                                    ui.image((texture.id(), size * scale));
                                }
                                None => {
                                    ui.label(RichText::new(format!("{} file", blob.kind)).italics());
                                }
                            }
                            ui.label(RichText::new(&blob.name).monospace());
                            let size = blob.data.len() * 3 / 4;
                            ui.label(
                                RichText::new(format!("{} KB", size.div_ceil(1024)))
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            );
                            ui.horizontal(|ui| {
                                if thumbnail == blob.name {
                                    ui.label(RichText::new("thumbnail").small().strong());
                                } else if ui.small_button("Use as thumbnail").clicked() {
                                    make_thumb = Some(blob.name.clone());
                                }
                                if ui.small_button("✕").on_hover_text("Remove").clicked() {
                                    remove = Some(index);
                                }
                            });
                        });
                    });
            }
        });
        if draft.blobs.is_empty() {
            ui.label("No pictures yet. Pictures can be the plugin's thumbnail, sit on its page with ![](name), or show in its windows with ui.image!.");
        }
    });
    if let Some(index) = remove {
        draft.blobs.remove(index);
        draft.dirty = true;
        draft.reparse();
    }
    if let Some(name) = make_thumb {
        if let Some(doc) = draft.doc.as_mut() {
            let mut meta = doc.meta();
            meta.thumbnail = name;
            doc.set_meta(&meta);
            draft.regenerate();
        }
    }
}

// ---------------------------------------------------------------- pro

/// The word being typed before the cursor: letters, digits, `_`, `.`, `!`.
fn word_before(text: &str, cursor: usize) -> (usize, String) {
    let chars = text.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    let mut start = cursor;
    while start > 0
        && (chars[start - 1].is_ascii_alphanumeric() || matches!(chars[start - 1], '_' | '.'))
    {
        start -= 1;
    }
    (start, chars[start..cursor].iter().collect())
}

fn pro(app: &mut EditorApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let theme = app.settings.theme.clone();
    let font = FontId::monospace(13.0);
    let editor_id = egui::Id::new("plugin_pro_editor");
    // Tab takes a suggestion, before the text field sees it.
    let suggestion = app.plugins.studio.suggestions.first().copied();
    let mut take_suggestion = None;
    if let Some(name) = suggestion {
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Tab)) {
            take_suggestion = Some(name);
        }
    }
    let side_width = 280.0_f32.min(ui.available_width() * 0.35);
    egui::SidePanel::right("plugin_pro_reference")
        .resizable(true)
        .default_width(side_width)
        .show_inside(ui, |ui| reference(app, ui, editor_id));
    egui::TopBottomPanel::bottom("plugin_pro_console")
        .resizable(true)
        .default_height(130.0)
        .show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Console").strong());
                if ui.small_button("Clear").clicked() {
                    app.plugins.console.clear();
                }
            });
            let name = app.plugins.studio.draft.as_ref().map(Draft::name);
            super::manager::console(ui, app, name.as_deref());
        });
    let draft = app.plugins.studio.draft.as_mut().expect("a plugin is open");
    let error_line = draft.error.as_ref().map(|error| error.line);
    let line_count = draft.text.lines().count().max(1) + usize::from(draft.text.ends_with('\n'));
    let row_height = ctx.fonts(|fonts| fonts.row_height(&font));
    let theme_for_layout = theme.clone();
    let font_for_layout = font.clone();
    let mut layouter = move |ui: &Ui, text: &str, _wrap: f32| {
        let job = super::highlight::layout(
            text,
            &theme_for_layout,
            font_for_layout.clone(),
            f32::INFINITY,
            error_line,
        );
        ui.fonts(|fonts| fonts.layout_job(job))
    };
    if let Some(name) = take_suggestion {
        if let Some(state) = egui::TextEdit::load_state(&ctx, editor_id) {
            if let Some(range) = state.cursor.char_range() {
                let cursor = range.primary.index;
                let (start, _) = word_before(&draft.text, cursor);
                let byte = |chars: usize| {
                    draft
                        .text
                        .char_indices()
                        .nth(chars)
                        .map_or(draft.text.len(), |(at, _)| at)
                };
                let (from, to) = (byte(start), byte(cursor));
                draft.text.replace_range(from..to, name);
                let mut state = state;
                let new_cursor = start + name.chars().count();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(new_cursor),
                    )));
                state.store(&ctx, editor_id);
                draft.dirty = true;
                draft.reparse();
            }
        }
    }
    let mut changed = false;
    let mut cursor_rect = None;
    egui::ScrollArea::both()
        .id_source("plugin_pro_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let digits = line_count.to_string().len().max(2);
                let char_width = ctx.fonts(|fonts| fonts.glyph_width(&font, '0'));
                let gutter = egui::vec2(
                    char_width * digits as f32 + 14.0,
                    row_height * line_count as f32 + 4.0,
                );
                let (rect, _) = ui.allocate_exact_size(gutter, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, theme.syntax_gutter_bg.to_color32());
                let clip = ui.clip_rect();
                let first = (((clip.top() - rect.top()) / row_height).floor().max(0.0)) as usize;
                let last = ((((clip.bottom() - rect.top()) / row_height).ceil()) as usize + 1)
                    .min(line_count);
                for line in first..last {
                    let n = line + 1;
                    let color = if Some(n as u32) == error_line {
                        Color32::from_rgb(235, 87, 87)
                    } else {
                        theme.syntax_gutter_fg.to_color32()
                    };
                    painter.text(
                        egui::pos2(
                            rect.right() - 6.0,
                            rect.top() + 2.0 + line as f32 * row_height,
                        ),
                        egui::Align2::RIGHT_TOP,
                        n.to_string(),
                        font.clone(),
                        color,
                    );
                }
                ui.add_space(4.0);
                let output = egui::TextEdit::multiline(&mut draft.text)
                    .id(editor_id)
                    .desired_rows(line_count.max(30))
                    .desired_width(ui.available_width().max(400.0))
                    .frame(false)
                    .font(font.clone())
                    .code_editor()
                    .lock_focus(true)
                    .layouter(&mut layouter)
                    .show(ui);
                if output.response.changed() {
                    changed = true;
                }
                if output.response.has_focus() {
                    if let Some(range) = output.cursor_range {
                        let rect = output.galley.pos_from_cursor(&range.primary);
                        cursor_rect = Some((
                            rect.translate(output.galley_pos.to_vec2()),
                            range.primary.ccursor.index,
                        ));
                    }
                }
            });
        });
    if changed {
        auto_indent(&ctx, editor_id, &mut draft.text);
        draft.dirty = true;
        draft.reparse();
    }
    // Suggestions under the cursor.
    let studio = &mut app.plugins.studio;
    studio.suggestions.clear();
    if let (Some((rect, cursor)), Some(draft)) = (cursor_rect, studio.draft.as_ref()) {
        let (_, word) = word_before(&draft.text, cursor);
        if word.len() >= 2 {
            let found = api::complete(&word);
            studio.suggestions = found.iter().map(|entry| entry.name).collect();
            studio.suggestion_prefix = word;
            if !found.is_empty() {
                egui::Area::new(egui::Id::new("plugin_suggestions"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(rect.left_bottom() + vec2(0.0, 4.0))
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            for (index, entry) in found.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    let name = RichText::new(entry.name)
                                        .monospace()
                                        .color(entry.cat.color());
                                    ui.label(if index == 0 { name.strong() } else { name });
                                    ui.label(
                                        RichText::new(entry.doc)
                                            .small()
                                            .color(ui.visuals().weak_text_color()),
                                    );
                                });
                            }
                            ui.label(RichText::new("Tab takes the first").small().italics());
                        });
                    });
            }
        }
    }
}

/// After Enter, the new line starts as indented as the one above, or a
/// step more after a `{`.
fn auto_indent(ctx: &Context, id: egui::Id, text: &mut String) {
    let Some(mut state) = egui::TextEdit::load_state(ctx, id) else {
        return;
    };
    let Some(range) = state.cursor.char_range() else {
        return;
    };
    let cursor = range.primary.index;
    let chars = text.chars().collect::<Vec<_>>();
    if cursor == 0 || cursor > chars.len() || chars[cursor - 1] != '\n' {
        return;
    }
    // Only right after Enter: nothing typed on the new line yet.
    if chars.get(cursor).is_some_and(|c| *c != '\n') {
        return;
    }
    let line_start = chars[..cursor - 1]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |at| at + 1);
    let previous: String = chars[line_start..cursor - 1].iter().collect();
    let mut indent: String = previous.chars().take_while(|c| *c == ' ').collect();
    if previous.trim_end().ends_with('{') {
        indent.push_str("  ");
    }
    if indent.is_empty() {
        return;
    }
    let at = text
        .char_indices()
        .nth(cursor)
        .map_or(text.len(), |(at, _)| at);
    text.insert_str(at, &indent);
    let new_cursor = cursor + indent.chars().count();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(new_cursor),
        )));
    state.store(ctx, id);
}

/// Every word, by category, searchable; clicking one types it.
fn reference(app: &mut EditorApp, ui: &mut Ui, editor_id: egui::Id) {
    let ctx = ui.ctx().clone();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Reference").strong());
        if ui
            .small_button("+ Picture")
            .on_hover_text("Add a picture to the plugin")
            .clicked()
        {
            app.queue(crate::app::Action::PluginStudioPickPicture);
        }
    });
    ui.add(
        egui::TextEdit::singleline(&mut app.plugins.studio.reference_filter).hint_text("search"),
    );
    let filter = app.plugins.studio.reference_filter.to_lowercase();
    let mut insert = None;
    egui::ScrollArea::vertical()
        .id_source("plugin_reference")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let events = api::EVENTS
                .iter()
                .filter(|event| {
                    filter.is_empty()
                        || event.event.contains(&filter)
                        || event.doc.to_lowercase().contains(&filter)
                })
                .collect::<Vec<_>>();
            if !events.is_empty() {
                egui::CollapsingHeader::new(RichText::new("When...").color(Cat::Events.color()))
                    .default_open(!filter.is_empty())
                    .show(ui, |ui| {
                        for event in events {
                            let mut head = format!("when {}", event.event);
                            if let Some(arg) = event.arg {
                                head.push_str(&format!(" \"{arg}\""));
                            }
                            if !event.params.is_empty() {
                                head.push(' ');
                                head.push_str(&event.params.join(", "));
                            }
                            let response = ui.add(
                                egui::Label::new(RichText::new(&head).monospace())
                                    .sense(egui::Sense::click()),
                            );
                            if response.on_hover_text(event.doc).clicked() {
                                insert = Some(format!("{head} {{\n  \n}}\n"));
                            }
                        }
                    });
            }
            for cat in Cat::ALL {
                let entries = api::ENTRIES
                    .iter()
                    .filter(|entry| entry.cat == cat)
                    .filter(|entry| {
                        filter.is_empty()
                            || entry.name.contains(&filter)
                            || entry.doc.to_lowercase().contains(&filter)
                    })
                    .collect::<Vec<_>>();
                if entries.is_empty() {
                    continue;
                }
                egui::CollapsingHeader::new(RichText::new(cat.label()).color(cat.color()))
                    .default_open(!filter.is_empty())
                    .show(ui, |ui| {
                        for entry in entries {
                            let mut call = if entry.args.is_empty() {
                                entry.name.to_string()
                            } else if entry.name.ends_with('!') && !entry.value {
                                format!("{} {}", entry.name, entry.args.join(", "))
                            } else {
                                format!("{}({})", entry.name, entry.args.join(", "))
                            };
                            if entry.value {
                                call.push_str("  →");
                            }
                            let response = ui.add(
                                egui::Label::new(RichText::new(&call).monospace())
                                    .sense(egui::Sense::click()),
                            );
                            if response.on_hover_text(entry.doc).clicked() {
                                insert = Some(
                                    if entry.name.starts_with("obj.")
                                        || entry.name.starts_with("layer.")
                                        || entry.name.starts_with("timeline.")
                                    {
                                        entry
                                            .name
                                            .split_once('.')
                                            .map_or(String::new(), |(_, rest)| format!(".{rest}"))
                                    } else {
                                        entry.name.to_string()
                                    },
                                );
                            }
                        }
                    });
            }
        });
    if let Some(text) = insert {
        if let Some(draft) = app.plugins.studio.draft.as_mut() {
            insert_markup(&ctx, editor_id, &mut draft.text, &text, "");
            draft.dirty = true;
            draft.reparse();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_before_the_cursor_includes_dots() {
        assert_eq!(
            word_before("x = editor.go", 13),
            (4, "editor.go".to_string())
        );
        assert_eq!(word_before("say! ", 5), (5, String::new()));
    }

    #[test]
    fn drafts_keep_pictures_out_of_the_text() {
        let mut doc = PluginDoc::parse(&template("Pics", "me")).unwrap();
        doc.set_blob(Blob::new("thumbnail", "png", &[1, 2, 3]));
        let draft = Draft::new(&doc.source(), None);
        assert!(!draft.text.contains("blob thumbnail"));
        assert_eq!(draft.blobs.len(), 1);
        assert!(draft.error.is_none());
        assert_eq!(draft.doc.as_ref().unwrap().thumbnail(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn blob_names_stay_unique() {
        let blobs = vec![Blob::new("shot", "png", &[1])];
        assert_eq!(unique_blob_name(&blobs, "shot"), "shot2");
        assert_eq!(unique_blob_name(&blobs, "My Pic"), "my_pic");
    }
}
