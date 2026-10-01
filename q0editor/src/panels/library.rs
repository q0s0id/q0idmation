use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{
    pos2, vec2, Align, Color32, FontId, Layout, Rect, Response, Sense, Stroke, TextStyle, Ui,
    WidgetInfo, WidgetType,
};
use q0s_format::v2::{Asset, Placement, ProjectV2, Rgba, Target, Transform2D, Tween};

use crate::app::{Action, EditorApp, LibrarySelection, LibrarySource, ProjectLibrarySummary};
use crate::render::{active_placements_at, placement_bbox, q0rg_frame_bbox, StageView};
use crate::state::{LibraryDragPayload, LibraryItem, LibraryRename, Selection};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LibraryKey {
    Q0rg(u16),
    Asset(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibraryIcon {
    Stage,
    Symbol,
    Vector,
    Bitmap,
    Video,
    Audio,
}

impl LibraryIcon {
    fn for_q0rg(q0rg_id: u16, entry_q0rg_id: u16) -> Self {
        if q0rg_id == entry_q0rg_id {
            Self::Stage
        } else {
            Self::Symbol
        }
    }

    fn for_asset(asset: &Asset) -> Self {
        match asset {
            Asset::Vector(_) => Self::Vector,
            Asset::Bitmap(_) => Self::Bitmap,
            Asset::Q0v(media) => {
                if crate::audio::q0v_is_audio_only_bytes(&media.bytes) {
                    Self::Audio
                } else {
                    Self::Video
                }
            }
            Asset::Rig(_) => Self::Symbol,
        }
    }
}

#[derive(Debug, Clone)]
struct LibraryRow {
    tab_id: u64,
    library_label: String,
    key: LibraryKey,
    icon: LibraryIcon,
    name: String,
    kind: &'static str,
    uses: usize,
    detail: String,
    folder: Option<String>,
}

const LIBRARY_ROOT_FILTER: &str = "\0root";

#[derive(Debug, Clone)]
pub(crate) struct LibraryCache {
    key: u64,
    rows: Arc<Vec<LibraryRow>>,
    item_keys: HashSet<(u64, LibraryKey)>,
    audio_only_items: HashSet<(u64, LibraryKey)>,
    preview_bounds: Option<(LibraryPreviewBoundsKey, Option<(f32, f32, f32, f32)>)>,
    preview_backgrounds: HashMap<LibraryPreviewBackgroundKey, (PreviewBackground, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LibraryPreviewBoundsKey {
    tab_id: u64,
    item: LibraryKey,
    frame: u16,
    project_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LibraryPreviewBackgroundKey {
    tab_id: u64,
    item: LibraryKey,
    frame: u16,
    library_revision: u64,
}

impl Default for LibraryCache {
    fn default() -> Self {
        Self {
            key: u64::MAX,
            rows: Arc::new(Vec::new()),
            item_keys: HashSet::new(),
            audio_only_items: HashSet::new(),
            preview_bounds: None,
            preview_backgrounds: HashMap::new(),
        }
    }
}

#[cfg(test)]
fn hash_library_project(project: &ProjectV2, hasher: &mut impl Hasher) {
    project.meta.entry_q0rg_id.hash(hasher);
    project.q0rgs.len().hash(hasher);
    project.assets.len().hash(hasher);
    project.asset_names.len().hash(hasher);
    for q0rg in &project.q0rgs {
        q0rg.q0rg_id.hash(hasher);
        q0rg.name.hash(hasher);
        q0rg.frame_count.hash(hasher);
        q0rg.layers.len().hash(hasher);
        for layer in &q0rg.layers {
            layer.placements.len().hash(hasher);
            for placement in &layer.placements {
                match placement.target {
                    Target::Q0rg(id) => {
                        0_u8.hash(hasher);
                        id.hash(hasher);
                    }
                    Target::Asset(id) => {
                        1_u8.hash(hasher);
                        id.hash(hasher);
                    }
                }
            }
        }
    }
    for asset in &project.assets {
        let id = asset.id();
        id.hash(hasher);
        project.asset_names.get(&id).hash(hasher);
        match asset {
            Asset::Bitmap(bitmap) => {
                0_u8.hash(hasher);
                bitmap.width.hash(hasher);
                bitmap.height.hash(hasher);
            }
            Asset::Vector(vector) => {
                1_u8.hash(hasher);
                vector.paths.len().hash(hasher);
            }
            Asset::Q0v(video) => {
                2_u8.hash(hasher);
                video.bytes.len().hash(hasher);
            }
            Asset::Rig(rig) => {
                3_u8.hash(hasher);
                rig.nodes.len().hash(hasher);
                rig.controls.len().hash(hasher);
            }
        }
    }
}

fn library_rows_cache_key(
    app: &EditorApp,
    source: LibrarySource,
    libraries: &[ProjectLibrarySummary],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    match source {
        LibrarySource::All => 0_u8.hash(&mut hasher),
        LibrarySource::Project(tab_id) => {
            1_u8.hash(&mut hasher);
            tab_id.hash(&mut hasher);
        }
    }
    app.library_search.hash(&mut hasher);
    app.library_sort_ascending.hash(&mut hasher);
    app.library_sort_by_uses.hash(&mut hasher);
    for library in libraries {
        if matches!(source, LibrarySource::Project(tab_id) if tab_id != library.tab_id) {
            continue;
        }
        library.tab_id.hash(&mut hasher);
        library.label.hash(&mut hasher);
        let Some(revision) = app.project_library_revision_for_tab(library.tab_id) else {
            continue;
        };
        revision.hash(&mut hasher);
    }
    hasher.finish()
}

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    use crate::ui::{icon_button, icon_button_enabled, Icon};
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Library").size(15.0).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, Icon::Symbol, "Create q0rg symbol").clicked() {
                app.queue(Action::AddQ0rg);
            }
            if icon_button(
                ui,
                Icon::Import,
                "Import images, audio or video into the library",
            )
            .clicked()
            {
                app.queue(Action::ImportMedia);
            }
        });
    });
    ui.separator();

    let libraries = app.project_library_summaries();
    let active_tab = app.active_project_tab_id();
    let source_is_valid = match app.library_source {
        Some(LibrarySource::All) => !libraries.is_empty(),
        Some(LibrarySource::Project(tab_id)) => libraries.iter().any(|lib| lib.tab_id == tab_id),
        None => false,
    };
    if !source_is_valid {
        app.library_source = active_tab
            .filter(|tab_id| libraries.iter().any(|lib| lib.tab_id == *tab_id))
            .map(LibrarySource::Project)
            .or_else(|| {
                libraries
                    .first()
                    .map(|lib| LibrarySource::Project(lib.tab_id))
            });
        app.library_selection = None;
        app.library_multi_selection.clear();
        app.library_selection_anchor = None;
    }

    if libraries.is_empty() {
        ui.label(
            egui::RichText::new("No open q1s libraries")
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
        return;
    }

    let source = app.library_source.unwrap_or(LibrarySource::All);
    let selected_source_text = match source {
        LibrarySource::All => "All libraries".to_string(),
        LibrarySource::Project(tab_id) => libraries
            .iter()
            .find(|lib| lib.tab_id == tab_id)
            .map(|lib| lib.label.clone())
            .unwrap_or_else(|| "All libraries".to_string()),
    };
    let mut requested_source = source;
    egui::ComboBox::from_id_source("library_project_picker")
        .width(ui.available_width())
        .wrap(true)
        .selected_text(selected_source_text)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut requested_source, LibrarySource::All, "All libraries");
            ui.separator();
            for library in &libraries {
                ui.selectable_value(
                    &mut requested_source,
                    LibrarySource::Project(library.tab_id),
                    &library.label,
                );
            }
        });
    if requested_source != source {
        app.library_source = Some(requested_source);
        app.library_selection = None;
        app.library_multi_selection.clear();
        app.library_selection_anchor = None;
    }
    let source = app.library_source.unwrap_or(requested_source);
    let showing_all = matches!(source, LibrarySource::All);
    let folder_paths = match source {
        LibrarySource::Project(tab_id) => app
            .project_for_document_tab(tab_id)
            .map(|project| project.runtime.library_folders.clone())
            .unwrap_or_default(),
        LibrarySource::All => Vec::new(),
    };
    if !app.library_folder_filter.is_empty()
        && app.library_folder_filter != LIBRARY_ROOT_FILTER
        && !folder_paths.contains(&app.library_folder_filter)
    {
        app.library_folder_filter.clear();
    }
    if !showing_all {
        let selected_folder = app.library_folder_filter.clone();
        let selected_text = if selected_folder.is_empty() {
            "All folders".to_string()
        } else if selected_folder == LIBRARY_ROOT_FILTER {
            "Unfoldered".to_string()
        } else {
            selected_folder.clone()
        };
        let can_manage_folders =
            matches!(source, LibrarySource::Project(tab_id) if Some(tab_id) == active_tab);
        ui.horizontal(|ui| {
            ui.label("Folder");
            egui::ComboBox::from_id_source("library_folder_picker")
                .width(ui.available_width())
                .wrap(true)
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut app.library_folder_filter,
                        String::new(),
                        "All folders",
                    );
                    ui.selectable_value(
                        &mut app.library_folder_filter,
                        LIBRARY_ROOT_FILTER.to_string(),
                        "Unfoldered",
                    );
                    for folder in &folder_paths {
                        let depth = folder.matches('/').count();
                        let label = format!("{}folder: {folder}", "  ".repeat(depth));
                        ui.selectable_value(&mut app.library_folder_filter, folder.clone(), label);
                    }
                });
        });
        ui.horizontal_wrapped(|ui| {
            if icon_button_enabled(
                ui,
                can_manage_folders,
                Icon::Folder,
                "Create library folder",
            )
            .clicked()
            {
                let parent = if selected_folder.is_empty() || selected_folder == LIBRARY_ROOT_FILTER
                {
                    None
                } else {
                    Some(selected_folder.clone())
                };
                app.library_folder_draft = Some(crate::app::LibraryFolderDraft {
                    parent,
                    rename_path: None,
                    name: String::new(),
                });
            }
            let rename_selected = can_manage_folders
                && !selected_folder.is_empty()
                && selected_folder != LIBRARY_ROOT_FILTER;
            if icon_button_enabled(ui, rename_selected, Icon::Rename, "Rename selected folder")
                .clicked()
            {
                let current_name = selected_folder
                    .rsplit('/')
                    .next()
                    .unwrap_or(&selected_folder);
                app.library_folder_draft = Some(crate::app::LibraryFolderDraft {
                    parent: selected_folder
                        .rsplit_once('/')
                        .map(|(parent, _)| parent.to_string()),
                    rename_path: Some(selected_folder.clone()),
                    name: current_name.to_string(),
                });
            }
            if icon_button_enabled(
                ui,
                rename_selected,
                Icon::Delete,
                "Remove folder and move its items to the parent folder",
            )
            .clicked()
            {
                app.queue(Action::DeleteLibraryFolder(selected_folder));
                app.library_folder_filter.clear();
            }
        });
    }
    if let Some(draft) = app.library_folder_draft.as_mut() {
        let mut submit = false;
        let mut cancel = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(if draft.rename_path.is_some() {
                "Rename folder"
            } else {
                "New folder"
            });
            let response = ui.add(
                egui::TextEdit::singleline(&mut draft.name)
                    .hint_text("folder name")
                    .desired_width(125.0),
            );
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            let valid = !draft.name.trim().is_empty() && !draft.name.contains(['/', '\\']);
            if icon_button_enabled(ui, valid, Icon::Check, "Save folder name").clicked() {
                submit = true;
            }
            if icon_button(ui, Icon::Close, "Cancel folder name").clicked() {
                cancel = true;
            }
        });
        if submit {
            let name = draft.name.trim().to_string();
            if !name.is_empty() && !name.contains('/') && !name.contains('\\') {
                let new_path = draft
                    .parent
                    .as_ref()
                    .map(|parent| format!("{parent}/{name}"))
                    .unwrap_or_else(|| name.clone());
                if let Some(old_path) = draft.rename_path.clone() {
                    app.queue(Action::RenameLibraryFolder(old_path, new_path.clone()));
                } else {
                    app.queue(Action::CreateLibraryFolder(new_path.clone()));
                }
                app.library_folder_filter = new_path;
                app.library_folder_draft = None;
            }
        } else if cancel {
            app.library_folder_draft = None;
        }
    }

    let cache_key = library_rows_cache_key(app, source, &libraries);
    if app.library_cache.key != cache_key {
        let mut rows = collect_rows(app, source, &libraries);
        let needle = app.library_search.trim().to_lowercase();
        if !needle.is_empty() {
            rows.retain(|row| {
                row.name.to_lowercase().contains(&needle)
                    || row.kind.to_lowercase().contains(&needle)
                    || row.detail.to_lowercase().contains(&needle)
                    || row
                        .folder
                        .as_deref()
                        .is_some_and(|folder| folder.to_lowercase().contains(&needle))
                    || row.library_label.to_lowercase().contains(&needle)
            });
        }
        if app.library_sort_by_uses {
            rows.sort_by(|left, right| {
                left.folder
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&right.folder.as_deref().unwrap_or("").to_lowercase())
                    .then_with(|| {
                        if app.library_sort_ascending {
                            left.uses.cmp(&right.uses)
                        } else {
                            right.uses.cmp(&left.uses)
                        }
                    })
                    .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            });
        } else {
            rows.sort_by_cached_key(|row| {
                (
                    row.folder.as_deref().unwrap_or("").to_lowercase(),
                    row.name.to_lowercase(),
                    row.library_label.to_lowercase(),
                )
            });
        }
        if !app.library_sort_by_uses && !app.library_sort_ascending {
            rows.reverse();
        }
        let item_keys = rows.iter().map(|row| (row.tab_id, row.key)).collect();
        let audio_only_items = rows
            .iter()
            .filter(|row| row.kind == "audio")
            .map(|row| (row.tab_id, row.key))
            .collect();
        app.library_cache = LibraryCache {
            key: cache_key,
            item_keys,
            audio_only_items,
            preview_bounds: None,
            preview_backgrounds: HashMap::new(),
            rows: Arc::new(rows),
        };
    }
    let all_rows = Arc::clone(&app.library_cache.rows);
    let rows = match app.library_folder_filter.as_str() {
        "" => all_rows,
        LIBRARY_ROOT_FILTER => Arc::new(
            all_rows
                .iter()
                .filter(|row| row.folder.is_none())
                .cloned()
                .collect(),
        ),
        folder => Arc::new(
            all_rows
                .iter()
                .filter(|row| {
                    row.folder.as_deref().is_some_and(|item_folder| {
                        item_folder == folder || item_folder.starts_with(&format!("{folder}/"))
                    })
                })
                .cloned()
                .collect(),
        ),
    };
    app.library_multi_selection.retain(|selection| {
        rows.iter()
            .any(|row| row.tab_id == selection.tab_id && row.key == key_for_item(selection.item))
            && app
                .library_cache
                .item_keys
                .contains(&(selection.tab_id, key_for_item(selection.item)))
    });
    if app
        .library_selection
        .is_some_and(|selection| !app.library_multi_selection.contains(&selection))
    {
        app.library_selection = app.library_multi_selection.last().copied();
    }

    draw_preview(app, ui, app.library_selection);

    let needle = app.library_search.trim().to_lowercase();

    let selected_active_item = app
        .library_selection
        .filter(|selection| {
            Some(selection.tab_id) == active_tab && app.library_multi_selection.len() <= 1
        })
        .map(|selection| selection.item);
    let selected_audio_only = app.library_selection.is_some_and(|selection| {
        app.library_cache
            .audio_only_items
            .contains(&(selection.tab_id, key_for_item(selection.item)))
    });
    let mut place_selected: Option<LibraryItem> = None;
    let mut enter_selected = None;
    let mut rename_selected = None;
    let mut delete_selected = None;
    let mut delete_multi_selected = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!("Items: {}", rows.len()))
                .small()
                .color(app.settings.theme.text_dim.to_color32()),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if app.library_multi_selection.len() > 1 {
                    if ui
                        .button(format!("Delete {} selected", app.library_multi_selection.len()))
                        .clicked()
                    {
                        delete_multi_selected = true;
                    }
                } else if let Some(item) = selected_active_item {
                    let can_place = !matches!(item, LibraryItem::Q0rg(id) if id == app.session.current_q0rg_id)
                    && !selected_audio_only;
                if ui
                    .add_enabled(can_place, egui::Button::new("Place").small())
                    .clicked()
                {
                    place_selected = Some(item);
                }
                if icon_button_enabled(ui, can_rename_item(app, item), Icon::Rename, "Rename library item (F2)").clicked()
                {
                    rename_selected = Some(item);
                }
                let can_delete = !matches!(
                    item,
                    LibraryItem::Q0rg(id) if id == app.state.project.meta.entry_q0rg_id
                );
                if icon_button_enabled(ui, can_delete, Icon::Delete, "Delete library item")
                    .clicked()
                {
                    delete_selected = Some(item);
                }
                if let LibraryItem::Q0rg(id) = item {
                    if ui
                        .add_enabled(can_place, egui::Button::new("Edit").small())
                        .clicked()
                    {
                        enter_selected = Some(id);
                    }
                }
            }
        });
    });

    crate::ui::search_field(ui, &mut app.library_search, "Search library...");

    ui.add_space(3.0);
    let columns = if showing_all { 6 } else { 5 };
    egui::Grid::new("library_header")
        .num_columns(columns)
        .min_col_width(28.0)
        .spacing([8.0, 3.0])
        .show(ui, |ui| {
            if ui
                .selectable_label(
                    false,
                    if app.library_sort_by_uses {
                        if app.library_sort_ascending {
                            "Uses ↑"
                        } else {
                            "Uses ↓"
                        }
                    } else if app.library_sort_ascending {
                        "Name A-Z"
                    } else {
                        "Name Z-A"
                    },
                )
                .clicked()
            {
                if app.library_sort_by_uses {
                    app.library_sort_by_uses = false;
                    app.library_sort_ascending = true;
                } else {
                    app.library_sort_ascending = !app.library_sort_ascending;
                }
            }
            ui.label("Type");
            if ui
                .selectable_label(app.library_sort_by_uses, "Uses")
                .on_hover_text("Sort by use count, then by name")
                .clicked()
            {
                if app.library_sort_by_uses {
                    app.library_sort_ascending = !app.library_sort_ascending;
                } else {
                    app.library_sort_by_uses = true;
                    app.library_sort_ascending = false;
                }
            }
            ui.label("ID");
            ui.label("Folder");
            if showing_all {
                ui.label("Library");
            }
            ui.end_row();
        });
    ui.separator();

    if rows.is_empty() {
        ui.label(
            egui::RichText::new(if needle.is_empty() {
                "Import media or create a symbol to get started."
            } else {
                "No matching library items"
            })
            .small()
            .color(app.settings.theme.text_dim.to_color32()),
        );
    }

    let mut selected: Option<(LibrarySelection, usize, bool, bool, bool)> = None;
    let mut enter: Option<(u64, u16)> = None;
    let mut rename = None;
    let mut delete = None;
    let active_folder_paths = active_tab
        .and_then(|tab_id| app.project_for_document_tab(tab_id))
        .map(|project| project.runtime.library_folders.clone())
        .unwrap_or_default();
    // Only instantiate widgets for rows intersecting the viewport. A large FLA can
    // contain tens of thousands of library items; building every egui row every
    // frame is pure off-screen work.
    let row_height = ui.spacing().interact_size.y;
    egui::ScrollArea::vertical()
        .id_source(LIBRARY_ITEMS_SCROLL_ID)
        .auto_shrink([false, true])
        .show_rows(ui, row_height, rows.len(), |ui, visible_rows| {
            egui::Grid::new("library_items")
                .striped(true)
                .num_columns(columns)
                .min_col_width(28.0)
                .spacing([8.0, 2.0])
                .show(ui, |ui| {
                    for row_index in visible_rows.clone() {
                        let row = &rows[row_index];
                        let payload = item_for_key(row.key);
                        let row_selection = LibrarySelection {
                            tab_id: row.tab_id,
                            item: payload,
                        };
                        let is_selected = app.library_multi_selection.contains(&row_selection);
                        let row_is_active = Some(row.tab_id) == active_tab;
                        let drag_payload = LibraryDragPayload {
                            source_tab_id: row.tab_id,
                            item: payload,
                        };
                        let response = library_name_cell(
                            ui,
                            is_selected,
                            row.icon,
                            &truncate_label(&row.name, 28),
                            app.settings.theme.accent.to_color32(),
                            Sense::click_and_drag(),
                        );
                        // Every open project is a real source library. The payload
                        // carries its tab id so identical project-local asset/q0rg ids
                        // can never alias during a cross-project drop.
                        response.dnd_set_drag_payload(drag_payload);
                        if response.dragged() {
                            draw_library_drag_preview(
                                ui.ctx(),
                                row.icon,
                                &row.name,
                                app.settings.theme.accent.to_color32(),
                            );
                        }
                        let audio_only = row.kind == "audio";
                        let drag_hint = if audio_only {
                            if row_is_active {
                                "Drag onto timeline to place"
                            } else {
                                "Drag onto timeline to copy into current project"
                            }
                        } else if row_is_active {
                            "Drag onto stage to place"
                        } else {
                            "Drag onto stage to copy into current project"
                        };
                        let response = response.on_hover_text(format!(
                            "{}\n{}\n{}\nFolder: {}\n{drag_hint}",
                            row.name,
                            row.detail,
                            row.library_label,
                            row.folder.as_deref().unwrap_or("Unfoldered")
                        ));
                        if row_is_active {
                            response.clone().context_menu(|ui| {
                                if can_rename_item(app, payload) && ui.button("Rename").clicked() {
                                    rename = Some(payload);
                                    ui.close_menu();
                                }
                                let can_delete = !matches!(
                                    payload,
                                    LibraryItem::Q0rg(id)
                                        if id == app.state.project.meta.entry_q0rg_id
                                );
                                if can_delete && ui.button("Delete").clicked() {
                                    delete = Some(payload);
                                    ui.close_menu();
                                }
                                ui.menu_button("Move to folder", |ui| {
                                    if ui.button("Unfoldered").clicked() {
                                        app.queue(Action::MoveLibraryItemToFolder(payload, None));
                                        ui.close_menu();
                                    }
                                    for folder in &active_folder_paths {
                                        if ui.button(folder).clicked() {
                                            app.queue(Action::MoveLibraryItemToFolder(
                                                payload,
                                                Some(folder.clone()),
                                            ));
                                            ui.close_menu();
                                        }
                                    }
                                });
                            });
                        }
                        if response.clicked() || response.drag_started() {
                            let (control, shift) =
                                ui.input(|input| (input.modifiers.command, input.modifiers.shift));
                            selected = Some((
                                row_selection,
                                row_index,
                                control,
                                shift,
                                response.drag_started(),
                            ));
                        }
                        if response.double_clicked() {
                            if let LibraryKey::Q0rg(id) = row.key {
                                enter = Some((row.tab_id, id));
                            }
                        }
                        ui.label(row.kind);
                        ui.label(row.uses.to_string());
                        ui.label(match row.key {
                            LibraryKey::Q0rg(id) | LibraryKey::Asset(id) => id.to_string(),
                        });
                        ui.label(row.folder.as_deref().unwrap_or("—"));
                        if showing_all {
                            ui.label(&row.library_label);
                        }
                        ui.end_row();
                    }
                });
        });

    if let Some((selection, row_index, control, shift, dragging)) = selected {
        if dragging {
            app.library_multi_selection = vec![selection];
            app.library_selection = Some(selection);
            app.library_selection_anchor = Some(selection);
        } else if shift {
            let anchor = app
                .library_selection_anchor
                .filter(|anchor| anchor.tab_id == selection.tab_id);
            let anchor_index = anchor.and_then(|anchor| {
                rows.iter().position(|row| {
                    row.tab_id == anchor.tab_id && row.key == key_for_item(anchor.item)
                })
            });
            if let Some(anchor_index) = anchor_index {
                let start = anchor_index.min(row_index);
                let end = anchor_index
                    .max(row_index)
                    .min(rows.len().saturating_sub(1));
                app.library_multi_selection = rows[start..=end]
                    .iter()
                    .filter(|row| row.tab_id == selection.tab_id)
                    .map(|row| LibrarySelection {
                        tab_id: row.tab_id,
                        item: item_for_key(row.key),
                    })
                    .collect();
                app.library_selection = Some(selection);
            } else {
                app.library_multi_selection = vec![selection];
                app.library_selection = Some(selection);
                app.library_selection_anchor = Some(selection);
            }
        } else if control {
            if app
                .library_multi_selection
                .iter()
                .any(|selected| selected.tab_id != selection.tab_id)
            {
                app.library_multi_selection.clear();
                app.library_selection_anchor = Some(selection);
            }
            if let Some(index) = app
                .library_multi_selection
                .iter()
                .position(|item| *item == selection)
            {
                app.library_multi_selection.remove(index);
                app.library_selection = app.library_multi_selection.last().copied();
            } else {
                app.library_multi_selection.push(selection);
                app.library_selection = Some(selection);
            }
            if app
                .library_selection_anchor
                .map_or(true, |anchor| anchor.tab_id != selection.tab_id)
            {
                app.library_selection_anchor = Some(selection);
            }
        } else {
            app.library_multi_selection = vec![selection];
            app.library_selection = Some(selection);
            app.library_selection_anchor = Some(selection);
        }

        if app.library_multi_selection.len() == 1 {
            if let Some(selection) = app.library_multi_selection.first().copied() {
                if Some(selection.tab_id) == active_tab {
                    app.session.selection = match selection.item {
                        LibraryItem::Q0rg(id) => Selection::Q0rg(id),
                        LibraryItem::Asset(id) => Selection::Asset(id),
                    };
                }
            }
        } else {
            app.session.selection = Selection::None;
        }
    }
    if let Some(item) = place_selected {
        let center = q0s_format::v2::Vec2::new(
            app.state.project.meta.stage_width as f32 * 0.5,
            app.state.project.meta.stage_height as f32 * 0.5,
        );
        app.queue(Action::PlaceLibraryItemAt(item, center));
    }
    if let Some((tab_id, id)) = enter {
        if Some(tab_id) != active_tab {
            app.queue(Action::SwitchDocumentTab(tab_id));
        }
        app.queue(Action::EnterQ0rg(id));
    } else if let Some(id) = enter_selected {
        app.queue(Action::EnterQ0rg(id));
    }
    if ui.input(|input| input.key_pressed(egui::Key::F2)) {
        if app.library_multi_selection.len() <= 1 {
            rename_selected = selected_library_item(app).filter(|item| can_rename_item(app, *item));
        }
    }
    if let Some(item) = rename.or(rename_selected) {
        begin_rename(app, item);
    }
    if let Some(item) = delete.or(delete_selected) {
        app.queue(Action::DeleteLibraryItem(item));
    }
    if delete_multi_selected {
        let items = app
            .library_multi_selection
            .iter()
            .filter(|selection| Some(selection.tab_id) == active_tab)
            .map(|selection| selection.item)
            .collect();
        app.queue(Action::DeleteLibraryItems(items));
    }
    render_rename_dialog(app, ui.ctx());
}

fn selected_library_item(app: &EditorApp) -> Option<LibraryItem> {
    let selection = app.library_selection?;
    (Some(selection.tab_id) == app.active_project_tab_id()).then_some(selection.item)
}

fn key_for_item(item: LibraryItem) -> LibraryKey {
    match item {
        LibraryItem::Q0rg(id) => LibraryKey::Q0rg(id),
        LibraryItem::Asset(id) => LibraryKey::Asset(id),
    }
}

fn item_for_key(key: LibraryKey) -> LibraryItem {
    match key {
        LibraryKey::Q0rg(id) => LibraryItem::Q0rg(id),
        LibraryKey::Asset(id) => LibraryItem::Asset(id),
    }
}

pub(crate) fn library_drag_item_is_audio_only(
    app: &EditorApp,
    payload: LibraryDragPayload,
) -> bool {
    app.library_cache
        .audio_only_items
        .contains(&(payload.source_tab_id, key_for_item(payload.item)))
}

fn can_rename_item(app: &EditorApp, item: LibraryItem) -> bool {
    match item {
        LibraryItem::Q0rg(id) => app
            .state
            .project
            .q0rgs
            .iter()
            .any(|q0rg| q0rg.q0rg_id == id),
        LibraryItem::Asset(id) => {
            app.state
                .project
                .assets
                .iter()
                .any(|asset| asset.id() == id && matches!(asset, Asset::Vector(_) | Asset::Q0v(_)))
                && !app
                    .state
                    .project
                    .asset_names
                    .get(&id)
                    .is_some_and(|name| crate::text_tool::decode_source(name).is_some())
        }
    }
}

fn library_item_name(app: &EditorApp, item: LibraryItem) -> Option<String> {
    match item {
        LibraryItem::Q0rg(id) => app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == id)
            .map(|q0rg| q0rg.name.clone()),
        LibraryItem::Asset(id) => app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == id && matches!(asset, Asset::Vector(_) | Asset::Q0v(_)))
            .map(|asset| {
                app.state
                    .project
                    .asset_names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| match asset {
                        Asset::Q0v(media)
                            if crate::audio::q0v_is_audio_only_bytes(&media.bytes) =>
                        {
                            format!("Audio {id}")
                        }
                        Asset::Q0v(_) => format!("Video {id}"),
                        _ => format!("Vector {id}"),
                    })
            }),
    }
}

fn begin_rename(app: &mut EditorApp, item: LibraryItem) {
    let Some(draft) = library_item_name(app, item) else {
        return;
    };
    app.session.library_rename = Some(LibraryRename {
        item,
        draft,
        focus_requested: true,
    });
}

fn render_rename_dialog(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(mut rename) = app.session.library_rename.take() else {
        return;
    };
    let title = match rename.item {
        LibraryItem::Q0rg(_) => "Rename symbol",
        LibraryItem::Asset(_) => "Rename asset",
    };
    let mut open = true;
    let mut submit = false;
    let mut cancel = false;
    egui::Window::new(title)
        .id(egui::Id::new("library_rename_dialog"))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut rename.draft)
                    .desired_width(280.0)
                    .hint_text("Name"),
            );
            if rename.focus_requested {
                response.request_focus();
                rename.focus_requested = false;
            }
            if (response.has_focus() || response.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                && !rename.draft.trim().is_empty()
            {
                submit = true;
            }
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            ui.horizontal(|ui| {
                let valid = !rename.draft.trim().is_empty();
                if ui.add_enabled(valid, egui::Button::new("Rename")).clicked() {
                    submit = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

    if submit && open && !cancel && !rename.draft.trim().is_empty() {
        app.queue(Action::RenameLibraryItem(rename.item, rename.draft));
    } else if open && !cancel {
        app.session.library_rename = Some(rename);
    }
}

const LIBRARY_ITEMS_SCROLL_ID: &str = "library_items_scroll";

#[cfg(test)]
fn show_library_items_scroll<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    egui::ScrollArea::vertical()
        .id_source(LIBRARY_ITEMS_SCROLL_ID)
        .auto_shrink([false, true])
        .show(ui, add_contents)
        .inner
}
const LIBRARY_ICON_SIZE: f32 = 14.0;
const LIBRARY_ICON_GAP: f32 = 5.0;

fn draw_library_drag_preview(ctx: &egui::Context, icon: LibraryIcon, name: &str, accent: Color32) {
    let Some(pointer) = ctx.pointer_interact_pos() else {
        return;
    };
    let style = ctx.style();
    let visuals = &style.visuals;
    let text_color = visuals.widgets.active.fg_stroke.color;
    let font_id = TextStyle::Button.resolve(&style);
    let label = truncate_label(name, 28);
    let galley = ctx.fonts(|fonts| fonts.layout_no_wrap(label, font_id, text_color));
    let padding = vec2(8.0, 5.0);
    let content_height = galley.size().y.max(LIBRARY_ICON_SIZE);
    let size = vec2(
        padding.x * 2.0 + LIBRARY_ICON_SIZE + LIBRARY_ICON_GAP + galley.size().x,
        padding.y * 2.0 + content_height,
    );
    let rect = Rect::from_min_size(pointer + vec2(12.0, 12.0), size);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("library_drag_preview"),
    ));
    painter.add(egui::Shape::mesh(
        visuals
            .window_shadow
            .tessellate(rect, visuals.window_rounding),
    ));
    painter.rect(
        rect,
        visuals.window_rounding,
        visuals.window_fill,
        Stroke::new(1.0_f32, accent),
    );
    let content = rect.shrink2(padding);
    let icon_rect = Rect::from_center_size(
        pos2(content.left() + LIBRARY_ICON_SIZE * 0.5, content.center().y),
        vec2(LIBRARY_ICON_SIZE, LIBRARY_ICON_SIZE),
    );
    draw_library_icon(&painter, icon_rect, icon, text_color, accent);
    painter.galley(
        pos2(
            icon_rect.right() + LIBRARY_ICON_GAP,
            content.center().y - galley.size().y * 0.5,
        ),
        galley,
        text_color,
    );
}

fn library_name_cell(
    ui: &mut Ui,
    selected: bool,
    icon: LibraryIcon,
    name: &str,
    accent: Color32,
    sense: Sense,
) -> Response {
    let button_padding = ui.spacing().button_padding;
    let galley = egui::WidgetText::from(name.to_owned()).into_galley(
        ui,
        None,
        f32::INFINITY,
        TextStyle::Button,
    );
    let content_height = galley.size().y.max(LIBRARY_ICON_SIZE);
    let desired_size = vec2(
        button_padding.x * 2.0 + LIBRARY_ICON_SIZE + LIBRARY_ICON_GAP + galley.size().x,
        (button_padding.y * 2.0 + content_height).max(ui.spacing().interact_size.y),
    );
    let (rect, response) = ui.allocate_at_least(desired_size, sense);
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::SelectableLabel, selected, name.to_owned())
    });

    if ui.is_rect_visible(response.rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() || response.highlighted() || response.has_focus() {
            ui.painter().rect(
                rect.expand(visuals.expansion),
                visuals.rounding,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
            );
        }

        let content = rect.shrink2(button_padding);
        let icon_rect = Rect::from_center_size(
            pos2(content.left() + LIBRARY_ICON_SIZE * 0.5, content.center().y),
            vec2(LIBRARY_ICON_SIZE, LIBRARY_ICON_SIZE),
        );
        draw_library_icon(ui.painter(), icon_rect, icon, visuals.text_color(), accent);
        let text_pos = pos2(
            icon_rect.right() + LIBRARY_ICON_GAP,
            content.center().y - galley.size().y * 0.5,
        );
        ui.painter().galley(text_pos, galley, visuals.text_color());
    }

    response
}

fn draw_library_icon(
    painter: &egui::Painter,
    rect: Rect,
    icon: LibraryIcon,
    foreground: Color32,
    accent: Color32,
) {
    let rect = rect.shrink(1.0);
    let strong = Stroke::new(1.35_f32, accent);
    let quiet = Stroke::new(1.1_f32, foreground);

    match icon {
        LibraryIcon::Stage => {
            let screen = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - 3.5));
            painter.rect_stroke(screen, 1.5, strong);
            let stem_top = pos2(screen.center().x, screen.bottom());
            let stem_bottom = pos2(screen.center().x, rect.bottom() - 0.5);
            painter.line_segment([stem_top, stem_bottom], quiet);
            painter.line_segment(
                [
                    pos2(rect.center().x - 3.0, rect.bottom() - 0.5),
                    pos2(rect.center().x + 3.0, rect.bottom() - 0.5),
                ],
                quiet,
            );
        }
        LibraryIcon::Symbol => {
            let rear = Rect::from_min_max(
                pos2(rect.left() + 3.0, rect.top()),
                pos2(rect.right(), rect.bottom() - 3.0),
            );
            let front = Rect::from_min_max(
                pos2(rect.left(), rect.top() + 3.0),
                pos2(rect.right() - 3.0, rect.bottom()),
            );
            painter.rect_stroke(rear, 1.5, quiet);
            painter.rect_stroke(front, 1.5, strong);
        }
        LibraryIcon::Vector => {
            let a = pos2(rect.left() + 0.5, rect.bottom() - 1.0);
            let b = pos2(rect.center().x, rect.top() + 1.0);
            let c = pos2(rect.right() - 0.5, rect.bottom() - 2.0);
            painter.line_segment([a, b], strong);
            painter.line_segment([b, c], strong);
            for point in [a, b, c] {
                painter.circle_filled(point, 1.8, foreground);
                painter.circle_stroke(point, 1.8, Stroke::new(0.8_f32, accent));
            }
        }
        LibraryIcon::Video => {
            painter.rect_stroke(rect, 1.5, quiet);
            let center = rect.center();
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(center.x - 2.0, center.y - 3.5),
                    pos2(center.x - 2.0, center.y + 3.5),
                    pos2(center.x + 4.0, center.y),
                ],
                accent,
                Stroke::NONE,
            ));
        }
        LibraryIcon::Audio => {
            let mid = rect.center().y;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(rect.left() + 1.0, mid - 2.0),
                    pos2(rect.left() + 4.0, mid - 2.0),
                    pos2(rect.left() + 7.0, mid - 5.0),
                    pos2(rect.left() + 7.0, mid + 5.0),
                    pos2(rect.left() + 4.0, mid + 2.0),
                    pos2(rect.left() + 1.0, mid + 2.0),
                ],
                accent,
                Stroke::NONE,
            ));
            painter.line_segment(
                [
                    pos2(rect.left() + 9.0, mid - 3.5),
                    pos2(rect.left() + 11.0, mid),
                ],
                quiet,
            );
            painter.line_segment(
                [
                    pos2(rect.left() + 11.0, mid),
                    pos2(rect.left() + 9.0, mid + 3.5),
                ],
                quiet,
            );
            painter.line_segment(
                [pos2(rect.left() + 11.0, mid - 5.0), pos2(rect.right(), mid)],
                strong,
            );
            painter.line_segment(
                [pos2(rect.right(), mid), pos2(rect.left() + 11.0, mid + 5.0)],
                strong,
            );
        }
        LibraryIcon::Bitmap => {
            painter.rect_stroke(rect, 1.5, quiet);
            painter.circle_filled(pos2(rect.right() - 3.0, rect.top() + 3.0), 1.4, accent);
            let left = pos2(rect.left() + 1.5, rect.bottom() - 2.0);
            let peak = pos2(rect.left() + 5.0, rect.top() + 5.5);
            let valley = pos2(rect.left() + 7.5, rect.bottom() - 4.0);
            let right = pos2(rect.right() - 1.5, rect.bottom() - 2.0);
            painter.line_segment([left, peak], strong);
            painter.line_segment([peak, valley], strong);
            painter.line_segment([valley, right], strong);
        }
    }
}

fn collect_rows(
    app: &EditorApp,
    source: LibrarySource,
    libraries: &[ProjectLibrarySummary],
) -> Vec<LibraryRow> {
    let mut rows = Vec::new();
    for library in libraries {
        if matches!(source, LibrarySource::Project(tab_id) if tab_id != library.tab_id) {
            continue;
        }
        if let Some(project) = app.project_for_document_tab(library.tab_id) {
            rows.extend(collect_rows_for_project(
                project,
                library.tab_id,
                &library.label,
            ));
        }
    }
    rows
}

fn library_use_counts(project: &ProjectV2) -> (HashMap<u16, usize>, HashMap<u16, usize>) {
    let mut q0rg_uses = HashMap::with_capacity(project.q0rgs.len());
    let mut asset_uses = HashMap::with_capacity(project.assets.len());
    for placement in project
        .q0rgs
        .iter()
        .flat_map(|owner| owner.layers.iter())
        .flat_map(|layer| layer.placements.iter())
    {
        match placement.target {
            Target::Q0rg(id) => *q0rg_uses.entry(id).or_insert(0) += 1,
            Target::Asset(id) => *asset_uses.entry(id).or_insert(0) += 1,
        }
    }
    (q0rg_uses, asset_uses)
}

fn collect_rows_for_project(
    project: &ProjectV2,
    tab_id: u64,
    library_label: &str,
) -> Vec<LibraryRow> {
    // Usage counts used to scan every placement once per library row. On imported
    // FLA projects with thousands of symbols/assets that made Library O(items *
    // placements) every UI frame. Count all targets once instead.
    let (q0rg_uses, asset_uses) = library_use_counts(project);
    let mut rows = Vec::with_capacity(project.q0rgs.len() + project.assets.len());
    for q0rg in &project.q0rgs {
        if q0rg.q0rg_id == project.meta.entry_q0rg_id {
            continue;
        }
        let uses = q0rg_uses.get(&q0rg.q0rg_id).copied().unwrap_or(0);
        rows.push(LibraryRow {
            tab_id,
            library_label: library_label.to_string(),
            key: LibraryKey::Q0rg(q0rg.q0rg_id),
            icon: LibraryIcon::for_q0rg(q0rg.q0rg_id, project.meta.entry_q0rg_id),
            name: q0rg.name.clone(),
            kind: "q0rg",
            uses,
            detail: format!("{} frames / {} layers", q0rg.frame_count, q0rg.layers.len()),
            folder: project
                .runtime
                .library_item_folders
                .get(&q0s_format::v2::LibraryItemReference::Q0rg(q0rg.q0rg_id))
                .cloned(),
        });
    }
    for asset in &project.assets {
        if matches!(asset, Asset::Rig(_)) {
            continue;
        }
        let id = asset.id();
        let uses = asset_uses.get(&id).copied().unwrap_or(0);
        let (name, kind, detail) = match asset {
            Asset::Bitmap(bitmap) => (
                format!("Bitmap {id}"),
                "bitmap",
                format!("{} x {} px", bitmap.width, bitmap.height),
            ),
            Asset::Vector(vector) => {
                if let Some((name, size)) = project.asset_names.get(&id).and_then(|name| {
                    crate::text_tool::decode_source(name).map(|(_, size)| {
                        (
                            crate::text_tool::display_name(name).unwrap_or_default(),
                            size,
                        )
                    })
                }) {
                    (
                        name,
                        "text",
                        format!("{} px / {} contours", size as u16, vector.paths.len()),
                    )
                } else {
                    (
                        project
                            .asset_names
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| format!("Vector {id}")),
                        "vector",
                        format!("{} contours", vector.paths.len()),
                    )
                }
            }
            Asset::Q0v(video) => {
                let header = q0video::q0v::probe_header(&video.bytes).ok();
                if let Some(header) = header
                    .as_ref()
                    .filter(|header| header.spec.audio && !header.spec.video)
                {
                    let seconds = header.audio_samples_per_channel as f64
                        / f64::from(header.spec.audio_sample_rate.max(1));
                    (
                        project
                            .asset_names
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| format!("Audio {id}")),
                        "audio",
                        format!(
                            "{seconds:.2}s / {} hz / {} ch",
                            header.spec.audio_sample_rate, header.spec.audio_channels
                        ),
                    )
                } else {
                    (
                        project
                            .asset_names
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| format!("Video {id}")),
                        "q0v",
                        header
                            .map(|header| {
                                format!(
                                    "{} x {} px / {} frames / {} fps",
                                    header.spec.width,
                                    header.spec.height,
                                    header.spec.timeline_frames,
                                    header.spec.fps
                                )
                            })
                            .unwrap_or_else(|| "invalid q0v".to_string()),
                    )
                }
            }
            Asset::Rig(rig) => (
                format!("Rig {}", rig.owner_q0rg_id),
                "rig",
                format!(
                    "{} bones / {} controls",
                    rig.nodes.len(),
                    rig.controls.len()
                ),
            ),
        };
        rows.push(LibraryRow {
            tab_id,
            library_label: library_label.to_string(),
            key: LibraryKey::Asset(id),
            icon: LibraryIcon::for_asset(asset),
            name,
            kind,
            uses,
            detail,
            folder: project
                .runtime
                .library_item_folders
                .get(&q0s_format::v2::LibraryItemReference::Asset(id))
                .cloned(),
        });
    }
    rows
}

fn draw_preview(app: &mut EditorApp, ui: &mut Ui, selection: Option<LibrarySelection>) {
    let height = (ui.clip_rect().bottom() - ui.cursor().top() - 145.0).clamp(64.0, 142.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let border = app.settings.theme.stroke_dark.to_color32();
    let text_dim = app.settings.theme.text_dim.to_color32();
    let deep_bg = app.settings.theme.deep_bg.to_color32();
    let window_fill = ui.visuals().window_fill();
    let preview_light = app.settings.theme.library_preview_light.to_color32();
    let preview_dark = app.settings.theme.library_preview_dark.to_color32();
    let painter = ui.painter_at(rect).with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, deep_bg);
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, border));

    let Some(selection) = selection else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Select a library item",
            FontId::proportional(12.0),
            text_dim,
        );
        return;
    };
    let key = key_for_item(selection.item);
    let frame = match key {
        LibraryKey::Q0rg(id) => app.project_library_preview_frame(selection.tab_id, id),
        LibraryKey::Asset(_) => 0,
    };
    let bounds_key = LibraryPreviewBoundsKey {
        tab_id: selection.tab_id,
        item: key,
        frame,
        project_revision: app.project_revision_for_tab(selection.tab_id).unwrap_or(0),
    };
    let cached_bounds = app
        .library_cache
        .preview_bounds
        .filter(|(cached_key, _)| *cached_key == bounds_key)
        .map(|(_, bounds)| bounds);
    let background_key = LibraryPreviewBackgroundKey {
        tab_id: selection.tab_id,
        item: key,
        frame,
        library_revision: app
            .project_library_revision_for_tab(selection.tab_id)
            .unwrap_or(0),
    };
    let cached_background = app
        .library_cache
        .preview_backgrounds
        .get(&background_key)
        .cloned();
    let ctx = ui.ctx().clone();
    let preview = app.with_project_library_view_mut(
        selection.tab_id,
        |project, _session, textures, render_revision| {
            let (background, label) = cached_background.unwrap_or_else(|| {
                (
                    preview_background_for_key(project, key, frame),
                    preview_label_for_key(project, key),
                )
            });
            painter.text(
                rect.left_top() + vec2(7.0, 6.0),
                egui::Align2::LEFT_TOP,
                label.clone(),
                FontId::proportional(11.0),
                text_dim,
            );

            let content_rect =
                Rect::from_min_max(rect.min + vec2(8.0, 23.0), rect.max - vec2(8.0, 8.0));
            draw_preview_background(
                &painter,
                content_rect,
                background,
                preview_light,
                preview_dark,
            );
            painter.rect_stroke(
                content_rect,
                0.0,
                Stroke::new(1.0_f32, border.gamma_multiply(0.8)),
            );
            let bbox = cached_bounds.unwrap_or_else(|| match key {
                LibraryKey::Q0rg(id) => q0rg_frame_bbox(project, id, frame),
                LibraryKey::Asset(id) => placement_bbox(
                    project,
                    &Placement {
                        instance_id: 0,
                        frame: 0,
                        target: Target::Asset(id),
                        transform: Transform2D::IDENTITY,
                        tween: Tween::None,
                        fx: Default::default(),
                    },
                ),
            });

            let Some(bounds) = bbox else {
                painter.rect_filled(content_rect, 4.0, window_fill);
                painter.text(
                    content_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No artwork yet",
                    FontId::proportional(12.0),
                    text_dim,
                );
                return (bounds_key, bbox, background_key, background, label);
            };
            let view = preview_view(content_rect, bounds);
            match key {
                LibraryKey::Q0rg(id) => crate::render::render_stage_with_revision(
                    &painter,
                    project,
                    id,
                    frame,
                    &view,
                    textures,
                    &ctx,
                    (selection.tab_id, render_revision),
                ),
                LibraryKey::Asset(id) => crate::render::render_asset_preview_with_revision(
                    &painter,
                    project,
                    id,
                    &view,
                    textures,
                    &ctx,
                    Some((selection.tab_id, render_revision)),
                ),
            }
            (bounds_key, bbox, background_key, background, label)
        },
    );
    if let Some((bounds_key, bounds, background_key, background, label)) = preview {
        app.library_cache.preview_bounds = Some((bounds_key, bounds));
        if app.library_cache.preview_backgrounds.len() >= 256
            && !app
                .library_cache
                .preview_backgrounds
                .contains_key(&background_key)
        {
            app.library_cache.preview_backgrounds.clear();
        }
        app.library_cache
            .preview_backgrounds
            .insert(background_key, (background, label));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewBackground {
    Light,
    Dark,
    Checker,
}

#[derive(Debug, Default, Clone, Copy)]
struct PreviewColorStats {
    visible_weight: f32,
    luminance_sum: f32,
    dark_weight: f32,
    light_weight: f32,
    has_transparency: bool,
}

impl PreviewColorStats {
    fn add(&mut self, color: Rgba, weight: f32) {
        if color.a < 250 {
            self.has_transparency = true;
        }
        let alpha = f32::from(color.a) / 255.0;
        let effective_weight = weight.max(0.0) * alpha;
        if effective_weight <= f32::EPSILON {
            return;
        }
        let luminance = (0.2126 * f32::from(color.r)
            + 0.7152 * f32::from(color.g)
            + 0.0722 * f32::from(color.b))
            / 255.0;
        self.visible_weight += effective_weight;
        self.luminance_sum += luminance * effective_weight;
        if luminance <= 0.35 {
            self.dark_weight += effective_weight;
        }
        if luminance >= 0.65 {
            self.light_weight += effective_weight;
        }
    }

    fn background(self) -> PreviewBackground {
        if self.visible_weight <= f32::EPSILON {
            return PreviewBackground::Checker;
        }
        let dark_ratio = self.dark_weight / self.visible_weight;
        let light_ratio = self.light_weight / self.visible_weight;
        if self.has_transparency || (dark_ratio >= 0.15 && light_ratio >= 0.15) {
            return PreviewBackground::Checker;
        }
        let average = self.luminance_sum / self.visible_weight;
        if average >= 0.58 {
            PreviewBackground::Dark
        } else if average <= 0.42 {
            PreviewBackground::Light
        } else {
            PreviewBackground::Checker
        }
    }
}

fn preview_background_for_key(
    project: &ProjectV2,
    key: LibraryKey,
    frame: u16,
) -> PreviewBackground {
    let mut stats = PreviewColorStats::default();
    match key {
        LibraryKey::Asset(id) => collect_asset_color_stats(project, id, &mut stats),
        LibraryKey::Q0rg(id) => collect_q0rg_color_stats(project, id, frame, 0, &mut stats),
    }
    stats.background()
}

fn preview_label_for_key(project: &ProjectV2, key: LibraryKey) -> String {
    match key {
        LibraryKey::Q0rg(id) => project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == id)
            .map(|q0rg| q0rg.name.clone())
            .unwrap_or_else(|| "Missing q0rg".to_string()),
        LibraryKey::Asset(id) => project
            .assets
            .iter()
            .find(|asset| asset.id() == id)
            .map(|asset| match asset {
                Asset::Vector(_) => project
                    .asset_names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| "Vector asset".to_string()),
                Asset::Bitmap(_) => "Bitmap asset".to_string(),
                Asset::Q0v(_) => project
                    .asset_names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| "Video asset".to_string()),
                Asset::Rig(_) => "Rig metadata".to_string(),
            })
            .unwrap_or_else(|| "Missing asset".to_string()),
    }
}

fn collect_q0rg_color_stats(
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    depth: u8,
    stats: &mut PreviewColorStats,
) {
    if depth > 8 {
        return;
    }
    let Some(q0rg) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return;
    };
    let local_frame = if q0rg.frame_count > 0 {
        frame % q0rg.frame_count
    } else {
        0
    };
    for layer in &q0rg.layers {
        for (placement_idx, _) in active_placements_at(layer, local_frame) {
            let Some(placement) = layer.placements.get(placement_idx) else {
                continue;
            };
            match placement.target {
                Target::Asset(id) => collect_asset_color_stats(project, id, stats),
                Target::Q0rg(child_id) if child_id != q0rg_id => {
                    collect_q0rg_color_stats(project, child_id, local_frame, depth + 1, stats);
                }
                Target::Q0rg(_) => {}
            }
        }
    }
}

fn collect_asset_color_stats(project: &ProjectV2, asset_id: u16, stats: &mut PreviewColorStats) {
    let Some(asset) = project.assets.iter().find(|asset| asset.id() == asset_id) else {
        return;
    };
    match asset {
        Asset::Vector(vector) => {
            if let Some(fill) = vector.fill {
                stats.add(fill, 2.0);
            }
            if let Some(stroke) = vector.stroke {
                stats.add(stroke.color, 1.0);
            }
        }
        Asset::Bitmap(bitmap) => {
            let pixel_count = bitmap.rgba.len() / 4;
            let stride = (pixel_count / 1024).max(1);
            for pixel in bitmap.rgba.chunks_exact(4).step_by(stride) {
                stats.add(
                    Rgba {
                        r: pixel[0],
                        g: pixel[1],
                        b: pixel[2],
                        a: pixel[3],
                    },
                    1.0,
                );
            }
        }
        Asset::Q0v(video) => {
            let Ok(header) = q0video::q0v::probe_header(&video.bytes) else {
                return;
            };
            if !header.spec.video {
                return;
            }
            let Ok(media) = q0video::q0v::Q0vFile::parse(video.bytes.clone()) else {
                return;
            };
            let Ok(rgba) = media.decode_frame_rgba(0) else {
                return;
            };
            let pixel_count = rgba.len() / 4;
            let stride = (pixel_count / 1024).max(1);
            for pixel in rgba.chunks_exact(4).step_by(stride) {
                stats.add(
                    Rgba {
                        r: pixel[0],
                        g: pixel[1],
                        b: pixel[2],
                        a: pixel[3],
                    },
                    1.0,
                );
            }
        }
        Asset::Rig(_) => {}
    }
}

fn draw_preview_background(
    painter: &egui::Painter,
    rect: Rect,
    background: PreviewBackground,
    light: egui::Color32,
    dark: egui::Color32,
) {
    match background {
        PreviewBackground::Light => {
            painter.rect_filled(rect, 0.0, light);
        }
        PreviewBackground::Dark => {
            painter.rect_filled(rect, 0.0, dark);
        }
        PreviewBackground::Checker => {
            painter.rect_filled(rect, 0.0, light);
            let tile = 12.0_f32;
            let columns = (rect.width() / tile).ceil() as usize;
            let rows = (rect.height() / tile).ceil() as usize;
            for row in 0..rows {
                for column in 0..columns {
                    if (row + column) % 2 == 0 {
                        continue;
                    }
                    let min = pos2(
                        rect.min.x + column as f32 * tile,
                        rect.min.y + row as f32 * tile,
                    );
                    let max = pos2(
                        (min.x + tile).min(rect.max.x),
                        (min.y + tile).min(rect.max.y),
                    );
                    painter.rect_filled(Rect::from_min_max(min, max), 0.0, dark);
                }
            }
        }
    }
}

pub(crate) fn draw_q0rg_frame_preview_background(
    painter: &egui::Painter,
    project: &ProjectV2,
    q0rg_id: u16,
    frame: u16,
    rect: Rect,
    light: Color32,
    dark: Color32,
) {
    let background = preview_background_for_key(project, LibraryKey::Q0rg(q0rg_id), frame);
    draw_preview_background(painter, rect, background, light, dark);
}

pub(crate) fn preview_view(rect: Rect, bounds: (f32, f32, f32, f32)) -> StageView {
    let width = (bounds.2 - bounds.0).abs().max(1.0);
    let height = (bounds.3 - bounds.1).abs().max(1.0);
    let scale = (rect.width() / width).min(rect.height() / height) * 0.82;
    let center_x = (bounds.0 + bounds.2) * 0.5;
    let center_y = (bounds.1 + bounds.3) * 0.5;
    StageView {
        origin: pos2(
            rect.center().x - center_x * scale,
            rect.center().y - center_y * scale,
        ),
        scale,
        stage_rect: rect,
        edit_transform: q0s_format::transform::Affine::IDENTITY,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_project_fingerprint_changes_with_row_visible_metadata() {
        fn fingerprint(project: &ProjectV2) -> u64 {
            let mut hasher = DefaultHasher::new();
            hash_library_project(project, &mut hasher);
            hasher.finish()
        }

        let mut project = crate::state::default_project();
        let initial = fingerprint(&project);
        assert_eq!(initial, fingerprint(&project));
        project.q0rgs[0].name.push_str(" changed");
        assert_ne!(initial, fingerprint(&project));
    }

    #[test]
    fn library_use_counts_scan_targets_once_and_count_both_kinds() {
        let mut project = crate::state::default_project();
        project
            .assets
            .push(Asset::Vector(q0s_format::v2::VectorAsset {
                asset_id: 77,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            }));
        project.q0rgs.push(q0s_format::v2::Q0rg {
            q0rg_id: 2,
            name: "child".into(),
            frame_count: 1,
            script: String::new(),
            layers: vec![q0s_format::v2::Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "child layer".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            }],
        });
        let layer = &mut project.q0rgs[0].layers[0];
        layer.placements.extend([
            Placement {
                instance_id: 1,
                frame: 0,
                target: Target::Q0rg(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 2,
                frame: 0,
                target: Target::Q0rg(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
            Placement {
                instance_id: 3,
                frame: 0,
                target: Target::Asset(77),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
                fx: Default::default(),
            },
        ]);
        let (q0rg_uses, asset_uses) = library_use_counts(&project);
        assert_eq!(q0rg_uses.get(&2), Some(&2));
        assert_eq!(asset_uses.get(&77), Some(&1));
    }

    #[test]
    fn entry_stage_is_not_a_draggable_library_item() {
        let app = EditorApp::default();
        let rows = collect_rows_for_project(&app.state.project, 1, "test.q1s");
        assert!(rows.iter().all(|row| !matches!(
            row.key,
            LibraryKey::Q0rg(id) if id == app.state.project.meta.entry_q0rg_id
        )));
    }

    #[test]
    fn library_item_scroll_offset_stays_stable_across_small_panel_resizes() {
        fn render_at_height(ctx: &egui::Context, height: f32) -> egui::Id {
            let mut scroll_id = None;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, height))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.add_space(210.0);
                        scroll_id =
                            Some(ui.make_persistent_id(egui::Id::new(LIBRARY_ITEMS_SCROLL_ID)));
                        show_library_items_scroll(ui, |ui| {
                            for row in 0..80 {
                                ui.label(format!("library row {row}"));
                            }
                        });
                    });
                },
            );
            scroll_id.expect("library item scroll id")
        }

        let ctx = egui::Context::default();
        let id = render_at_height(&ctx, 400.0);
        let mut state = egui::scroll_area::State::load(&ctx, id).expect("scroll state");
        state.offset.y = 80.0;
        state.store(&ctx, id);

        for height in [418.0, 404.0, 416.0, 402.0] {
            assert_eq!(render_at_height(&ctx, height), id);
            let state =
                egui::scroll_area::State::load(&ctx, id).expect("scroll state after resize");
            assert!(
                (state.offset.y - 80.0).abs() < 0.01,
                "library list jumped to {} at panel height {height}",
                state.offset.y
            );
        }
    }

    #[test]
    fn library_icons_are_typed_geometry_not_font_glyphs() {
        assert_eq!(LibraryIcon::for_q0rg(1, 1), LibraryIcon::Stage);
        assert_eq!(LibraryIcon::for_q0rg(2, 1), LibraryIcon::Symbol);

        let vector = Asset::Vector(q0s_format::v2::VectorAsset {
            asset_id: 1,
            paths: Vec::new(),
            fill: None,
            stroke: None,
        });
        let bitmap = Asset::Bitmap(q0s_format::v2::BitmapAsset {
            asset_id: 2,
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 0],
        });
        assert_eq!(LibraryIcon::for_asset(&vector), LibraryIcon::Vector);
        assert_eq!(LibraryIcon::for_asset(&bitmap), LibraryIcon::Bitmap);
    }

    #[test]
    fn vector_custom_name_is_used_by_library_and_bitmap_stays_non_renamable() {
        let mut app = EditorApp::default();
        app.state
            .project
            .assets
            .push(Asset::Vector(q0s_format::v2::VectorAsset {
                asset_id: 7,
                paths: Vec::new(),
                fill: None,
                stroke: None,
            }));
        app.state
            .project
            .assets
            .push(Asset::Bitmap(q0s_format::v2::BitmapAsset {
                asset_id: 8,
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 0],
            }));
        app.state
            .project
            .asset_names
            .insert(7, "hero vector".to_string());

        let rows = collect_rows_for_project(&app.state.project, 1, "test.q1s");
        assert_eq!(
            rows.iter()
                .find(|row| row.key == LibraryKey::Asset(7))
                .map(|row| row.name.as_str()),
            Some("hero vector")
        );
        assert!(can_rename_item(&app, LibraryItem::Asset(7)));
        assert!(!can_rename_item(&app, LibraryItem::Asset(8)));
    }

    #[test]
    fn adaptive_preview_uses_light_background_for_dark_art() {
        let mut stats = PreviewColorStats::default();
        stats.add(
            Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            1.0,
        );
        assert_eq!(stats.background(), PreviewBackground::Light);
    }

    #[test]
    fn adaptive_preview_uses_dark_background_for_light_art() {
        let mut stats = PreviewColorStats::default();
        stats.add(
            Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            1.0,
        );
        assert_eq!(stats.background(), PreviewBackground::Dark);
    }

    #[test]
    fn adaptive_preview_uses_checker_for_mixed_or_transparent_art() {
        let mut mixed = PreviewColorStats::default();
        mixed.add(
            Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            1.0,
        );
        mixed.add(
            Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            1.0,
        );
        assert_eq!(mixed.background(), PreviewBackground::Checker);

        let mut transparent = PreviewColorStats::default();
        transparent.add(
            Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 128,
            },
            1.0,
        );
        assert_eq!(transparent.background(), PreviewBackground::Checker);
    }

    #[test]
    fn preview_view_centres_non_origin_symbol_bounds() {
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 100.0));
        let view = preview_view(rect, (100.0, 50.0, 140.0, 70.0));
        let center = pos2(
            view.origin.x + 120.0 * view.scale,
            view.origin.y + 60.0 * view.scale,
        );
        assert!((center.x - rect.center().x).abs() < 0.01);
        assert!((center.y - rect.center().y).abs() < 0.01);
    }
}
