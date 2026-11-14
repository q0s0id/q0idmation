//! The name-and-folder prompt shown before a symbol is made: a new empty
//! one, the selection converted into one, or an independent copy of the
//! symbol behind an instance.

use std::collections::HashSet;

use egui::{Context, Id};
use q0s_format::v2::{LibraryItemReference, Target};

use crate::app::{Action, EditorApp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolRequest {
    /// A new, empty symbol.
    New,
    /// The selection turned into a symbol.
    Convert,
    /// An independent copy of the symbol placed at (timeline, layer, placement).
    Duplicate(u16, u16, usize),
}

impl SymbolRequest {
    fn title(self) -> &'static str {
        match self {
            Self::New => "New symbol",
            Self::Convert => "Convert to symbol",
            Self::Duplicate(..) => "Duplicate symbol",
        }
    }

    fn action(self) -> Action {
        match self {
            Self::New => Action::AddQ0rg,
            Self::Convert => Action::ConvertSelectionToQ0rg,
            Self::Duplicate(q0rg_id, layer_id, placement) => {
                Action::DuplicateSymbol(q0rg_id, layer_id, placement)
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Prompt {
    request: SymbolRequest,
    name: String,
    /// Slash-separated library folder; empty is the library root.
    folder: String,
    /// Optional folder made inside `folder` for the symbol.
    new_folder: String,
    focused: bool,
}

fn prompt_id() -> Id {
    Id::new("q0editor_symbol_prompt")
}

/// The symbol a duplicate would copy.
fn duplicate_source(app: &EditorApp, q0rg_id: u16, layer_id: u16, placement: usize) -> Option<u16> {
    let placement = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)?
        .placements
        .get(placement)?;
    match placement.target {
        Target::Q0rg(id) => Some(id),
        Target::Asset(_) => None,
    }
}

/// Asks for the symbol's name and folder; the symbol is made when the user
/// confirms.
pub(crate) fn open(app: &mut EditorApp, ctx: &Context, request: SymbolRequest) {
    if request == SymbolRequest::Convert
        && !crate::selection_edit::selection_can_clip(&app.session.selection)
    {
        app.session.status = "Convert to Symbol: select graphics or objects first".to_string();
        return;
    }
    let project = &app.state.project;
    let next_id = project
        .q0rgs
        .iter()
        .map(|q0rg| q0rg.q0rg_id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let name = crate::app::unique_symbol_name(project, &format!("Symbol {next_id}"), None);
    let folder = match request {
        SymbolRequest::Duplicate(q0rg_id, layer_id, placement) => {
            duplicate_source(app, q0rg_id, layer_id, placement)
                .and_then(|source| {
                    project
                        .runtime
                        .library_item_folders
                        .get(&LibraryItemReference::Q0rg(source))
                        .cloned()
                })
                .unwrap_or_default()
        }
        SymbolRequest::New | SymbolRequest::Convert => String::new(),
    };
    ctx.data_mut(|data| {
        data.insert_temp(
            prompt_id(),
            Prompt {
                request,
                name,
                folder,
                new_folder: String::new(),
                focused: false,
            },
        );
    });
}

/// A folder path with its slashes normalised and its parts trimmed; empty
/// when nothing usable is left.
fn clean_folder(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// Draws the prompt while one is open. Call once per frame.
pub(crate) fn render(app: &mut EditorApp, ctx: &Context) {
    let Some(mut prompt) = ctx.data(|data| data.get_temp::<Prompt>(prompt_id())) else {
        return;
    };
    let mut folders = app.state.project.runtime.library_folders.clone();
    folders.sort();
    let dim = app.settings.theme.text_dim.to_color32();
    let mut open = true;
    let mut confirm = false;
    let mut cancel = false;
    egui::Window::new(prompt.request.title())
        .id(Id::new("q0editor_symbol_prompt_window"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Grid::new("symbol_prompt_fields")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    let name = ui.add(
                        egui::TextEdit::singleline(&mut prompt.name)
                            .desired_width(260.0)
                            .hint_text("Symbol name"),
                    );
                    if !prompt.focused {
                        name.request_focus();
                        prompt.focused = true;
                    }
                    if name.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                        confirm = true;
                    }
                    ui.end_row();

                    ui.label("Folder");
                    let shown = if prompt.folder.is_empty() {
                        "Library root".to_string()
                    } else {
                        prompt.folder.clone()
                    };
                    egui::ComboBox::from_id_source("symbol_prompt_folder")
                        .selected_text(shown)
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut prompt.folder, String::new(), "Library root");
                            for folder in &folders {
                                let depth = folder.matches('/').count();
                                let leaf = folder.rsplit('/').next().unwrap_or(folder);
                                let label = format!("{}{leaf}", "    ".repeat(depth + 1));
                                ui.selectable_value(&mut prompt.folder, folder.clone(), label)
                                    .on_hover_text(folder);
                            }
                        });
                    ui.end_row();

                    ui.label("New folder");
                    ui.add(
                        egui::TextEdit::singleline(&mut prompt.new_folder)
                            .desired_width(260.0)
                            .hint_text("optional: made inside the folder above"),
                    );
                    ui.end_row();
                });
            let target = target_folder(&prompt);
            ui.label(
                egui::RichText::new(if target.is_empty() {
                    "Goes to the library root.".to_string()
                } else {
                    format!("Goes to {target}.")
                })
                .small()
                .color(dim),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let ready = !prompt.name.trim().is_empty();
                if ui.add_enabled(ready, egui::Button::new("OK")).clicked() {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });
    let confirm = confirm && !prompt.name.trim().is_empty();
    if !open || cancel || confirm {
        ctx.data_mut(|data| data.remove::<Prompt>(prompt_id()));
    } else {
        ctx.data_mut(|data| data.insert_temp(prompt_id(), prompt.clone()));
    }
    if confirm {
        make(app, ctx, &prompt);
    }
}

fn target_folder(prompt: &Prompt) -> String {
    let folder = clean_folder(&prompt.folder);
    let new_folder = clean_folder(&prompt.new_folder);
    match (folder.is_empty(), new_folder.is_empty()) {
        (_, true) => folder,
        (true, false) => new_folder,
        (false, false) => format!("{folder}/{new_folder}"),
    }
}

/// Runs the request and gives the symbol it made the chosen name and folder.
fn make(app: &mut EditorApp, ctx: &Context, prompt: &Prompt) {
    let before: HashSet<u16> = app
        .state
        .project
        .q0rgs
        .iter()
        .map(|q0rg| q0rg.q0rg_id)
        .collect();
    app.handle(ctx, prompt.request.action());
    let project = &mut app.state.project;
    let added: Vec<u16> = project
        .q0rgs
        .iter()
        .map(|q0rg| q0rg.q0rg_id)
        .filter(|id| !before.contains(id))
        .collect();
    // A duplicate copies nested symbols too; the one asked for is the copy
    // that none of the other copies places.
    let nested: HashSet<u16> = project
        .q0rgs
        .iter()
        .filter(|q0rg| added.contains(&q0rg.q0rg_id))
        .flat_map(|q0rg| &q0rg.layers)
        .flat_map(|layer| &layer.placements)
        .filter_map(|placement| match placement.target {
            Target::Q0rg(id) => Some(id),
            Target::Asset(_) => None,
        })
        .collect();
    let Some(made) = added.iter().copied().find(|id| !nested.contains(id)) else {
        // The request failed; its own status message explains why.
        return;
    };
    let name = crate::app::unique_symbol_name(project, &prompt.name, Some(made));
    if let Some(q0rg) = project.q0rgs.iter_mut().find(|q0rg| q0rg.q0rg_id == made) {
        q0rg.name = name.clone();
    }
    let folder = target_folder(prompt);
    let item = LibraryItemReference::Q0rg(made);
    if folder.is_empty() {
        project.runtime.library_item_folders.remove(&item);
    } else {
        let mut path = String::new();
        for part in folder.split('/') {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(part);
            if !project.runtime.library_folders.contains(&path) {
                project.runtime.library_folders.push(path.clone());
            }
        }
        project
            .runtime
            .library_item_folders
            .insert(item, folder.clone());
    }
    app.state.mark_dirty();
    app.session.status = if folder.is_empty() {
        format!("symbol \"{name}\" made")
    } else {
        format!("symbol \"{name}\" made in {folder}")
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(request: SymbolRequest, name: &str, folder: &str, new_folder: &str) -> Prompt {
        Prompt {
            request,
            name: name.to_string(),
            folder: folder.to_string(),
            new_folder: new_folder.to_string(),
            focused: true,
        }
    }

    #[test]
    fn a_new_symbol_takes_the_name_and_folder_it_was_given() {
        let ctx = Context::default();
        let mut app = EditorApp::default();
        open(&mut app, &ctx, SymbolRequest::New);
        let shown = ctx
            .data(|data| data.get_temp::<Prompt>(prompt_id()))
            .expect("prompt open");
        assert!(shown.name.starts_with("Symbol "), "{}", shown.name);
        assert!(shown.folder.is_empty(), "new symbols default to the root");

        make(
            &mut app,
            &ctx,
            &prompt(SymbolRequest::New, "  Hero ", "", "Characters / Main"),
        );
        let hero = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.name == "Hero")
            .expect("named symbol");
        assert_eq!(
            app.state
                .project
                .runtime
                .library_item_folders
                .get(&LibraryItemReference::Q0rg(hero.q0rg_id))
                .map(String::as_str),
            Some("Characters/Main")
        );
        assert!(app
            .state
            .project
            .runtime
            .library_folders
            .iter()
            .any(|folder| folder == "Characters"));
        q0s_format::v2::validate(&app.state.project).expect("valid project");

        // A second one with the same name gets a free name, not a clash.
        make(&mut app, &ctx, &prompt(SymbolRequest::New, "Hero", "", ""));
        assert!(app
            .state
            .project
            .q0rgs
            .iter()
            .any(|q0rg| q0rg.name == "Hero #2"));
    }

    #[test]
    fn converting_nothing_does_not_ask() {
        let ctx = Context::default();
        let mut app = EditorApp::default();
        open(&mut app, &ctx, SymbolRequest::Convert);
        assert!(ctx
            .data(|data| data.get_temp::<Prompt>(prompt_id()))
            .is_none());
    }

    #[test]
    fn folders_are_cleaned_and_joined() {
        let mut p = prompt(SymbolRequest::New, "x", "A/B", " C\\D ");
        assert_eq!(target_folder(&p), "A/B/C/D");
        p.new_folder.clear();
        assert_eq!(target_folder(&p), "A/B");
        p.folder = " / ".into();
        assert_eq!(target_folder(&p), "");
    }
}
