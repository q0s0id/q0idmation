use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::App;
use egui::Context;
use q0s_format::transform::Affine;
use q0s_format::v2::{
    Anchor, Asset, Layer, LayerKey, LayerKind, LayerMetadata, Path as VPath, Placement, Q0rg,
    Q0vAsset, Target, Transform2D, Tween, Vec2, VectorAsset,
};

use crate::file_io;
use crate::panels;
use crate::render::TextureCache;
use crate::state::{
    default_project, History, LibraryItem, PathRef, PlacementRef, ProjectState, Selection, Session,
    Tool, ToolState,
};

pub struct EditorApp {
    pub state: ProjectState,
    pub session: Session,
    pub history: History,
    pub textures: TextureCache,
    /// Persistent theme, drawing, and script-editor settings.
    /// `<config>/q0editor/settings.json`.
    pub settings: crate::settings::Settings,
    /// Separate non-modal export workshop and immutable export queue.
    pub q0enc: crate::q0enc::Q0EncState,
    /// Set on the first `update()` so we apply the loaded theme once a
    /// real `Context` is available (the egui ctx isn't valid in
    /// `Default::default`).
    theme_applied: bool,
    /// Set on first `update()` after `q0lang::fonts::install` runs, so
    /// the persisted font choice (default `Consolas`) lands in egui before
    /// any TextEdit asks for `FontFamily::Name("q0lang")`.
    q0lang_font_installed: bool,
    pending: Vec<Action>,
    unsaved_action: Option<Action>,
    pending_asset_delete: Option<(u16, usize)>,
    pending_q0rg_delete: Option<(u16, usize)>,
    pending_layer_delete: Option<(u16, u16, usize, usize)>,
    pending_frame_truncate: Option<(u16, u16, usize, usize)>,
    media_import_job: Option<MediaImportJob>,
    project_tabs: Vec<ProjectTab>,
    active_project_tab: Option<u64>,
    last_project_tab: Option<u64>,
    next_project_tab_id: u64,
    show_home: bool,
    project_active: bool,
    new_project_dialog: Option<NewProjectSpec>,
    pub close_requested: bool,
}

struct MediaImportJob {
    path: PathBuf,
    receiver: Receiver<Result<Vec<u8>, String>>,
}

struct ProjectWorkspace {
    state: ProjectState,
    session: Session,
    history: History,
    textures: TextureCache,
    q0enc: crate::q0enc::Q0EncState,
    pending_asset_delete: Option<(u16, usize)>,
    pending_q0rg_delete: Option<(u16, usize)>,
    pending_layer_delete: Option<(u16, u16, usize, usize)>,
    pending_frame_truncate: Option<(u16, u16, usize, usize)>,
    media_import_job: Option<MediaImportJob>,
}

struct ProjectTab {
    id: u64,
    workspace: Option<ProjectWorkspace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTabSummary {
    pub id: u64,
    pub title: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProjectSpec {
    pub name: String,
    pub width: u16,
    pub height: u16,
    pub fps: u16,
    pub frame_count: u16,
}

impl Default for NewProjectSpec {
    fn default() -> Self {
        Self {
            name: "untitled".to_string(),
            width: crate::state::DEFAULT_STAGE_WIDTH,
            height: crate::state::DEFAULT_STAGE_HEIGHT,
            fps: 24,
            frame_count: 24,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    NewProject,
    CreateProject(NewProjectSpec),
    ShowHome,
    ResumeProject,
    CycleDocumentTab(i8),
    SwitchProjectTab(u64),
    CloseProjectTab(u64),
    OpenProject,
    OpenProjectFromPath(PathBuf),
    ImportBitmap,
    ImportBitmapFromPath(PathBuf),
    ImportMedia,
    ImportMediaFromPath(PathBuf),
    SaveProject,
    SaveProjectAs,
    OpenQ0Enc,
    ToggleQ0Enc,
    Exit,
    Undo,
    Redo,
    SelectTool(Tool),
    TogglePlay,
    FirstFrame,
    PreviousFrame,
    NextFrame,
    LastFrame,
    InsertFrame,
    InsertKeyframe,
    InsertBlankKeyframe,
    AddLayer,
    AddLayerFolder,
    RenameLayer(u16, u16, String),
    MoveLayer(u16, u16, i8),
    DropLayer(u16, u16, LayerDropTarget),
    IndentLayer(u16, u16),
    OutdentLayer(u16, u16),
    ToggleLayerFolder(u16, u16),
    DeleteLayer(u16, u16),
    AddQ0rg,
    RenameLibraryItem(LibraryItem, String),
    DeleteLibraryItem(LibraryItem),
    SetQ0rgFrameCount(u16, u16),
    DeleteSelection,
    ConvertSelectionToQ0rg,
    BreakApartSelection,
    ConvertStrokeToFill,
    PlaceQ0rgInstance(u16),
    PlaceLibraryItemAt(LibraryItem, q0s_format::v2::Vec2),
    PlaceLibraryItemOnTimeline(LibraryItem, u16, u16),
    EnterQ0rg(u16),
    BreadcrumbJumpTo(usize),
    ExitQ0rg,
    ToggleCredits,
    ToggleSettings,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    CopySelection,
    CutSelection,
    Paste,
    DuplicateSelection,
    BringToFront,
    SendToBack,
    RemoveFrame,
    ClearKeyframe,
    ToggleMotionTween,
    /// Open the q0lang script editor for the given q0rg, or for the
    /// currently-selected one when `None`.
    OpenQ0langEditor(Option<u16>),
    CloseQ0langEditor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerDropTarget {
    Before {
        layer_id: u16,
        parent_folder_id: Option<u16>,
    },
    After {
        layer_id: u16,
        parent_folder_id: Option<u16>,
    },
    IntoFolder(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LayerTreeNode {
    layer_id: u16,
    children: Vec<u16>,
}

impl Default for EditorApp {
    fn default() -> Self {
        let project = default_project();
        let mut session = Session::for_project(&project);
        let settings = crate::settings::Settings::load();
        let mut q0enc = crate::q0enc::Q0EncState::default();
        q0enc.apply_persisted_preferences(
            settings.last_export_directory.clone(),
            settings.png_sequence_create_subfolder,
        );
        // Mirror persisted drawing defaults into the in-memory session.
        session.brush_cap = settings.brush_cap.into();
        session.brush = settings.brush.to_runtime();
        Self {
            state: ProjectState {
                project,
                file_path: None,
                dirty: false,
            },
            session,
            history: History::new(),
            textures: TextureCache::default(),
            settings,
            q0enc,
            theme_applied: false,
            q0lang_font_installed: false,
            pending: Vec::new(),
            unsaved_action: None,
            pending_asset_delete: None,
            pending_q0rg_delete: None,
            pending_layer_delete: None,
            pending_frame_truncate: None,
            media_import_job: None,
            project_tabs: Vec::new(),
            active_project_tab: None,
            last_project_tab: None,
            next_project_tab_id: 1,
            show_home: true,
            project_active: false,
            new_project_dialog: None,
            close_requested: false,
        }
    }
}

impl EditorApp {
    pub fn queue(&mut self, action: Action) {
        self.pending.push(action);
    }

    pub fn home_visible(&self) -> bool {
        self.show_home
    }

    pub fn has_active_project(&self) -> bool {
        self.project_active
    }

    pub fn has_open_projects(&self) -> bool {
        !self.project_tabs.is_empty()
    }

    pub fn project_tab_summaries(&self) -> Vec<ProjectTabSummary> {
        self.project_tabs
            .iter()
            .filter_map(|tab| {
                let (title, path, dirty) = if self.active_project_tab == Some(tab.id) {
                    (
                        self.state.project.meta.name.clone(),
                        self.state.file_path.clone(),
                        self.state.dirty,
                    )
                } else {
                    let workspace = tab.workspace.as_ref()?;
                    (
                        workspace.state.project.meta.name.clone(),
                        workspace.state.file_path.clone(),
                        workspace.state.dirty,
                    )
                };
                Some(ProjectTabSummary {
                    id: tab.id,
                    title: if title.trim().is_empty() {
                        "untitled".to_string()
                    } else {
                        title
                    },
                    path,
                    dirty,
                    active: self.active_project_tab == Some(tab.id),
                })
            })
            .collect()
    }

    fn workspace_for_project(
        &self,
        project: q0s_format::v2::ProjectV2,
        file_path: Option<PathBuf>,
    ) -> ProjectWorkspace {
        let mut session = Session::for_project(&project);
        session.brush_cap = self.settings.brush_cap.into();
        session.brush = self.settings.brush.to_runtime();
        let mut q0enc = crate::q0enc::Q0EncState::default();
        q0enc.apply_persisted_preferences(
            self.settings.last_export_directory.clone(),
            self.settings.png_sequence_create_subfolder,
        );
        ProjectWorkspace {
            state: ProjectState {
                project,
                file_path,
                dirty: false,
            },
            session,
            history: History::new(),
            textures: TextureCache::default(),
            q0enc,
            pending_asset_delete: None,
            pending_q0rg_delete: None,
            pending_layer_delete: None,
            pending_frame_truncate: None,
            media_import_job: None,
        }
    }

    fn take_active_workspace(&mut self) -> ProjectWorkspace {
        let show_settings = self.session.show_settings;
        let show_credits = self.session.show_credits;
        let mut blank = self.workspace_for_project(default_project(), None);
        blank.session.show_settings = show_settings;
        blank.session.show_credits = show_credits;
        ProjectWorkspace {
            state: std::mem::replace(&mut self.state, blank.state),
            session: std::mem::replace(&mut self.session, blank.session),
            history: std::mem::replace(&mut self.history, blank.history),
            textures: std::mem::replace(&mut self.textures, blank.textures),
            q0enc: std::mem::replace(&mut self.q0enc, blank.q0enc),
            pending_asset_delete: self.pending_asset_delete.take(),
            pending_q0rg_delete: self.pending_q0rg_delete.take(),
            pending_layer_delete: self.pending_layer_delete.take(),
            pending_frame_truncate: self.pending_frame_truncate.take(),
            media_import_job: self.media_import_job.take(),
        }
    }

    fn install_workspace(&mut self, mut workspace: ProjectWorkspace) {
        let show_settings = self.session.show_settings;
        let show_credits = self.session.show_credits;
        workspace.session.show_settings = show_settings;
        workspace.session.show_credits = show_credits;
        self.state = workspace.state;
        self.session = workspace.session;
        self.history = workspace.history;
        self.textures = workspace.textures;
        self.q0enc = workspace.q0enc;
        self.pending_asset_delete = workspace.pending_asset_delete;
        self.pending_q0rg_delete = workspace.pending_q0rg_delete;
        self.pending_layer_delete = workspace.pending_layer_delete;
        self.pending_frame_truncate = workspace.pending_frame_truncate;
        self.media_import_job = workspace.media_import_job;
    }

    fn active_workspace_busy(&self) -> bool {
        self.media_import_job.is_some()
            || self.q0enc.has_active_job()
            || self.unsaved_action.is_some()
    }

    fn store_active_workspace(&mut self) {
        let Some(active_id) = self.active_project_tab else {
            return;
        };
        let workspace = self.take_active_workspace();
        if let Some(tab) = self.project_tabs.iter_mut().find(|tab| tab.id == active_id) {
            tab.workspace = Some(workspace);
        }
        self.active_project_tab = None;
        self.project_active = false;
    }

    fn switch_to_home(&mut self) -> bool {
        if self.show_home {
            return true;
        }
        if self.active_workspace_busy() {
            self.session.status =
                "finish or cancel the active import/export before switching tabs".to_string();
            return false;
        }
        self.last_project_tab = self.active_project_tab;
        self.store_active_workspace();
        self.show_home = true;
        self.session.playing = false;
        self.session.status = "home".to_string();
        true
    }

    fn cycle_document_tab(&mut self, direction: i8) {
        let mut documents = Vec::with_capacity(self.project_tabs.len() + 1);
        documents.push(None);
        documents.extend(self.project_tabs.iter().map(|tab| Some(tab.id)));
        if documents.len() <= 1 {
            return;
        }
        let current = if self.show_home {
            0
        } else {
            documents
                .iter()
                .position(|tab| *tab == self.active_project_tab)
                .unwrap_or(0)
        };
        let next = if direction < 0 {
            current.checked_sub(1).unwrap_or(documents.len() - 1)
        } else {
            (current + 1) % documents.len()
        };
        match documents[next] {
            Some(tab_id) => {
                self.switch_to_project_tab(tab_id);
            }
            None => {
                self.switch_to_home();
            }
        }
    }

    fn switch_to_project_tab(&mut self, tab_id: u64) -> bool {
        if self.active_project_tab == Some(tab_id) {
            self.show_home = false;
            self.project_active = true;
            return true;
        }
        let Some(target_index) = self.project_tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        if self.active_workspace_busy() {
            self.session.status =
                "finish or cancel the active import/export before switching tabs".to_string();
            return false;
        }
        if self.active_project_tab.is_some() {
            self.store_active_workspace();
        }
        let Some(workspace) = self.project_tabs[target_index].workspace.take() else {
            return false;
        };
        self.install_workspace(workspace);
        self.active_project_tab = Some(tab_id);
        self.last_project_tab = Some(tab_id);
        self.project_active = true;
        self.show_home = false;
        true
    }

    fn activate_new_workspace(&mut self, workspace: ProjectWorkspace) -> bool {
        if self.active_workspace_busy() {
            self.session.status =
                "finish or cancel the active import/export before opening another project"
                    .to_string();
            return false;
        }
        if self.active_project_tab.is_some() {
            self.store_active_workspace();
        }
        let tab_id = self.next_project_tab_id;
        self.next_project_tab_id = self.next_project_tab_id.saturating_add(1);
        self.project_tabs.push(ProjectTab {
            id: tab_id,
            workspace: None,
        });
        self.install_workspace(workspace);
        self.active_project_tab = Some(tab_id);
        self.last_project_tab = Some(tab_id);
        self.project_active = true;
        self.show_home = false;
        true
    }

    fn close_project_tab(&mut self, tab_id: u64) {
        if self.active_project_tab != Some(tab_id) && !self.switch_to_project_tab(tab_id) {
            return;
        }
        if self.active_workspace_busy() {
            self.session.status =
                "finish or cancel the active import/export before closing this tab".to_string();
            return;
        }
        if self.state.dirty {
            self.unsaved_action = Some(Action::CloseProjectTab(tab_id));
            return;
        }
        let Some(index) = self.project_tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        self.project_tabs.remove(index);
        let _discarded = self.take_active_workspace();
        self.active_project_tab = None;
        self.project_active = false;
        let replacement = self
            .project_tabs
            .get(index)
            .or_else(|| index.checked_sub(1).and_then(|i| self.project_tabs.get(i)))
            .map(|tab| tab.id);
        if let Some(replacement) = replacement {
            self.switch_to_project_tab(replacement);
        } else {
            self.show_home = true;
            self.last_project_tab = None;
            self.session.status = "home".to_string();
        }
    }

    fn first_dirty_project_tab(&self) -> Option<u64> {
        self.project_tabs.iter().find_map(|tab| {
            let dirty = if self.active_project_tab == Some(tab.id) {
                self.state.dirty
            } else {
                tab.workspace
                    .as_ref()
                    .is_some_and(|workspace| workspace.state.dirty)
            };
            dirty.then_some(tab.id)
        })
    }

    fn has_dirty_projects(&self) -> bool {
        self.first_dirty_project_tab().is_some()
    }

    fn find_project_tab_by_path(&self, path: &std::path::Path) -> Option<u64> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.project_tabs.iter().find_map(|tab| {
            let candidate = if self.active_project_tab == Some(tab.id) {
                self.state.file_path.as_ref()
            } else {
                tab.workspace
                    .as_ref()
                    .and_then(|workspace| workspace.state.file_path.as_ref())
            }?;
            let candidate =
                std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
            (candidate == canonical).then_some(tab.id)
        })
    }

    fn another_project_tab_uses_path(&self, path: &std::path::Path) -> bool {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.project_tabs.iter().any(|tab| {
            if self.active_project_tab == Some(tab.id) {
                return false;
            }
            let Some(candidate) = tab
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.state.file_path.as_ref())
            else {
                return false;
            };
            let candidate =
                std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
            candidate == canonical
        })
    }

    fn create_project(&mut self, mut spec: NewProjectSpec) {
        spec.width = spec.width.clamp(1, 8192);
        spec.height = spec.height.clamp(1, 8192);
        spec.fps = spec.fps.clamp(1, 240);
        spec.frame_count = spec.frame_count.max(1);
        let trimmed_name = spec.name.trim();

        let mut project = default_project();
        project.meta.name = if trimmed_name.is_empty() {
            "untitled".to_string()
        } else {
            trimmed_name.to_string()
        };
        project.meta.stage_width = spec.width;
        project.meta.stage_height = spec.height;
        project.meta.fps = spec.fps;
        project.q0rgs[0].frame_count = spec.frame_count;

        let workspace = self.workspace_for_project(project, None);
        if !self.activate_new_workspace(workspace) {
            self.new_project_dialog = Some(spec);
            return;
        }
        self.new_project_dialog = None;
        self.session.status = format!(
            "new project: {}x{}, {} fps, {} frames",
            spec.width, spec.height, spec.fps, spec.frame_count
        );
    }

    fn drain_actions(&mut self, ctx: &Context) {
        let actions = std::mem::take(&mut self.pending);
        for action in actions {
            self.handle(ctx, action);
        }
    }

    /// Process queued actions with a default UI context. This is useful for
    /// headless callers that only queue model operations.
    #[doc(hidden)]
    pub fn flush_pending_actions(&mut self) {
        let ctx = Context::default();
        self.drain_actions(&ctx);
    }

    fn handle(&mut self, ctx: &Context, action: Action) {
        if self.media_import_job.is_some()
            && matches!(
                &action,
                Action::CreateProject(_)
                    | Action::OpenProject
                    | Action::OpenProjectFromPath(_)
                    | Action::Exit
            )
        {
            self.session.status =
                "media import is still running; wait for it to finish".to_string();
            return;
        }
        match action {
            Action::NewProject => {
                self.new_project_dialog = Some(NewProjectSpec::default());
            }
            Action::CreateProject(spec) => {
                self.create_project(spec);
            }
            Action::ShowHome => {
                self.switch_to_home();
            }
            Action::ResumeProject => {
                if let Some(tab_id) = self.last_project_tab {
                    self.switch_to_project_tab(tab_id);
                }
            }
            Action::CycleDocumentTab(direction) => {
                self.cycle_document_tab(direction);
            }
            Action::SwitchProjectTab(tab_id) => {
                self.switch_to_project_tab(tab_id);
            }
            Action::CloseProjectTab(tab_id) => {
                self.close_project_tab(tab_id);
            }
            Action::OpenProject => {
                if let Some(path) = file_io::pick_open_path() {
                    self.handle(ctx, Action::OpenProjectFromPath(path));
                }
            }
            Action::OpenProjectFromPath(path) => {
                self.open_from_path(&path);
            }
            Action::ImportBitmap => {
                if let Some(path) = file_io::pick_bitmap_import_path() {
                    self.import_bitmap_from_path(&path);
                }
            }
            Action::ImportBitmapFromPath(path) => {
                self.import_bitmap_from_path(&path);
            }
            Action::ImportMedia => {
                if let Some(path) = file_io::pick_media_import_path() {
                    self.import_media_from_path(ctx, &path);
                }
            }
            Action::ImportMediaFromPath(path) => {
                self.import_media_from_path(ctx, &path);
            }
            Action::SaveProject => {
                let path = self.state.file_path.clone();
                match path {
                    Some(p) => {
                        self.save_to(&p);
                    }
                    None => self.handle(ctx, Action::SaveProjectAs),
                }
            }
            Action::SaveProjectAs => {
                let suggested = self.state.file_path.clone();
                if let Some(path) = file_io::pick_save_path(suggested.as_deref()) {
                    self.save_to(&path);
                }
            }
            Action::OpenQ0Enc => {
                self.q0enc
                    .open_for_project(&self.state.project, self.state.file_path.as_deref());
                self.session.status = "q0enc opened".to_string();
            }
            Action::ToggleQ0Enc => {
                self.q0enc
                    .toggle_for_project(&self.state.project, self.state.file_path.as_deref());
                self.session.status = if self.q0enc.open {
                    "q0enc opened"
                } else {
                    "q0enc closed"
                }
                .to_string();
            }
            Action::Exit => {
                if self.active_workspace_busy() {
                    self.session.status =
                        "finish or cancel the active import/export before closing q0editor"
                            .to_string();
                } else if let Some(dirty_tab) = self.first_dirty_project_tab() {
                    if self.active_project_tab != Some(dirty_tab) {
                        self.switch_to_project_tab(dirty_tab);
                    }
                    self.unsaved_action = Some(Action::Exit);
                } else {
                    self.close_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::Undo => {
                let current = self.state.project.clone();
                if let Some(prior) = self.history.pop_undo(&current) {
                    self.state.project = prior;
                    self.session.reconcile_with(&self.state.project);
                    self.textures.invalidate();
                    self.state.dirty = true;
                    self.session.status = "undo".to_string();
                } else {
                    self.session.status = "nothing to undo".to_string();
                }
            }
            Action::Redo => {
                let current = self.state.project.clone();
                if let Some(next) = self.history.pop_redo(&current) {
                    self.state.project = next;
                    self.session.reconcile_with(&self.state.project);
                    self.textures.invalidate();
                    self.state.dirty = true;
                    self.session.status = "redo".to_string();
                } else {
                    self.session.status = "nothing to redo".to_string();
                }
            }
            Action::SelectTool(tool) => {
                self.session.current_tool = tool;
                self.session.tool_state = ToolState::Idle;
                self.session.status = format!("tool: {}", tool.label());
            }
            Action::TogglePlay => {
                self.session.playing = !self.session.playing;
                self.session.last_tick = Instant::now();
                self.session.status = if self.session.playing {
                    "playing"
                } else {
                    "paused"
                }
                .to_string();
            }
            Action::FirstFrame => self.go_to_frame(0),
            Action::PreviousFrame => {
                self.go_to_frame(self.session.current_frame.saturating_sub(1));
            }
            Action::NextFrame => {
                self.go_to_frame(self.session.current_frame.saturating_add(1));
            }
            Action::LastFrame => self.go_to_frame(u16::MAX),
            Action::InsertFrame => {
                // F5: step the playhead one frame forward, extending
                // frame_count if we're at the tail. No new placement Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС›
                // existing spans hold one frame longer.
                let extended = self.advance_playhead_extending();
                let next_frame = self.session.current_frame;
                self.session.status = if extended {
                    format!("frame inserted, now at {}", next_frame + 1)
                } else {
                    format!("frame {}", next_frame + 1)
                };
            }
            Action::InsertKeyframe => {
                if self.current_layer_is_folder() {
                    self.session.status = "folders cannot contain keyframes".to_string();
                } else {
                    self.insert_keyframe_smart();
                }
            }
            Action::InsertBlankKeyframe => {
                if self.current_layer_is_folder() {
                    self.session.status = "folders cannot contain keyframes".to_string();
                } else {
                    self.insert_blank_keyframe_smart();
                }
            }
            Action::AddLayer => {
                let q0rg_id = self.session.current_q0rg_id;
                let selected_id = self.session.current_layer_id;
                let (insert_at, parent_folder_id) = {
                    let Some(q0rg) = self
                        .state
                        .project
                        .q0rgs
                        .iter()
                        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                    else {
                        return;
                    };
                    match q0rg
                        .layers
                        .iter()
                        .position(|layer| layer.layer_id == selected_id)
                    {
                        Some(index) => {
                            let metadata = self.state.project.layer_metadata(q0rg_id, selected_id);
                            if metadata.kind == LayerKind::Folder {
                                let (_, end) = self
                                    .layer_row_block_range(q0rg_id, selected_id)
                                    .unwrap_or((index, index + 1));
                                (end, None)
                            } else {
                                (index + 1, metadata.parent_folder_id)
                            }
                        }
                        None => (q0rg.layers.len(), None),
                    }
                };
                self.history.snapshot(&self.state.project);
                let Some(q0rg) = self
                    .state
                    .project
                    .q0rgs
                    .iter_mut()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                else {
                    return;
                };
                let next_id = q0rg
                    .layers
                    .iter()
                    .map(|layer| layer.layer_id)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1)
                    .max(1);
                q0rg.layers.insert(
                    insert_at.min(q0rg.layers.len()),
                    Layer {
                        layer_id: next_id,
                        name: format!("Layer {next_id}"),
                        explicit_keyframes: Vec::new(),
                        placements: Vec::new(),
                    },
                );
                if let Some(parent_folder_id) = parent_folder_id {
                    self.state.project.layer_metadata.insert(
                        LayerKey::new(q0rg_id, next_id),
                        LayerMetadata {
                            kind: LayerKind::Normal,
                            parent_folder_id: Some(parent_folder_id),
                            collapsed: false,
                        },
                    );
                }
                self.session.current_layer_id = next_id;
                self.state.dirty = true;
                self.session.status = "layer added".to_string();
            }
            Action::AddLayerFolder => {
                let q0rg_id = self.session.current_q0rg_id;
                let selected_id = self.session.current_layer_id;
                let insert_at = {
                    let Some(q0rg) = self
                        .state
                        .project
                        .q0rgs
                        .iter()
                        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                    else {
                        return;
                    };
                    match q0rg
                        .layers
                        .iter()
                        .position(|layer| layer.layer_id == selected_id)
                    {
                        Some(index) => {
                            let metadata = self.state.project.layer_metadata(q0rg_id, selected_id);
                            let block_id = metadata.parent_folder_id.unwrap_or(selected_id);
                            self.layer_row_block_range(q0rg_id, block_id)
                                .map(|(_, end)| end)
                                .unwrap_or(index + 1)
                        }
                        None => q0rg.layers.len(),
                    }
                };
                self.history.snapshot(&self.state.project);
                let Some(q0rg) = self
                    .state
                    .project
                    .q0rgs
                    .iter_mut()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                else {
                    return;
                };
                let next_id = q0rg
                    .layers
                    .iter()
                    .map(|layer| layer.layer_id)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1)
                    .max(1);
                q0rg.layers.insert(
                    insert_at.min(q0rg.layers.len()),
                    Layer {
                        layer_id: next_id,
                        name: format!("Folder {next_id}"),
                        explicit_keyframes: Vec::new(),
                        placements: Vec::new(),
                    },
                );
                self.state.project.layer_metadata.insert(
                    LayerKey::new(q0rg_id, next_id),
                    LayerMetadata {
                        kind: LayerKind::Folder,
                        parent_folder_id: None,
                        collapsed: false,
                    },
                );
                self.session.current_layer_id = next_id;
                self.state.dirty = true;
                self.session.status = "layer folder added".to_string();
            }
            Action::RenameLayer(q0rg_id, layer_id, requested_name) => {
                let name = requested_name.trim().to_string();
                if name.is_empty() {
                    self.session.status = "layer name cannot be empty".to_string();
                    return;
                }
                if name.len() > usize::from(u16::MAX) {
                    self.session.status = "layer name is too long".to_string();
                    return;
                }
                let Some(current_name) = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q| q.q0rg_id == q0rg_id)
                    .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
                    .map(|layer| layer.name.as_str())
                else {
                    self.session.status = "layer no longer exists".to_string();
                    return;
                };
                if current_name == name {
                    self.session.status = "layer name unchanged".to_string();
                    return;
                }
                self.history.snapshot(&self.state.project);
                if let Some(layer) = self
                    .state
                    .project
                    .q0rgs
                    .iter_mut()
                    .find(|q| q.q0rg_id == q0rg_id)
                    .and_then(|q| q.layers.iter_mut().find(|layer| layer.layer_id == layer_id))
                {
                    layer.name = name.clone();
                    self.state.dirty = true;
                    self.session.status = format!("renamed layer to {name}");
                }
            }
            Action::MoveLayer(q0rg_id, layer_id, direction) => {
                self.move_layer_block(q0rg_id, layer_id, direction);
            }
            Action::DropLayer(q0rg_id, layer_id, target) => {
                self.drop_layer(q0rg_id, layer_id, target);
            }
            Action::IndentLayer(q0rg_id, layer_id) => {
                self.indent_layer(q0rg_id, layer_id);
            }
            Action::OutdentLayer(q0rg_id, layer_id) => {
                self.outdent_layer(q0rg_id, layer_id);
            }
            Action::ToggleLayerFolder(q0rg_id, layer_id) => {
                let key = LayerKey::new(q0rg_id, layer_id);
                let metadata = self.state.project.layer_metadata(q0rg_id, layer_id);
                if metadata.kind != LayerKind::Folder {
                    return;
                }
                self.history.snapshot(&self.state.project);
                self.state.project.layer_metadata.insert(
                    key,
                    LayerMetadata {
                        collapsed: !metadata.collapsed,
                        ..metadata
                    },
                );
                self.state.dirty = true;
                self.session.status = if metadata.collapsed {
                    "folder expanded"
                } else {
                    "folder collapsed"
                }
                .to_string();
            }
            Action::DeleteLayer(q0rg_id, layer_id) => {
                self.request_layer_delete(q0rg_id, layer_id);
            }
            Action::AddQ0rg => {
                self.history.snapshot(&self.state.project);
                let next_id = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .map(|q| q.q0rg_id)
                    .max()
                    .unwrap_or(0)
                    + 1;
                self.state.project.q0rgs.push(Q0rg {
                    q0rg_id: next_id,
                    name: format!("Symbol {next_id}"),
                    frame_count: 24,
                    script: String::new(),
                    layers: vec![Layer {
                        layer_id: 1,
                        name: "Layer 1".to_string(),
                        explicit_keyframes: Vec::new(),
                        placements: Vec::new(),
                    }],
                });
                self.session.selection = Selection::Q0rg(next_id);
                self.state.dirty = true;
                self.session.status = "Symbol created".to_string();
            }
            Action::RenameLibraryItem(item, requested_name) => {
                let name = requested_name.trim().to_string();
                if name.is_empty() {
                    self.session.status = "name cannot be empty".to_string();
                    return;
                }
                if name.len() > usize::from(u16::MAX) {
                    self.session.status = "name is too long".to_string();
                    return;
                }

                match item {
                    LibraryItem::Q0rg(q0rg_id) => {
                        let Some(current_name) = self
                            .state
                            .project
                            .q0rgs
                            .iter()
                            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                            .map(|q0rg| q0rg.name.as_str())
                        else {
                            self.session.status = "symbol no longer exists".to_string();
                            return;
                        };
                        if current_name == name {
                            self.session.status = "symbol name unchanged".to_string();
                            return;
                        }
                        self.history.snapshot(&self.state.project);
                        if let Some(q0rg) = self
                            .state
                            .project
                            .q0rgs
                            .iter_mut()
                            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                        {
                            q0rg.name = name.clone();
                        }
                        self.state.dirty = true;
                        self.session.status = format!("renamed symbol to {name}");
                    }
                    LibraryItem::Asset(asset_id) => {
                        let Some(asset) = self
                            .state
                            .project
                            .assets
                            .iter()
                            .find(|asset| asset.id() == asset_id)
                        else {
                            self.session.status = "vector no longer exists".to_string();
                            return;
                        };
                        if !matches!(asset, Asset::Vector(_)) {
                            self.session.status = "only vectors can be renamed here".to_string();
                            return;
                        }
                        let default_name = format!("Vector {asset_id}");
                        let current_name = self
                            .state
                            .project
                            .asset_names
                            .get(&asset_id)
                            .map(String::as_str)
                            .unwrap_or(default_name.as_str());
                        if current_name == name {
                            self.session.status = "vector name unchanged".to_string();
                            return;
                        }
                        self.history.snapshot(&self.state.project);
                        if name == default_name {
                            self.state.project.asset_names.remove(&asset_id);
                        } else {
                            self.state
                                .project
                                .asset_names
                                .insert(asset_id, name.clone());
                        }
                        self.state.dirty = true;
                        self.session.status = format!("renamed vector to {name}");
                    }
                }
            }
            Action::DeleteLibraryItem(item) => {
                self.request_library_item_delete(item);
            }
            Action::SetQ0rgFrameCount(q0rg_id, frame_count) => {
                self.request_frame_count_change(q0rg_id, frame_count);
            }
            Action::DeleteSelection => {
                if let Selection::Q0rg(q0rg_id) = self.session.selection {
                    self.request_library_item_delete(LibraryItem::Q0rg(q0rg_id));
                    return;
                }
                if let Some((cell_count, removed_items, changed)) = self.delete_timeline_selection()
                {
                    self.session.status = if !changed {
                        format!("{cell_count} selected timeline frame(s) already blank")
                    } else if removed_items == 0 {
                        format!("cleared {cell_count} timeline frame(s)")
                    } else {
                        format!(
                            "cleared {removed_items} item(s) from {cell_count} timeline frame(s)"
                        )
                    };
                    return;
                }
                if let Selection::Asset(asset_id) = self.session.selection {
                    let references = self.asset_reference_count(asset_id);
                    if references > 0 {
                        self.pending_asset_delete = Some((asset_id, references));
                        self.session.status = format!(
                            "asset {asset_id} is used {references} time(s); confirmation required"
                        );
                        return;
                    }
                }
                let removed = self.delete_selection();
                if removed {
                    self.session.status = "deleted".to_string();
                } else {
                    self.session.status = "nothing to delete".to_string();
                }
            }
            Action::ConvertSelectionToQ0rg => {
                self.convert_selection_to_q0rg();
            }
            Action::BreakApartSelection => {
                self.break_apart_selection();
            }
            Action::ConvertStrokeToFill => {
                self.convert_stroke_to_fill();
            }
            Action::PlaceQ0rgInstance(q0rg_id) => {
                let center = q0s_format::v2::Vec2::new(
                    self.state.project.meta.stage_width as f32 * 0.5,
                    self.state.project.meta.stage_height as f32 * 0.5,
                );
                self.place_library_item_at(LibraryItem::Q0rg(q0rg_id), center);
            }
            Action::PlaceLibraryItemAt(item, position) => {
                self.place_library_item_at(item, position);
            }
            Action::PlaceLibraryItemOnTimeline(item, layer_id, frame) => {
                self.place_library_item_on_timeline(item, layer_id, frame);
            }
            Action::EnterQ0rg(child_id) => {
                self.enter_q0rg(child_id);
            }
            Action::BreadcrumbJumpTo(depth) => {
                self.breadcrumb_jump_to(depth);
            }
            Action::ExitQ0rg => {
                if !self.session.breadcrumb.is_empty() {
                    let parent = self.session.breadcrumb.pop().unwrap();
                    self.session.current_q0rg_id = parent;
                    self.refresh_layer_for_current_q0rg();
                    self.session.current_frame = 0;
                    self.session.timeline_selection = None;
                    self.session.tool_state = crate::state::ToolState::Idle;
                    self.session.selection = Selection::None;
                }
            }
            Action::ToggleCredits => {
                let opening = !self.session.show_credits;
                self.session.show_credits = opening;
                if opening {
                    self.session.credits_opened_at = Instant::now();
                }
            }
            Action::ToggleSettings => {
                self.session.show_settings = !self.session.show_settings;
            }
            Action::ZoomIn => {
                let z = (self.session.viewport.zoom * 1.25).clamp(0.05, 32.0);
                self.session.viewport.zoom = z;
                self.session.status = format!("zoom {:.0}%", z * 100.0);
            }
            Action::ZoomOut => {
                let z = (self.session.viewport.zoom / 1.25).clamp(0.05, 32.0);
                self.session.viewport.zoom = z;
                self.session.status = format!("zoom {:.0}%", z * 100.0);
            }
            Action::ZoomReset => {
                self.session.viewport.zoom = 1.0;
                self.session.viewport.pan = q0s_format::v2::Vec2::new(0.0, 0.0);
                self.session.status = "zoom 100% (fit)".to_string();
            }
            Action::CopySelection => {
                let payload = crate::selection_edit::capture_clipboard(
                    &self.state.project,
                    &self.session.selection,
                );
                if payload.is_empty() {
                    self.session.status = "nothing to copy".to_string();
                } else {
                    self.session.clipboard = Some(payload);
                    self.session.status = "copied".to_string();
                }
            }
            Action::CutSelection => {
                let payload = crate::selection_edit::capture_clipboard(
                    &self.state.project,
                    &self.session.selection,
                );
                if payload.is_empty() {
                    self.session.status = "nothing to cut".to_string();
                } else {
                    self.session.clipboard = Some(payload);
                    if self.delete_selection() {
                        self.session.status = "cut".to_string();
                    }
                }
            }
            Action::Paste => {
                if self.current_layer_is_folder() {
                    self.session.status = "folders cannot contain artwork".to_string();
                    return;
                }
                if let Some(payload) = self.session.clipboard.clone() {
                    if payload.is_empty() {
                        self.session.status = "clipboard empty".to_string();
                    } else {
                        self.history.snapshot(&self.state.project);
                        let result = crate::selection_edit::paste_payload(
                            &mut self.state.project,
                            self.session.current_q0rg_id,
                            self.session.current_layer_id,
                            self.session.current_frame,
                            &payload,
                            q0s_format::v2::Vec2::new(10.0, 10.0),
                        );
                        self.session.selection =
                            crate::selection_edit::selection_from_paste(result);
                        self.state.dirty = true;
                        self.textures.invalidate();
                        self.session.status = "pasted".to_string();
                    }
                } else {
                    self.session.status = "clipboard empty".to_string();
                }
            }
            Action::DuplicateSelection => {
                if self.current_layer_is_folder() {
                    self.session.status = "folders cannot contain artwork".to_string();
                    return;
                }
                let payload = crate::selection_edit::capture_clipboard(
                    &self.state.project,
                    &self.session.selection,
                );
                if payload.is_empty() {
                    self.session.status = "nothing to duplicate".to_string();
                } else {
                    self.history.snapshot(&self.state.project);
                    let result = crate::selection_edit::paste_payload(
                        &mut self.state.project,
                        self.session.current_q0rg_id,
                        self.session.current_layer_id,
                        self.session.current_frame,
                        &payload,
                        q0s_format::v2::Vec2::new(10.0, 10.0),
                    );
                    self.session.selection = crate::selection_edit::selection_from_paste(result);
                    self.state.dirty = true;
                    self.textures.invalidate();
                    self.session.status = "duplicated".to_string();
                }
            }
            Action::BringToFront => {
                self.reorder_selection(true);
            }
            Action::SendToBack => {
                self.reorder_selection(false);
            }
            Action::RemoveFrame => self.remove_frame(),
            Action::ClearKeyframe => self.clear_keyframe(),
            Action::ToggleMotionTween => self.toggle_motion_tween(),
            Action::OpenQ0langEditor(target) => {
                let resolved = target.unwrap_or(self.session.current_q0rg_id);
                if self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .any(|q| q.q0rg_id == resolved)
                {
                    self.session.q0lang_target = Some(resolved);
                    self.session.show_q0lang_editor = true;
                    self.session.status = "Script editor opened".to_string();
                } else {
                    self.session.status = "No symbol is available to edit".to_string();
                }
            }
            Action::CloseQ0langEditor => {
                self.session.show_q0lang_editor = false;
            }
        }
    }

    fn reorder_selection(&mut self, to_front: bool) {
        if let Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } = self.session.selection.clone()
        {
            self.history.snapshot(&self.state.project);
            if let Some(q) = self
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q| q.q0rg_id == q0rg_id)
            {
                if let Some(layer) = q.layers.iter_mut().find(|l| l.layer_id == layer_id) {
                    if placement_idx < layer.placements.len() {
                        let item = layer.placements.remove(placement_idx);
                        let new_idx = if to_front { layer.placements.len() } else { 0 };
                        layer.placements.insert(new_idx, item);
                        self.session.selection = Selection::Placement {
                            q0rg_id,
                            layer_id,
                            placement_idx: new_idx,
                        };
                        self.state.dirty = true;
                        self.session.status = if to_front {
                            "brought to front"
                        } else {
                            "sent to back"
                        }
                        .to_string();
                    }
                }
            }
        }
    }

    fn current_layer_is_folder(&self) -> bool {
        self.state
            .project
            .layer_is_folder(self.session.current_q0rg_id, self.session.current_layer_id)
    }

    fn request_library_item_delete(&mut self, item: LibraryItem) {
        match item {
            LibraryItem::Asset(asset_id) => {
                if !self
                    .state
                    .project
                    .assets
                    .iter()
                    .any(|asset| asset.id() == asset_id)
                {
                    self.session.status = "library item no longer exists".to_string();
                    return;
                }
                let references = self.asset_reference_count(asset_id);
                if references > 0 {
                    self.pending_asset_delete = Some((asset_id, references));
                    self.session.status = format!(
                        "asset {asset_id} is used {references} time(s); confirmation required"
                    );
                } else {
                    self.session.selection = Selection::Asset(asset_id);
                    if self.delete_selection() {
                        self.session.status = format!("deleted asset {asset_id}");
                    }
                }
            }
            LibraryItem::Q0rg(q0rg_id) => {
                if q0rg_id == self.state.project.meta.entry_q0rg_id {
                    self.session.status = "the stage symbol cannot be deleted".to_string();
                    return;
                }
                if !self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .any(|q| q.q0rg_id == q0rg_id)
                {
                    self.session.status = "symbol no longer exists".to_string();
                    return;
                }
                let references = self.q0rg_reference_count(q0rg_id);
                if references > 0 {
                    self.pending_q0rg_delete = Some((q0rg_id, references));
                    self.session.status = format!(
                        "symbol {q0rg_id} is used {references} time(s); confirmation required"
                    );
                } else {
                    self.apply_q0rg_delete(q0rg_id);
                }
            }
        }
    }

    fn q0rg_reference_count(&self, q0rg_id: u16) -> usize {
        self.state
            .project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .flat_map(|layer| &layer.placements)
            .filter(|placement| matches!(placement.target, Target::Q0rg(id) if id == q0rg_id))
            .count()
    }

    fn apply_q0rg_delete(&mut self, q0rg_id: u16) {
        if q0rg_id == self.state.project.meta.entry_q0rg_id {
            return;
        }
        self.history.snapshot(&self.state.project);
        self.state.project.q0rgs.retain(|q| q.q0rg_id != q0rg_id);
        for q0rg in &mut self.state.project.q0rgs {
            for layer in &mut q0rg.layers {
                layer.placements.retain(
                    |placement| !matches!(placement.target, Target::Q0rg(id) if id == q0rg_id),
                );
            }
        }
        self.state
            .project
            .layer_metadata
            .retain(|key, _| key.q0rg_id != q0rg_id);
        self.session.reconcile_with(&self.state.project);
        self.state.dirty = true;
        self.textures.invalidate();
        self.session.status = format!("deleted symbol {q0rg_id}");
    }

    fn request_layer_delete(&mut self, q0rg_id: u16, layer_id: u16) {
        let Some((start, end)) = self.layer_row_block_range(q0rg_id, layer_id) else {
            return;
        };
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        let layer_count = end - start;
        let item_count = q0rg.layers[start..end]
            .iter()
            .map(|layer| layer.placements.len() + layer.explicit_keyframes.len())
            .sum();
        if item_count > 0 || layer_count > 1 {
            self.pending_layer_delete = Some((q0rg_id, layer_id, layer_count, item_count));
            self.session.status = "layer deletion requires confirmation".to_string();
        } else {
            self.apply_layer_delete(q0rg_id, layer_id);
        }
    }

    fn apply_layer_delete(&mut self, q0rg_id: u16, layer_id: u16) {
        let Some(q0rg_index) = self
            .state
            .project
            .q0rgs
            .iter()
            .position(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        let Some((start, end)) = self.layer_row_block_range(q0rg_id, layer_id) else {
            return;
        };
        let metadata = self.state.project.layer_metadata(q0rg_id, layer_id);
        self.history.snapshot(&self.state.project);
        let removed_ids: Vec<u16> = self.state.project.q0rgs[q0rg_index].layers[start..end]
            .iter()
            .map(|layer| layer.layer_id)
            .collect();
        self.state.project.q0rgs[q0rg_index]
            .layers
            .drain(start..end);
        self.state
            .project
            .layer_metadata
            .retain(|key, _| key.q0rg_id != q0rg_id || !removed_ids.contains(&key.layer_id));

        let has_drawable_layer = self.state.project.q0rgs[q0rg_index]
            .layers
            .iter()
            .any(|layer| !self.state.project.layer_is_folder(q0rg_id, layer.layer_id));
        if !has_drawable_layer {
            let next_id = self.state.project.q0rgs[q0rg_index]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .max()
                .unwrap_or(0)
                .saturating_add(1)
                .max(1);
            self.state.project.q0rgs[q0rg_index].layers.push(Layer {
                layer_id: next_id,
                name: format!("Layer {next_id}"),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            });
        }
        self.session.layer_rename = None;
        self.session.reconcile_with(&self.state.project);
        self.state.dirty = true;
        self.textures.invalidate();
        self.session.status = if metadata.kind == LayerKind::Folder {
            "deleted layer folder"
        } else {
            "deleted layer"
        }
        .to_string();
    }

    /// Return the model-order range occupied by a timeline row. Ordinary
    /// layers occupy one slot. Folder rows own their contiguous children,
    /// which are stored immediately before the folder so reverse UI order
    /// shows the folder first while render order stays back-to-front.
    fn layer_row_block_range(&self, q0rg_id: u16, layer_id: u16) -> Option<(usize, usize)> {
        let q0rg = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
        let index = q0rg
            .layers
            .iter()
            .position(|layer| layer.layer_id == layer_id)?;
        if !self.state.project.layer_is_folder(q0rg_id, layer_id) {
            return Some((index, index + 1));
        }
        let mut start = index;
        while start > 0
            && self
                .state
                .project
                .layer_parent_folder(q0rg_id, q0rg.layers[start - 1].layer_id)
                == Some(layer_id)
        {
            start -= 1;
        }
        Some((start, index + 1))
    }

    fn top_level_block_range_at_index(&self, q0rg_id: u16, index: usize) -> Option<(usize, usize)> {
        let q0rg = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
        let layer = q0rg.layers.get(index)?;
        let metadata = self.state.project.layer_metadata(q0rg_id, layer.layer_id);
        let row_id = metadata.parent_folder_id.unwrap_or(layer.layer_id);
        self.layer_row_block_range(q0rg_id, row_id)
    }

    fn move_layer_block(&mut self, q0rg_id: u16, layer_id: u16, direction: i8) {
        if direction == 0 {
            return;
        }
        let Some(q0rg_index) = self
            .state
            .project
            .q0rgs
            .iter()
            .position(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        let Some(index) = self.state.project.q0rgs[q0rg_index]
            .layers
            .iter()
            .position(|layer| layer.layer_id == layer_id)
        else {
            return;
        };
        let metadata = self.state.project.layer_metadata(q0rg_id, layer_id);

        if let Some(parent_id) = metadata.parent_folder_id {
            let Some(folder_index) = self.state.project.q0rgs[q0rg_index]
                .layers
                .iter()
                .position(|layer| layer.layer_id == parent_id)
            else {
                return;
            };
            let mut first_child = folder_index;
            while first_child > 0
                && self.state.project.layer_parent_folder(
                    q0rg_id,
                    self.state.project.q0rgs[q0rg_index].layers[first_child - 1].layer_id,
                ) == Some(parent_id)
            {
                first_child -= 1;
            }
            let swap_with = if direction < 0 {
                if index + 1 >= folder_index {
                    return;
                }
                index + 1
            } else {
                if index <= first_child {
                    return;
                }
                index - 1
            };
            self.history.snapshot(&self.state.project);
            self.state.project.q0rgs[q0rg_index]
                .layers
                .swap(index, swap_with);
        } else {
            let Some((start, end)) = self.layer_row_block_range(q0rg_id, layer_id) else {
                return;
            };
            let len = self.state.project.q0rgs[q0rg_index].layers.len();
            if direction < 0 {
                if end >= len {
                    return;
                }
                let Some((next_start, next_end)) =
                    self.top_level_block_range_at_index(q0rg_id, end)
                else {
                    return;
                };
                if next_start != end {
                    return;
                }
                self.history.snapshot(&self.state.project);
                let current_len = end - start;
                self.state.project.q0rgs[q0rg_index].layers[start..next_end]
                    .rotate_left(current_len);
            } else {
                if start == 0 {
                    return;
                }
                let Some((previous_start, previous_end)) =
                    self.top_level_block_range_at_index(q0rg_id, start - 1)
                else {
                    return;
                };
                if previous_end != start {
                    return;
                }
                self.history.snapshot(&self.state.project);
                let current_len = end - start;
                self.state.project.q0rgs[q0rg_index].layers[previous_start..end]
                    .rotate_right(current_len);
            }
        }
        self.state.dirty = true;
        self.session.status = if direction < 0 {
            "layer moved up"
        } else {
            "layer moved down"
        }
        .to_string();
    }

    fn drop_layer(&mut self, q0rg_id: u16, layer_id: u16, target: LayerDropTarget) {
        let Some(q0rg_index) = self
            .state
            .project
            .q0rgs
            .iter()
            .position(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        let current_layers = self.state.project.q0rgs[q0rg_index].layers.clone();
        if !current_layers
            .iter()
            .any(|layer| layer.layer_id == layer_id)
        {
            return;
        }
        let dragged_metadata = self.state.project.layer_metadata(q0rg_id, layer_id);
        let dragged_is_folder = dragged_metadata.kind == LayerKind::Folder;

        // Build the front-to-back tree used by the timeline. The file model is
        // stored back-to-front, with each folder's children immediately before
        // the folder row, so rebuilding through this tree keeps both contracts
        // intact for arbitrary drag-and-drop moves.
        let mut nodes: Vec<LayerTreeNode> = current_layers
            .iter()
            .rev()
            .filter(|layer| {
                self.state
                    .project
                    .layer_parent_folder(q0rg_id, layer.layer_id)
                    .is_none()
            })
            .map(|layer| LayerTreeNode {
                layer_id: layer.layer_id,
                children: if self.state.project.layer_is_folder(q0rg_id, layer.layer_id) {
                    current_layers
                        .iter()
                        .rev()
                        .filter(|child| {
                            self.state
                                .project
                                .layer_parent_folder(q0rg_id, child.layer_id)
                                == Some(layer.layer_id)
                        })
                        .map(|child| child.layer_id)
                        .collect()
                } else {
                    Vec::new()
                },
            })
            .collect();

        let mut dragged_node =
            if let Some(index) = nodes.iter().position(|node| node.layer_id == layer_id) {
                nodes.remove(index)
            } else {
                let mut removed = false;
                for node in &mut nodes {
                    if let Some(index) = node.children.iter().position(|child| *child == layer_id) {
                        node.children.remove(index);
                        removed = true;
                        break;
                    }
                }
                if !removed {
                    return;
                }
                LayerTreeNode {
                    layer_id,
                    children: Vec::new(),
                }
            };

        let inserted = match target {
            LayerDropTarget::IntoFolder(folder_id) => {
                if dragged_is_folder || folder_id == layer_id {
                    false
                } else if let Some(folder) = nodes
                    .iter_mut()
                    .find(|node| node.layer_id == folder_id)
                    .filter(|node| self.state.project.layer_is_folder(q0rg_id, node.layer_id))
                {
                    folder.children.insert(0, layer_id);
                    true
                } else {
                    false
                }
            }
            LayerDropTarget::Before {
                layer_id: target_id,
                parent_folder_id,
            }
            | LayerDropTarget::After {
                layer_id: target_id,
                parent_folder_id,
            } => {
                let before = matches!(
                    target,
                    LayerDropTarget::Before {
                        layer_id: _,
                        parent_folder_id: _
                    }
                );
                if let Some(folder_id) = parent_folder_id {
                    if dragged_is_folder || folder_id == layer_id {
                        false
                    } else if let Some(folder) =
                        nodes.iter_mut().find(|node| node.layer_id == folder_id)
                    {
                        if let Some(target_index) =
                            folder.children.iter().position(|child| *child == target_id)
                        {
                            let insert_at = target_index + usize::from(!before);
                            folder.children.insert(insert_at, layer_id);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    let target_block_id = self
                        .state
                        .project
                        .layer_parent_folder(q0rg_id, target_id)
                        .unwrap_or(target_id);
                    if target_block_id == layer_id {
                        false
                    } else if let Some(target_index) = nodes
                        .iter()
                        .position(|node| node.layer_id == target_block_id)
                    {
                        let insert_at = target_index + usize::from(!before);
                        if !dragged_is_folder {
                            dragged_node.children.clear();
                        }
                        nodes.insert(insert_at, dragged_node);
                        true
                    } else {
                        false
                    }
                }
            }
        };
        if !inserted {
            return;
        }

        let mut parent_by_child = std::collections::HashMap::new();
        let mut model_ids = Vec::with_capacity(current_layers.len());
        for node in nodes.iter().rev() {
            if self.state.project.layer_is_folder(q0rg_id, node.layer_id) {
                for child_id in node.children.iter().rev() {
                    parent_by_child.insert(*child_id, node.layer_id);
                    model_ids.push(*child_id);
                }
            }
            model_ids.push(node.layer_id);
        }
        if model_ids.len() != current_layers.len() {
            return;
        }

        let current_ids: Vec<u16> = current_layers.iter().map(|layer| layer.layer_id).collect();
        let parent_changed = current_layers.iter().any(|layer| {
            self.state
                .project
                .layer_parent_folder(q0rg_id, layer.layer_id)
                != parent_by_child.get(&layer.layer_id).copied()
        });
        if current_ids == model_ids && !parent_changed {
            return;
        }

        self.history.snapshot(&self.state.project);
        let mut layers_by_id: std::collections::HashMap<u16, Layer> = current_layers
            .into_iter()
            .map(|layer| (layer.layer_id, layer))
            .collect();
        let mut reordered = Vec::with_capacity(model_ids.len());
        for id in model_ids {
            let Some(layer) = layers_by_id.remove(&id) else {
                return;
            };
            reordered.push(layer);
        }
        self.state.project.q0rgs[q0rg_index].layers = reordered;

        self.state
            .project
            .layer_metadata
            .retain(|key, metadata| key.q0rg_id != q0rg_id || metadata.kind == LayerKind::Folder);
        for (child_id, folder_id) in parent_by_child {
            self.state.project.layer_metadata.insert(
                LayerKey::new(q0rg_id, child_id),
                LayerMetadata {
                    kind: LayerKind::Normal,
                    parent_folder_id: Some(folder_id),
                    collapsed: false,
                },
            );
        }
        self.session.current_layer_id = layer_id;
        self.session.timeline_selection = None;
        self.state.dirty = true;
        self.session.status = "layer moved by drag-and-drop".to_string();
    }

    fn indent_layer(&mut self, q0rg_id: u16, layer_id: u16) {
        let metadata = self.state.project.layer_metadata(q0rg_id, layer_id);
        if metadata.kind == LayerKind::Folder || metadata.parent_folder_id.is_some() {
            return;
        }
        let Some(q0rg_index) = self
            .state
            .project
            .q0rgs
            .iter()
            .position(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        let Some(index) = self.state.project.q0rgs[q0rg_index]
            .layers
            .iter()
            .position(|layer| layer.layer_id == layer_id)
        else {
            return;
        };
        if index + 1 >= self.state.project.q0rgs[q0rg_index].layers.len() {
            self.session.status = "put a folder directly above the layer first".to_string();
            return;
        }
        let Some((next_start, next_end)) = self.top_level_block_range_at_index(q0rg_id, index + 1)
        else {
            return;
        };
        if next_start != index + 1 {
            self.session.status = "put a folder directly above the layer first".to_string();
            return;
        }
        let folder_id = self.state.project.q0rgs[q0rg_index].layers[next_end - 1].layer_id;
        if !self.state.project.layer_is_folder(q0rg_id, folder_id) {
            self.session.status = "put a folder directly above the layer first".to_string();
            return;
        }
        self.history.snapshot(&self.state.project);
        self.state.project.layer_metadata.insert(
            LayerKey::new(q0rg_id, layer_id),
            LayerMetadata {
                kind: LayerKind::Normal,
                parent_folder_id: Some(folder_id),
                collapsed: false,
            },
        );
        self.state.dirty = true;
        self.session.status = "layer moved into folder".to_string();
    }

    fn outdent_layer(&mut self, q0rg_id: u16, layer_id: u16) {
        let Some(folder_id) = self.state.project.layer_parent_folder(q0rg_id, layer_id) else {
            return;
        };
        let Some(q0rg_index) = self
            .state
            .project
            .q0rgs
            .iter()
            .position(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        let Some(index) = self.state.project.q0rgs[q0rg_index]
            .layers
            .iter()
            .position(|layer| layer.layer_id == layer_id)
        else {
            return;
        };
        let Some((block_start, _)) = self.layer_row_block_range(q0rg_id, folder_id) else {
            return;
        };
        self.history.snapshot(&self.state.project);
        let layer = self.state.project.q0rgs[q0rg_index].layers.remove(index);
        let insert_at = block_start.min(self.state.project.q0rgs[q0rg_index].layers.len());
        self.state.project.q0rgs[q0rg_index]
            .layers
            .insert(insert_at, layer);
        self.state
            .project
            .layer_metadata
            .remove(&LayerKey::new(q0rg_id, layer_id));
        self.state.dirty = true;
        self.session.status = "layer moved out of folder".to_string();
    }

    fn asset_reference_count(&self, asset_id: u16) -> usize {
        self.state
            .project
            .q0rgs
            .iter()
            .flat_map(|q0rg| &q0rg.layers)
            .flat_map(|layer| &layer.placements)
            .filter(|placement| matches!(placement.target, Target::Asset(id) if id == asset_id))
            .count()
    }

    fn request_frame_count_change(&mut self, q0rg_id: u16, requested: u16) {
        let requested = requested.max(1);
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            self.session.status = "symbol is no longer available".to_string();
            return;
        };
        if q0rg.frame_count == requested {
            return;
        }

        let removed_keyframes = q0rg
            .layers
            .iter()
            .map(|layer| {
                layer
                    .keyframe_frames()
                    .into_iter()
                    .filter(|frame| *frame >= requested)
                    .count()
            })
            .sum();
        let removed_tweens = q0rg
            .layers
            .iter()
            .flat_map(|layer| &layer.placements)
            .filter(|placement| {
                matches!(placement.tween, Tween::Linear { to_frame } if to_frame >= requested)
            })
            .count();

        if requested < q0rg.frame_count && (removed_keyframes > 0 || removed_tweens > 0) {
            self.pending_frame_truncate =
                Some((q0rg_id, requested, removed_keyframes, removed_tweens));
            self.session.status = "reducing frames requires confirmation".to_string();
            return;
        }
        self.apply_frame_count(q0rg_id, requested);
    }

    fn apply_frame_count(&mut self, q0rg_id: u16, frame_count: u16) {
        self.history.snapshot(&self.state.project);
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        q0rg.frame_count = frame_count.max(1);
        for layer in &mut q0rg.layers {
            layer
                .explicit_keyframes
                .retain(|frame| *frame < q0rg.frame_count);
            layer
                .placements
                .retain(|placement| placement.frame < q0rg.frame_count);
            for placement in &mut layer.placements {
                if matches!(
                    placement.tween,
                    Tween::Linear { to_frame } if to_frame >= q0rg.frame_count
                ) {
                    placement.tween = Tween::None;
                }
            }
        }
        if self.session.current_q0rg_id == q0rg_id {
            self.session.current_frame = self
                .session
                .current_frame
                .min(q0rg.frame_count.saturating_sub(1));
        }
        self.session.selection = Selection::None;
        self.state.dirty = true;
        self.session.status = format!("symbol length: {} frame(s)", q0rg.frame_count);
    }

    /// Convert the current selection Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС› one placement *or* a marquee multi Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС›
    /// into a fresh q0rg symbol. Internal placements keep their original
    /// stage-space transforms so the visual result is byte-identical to
    /// before; the new outer placement uses IDENTITY transform and points
    /// at the new q0rg, so dragging it moves the whole group as one unit.
    fn convert_selection_to_q0rg(&mut self) {
        if let Selection::RawArea {
            placements,
            objects,
            bounds_min,
            bounds_max,
        } = self.session.selection.clone()
        {
            let payload = crate::selection_edit::capture_clipboard(
                &self.state.project,
                &self.session.selection,
            );
            if payload.is_empty() {
                self.session.status = "Convert to Symbol: selection is empty".to_string();
                return;
            }
            self.history.snapshot(&self.state.project);
            let _ = crate::selection_edit::remove_raw_area_and_objects(
                &mut self.state.project,
                &placements,
                &objects,
                bounds_min,
                bounds_max,
                self.session.current_frame,
            );

            let new_q0rg_id = self
                .state
                .project
                .q0rgs
                .iter()
                .map(|q0rg| q0rg.q0rg_id)
                .max()
                .unwrap_or(0)
                .saturating_add(1)
                .max(1);
            self.state.project.q0rgs.push(Q0rg {
                q0rg_id: new_q0rg_id,
                name: format!("Symbol {new_q0rg_id}"),
                frame_count: 1,
                script: String::new(),
                layers: vec![Layer {
                    layer_id: 1,
                    name: "Layer 1".to_string(),
                    explicit_keyframes: Vec::new(),
                    placements: Vec::new(),
                }],
            });
            crate::selection_edit::paste_payload(
                &mut self.state.project,
                new_q0rg_id,
                1,
                0,
                &payload,
                q0s_format::v2::Vec2::new(0.0, 0.0),
            );

            let parent_q0rg_id = self.session.current_q0rg_id;
            let layer_id = self.session.current_layer_id;
            let frame = self.session.current_frame;
            let mut outer_idx = None;
            if let Some(layer) = self
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q0rg| q0rg.q0rg_id == parent_q0rg_id)
                .and_then(|q0rg| {
                    q0rg.layers
                        .iter_mut()
                        .find(|layer| layer.layer_id == layer_id)
                })
            {
                outer_idx = Some(layer.placements.len());
                layer.placements.push(Placement {
                    frame,
                    target: Target::Q0rg(new_q0rg_id),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                });
            }
            if let Some(placement_idx) = outer_idx {
                self.session.selection = Selection::Placement {
                    q0rg_id: parent_q0rg_id,
                    layer_id,
                    placement_idx,
                };
            } else {
                self.session.selection = Selection::None;
            }
            self.state.dirty = true;
            self.textures.invalidate();
            self.session.status = format!("Symbol {new_q0rg_id} created from mixed selection");
            return;
        }
        // A connected raw fill/path is not a display-object placement, but it
        // is still valid artwork for Convert to Symbol. Materialise only the
        // selected contours into temporary standalone placements; neighbouring
        // raw graphics remain in their original drawing surface.
        let raw_refs = match self.session.selection.clone() {
            Selection::Path {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            } => Some(vec![crate::state::PathRef {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            }]),
            Selection::Paths(refs) if !refs.is_empty() => Some(refs),
            _ => None,
        };
        let mut raw_materialized = false;
        if let Some(raw_refs) = raw_refs {
            self.history.snapshot(&self.state.project);
            let Some(placements) =
                crate::tools::materialize_raw_paths_as_placements(self, &raw_refs)
            else {
                self.session.status =
                    "Convert to Symbol: raw selection is no longer available".to_string();
                return;
            };
            self.session.selection = match placements.len() {
                0 => Selection::None,
                1 => Selection::Placement {
                    q0rg_id: placements[0].q0rg_id,
                    layer_id: placements[0].layer_id,
                    placement_idx: placements[0].placement_idx,
                },
                _ => Selection::Multi(placements),
            };
            raw_materialized = true;
        }

        // Normalise selection Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р Р†РІР‚С›РЎС›Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†Р вЂљРЎвЂќР В Р’В Р В Р вЂ№Р В Р Р‹Р Р†Р вЂљРЎвЂќ list of refs. Bail early on irrelevant
        // selection states with a clear status message instead of silently
        // doing nothing.
        let refs: Vec<crate::state::PlacementRef> = match &self.session.selection {
            Selection::Placement {
                q0rg_id,
                layer_id,
                placement_idx,
            } => vec![crate::state::PlacementRef {
                q0rg_id: *q0rg_id,
                layer_id: *layer_id,
                placement_idx: *placement_idx,
            }],
            Selection::Multi(items) if !items.is_empty() => items.clone(),
            _ => {
                self.session.status =
                    "Convert to Symbol: select graphics or one or more objects first".to_string();
                return;
            }
        };

        // All refs must live in the same q0rg (we only ever populate a
        // marquee on `current_q0rg_id`, so this is just defensive).
        let parent_q0rg_id = refs[0].q0rg_id;
        if !refs.iter().all(|r| r.q0rg_id == parent_q0rg_id) {
            self.session.status = "Select objects from one symbol before converting".to_string();
            return;
        }

        // Resolve every ref to a (layer_id, original_placement) pair.
        // Drop dangling refs silently Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС› happens if the project mutated
        // since marquee finalised but before convert ran.
        let mut resolved: Vec<(u16, Placement)> = Vec::new();
        if let Some(q) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == parent_q0rg_id)
        {
            for r in &refs {
                if let Some(layer) = q.layers.iter().find(|l| l.layer_id == r.layer_id) {
                    if let Some(p) = layer.placements.get(r.placement_idx) {
                        resolved.push((r.layer_id, p.clone()));
                    }
                }
            }
        }
        if resolved.is_empty() {
            self.session.status = "Convert: nothing to convert".to_string();
            return;
        }

        if !raw_materialized {
            self.history.snapshot(&self.state.project);
        }

        // Group resolved placements by their original layer so we can
        // recreate the same layer stack inside the new q0rg, preserving
        // z-order. Capture layer names from the original q0rg too.
        let mut groups: std::collections::BTreeMap<u16, (String, Vec<Placement>)> =
            std::collections::BTreeMap::new();
        if let Some(q) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == parent_q0rg_id)
        {
            // Walk q.layers in their actual order and only collect ones
            // that have at least one selected placement Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС› keeps stable
            // ordering whether selection came from marquee (which iterates
            // top-down) or single-click.
            for layer in &q.layers {
                let mine: Vec<Placement> = resolved
                    .iter()
                    .filter(|(lid, _)| *lid == layer.layer_id)
                    .map(|(_, p)| Placement {
                        // Pinned to frame 0 inside the new q0rg Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС› single
                        // keyframe, no implicit time-shift. Rotation/scale/
                        // skew stay so the rendering is unchanged.
                        frame: 0,
                        target: p.target,
                        transform: p.transform,
                        tween: Tween::None,
                    })
                    .collect();
                if !mine.is_empty() {
                    groups.insert(layer.layer_id, (layer.name.clone(), mine));
                }
            }
        }

        let new_q0rg_id = self
            .state
            .project
            .q0rgs
            .iter()
            .map(|q| q.q0rg_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);

        // New q0rg: same layer count + names as the affected source layers,
        // re-numbered to dense 1..=N so the inner ids don't collide with
        // anything else.
        let new_layers: Vec<Layer> = groups
            .into_values()
            .enumerate()
            .map(|(i, (name, placements))| Layer {
                layer_id: (i as u16).saturating_add(1).max(1),
                name,
                explicit_keyframes: Vec::new(),
                placements,
            })
            .collect();
        self.state.project.q0rgs.push(Q0rg {
            q0rg_id: new_q0rg_id,
            name: format!("Symbol {new_q0rg_id}"),
            frame_count: 1,
            script: String::new(),
            layers: new_layers,
        });

        // Now delete the original placements from their source layers.
        // Sort by index *desc* so removals don't shift earlier indices.
        let mut sorted = refs.clone();
        sorted.sort_by(|a, b| {
            a.layer_id
                .cmp(&b.layer_id)
                .then(b.placement_idx.cmp(&a.placement_idx))
        });
        if let Some(q) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == parent_q0rg_id)
        {
            for r in sorted {
                if let Some(layer) = q.layers.iter_mut().find(|l| l.layer_id == r.layer_id) {
                    if r.placement_idx < layer.placements.len() {
                        layer.placements.remove(r.placement_idx);
                    }
                }
            }
        }

        // Insert one outer placement in the user's *current* layer pointing
        // at the new q0rg Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС› IDENTITY transform because each inner placement
        // already carries its full stage-space transform.
        let target_layer_id = self.session.current_layer_id;
        let frame = self.session.current_frame;
        let mut new_outer_idx: Option<usize> = None;
        if let Some(q) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == parent_q0rg_id)
        {
            // Fall back to the first layer if `current_layer_id` is gone.
            let resolved_layer_id = q
                .layers
                .iter()
                .find(|l| l.layer_id == target_layer_id)
                .or_else(|| q.layers.first())
                .map(|l| l.layer_id);
            let layer =
                resolved_layer_id.and_then(|id| q.layers.iter_mut().find(|l| l.layer_id == id));
            if let Some(layer) = layer {
                let idx = layer.placements.len();
                layer.placements.push(Placement {
                    frame,
                    target: Target::Q0rg(new_q0rg_id),
                    transform: Transform2D::IDENTITY,
                    tween: Tween::None,
                });
                new_outer_idx = Some(idx);
                self.session.current_layer_id = layer.layer_id;
            }
        }

        if let Some(idx) = new_outer_idx {
            self.session.selection = Selection::Placement {
                q0rg_id: parent_q0rg_id,
                layer_id: self.session.current_layer_id,
                placement_idx: idx,
            };
        } else {
            self.session.selection = Selection::None;
        }
        self.state.dirty = true;
        self.session.status = format!(
            "Symbol {new_q0rg_id} created from {} object{}",
            refs.len(),
            if refs.len() == 1 { "" } else { "s" }
        );
    }

    /// Convert the stroked-path asset under the current placement into a
    /// filled polygon by tracing each path's centerline at Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В РІР‚В Р В Р вЂ Р В РІР‚С™Р РЋРІР‚С”Р В Р Р‹Р РЋРІР‚С”Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р Р†РІР‚С›РЎС›Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В±half stroke width.
    /// A new asset is created (the original is left intact in case other
    /// placements share it) and the placement is repointed at the new asset.
    fn convert_stroke_to_fill(&mut self) {
        let Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } = self.session.selection.clone()
        else {
            self.session.status = "Stroke to Fill: select a stroke first".to_string();
            return;
        };
        let asset_id = match self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
            .and_then(|l| l.placements.get(placement_idx))
            .map(|p| p.target)
        {
            Some(Target::Asset(id)) => id,
            _ => {
                self.session.status = "Stroke to Fill works on vector shapes".to_string();
                return;
            }
        };
        let (paths, stroke_color, stroke_width, stroke_cap) = match self
            .state
            .project
            .assets
            .iter()
            .find(|a| a.id() == asset_id)
        {
            Some(Asset::Vector(v)) => match v.stroke {
                Some(s) => (v.paths.clone(), s.color, s.width, s.cap),
                None => {
                    self.session.status = "asset has no stroke to convert".to_string();
                    return;
                }
            },
            _ => {
                self.session.status = "asset is not a vector".to_string();
                return;
            }
        };

        let half_width = (stroke_width * 0.5).max(0.25);
        let new_paths: Vec<VPath> = paths
            .into_iter()
            .filter_map(|p| {
                let centerline = crate::render::flatten_path(&p);
                if centerline.len() < 2 {
                    return None;
                }
                let outline = q0s_format::geom::brush_outline_with_caps(
                    &centerline,
                    half_width,
                    8,
                    stroke_cap,
                );
                if outline.len() < 3 {
                    return None;
                }
                Some(VPath {
                    anchors: outline
                        .into_iter()
                        .map(|pt| Anchor {
                            point: pt,
                            in_handle: None,
                            out_handle: None,
                        })
                        .collect(),
                    closed: true,
                })
            })
            .collect();
        if new_paths.is_empty() {
            self.session.status = "stroke too short to convert".to_string();
            return;
        }

        self.history.snapshot(&self.state.project);
        let new_asset_id = crate::tools::next_asset_id_pub(&self.state.project);
        self.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: new_asset_id,
            paths: new_paths,
            fill: Some(stroke_color),
            stroke: None,
        }));
        if let Some(q) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
        {
            if let Some(layer) = q.layers.iter_mut().find(|l| l.layer_id == layer_id) {
                if let Some(pl) = layer.placements.get_mut(placement_idx) {
                    pl.target = Target::Asset(new_asset_id);
                }
            }
        }
        self.state.dirty = true;
        self.session.status = "stroke converted to fill".to_string();
    }

    fn import_bitmap_from_path(&mut self, path: &std::path::Path) {
        let Some(asset_id) = self
            .state
            .project
            .assets
            .iter()
            .map(Asset::id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
        else {
            self.session.status = "bitmap import failed: asset id space exhausted".to_string();
            return;
        };
        match crate::bitmap_import::decode_bitmap_path(path, asset_id) {
            Ok(bitmap) => {
                let width = bitmap.width;
                let height = bitmap.height;
                self.history.snapshot(&self.state.project);
                self.state.project.assets.push(Asset::Bitmap(bitmap));
                self.state.dirty = true;
                self.session.selection = Selection::Asset(asset_id);
                self.textures.invalidate();
                self.session.status = format!(
                    "imported bitmap {} ({}x{}) as asset {}",
                    path.display(),
                    width,
                    height,
                    asset_id
                );
            }
            Err(error) => {
                self.session.status = format!("bitmap import failed: {error}");
            }
        }
    }

    fn import_media_from_path(&mut self, ctx: &Context, path: &std::path::Path) {
        if crate::bitmap_import::is_supported_bitmap_path(path) {
            self.import_bitmap_from_path(path);
            return;
        }
        if self.media_import_job.is_some() {
            self.session.status = "another media import is already running".to_string();
            return;
        }

        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        let source_path = path.to_path_buf();
        let receiver = if extension.eq_ignore_ascii_case("q0v") {
            let (sender, receiver) = mpsc::channel();
            let worker_path = source_path.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("q0editor-q0v-import".to_string())
                .spawn(move || {
                    let result = std::fs::read(&worker_path)
                        .map_err(|error| format!("q0v import failed: {error}"));
                    let _ = sender.send(result);
                })
            {
                self.session.status = format!("start q0v import worker failed: {error}");
                return;
            }
            receiver
        } else if extension.eq_ignore_ascii_case("mp4") {
            match q0video::import::spawn_mp4_to_q0v(source_path.clone()) {
                Ok(receiver) => receiver,
                Err(error) => {
                    self.session.status = format!("mp4 import failed: {error}");
                    return;
                }
            }
        } else {
            self.session.status = format!(
                "unsupported media file: {} (expected png, jpg, webp, mp4 or q0v)",
                path.display()
            );
            return;
        };

        self.media_import_job = Some(MediaImportJob {
            path: source_path,
            receiver,
        });
        self.session.status = if extension.eq_ignore_ascii_case("mp4") {
            format!("converting {} to q0v...", path.display())
        } else {
            format!("reading {}...", path.display())
        };
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    fn poll_media_import(&mut self) {
        let completed = match self.media_import_job.as_ref() {
            Some(job) => match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(
                    "media import worker stopped before returning a result".to_string(),
                )),
            },
            None => None,
        };
        let Some(result) = completed else {
            return;
        };
        let job = self
            .media_import_job
            .take()
            .expect("completed media import must still have a job");
        match result {
            Ok(bytes) => self.finish_media_import(&job.path, bytes),
            Err(error) => {
                let is_mp4 = job
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"));
                let prefix = if is_mp4 {
                    "mp4 import failed"
                } else {
                    "q0v import failed"
                };
                self.session.status = if error.starts_with(prefix) {
                    error
                } else {
                    format!("{prefix}: {error}")
                };
            }
        }
    }

    fn finish_media_import(&mut self, path: &std::path::Path, bytes: Vec<u8>) {
        const MAX_IMPORTED_Q0V_BYTES: usize = 240 * 1024 * 1024;
        if bytes.len() > MAX_IMPORTED_Q0V_BYTES {
            self.session.status = format!(
                "video import failed: converted q0v is {} MiB; project limit is 240 MiB",
                bytes.len() / (1024 * 1024)
            );
            return;
        }
        let media = match q0video::q0v::Q0vFile::parse(bytes.clone()) {
            Ok(media) => media,
            Err(error) => {
                self.session.status = format!("q0v import failed: {error}");
                return;
            }
        };
        let Some(asset_id) = self
            .state
            .project
            .assets
            .iter()
            .map(Asset::id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
        else {
            self.session.status = "video import failed: asset id space exhausted".to_string();
            return;
        };
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("Video")
            .to_string();

        self.history.snapshot(&self.state.project);
        self.state
            .project
            .assets
            .push(Asset::Q0v(Q0vAsset { asset_id, bytes }));
        self.state
            .project
            .asset_names
            .insert(asset_id, name.clone());
        self.state.dirty = true;
        self.session.selection = Selection::Asset(asset_id);
        self.textures.invalidate();
        self.session.status = format!(
            "imported {name} as q0v asset {asset_id} ({}x{}, {} frames at {} fps; audio import pending)",
            media.spec.width,
            media.spec.height,
            media.spec.timeline_frames,
            media.spec.fps
        );
    }
    pub fn can_break_apart_selection(&self) -> bool {
        let Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } = self.session.selection
        else {
            return false;
        };
        let target = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| layer.placements.get(placement_idx))
            .map(|placement| placement.target);
        match target {
            Some(Target::Q0rg(_)) => true,
            Some(Target::Asset(asset_id)) => matches!(
                self.state
                    .project
                    .assets
                    .iter()
                    .find(|asset| asset.id() == asset_id),
                Some(Asset::Vector(_))
            ),
            None => false,
        }
    }

    fn break_apart_selection(&mut self) {
        let Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } = self.session.selection
        else {
            self.session.status = "Break Apart requires a symbol or vector object".to_string();
            return;
        };
        let Some((target, active_transform)) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .and_then(|layer| {
                let placement = layer.placements.get(placement_idx)?;
                let transform = crate::render::active_transform_for_placement(
                    layer,
                    placement_idx,
                    self.session.current_frame,
                )?;
                Some((placement.target, transform))
            })
        else {
            self.session.status = "Selected object is no longer available".to_string();
            return;
        };

        match target {
            Target::Asset(asset_id) => {
                if matches!(
                    self.state
                        .project
                        .assets
                        .iter()
                        .find(|asset| asset.id() == asset_id),
                    Some(Asset::Vector(_))
                ) {
                    self.break_apart_vector_placement(
                        q0rg_id,
                        layer_id,
                        placement_idx,
                        asset_id,
                        active_transform,
                    );
                } else {
                    self.session.status = "Bitmap objects cannot be broken apart".to_string();
                }
            }
            Target::Q0rg(child_id) => self.break_apart_q0rg_placement(
                q0rg_id,
                layer_id,
                placement_idx,
                child_id,
                active_transform,
            ),
        }
    }

    fn break_apart_vector_placement(
        &mut self,
        q0rg_id: u16,
        layer_id: u16,
        placement_idx: usize,
        asset_id: u16,
        active_transform: Transform2D,
    ) {
        let Some(Asset::Vector(source)) = self
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
            .cloned()
        else {
            self.session.status = "Vector source is missing".to_string();
            return;
        };
        let new_asset_id = crate::tools::next_asset_id_pub(&self.state.project);
        let baked = bake_vector_asset(
            &source,
            new_asset_id,
            Affine::from_transform(active_transform),
        );
        let frame = self.session.current_frame;

        self.history.snapshot(&self.state.project);
        let Some(mapped_idx) = crate::tools::materialize_placement_keyframe_for_edit(
            &mut self.state.project,
            q0rg_id,
            layer_id,
            placement_idx,
            frame,
        ) else {
            return;
        };
        self.state.project.assets.push(Asset::Vector(baked));
        let Some(placement) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == layer_id)
            })
            .and_then(|layer| layer.placements.get_mut(mapped_idx))
        else {
            self.state
                .project
                .assets
                .retain(|asset| asset.id() != new_asset_id);
            return;
        };
        placement.target = Target::Asset(new_asset_id);
        placement.transform = Transform2D::IDENTITY;
        placement.tween = Tween::None;
        self.session.selection =
            raw_vector_selection(q0rg_id, layer_id, mapped_idx, source.paths.len());
        self.session.status = "Vector broken apart into raw graphics".to_string();
        self.state.dirty = true;
        self.textures.invalidate();
    }

    fn break_apart_q0rg_placement(
        &mut self,
        parent_q0rg_id: u16,
        parent_layer_id: u16,
        placement_idx: usize,
        child_q0rg_id: u16,
        outer_transform: Transform2D,
    ) {
        let Some(child) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == child_q0rg_id)
            .cloned()
        else {
            self.session.status = "Symbol source is missing".to_string();
            return;
        };
        let local_frame = if child.frame_count > 0 {
            self.session.current_frame % child.frame_count
        } else {
            0
        };
        let outer = Affine::from_transform(outer_transform);
        let mut next_asset_id = crate::tools::next_asset_id_pub(&self.state.project);
        let mut pending_assets = Vec::new();
        let mut pieces: Vec<BreakApartPiece> = Vec::new();

        for child_layer in &child.layers {
            for (child_idx, child_transform) in
                crate::render::active_placements_at(child_layer, local_frame)
            {
                let Some(child_placement) = child_layer.placements.get(child_idx) else {
                    continue;
                };
                let composed = Affine::compose(outer, Affine::from_transform(child_transform));
                match child_placement.target {
                    Target::Asset(asset_id) => {
                        let Some(asset) = self
                            .state
                            .project
                            .assets
                            .iter()
                            .find(|asset| asset.id() == asset_id)
                            .cloned()
                        else {
                            continue;
                        };
                        match asset {
                            Asset::Vector(vector) => {
                                let baked = bake_vector_asset(&vector, next_asset_id, composed);
                                let path_count = baked.paths.len();
                                pending_assets.push(Asset::Vector(baked));
                                pieces.push(BreakApartPiece {
                                    placement: Placement {
                                        frame: self.session.current_frame,
                                        target: Target::Asset(next_asset_id),
                                        transform: Transform2D::IDENTITY,
                                        tween: Tween::None,
                                    },
                                    raw_path_count: path_count,
                                });
                                let Some(incremented) = next_asset_id.checked_add(1) else {
                                    self.session.status =
                                        "Break Apart ran out of asset ids".to_string();
                                    return;
                                };
                                next_asset_id = incremented;
                            }
                            Asset::Bitmap(_) | Asset::Q0v(_) => {
                                let Some(transform) = affine_to_transform(composed) else {
                                    self.session.status =
                                        "Break Apart cannot represent a singular bitmap transform"
                                            .to_string();
                                    return;
                                };
                                pieces.push(BreakApartPiece {
                                    placement: Placement {
                                        frame: self.session.current_frame,
                                        target: Target::Asset(asset_id),
                                        transform,
                                        tween: Tween::None,
                                    },
                                    raw_path_count: 0,
                                });
                            }
                        }
                    }
                    Target::Q0rg(nested_id) => {
                        let Some(transform) = affine_to_transform(composed) else {
                            self.session.status =
                                "Break Apart cannot represent a singular nested transform"
                                    .to_string();
                            return;
                        };
                        pieces.push(BreakApartPiece {
                            placement: Placement {
                                frame: self.session.current_frame,
                                target: Target::Q0rg(nested_id),
                                transform,
                                tween: Tween::None,
                            },
                            raw_path_count: 0,
                        });
                    }
                }
            }
        }

        self.history.snapshot(&self.state.project);
        let Some(mapped_idx) = crate::tools::materialize_placement_keyframe_for_edit(
            &mut self.state.project,
            parent_q0rg_id,
            parent_layer_id,
            placement_idx,
            self.session.current_frame,
        ) else {
            return;
        };
        self.state.project.assets.extend(pending_assets);
        let Some(parent_layer) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == parent_q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == parent_layer_id)
            })
        else {
            return;
        };
        parent_layer.placements.remove(mapped_idx);
        let placements: Vec<Placement> =
            pieces.iter().map(|piece| piece.placement.clone()).collect();
        parent_layer
            .placements
            .splice(mapped_idx..mapped_idx, placements);

        self.session.selection = break_apart_result_selection(
            &self.state.project,
            parent_q0rg_id,
            parent_layer_id,
            mapped_idx,
            &pieces,
        );
        self.session.status = format!("Symbol broken apart into {} item(s)", pieces.len());
        self.state.dirty = true;
        self.textures.invalidate();
    }

    fn place_library_item_on_timeline(
        &mut self,
        item: LibraryItem,
        target_layer_id: u16,
        frame: u16,
    ) {
        let q0rg_id = self.session.current_q0rg_id;
        let Some(q0rg_index) = self
            .state
            .project
            .q0rgs
            .iter()
            .position(|q0rg| q0rg.q0rg_id == q0rg_id)
        else {
            return;
        };
        if frame >= self.state.project.q0rgs[q0rg_index].frame_count
            || self.state.project.layer_is_folder(q0rg_id, target_layer_id)
        {
            self.session.status = "drop media on a real timeline frame".to_string();
            return;
        }

        let q0v = match item {
            LibraryItem::Asset(asset_id) => self
                .state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == asset_id)
                .and_then(|asset| match asset {
                    Asset::Q0v(video) => q0video::q0v::Q0vFile::parse(video.bytes.clone())
                        .ok()
                        .map(|media| (asset_id, media.spec)),
                    _ => None,
                }),
            LibraryItem::Q0rg(_) => None,
        };

        let Some((asset_id, media_spec)) = q0v else {
            self.session.current_layer_id = target_layer_id;
            self.session.current_frame = frame;
            let center = Vec2::new(
                self.state.project.meta.stage_width as f32 * 0.5,
                self.state.project.meta.stage_height as f32 * 0.5,
            );
            self.place_library_item_at(item, center);
            return;
        };

        let project_fps = u32::from(self.state.project.meta.fps.max(1));
        let duration_frames = u64::from(media_spec.timeline_frames)
            .saturating_mul(u64::from(project_fps))
            .div_ceil(u64::from(media_spec.fps.max(1)))
            .max(1);
        let end_exclusive = u64::from(frame).saturating_add(duration_frames);
        if end_exclusive > u64::from(u16::MAX) {
            self.session.status = "video is too long for this timeline".to_string();
            return;
        }
        let end_exclusive = end_exclusive as u16;
        let old_frame_count = self.state.project.q0rgs[q0rg_index].frame_count;
        let new_frame_count = old_frame_count.max(end_exclusive);
        let target_index = self.state.project.q0rgs[q0rg_index]
            .layers
            .iter()
            .position(|layer| layer.layer_id == target_layer_id)
            .unwrap_or_else(|| self.state.project.q0rgs[q0rg_index].layers.len());
        let parent_folder_id = self
            .state
            .project
            .layer_parent_folder(q0rg_id, target_layer_id);
        let next_layer_id = self.state.project.q0rgs[q0rg_index]
            .layers
            .iter()
            .map(|layer| layer.layer_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        let name = self
            .state
            .project
            .asset_names
            .get(&asset_id)
            .cloned()
            .unwrap_or_else(|| format!("Video {asset_id}"));
        let target = Target::Asset(asset_id);
        let center = Vec2::new(
            self.state.project.meta.stage_width as f32 * 0.5,
            self.state.project.meta.stage_height as f32 * 0.5,
        );
        let transform =
            crate::render::centered_target_transform(&self.state.project, target, center);
        let explicit_keyframes = if end_exclusive < new_frame_count {
            vec![end_exclusive]
        } else {
            Vec::new()
        };

        self.history.snapshot(&self.state.project);
        let q0rg = &mut self.state.project.q0rgs[q0rg_index];
        q0rg.frame_count = new_frame_count;
        q0rg.layers.insert(
            (target_index + 1).min(q0rg.layers.len()),
            Layer {
                layer_id: next_layer_id,
                name,
                explicit_keyframes,
                placements: vec![Placement {
                    frame,
                    target,
                    transform,
                    tween: Tween::None,
                }],
            },
        );
        if let Some(parent_folder_id) = parent_folder_id {
            self.state.project.layer_metadata.insert(
                LayerKey::new(q0rg_id, next_layer_id),
                LayerMetadata {
                    kind: LayerKind::Normal,
                    parent_folder_id: Some(parent_folder_id),
                    collapsed: false,
                },
            );
        }
        self.session.current_layer_id = next_layer_id;
        self.session.current_frame = frame;
        self.session.timeline_selection = Some(crate::state::TimelineSelection::single(
            next_layer_id,
            frame,
        ));
        self.session.selection = Selection::Placement {
            q0rg_id,
            layer_id: next_layer_id,
            placement_idx: 0,
        };
        self.state.dirty = true;
        self.textures.invalidate();
        self.session.status = format!(
            "placed q0v on timeline at frame {} ({} project frames)",
            frame + 1,
            duration_frames
        );
    }

    fn place_library_item_at(&mut self, item: LibraryItem, position: q0s_format::v2::Vec2) {
        if self.current_layer_is_folder() {
            self.session.status = "folders cannot contain artwork".to_string();
            return;
        }
        if let LibraryItem::Asset(asset_id) = item {
            if matches!(
                self.state
                    .project
                    .assets
                    .iter()
                    .find(|asset| asset.id() == asset_id),
                Some(Asset::Vector(_))
            ) {
                self.place_vector_asset_as_raw(asset_id, position);
                return;
            }
        }

        let parent_q0rg_id = self.session.current_q0rg_id;
        let target = match item {
            LibraryItem::Q0rg(target_q0rg_id) => {
                if !self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .any(|q0rg| q0rg.q0rg_id == target_q0rg_id)
                {
                    self.session.status = "That symbol is no longer available".to_string();
                    return;
                }
                if self.would_create_q0rg_cycle(parent_q0rg_id, target_q0rg_id) {
                    self.session.status = "A symbol cannot contain itself indirectly".to_string();
                    return;
                }
                Target::Q0rg(target_q0rg_id)
            }
            LibraryItem::Asset(asset_id) => {
                if !self
                    .state
                    .project
                    .assets
                    .iter()
                    .any(|asset| asset.id() == asset_id)
                {
                    self.session.status = "That asset is no longer available".to_string();
                    return;
                }
                Target::Asset(asset_id)
            }
        };

        let transform =
            crate::render::centered_target_transform(&self.state.project, target, position);

        let q0rg_id = parent_q0rg_id;
        let layer_id = self.session.current_layer_id;
        let frame = self.session.current_frame;
        self.history.snapshot(&self.state.project);
        if crate::tools::materialize_layer_keyframe_for_edit(
            &mut self.state.project,
            q0rg_id,
            layer_id,
            frame,
        )
        .is_none()
        {
            return;
        }
        if let Some(layer) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter_mut().find(|layer| layer.layer_id == layer_id))
        {
            let next_idx = layer.placements.len();
            layer.placements.push(Placement {
                frame,
                target,
                transform,
                tween: Tween::None,
            });
            self.state.dirty = true;
            self.session.selection = Selection::Placement {
                q0rg_id,
                layer_id,
                placement_idx: next_idx,
            };
            self.session.status = match item {
                LibraryItem::Q0rg(_) => "Symbol placed".to_string(),
                LibraryItem::Asset(_) => "Asset placed".to_string(),
            };
        }
    }

    fn place_vector_asset_as_raw(&mut self, asset_id: u16, position: Vec2) {
        let Some(Asset::Vector(source)) = self
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == asset_id)
            .cloned()
        else {
            self.session.status = "That vector is no longer available".to_string();
            return;
        };
        if source.paths.is_empty() {
            self.session.status = "Empty vector cannot be placed".to_string();
            return;
        }

        let transform = crate::render::centered_target_transform(
            &self.state.project,
            Target::Asset(asset_id),
            position,
        );
        let new_asset_id = crate::tools::next_asset_id_pub(&self.state.project);
        let baked = bake_vector_asset(&source, new_asset_id, Affine::from_transform(transform));
        let q0rg_id = self.session.current_q0rg_id;
        let layer_id = self.session.current_layer_id;
        let frame = self.session.current_frame;

        self.history.snapshot(&self.state.project);
        if crate::tools::materialize_layer_keyframe_for_edit(
            &mut self.state.project,
            q0rg_id,
            layer_id,
            frame,
        )
        .is_none()
        {
            return;
        }
        self.state.project.assets.push(Asset::Vector(baked));
        let Some(layer) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| {
                q0rg.layers
                    .iter_mut()
                    .find(|layer| layer.layer_id == layer_id)
            })
        else {
            self.state
                .project
                .assets
                .retain(|asset| asset.id() != new_asset_id);
            return;
        };
        let placement_idx = layer.placements.len();
        layer.placements.push(Placement {
            frame,
            target: Target::Asset(new_asset_id),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        });
        let path_count = source.paths.len();
        self.session.selection = raw_vector_selection(q0rg_id, layer_id, placement_idx, path_count);
        self.session.status = "Vector placed as raw graphics".to_string();
        self.state.dirty = true;
        self.textures.invalidate();
    }

    fn would_create_q0rg_cycle(&self, parent_q0rg_id: u16, child_q0rg_id: u16) -> bool {
        if parent_q0rg_id == child_q0rg_id {
            return true;
        }

        let mut pending = vec![child_q0rg_id];
        let mut visited = Vec::new();
        while let Some(q0rg_id) = pending.pop() {
            if q0rg_id == parent_q0rg_id {
                return true;
            }
            if visited.contains(&q0rg_id) {
                continue;
            }
            visited.push(q0rg_id);

            let Some(q0rg) = self
                .state
                .project
                .q0rgs
                .iter()
                .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            else {
                continue;
            };
            for placement in q0rg.layers.iter().flat_map(|layer| &layer.placements) {
                if let Target::Q0rg(nested_id) = placement.target {
                    pending.push(nested_id);
                }
            }
        }
        false
    }

    fn enter_q0rg(&mut self, child_id: u16) {
        if child_id == self.session.current_q0rg_id {
            return;
        }
        if !self
            .state
            .project
            .q0rgs
            .iter()
            .any(|q| q.q0rg_id == child_id)
        {
            return;
        }
        if self.session.breadcrumb.last().copied() == Some(self.session.current_q0rg_id) {
            // already pushed
        } else {
            self.session.breadcrumb.push(self.session.current_q0rg_id);
        }
        self.session.current_q0rg_id = child_id;
        self.refresh_layer_for_current_q0rg();
        self.session.current_frame = 0;
        self.session.timeline_selection = None;
        self.session.selection = Selection::None;
        self.session.tool_state = crate::state::ToolState::Idle;
        self.session.status = "Symbol opened".to_string();
    }

    fn breadcrumb_jump_to(&mut self, depth: usize) {
        // depth 0 = Stage (root); 1..=N = nested q0rg from breadcrumb.
        if depth == 0 {
            // Truncate to root: the stage q0rg lives at breadcrumb[0] if any,
            // but conceptually current_q0rg is the entry q0rg when breadcrumb is empty.
            if !self.session.breadcrumb.is_empty() {
                self.session.current_q0rg_id = self.session.breadcrumb[0];
                self.session.breadcrumb.clear();
            }
        } else if depth <= self.session.breadcrumb.len() {
            // Keep first `depth` entries; current_q0rg becomes breadcrumb[depth-1] is parent
            // and the existing current is being popped Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС› actually we want current to be
            // breadcrumb[depth] if it exists.
            let bc_keep = depth - 1;
            let new_current = self.session.breadcrumb[depth - 1];
            self.session.breadcrumb.truncate(bc_keep);
            self.session.current_q0rg_id = new_current;
        }
        self.refresh_layer_for_current_q0rg();
        self.session.current_frame = 0;
        self.session.timeline_selection = None;
        self.session.selection = Selection::None;
        self.session.tool_state = crate::state::ToolState::Idle;
    }

    fn refresh_layer_for_current_q0rg(&mut self) {
        if let Some(q) = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == self.session.current_q0rg_id)
        {
            self.session.current_layer_id = q.layers.first().map(|l| l.layer_id).unwrap_or(1);
        }
    }

    fn delete_raw_path_refs(&mut self, refs: &[crate::state::PathRef]) -> bool {
        if refs.is_empty() {
            return false;
        }
        self.history.snapshot(&self.state.project);
        let Some(refs) = crate::tools::prepare_raw_path_refs_for_edit(
            &mut self.state.project,
            refs,
            self.session.current_frame,
        ) else {
            return false;
        };

        let mut by_asset: std::collections::BTreeMap<u16, Vec<usize>> =
            std::collections::BTreeMap::new();
        for reference in &refs {
            let asset_id = self
                .state
                .project
                .q0rgs
                .iter()
                .find(|q0rg| q0rg.q0rg_id == reference.q0rg_id)
                .and_then(|q0rg| {
                    q0rg.layers
                        .iter()
                        .find(|layer| layer.layer_id == reference.layer_id)
                })
                .and_then(|layer| layer.placements.get(reference.placement_idx))
                .and_then(|placement| match placement.target {
                    Target::Asset(asset_id) => Some(asset_id),
                    Target::Q0rg(_) => None,
                });
            if let Some(asset_id) = asset_id {
                by_asset
                    .entry(asset_id)
                    .or_default()
                    .push(reference.path_idx);
            }
        }
        if by_asset.is_empty() {
            return false;
        }

        let mut empty_assets = std::collections::BTreeSet::new();
        let mut changed = false;
        for (asset_id, path_indices) in &mut by_asset {
            path_indices.sort_unstable();
            path_indices.dedup();
            path_indices.reverse();
            if let Some(Asset::Vector(vector)) = self
                .state
                .project
                .assets
                .iter_mut()
                .find(|asset| asset.id() == *asset_id)
            {
                for path_idx in path_indices.iter().copied() {
                    if path_idx < vector.paths.len() {
                        vector.paths.remove(path_idx);
                        changed = true;
                    }
                }
                if vector.paths.is_empty() {
                    empty_assets.insert(*asset_id);
                }
            }
        }
        if !changed {
            return false;
        }
        if !empty_assets.is_empty() {
            for q0rg in &mut self.state.project.q0rgs {
                for layer in &mut q0rg.layers {
                    layer.placements.retain(|placement| {
                        !matches!(placement.target, Target::Asset(id) if empty_assets.contains(&id))
                    });
                }
            }
            self.state
                .project
                .assets
                .retain(|asset| !empty_assets.contains(&asset.id()));
        }
        self.state.dirty = true;
        self.session.selection = Selection::None;
        self.textures.invalidate();
        true
    }

    /// Delete the selected timeline cells without removing time itself.
    /// Each selected cell becomes a real blank keyframe, so deleting a held
    /// frame also stops the previous keyframe from leaking through it.
    fn delete_timeline_selection(&mut self) -> Option<(usize, usize, bool)> {
        let selection = self.session.timeline_selection?;
        let q0rg_id = self.session.current_q0rg_id;
        let q0rg = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)?;
        if q0rg.frame_count == 0 {
            return Some((0, 0, false));
        }

        let anchor_layer = q0rg
            .layers
            .iter()
            .position(|layer| layer.layer_id == selection.anchor_layer_id)?;
        let focus_layer = q0rg
            .layers
            .iter()
            .position(|layer| layer.layer_id == selection.focus_layer_id)?;
        let first_layer = anchor_layer.min(focus_layer);
        let last_layer = anchor_layer.max(focus_layer);
        let first_frame = selection
            .anchor_frame
            .min(selection.focus_frame)
            .min(q0rg.frame_count - 1);
        let last_frame = selection
            .anchor_frame
            .max(selection.focus_frame)
            .min(q0rg.frame_count - 1);
        let editable_layer_ids: Vec<u16> = q0rg.layers[first_layer..=last_layer]
            .iter()
            .filter(|layer| !self.state.project.layer_is_folder(q0rg_id, layer.layer_id))
            .map(|layer| layer.layer_id)
            .collect();
        let cell_count = editable_layer_ids.len() * usize::from(last_frame - first_frame + 1);
        if editable_layer_ids.is_empty() {
            self.session.selection = Selection::None;
            self.session.timeline_selection = None;
            return Some((0, 0, false));
        }

        let needs_change = editable_layer_ids.iter().any(|layer_id| {
            let layer = q0rg
                .layers
                .iter()
                .find(|layer| layer.layer_id == *layer_id)
                .expect("selected editable layer still exists");
            (first_frame..=last_frame).any(|frame| !layer.is_blank_keyframe(frame))
                || layer.placements.iter().any(|placement| {
                    matches!(
                        placement.tween,
                        Tween::Linear { to_frame }
                            if (first_frame..=last_frame).contains(&to_frame)
                    )
                })
        });
        if !needs_change {
            self.session.selection = Selection::None;
            return Some((cell_count, 0, false));
        }

        self.history.snapshot(&self.state.project);
        let q0rg = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .expect("selected q0rg disappeared during timeline delete");
        let mut removed_items = 0usize;
        for layer_id in editable_layer_ids {
            let layer = q0rg
                .layers
                .iter_mut()
                .find(|layer| layer.layer_id == layer_id)
                .expect("selected editable layer disappeared during timeline delete");
            let before = layer.placements.len();
            layer
                .placements
                .retain(|placement| !(first_frame..=last_frame).contains(&placement.frame));
            removed_items += before - layer.placements.len();

            for placement in &mut layer.placements {
                if matches!(
                    placement.tween,
                    Tween::Linear { to_frame }
                        if (first_frame..=last_frame).contains(&to_frame)
                ) {
                    placement.tween = Tween::None;
                }
            }
            for frame in first_frame..=last_frame {
                layer.ensure_explicit_keyframe(frame);
            }
        }

        self.state.dirty = true;
        self.session.selection = Selection::None;
        Some((cell_count, removed_items, true))
    }

    fn delete_selection(&mut self) -> bool {
        self.session
            .clear_inactive_frame_selection(&self.state.project);
        let selection = self.session.selection.clone();
        match selection {
            Selection::Placement {
                q0rg_id,
                layer_id,
                placement_idx,
            } => {
                self.history.snapshot(&self.state.project);
                let Some(mapped_idx) = crate::tools::materialize_placement_keyframe_for_edit(
                    &mut self.state.project,
                    q0rg_id,
                    layer_id,
                    placement_idx,
                    self.session.current_frame,
                ) else {
                    return false;
                };
                if let Some(layer) = self
                    .state
                    .project
                    .q0rgs
                    .iter_mut()
                    .find(|q0rg| q0rg.q0rg_id == q0rg_id)
                    .and_then(|q0rg| {
                        q0rg.layers
                            .iter_mut()
                            .find(|layer| layer.layer_id == layer_id)
                    })
                {
                    if mapped_idx < layer.placements.len() {
                        layer.placements.remove(mapped_idx);
                        self.state.dirty = true;
                        self.session.selection = Selection::None;
                        return true;
                    }
                }
                false
            }
            Selection::Path {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            } => self.delete_raw_path_refs(&[crate::state::PathRef {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            }]),
            Selection::Paths(refs) => self.delete_raw_path_refs(&refs),
            Selection::PathPoints {
                path,
                mut anchor_indices,
                ..
            } => {
                anchor_indices.sort_unstable();
                anchor_indices.dedup();
                let anchor_count = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == path.q0rg_id)
                    .and_then(|q0rg| {
                        q0rg.layers
                            .iter()
                            .find(|layer| layer.layer_id == path.layer_id)
                    })
                    .and_then(|layer| layer.placements.get(path.placement_idx))
                    .and_then(|placement| match placement.target {
                        Target::Asset(asset_id) => self
                            .state
                            .project
                            .assets
                            .iter()
                            .find(|asset| asset.id() == asset_id),
                        Target::Q0rg(_) => None,
                    })
                    .and_then(|asset| match asset {
                        Asset::Vector(vector) => vector.paths.get(path.path_idx),
                        Asset::Bitmap(_) | Asset::Q0v(_) => None,
                    })
                    .map(|raw_path| raw_path.anchors.len())
                    .unwrap_or(0);
                if anchor_count.saturating_sub(anchor_indices.len()) < 3 {
                    return self.delete_raw_path_refs(&[path]);
                }

                self.history.snapshot(&self.state.project);
                let Some(mut mapped) = crate::tools::prepare_raw_path_refs_for_edit(
                    &mut self.state.project,
                    &[path],
                    self.session.current_frame,
                ) else {
                    return false;
                };
                let path = mapped.remove(0);
                let asset_id = self
                    .state
                    .project
                    .q0rgs
                    .iter()
                    .find(|q0rg| q0rg.q0rg_id == path.q0rg_id)
                    .and_then(|q0rg| {
                        q0rg.layers
                            .iter()
                            .find(|layer| layer.layer_id == path.layer_id)
                    })
                    .and_then(|layer| layer.placements.get(path.placement_idx))
                    .and_then(|placement| match placement.target {
                        Target::Asset(asset_id) => Some(asset_id),
                        Target::Q0rg(_) => None,
                    });
                let Some(asset_id) = asset_id else {
                    return false;
                };
                let Some(Asset::Vector(vector)) = self
                    .state
                    .project
                    .assets
                    .iter_mut()
                    .find(|asset| asset.id() == asset_id)
                else {
                    return false;
                };
                let Some(raw_path) = vector.paths.get_mut(path.path_idx) else {
                    return false;
                };
                for index in anchor_indices.into_iter().rev() {
                    if index < raw_path.anchors.len() {
                        raw_path.anchors.remove(index);
                    }
                }
                self.state.dirty = true;
                self.session.selection = Selection::None;
                true
            }
            Selection::RawArea {
                placements,
                objects,
                bounds_min,
                bounds_max,
            } => {
                self.history.snapshot(&self.state.project);
                let changed = crate::selection_edit::remove_raw_area_and_objects(
                    &mut self.state.project,
                    &placements,
                    &objects,
                    bounds_min,
                    bounds_max,
                    self.session.current_frame,
                );
                if changed {
                    self.state.dirty = true;
                    self.session.selection = Selection::None;
                    self.textures.invalidate();
                }
                changed
            }
            Selection::Asset(asset_id) => {
                self.history.snapshot(&self.state.project);
                self.state.project.assets.retain(|a| a.id() != asset_id);
                self.state.project.asset_names.remove(&asset_id);
                for q in &mut self.state.project.q0rgs {
                    for layer in &mut q.layers {
                        layer.placements.retain(|p| match p.target {
                            q0s_format::v2::Target::Asset(id) => id != asset_id,
                            _ => true,
                        });
                    }
                }
                self.state.dirty = true;
                self.session.selection = Selection::None;
                self.textures.invalidate();
                true
            }
            Selection::Multi(refs) => {
                if refs.is_empty() {
                    return false;
                }
                self.history.snapshot(&self.state.project);
                // Sort refs by (q0rg_id, layer_id, placement_idx desc) so we can
                // remove from the back of each layer without shifting earlier
                // indices.
                let mut sorted = refs.clone();
                sorted.sort_by(|a, b| {
                    a.q0rg_id
                        .cmp(&b.q0rg_id)
                        .then(a.layer_id.cmp(&b.layer_id))
                        .then(b.placement_idx.cmp(&a.placement_idx))
                });
                for r in sorted {
                    if let Some(q) = self
                        .state
                        .project
                        .q0rgs
                        .iter_mut()
                        .find(|q| q.q0rg_id == r.q0rg_id)
                    {
                        if let Some(layer) = q.layers.iter_mut().find(|l| l.layer_id == r.layer_id)
                        {
                            if r.placement_idx < layer.placements.len() {
                                layer.placements.remove(r.placement_idx);
                            }
                        }
                    }
                }
                self.state.dirty = true;
                self.session.selection = Selection::None;
                true
            }
            Selection::Q0rg(_) | Selection::None => false,
        }
    }

    /// Step `current_frame` forward one slot, extending the q0rg's
    /// `frame_count` when we'd land off the end. Returns whether the
    /// frame_count was bumped Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС› caller usually folds that into the
    /// status message ("frame inserted" vs. "moved to frame N").
    /// Snapshots history exactly when the count changes so undo gives
    /// the right granularity.
    fn advance_playhead_extending(&mut self) -> bool {
        let q0rg_id = self.session.current_q0rg_id;
        let next_frame = self.session.current_frame.saturating_add(1);
        let mut extended = false;
        let needs_extend = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .map(|q| next_frame >= q.frame_count)
            .unwrap_or(false);
        if needs_extend {
            self.history.snapshot(&self.state.project);
            if let Some(q) = self
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q| q.q0rg_id == q0rg_id)
            {
                q.frame_count = q.frame_count.saturating_add(1);
                extended = true;
            }
        }
        let last_frame = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .map(|q| q.frame_count.saturating_sub(1))
            .unwrap_or(0);
        self.session.current_frame = next_frame.min(last_frame);
        if extended {
            self.state.dirty = true;
        }
        extended
    }

    fn go_to_frame(&mut self, requested: u16) {
        let last_frame = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == self.session.current_q0rg_id)
            .map(|q| q.frame_count.saturating_sub(1))
            .unwrap_or(0);
        self.session.current_frame = requested.min(last_frame);
        self.session.status = format!("frame {}", self.session.current_frame + 1);
    }

    /// F6 / Insert Keyframe with Flash-like playhead semantics.
    ///
    /// On a held frame, bake the visible state at the current playhead and
    /// stay there. On an existing keyframe, create the next keyframe to the
    /// right and move the playhead onto it.
    fn insert_keyframe_smart(&mut self) {
        let current = self.session.current_frame;
        let has_exact_key = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == self.session.current_q0rg_id)
            .and_then(|q| {
                q.layers
                    .iter()
                    .find(|layer| layer.layer_id == self.session.current_layer_id)
            })
            .map(|layer| layer.has_keyframe(current))
            .unwrap_or(false);
        let target = if has_exact_key {
            current.saturating_add(1)
        } else {
            current
        };
        let inserted = self.insert_keyframe_at(target);
        let target_exists = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == self.session.current_q0rg_id)
            .map(|q| target < q.frame_count)
            .unwrap_or(false);
        if has_exact_key && (inserted > 0 || target_exists) {
            self.session.current_frame = target;
        }
    }

    /// F7 / Insert Blank Keyframe. On an existing keyframe, create the next
    /// blank key to the right; on a held frame, blank the current playhead.
    fn insert_blank_keyframe_smart(&mut self) {
        let current = self.session.current_frame;
        let has_exact_key = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == self.session.current_q0rg_id)
            .and_then(|q| {
                q.layers
                    .iter()
                    .find(|layer| layer.layer_id == self.session.current_layer_id)
            })
            .map(|layer| layer.has_keyframe(current))
            .unwrap_or(false);
        let target = if has_exact_key {
            current.saturating_add(1)
        } else {
            current
        };
        let inserted = self.insert_blank_keyframe_at(target);
        let target_exists = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == self.session.current_q0rg_id)
            .map(|q| target < q.frame_count)
            .unwrap_or(false);
        if has_exact_key && (inserted || target_exists) {
            self.session.current_frame = target;
        }
    }

    fn insert_blank_keyframe_at(&mut self, frame: u16) -> bool {
        let q0rg_id = self.session.current_q0rg_id;
        let layer_id = self.session.current_layer_id;
        let has_keyframe = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter().find(|layer| layer.layer_id == layer_id))
            .map(|layer| layer.has_keyframe(frame));
        let Some(has_keyframe) = has_keyframe else {
            self.session.status = "blank keyframe: current layer not found".to_string();
            return false;
        };
        if has_keyframe {
            self.session.status = format!("keyframe already exists at frame {}", frame + 1);
            return false;
        }

        self.history.snapshot(&self.state.project);
        let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
        else {
            return false;
        };
        if frame >= q0rg.frame_count {
            q0rg.frame_count = frame.saturating_add(1);
        }
        let Some(layer) = q0rg
            .layers
            .iter_mut()
            .find(|layer| layer.layer_id == layer_id)
        else {
            return false;
        };
        layer.ensure_explicit_keyframe(frame);
        self.state.dirty = true;
        self.session.selection = Selection::None;
        self.session.status = format!("inserted blank keyframe at frame {}", frame + 1);
        true
    }
    /// Capture every placement active on the current layer at `frame` as a
    /// real keyframe. Interpolated transforms are baked and the new entries
    /// stay next to their sources so z-order does not jump.
    fn insert_keyframe_at(&mut self, frame: u16) -> usize {
        let q0rg_id = self.session.current_q0rg_id;
        let layer_id = self.session.current_layer_id;
        let active: Vec<(usize, q0s_format::v2::Transform2D)> = match self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
        {
            Some(layer) => crate::render::active_placements_at(layer, frame),
            None => Vec::new(),
        };
        if active.is_empty() {
            return usize::from(self.insert_blank_keyframe_at(frame));
        }

        let insertions: Vec<(usize, Placement)> = {
            let layer = self
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == q0rg_id)
                .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id));
            let Some(layer) = layer else { return 0 };
            active
                .into_iter()
                .filter_map(|(idx, interp)| {
                    let src = layer.placements.get(idx)?;
                    (src.frame != frame).then_some((
                        idx,
                        Placement {
                            frame,
                            target: src.target,
                            transform: interp,
                            tween: Tween::None,
                        },
                    ))
                })
                .collect()
        };
        if insertions.is_empty() {
            self.session.status = format!("keyframes already exist at frame {}", frame + 1);
            return 0;
        }

        self.history.snapshot(&self.state.project);
        if let Some(q0rg) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
        {
            if frame >= q0rg.frame_count {
                q0rg.frame_count = frame.saturating_add(1);
            }
            if let Some(layer) = q0rg.layers.iter_mut().find(|l| l.layer_id == layer_id) {
                for (idx, placement) in insertions.iter().rev() {
                    let insert_at = idx.saturating_add(1).min(layer.placements.len());
                    layer.placements.insert(insert_at, placement.clone());
                }
            }
        }
        let new_count = insertions.len();
        self.state.dirty = true;
        self.session.selection = Selection::None;
        self.session.status = format!("inserted {new_count} keyframe(s) at frame {}", frame + 1);
        new_count
    }

    /// Clear every object from the current layer/frame while keeping a real
    /// blank keyframe there. This also works on a held frame by inserting the
    /// blank key that stops the previous contents.
    fn clear_keyframe(&mut self) {
        self.session.timeline_selection = Some(crate::state::TimelineSelection::single(
            self.session.current_layer_id,
            self.session.current_frame,
        ));
        match self.delete_timeline_selection() {
            Some((_, _, false)) => {
                self.session.status = format!(
                    "frame {} is already a blank keyframe",
                    self.session.current_frame + 1
                );
            }
            Some((_, removed, true)) if removed > 0 => {
                self.session.status = format!(
                    "cleared frame {} ({removed} item(s))",
                    self.session.current_frame + 1
                );
            }
            Some((_, _, true)) => {
                self.session.status = format!("cleared frame {}", self.session.current_frame + 1);
            }
            None => {
                self.session.status = "clear keyframe: current layer not found".to_string();
            }
        }
    }

    /// Decrease frame_count by 1 if possible. All placements / tweens beyond
    /// the new boundary are clipped: placements past it are removed; tweens
    /// that pointed past it become Tween::None.
    fn remove_frame(&mut self) {
        let q0rg_id = self.session.current_q0rg_id;
        let new_total = match self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == q0rg_id)
            .map(|q| q.frame_count)
        {
            Some(n) if n > 1 => n - 1,
            _ => {
                self.session.status = "can't remove frame from a 1-frame q0rg".to_string();
                return;
            }
        };

        self.history.snapshot(&self.state.project);
        if let Some(q) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
        {
            q.frame_count = new_total;
            for layer in &mut q.layers {
                layer.explicit_keyframes.retain(|frame| *frame < new_total);
                layer.placements.retain(|p| p.frame < new_total);
                for p in &mut layer.placements {
                    if let Tween::Linear { to_frame } = p.tween {
                        if to_frame >= new_total {
                            p.tween = Tween::None;
                        }
                    }
                }
            }
            self.state.dirty = true;
            self.session.current_frame = self.session.current_frame.min(new_total - 1);
            self.session.status = format!("frame removed, {new_total} total");
        }
    }

    /// Tween toggling on the selected placement.
    /// * Linear Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р Р†РІР‚С›РЎС›Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†Р вЂљРЎвЂќР В Р’В Р В Р вЂ№Р В Р Р‹Р Р†Р вЂљРЎвЂќ None (removes the tween in one click).
    /// * None + a later same-target keyframe exists Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р Р†РІР‚С›РЎС›Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†Р вЂљРЎвЂќР В Р’В Р В Р вЂ№Р В Р Р‹Р Р†Р вЂљРЎвЂќ Linear pointing at it.
    /// * None + no later keyframe Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р Р†РІР‚С›РЎС›Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†Р вЂљРЎвЂќР В Р’В Р В Р вЂ№Р В Р Р‹Р Р†Р вЂљРЎвЂќ **auto-create one** so a fresh tween
    ///   "just works" without the user manually scrubbing forward and
    ///   pressing F6. The new keyframe lands at `playhead` if the
    ///   playhead is past the selected frame, otherwise 10 frames after
    ///   the selected one (extending `frame_count` if necessary). Then
    ///   we set Linear to that frame and reselect the *new* keyframe so
    ///   dragging the object immediately animates.
    fn toggle_motion_tween(&mut self) {
        let Selection::Placement {
            q0rg_id,
            layer_id,
            placement_idx,
        } = self.session.selection.clone()
        else {
            self.session.status = "tween: select a placement first".to_string();
            return;
        };

        // Read-only snapshot of what we need.
        let (cur_tween, cur_frame, cur_target, cur_transform, next_frame) = {
            let Some(layer) = self
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == q0rg_id)
                .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
            else {
                return;
            };
            let Some(p) = layer.placements.get(placement_idx) else {
                return;
            };
            let next = layer
                .placements
                .iter()
                .filter(|n| n.target == p.target && n.frame > p.frame)
                .map(|n| n.frame)
                .min();
            (p.tween, p.frame, p.target, p.transform, next)
        };

        // Already linear Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р Р†РІР‚С›РЎС›Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†Р вЂљРЎвЂќР В Р’В Р В Р вЂ№Р В Р Р‹Р Р†Р вЂљРЎвЂќ just clear it.
        if matches!(cur_tween, Tween::Linear { .. }) {
            self.history.snapshot(&self.state.project);
            if let Some(p) = self
                .state
                .project
                .q0rgs
                .iter_mut()
                .find(|q| q.q0rg_id == q0rg_id)
                .and_then(|q| q.layers.iter_mut().find(|l| l.layer_id == layer_id))
                .and_then(|l| l.placements.get_mut(placement_idx))
            {
                p.tween = Tween::None;
                self.state.dirty = true;
                self.session.status = "tween removed".to_string();
            }
            return;
        }

        // From here we're going from None Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р Р†РІР‚С›РЎС›Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†Р вЂљРЎвЂќР В Р’В Р В Р вЂ№Р В Р Р‹Р Р†Р вЂљРЎвЂќ Linear. Decide where the
        // second keyframe sits (existing or auto-create).
        self.history.snapshot(&self.state.project);
        let target_frame: u16 = match next_frame {
            Some(f) => f,
            None => {
                let playhead = self.session.current_frame;
                let candidate = if playhead > cur_frame {
                    playhead
                } else {
                    cur_frame.saturating_add(10)
                };
                // Extend frame_count if `candidate` lies beyond the q0rg.
                if let Some(q) = self
                    .state
                    .project
                    .q0rgs
                    .iter_mut()
                    .find(|q| q.q0rg_id == q0rg_id)
                {
                    if candidate >= q.frame_count {
                        q.frame_count = candidate.saturating_add(1);
                    }
                }
                // Insert a fresh same-target keyframe at `candidate`.
                if let Some(layer) = self
                    .state
                    .project
                    .q0rgs
                    .iter_mut()
                    .find(|q| q.q0rg_id == q0rg_id)
                    .and_then(|q| q.layers.iter_mut().find(|l| l.layer_id == layer_id))
                {
                    layer.placements.push(Placement {
                        frame: candidate,
                        target: cur_target,
                        transform: cur_transform,
                        tween: Tween::None,
                    });
                }
                candidate
            }
        };

        // Set Linear on the source placement.
        if let Some(p) = self
            .state
            .project
            .q0rgs
            .iter_mut()
            .find(|q| q.q0rg_id == q0rg_id)
            .and_then(|q| q.layers.iter_mut().find(|l| l.layer_id == layer_id))
            .and_then(|l| l.placements.get_mut(placement_idx))
        {
            p.tween = Tween::Linear {
                to_frame: target_frame,
            };
        }

        // Move playhead to the new keyframe and reselect it so the user
        // can immediately drag/resize the destination pose.
        if next_frame.is_none() {
            self.session.current_frame = target_frame;
            // Find the index of the newly-created keyframe (last placement
            // matching frame == target_frame and target == cur_target).
            if let Some(layer) = self
                .state
                .project
                .q0rgs
                .iter()
                .find(|q| q.q0rg_id == q0rg_id)
                .and_then(|q| q.layers.iter().find(|l| l.layer_id == layer_id))
            {
                if let Some((idx, _)) = layer
                    .placements
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, p)| p.frame == target_frame && p.target == cur_target)
                {
                    self.session.selection = Selection::Placement {
                        q0rg_id,
                        layer_id,
                        placement_idx: idx,
                    };
                }
            }
        }

        self.state.dirty = true;
        self.session.status = if next_frame.is_some() {
            format!("tween to frame {}", target_frame + 1)
        } else {
            format!(
                "tween to new keyframe at frame {} (drag to set the end pose)",
                target_frame + 1
            )
        };
    }

    fn render_credits_dialog(&mut self, ctx: &Context) {
        let mut open = self.session.show_credits;
        egui::Window::new("Credits")
            .open(&mut open)
            .default_size([720.0, 620.0])
            .min_size([500.0, 440.0])
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_source("q0editor_credits_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        q0credits::show(
                            ui,
                            &q0credits::Palette {
                                accent: self.settings.theme.accent.to_color32(),
                                panel: self.settings.theme.panel.to_color32(),
                                window: self.settings.theme.window.to_color32(),
                                deep_bg: self.settings.theme.deep_bg.to_color32(),
                                text: self.settings.theme.text.to_color32(),
                                text_dim: self.settings.theme.text_dim.to_color32(),
                            },
                            self.session.credits_opened_at.elapsed().as_secs_f32(),
                        );
                    });
            });
        self.session.show_credits = open;
    }

    /// Settings dialog Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р Р†Р вЂљРІвЂћСћР В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р’В Р Р†Р вЂљР’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р РЋРІвЂћСћР В Р’В Р В Р вЂ№Р В Р вЂ Р Р†Р вЂљРЎвЂєР РЋРЎвЂєР В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В Р вЂ Р В РІР‚С™Р вЂ™Р’В Р В Р’В Р вЂ™Р’В Р В РІР‚в„ўР вЂ™Р’В Р В Р’В Р В РІР‚В Р В Р’В Р Р†Р вЂљРЎв„ўР В Р Р‹Р Р†РІР‚С›РЎС›Р В Р’В Р вЂ™Р’В Р В Р’В Р В РІР‚в„–Р В Р’В Р В Р вЂ№Р В Р Р‹Р Р†РІР‚С›РЎС› theme presets, custom colour pickers, brush
    /// cap, .q7s save/load. Mirrors the player's dialog so the two
    /// apps feel symmetric.
    fn render_settings_dialog(&mut self, ctx: &Context) {
        use crate::settings::Theme;

        let mut open = true;
        let mut close_requested = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .default_size([460.0, 540.0])
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                ui.heading("Theme presets");
                ui.horizontal_wrapped(|ui| {
                    for preset in Theme::PRESETS {
                        if ui.button(preset.name).clicked() {
                            self.settings.theme = Theme::from_builtin(preset);
                            self.settings.theme.apply(ctx);
                            self.settings.save();
                            self.session.status = format!("theme: {}", preset.name);
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("Save as... (.q7s)")
                        .on_hover_text("Persist current colours as a reusable .q7s theme")
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("q7s theme", &["q7s"])
                            .set_file_name("my-theme.q7s")
                            .save_file()
                        {
                            let name = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("Custom")
                                .to_string();
                            match self.settings.theme.write_q7s(&path, &name) {
                                Ok(()) => {
                                    self.session.status =
                                        format!("saved theme: {}", path.display())
                                }
                                Err(e) => {
                                    self.session.status = format!("save theme failed: {e}")
                                }
                            }
                        }
                    }
                    if ui
                        .button("Load... (.q7s)")
                        .on_hover_text("Apply a previously saved .q7s theme (works with q0player .q7s files too)")
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("q7s theme", &["q7s"])
                            .pick_file()
                        {
                            match Theme::read_q7s(&path) {
                                Ok((name, theme)) => {
                                    self.settings.theme = theme;
                                    self.settings.theme.apply(ctx);
                                    self.settings.save();
                                    self.session.status = format!("theme: {name}");
                                }
                                Err(e) => {
                                    self.session.status = format!("load theme failed: {e}")
                                }
                            }
                        }
                    }
                });
                ui.separator();

                let mut t = self.settings.theme.clone();
                egui::CollapsingHeader::new("Base palette")
                    .default_open(true)
                    .show(ui, |ui| {
                        egui::Grid::new("editor_theme_base")
                            .num_columns(2)
                            .spacing([10.0, 6.0])
                            .show(ui, |ui| {
                                color_row(ui, "Accent", &mut t.accent);
                                color_row(ui, "Panel", &mut t.panel);
                                color_row(ui, "Window", &mut t.window);
                                color_row(ui, "Inputs background", &mut t.deep_bg);
                                color_row(ui, "Text", &mut t.text);
                                color_row(ui, "Text dim", &mut t.text_dim);
                                color_row(ui, "Window stroke", &mut t.stroke_dark);
                            });
                    });
                egui::CollapsingHeader::new("Stage & canvas")
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(
                                "The stage paper stays white. This is the drawing canvas.",
                            )
                            .small()
                            .color(self.settings.theme.text_dim.to_color32()),
                        );
                        egui::Grid::new("editor_theme_stage")
                            .num_columns(2)
                            .spacing([10.0, 6.0])
                            .show(ui, |ui| {
                                color_row(ui, "Canvas (outside stage)", &mut t.canvas_bg);
                                color_row(ui, "Stage border", &mut t.stage_border);
                                color_row(ui, "Symbol edit mask", &mut t.symbol_edit_mask);
                                color_row(
                                    ui,
                                    "Library preview (light)",
                                    &mut t.library_preview_light,
                                );
                                color_row(
                                    ui,
                                    "Library preview (dark)",
                                    &mut t.library_preview_dark,
                                );
                            });
                    });
                egui::CollapsingHeader::new("Timeline")
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::Grid::new("editor_theme_timeline")
                            .num_columns(2)
                            .spacing([10.0, 6.0])
                            .show(ui, |ui| {
                                color_row(ui, "Header strip", &mut t.timeline_header);
                                color_row(ui, "Grid", &mut t.timeline_grid);
                                color_row(ui, "Grid (every 5)", &mut t.timeline_grid_5);
                                color_row(ui, "Playhead", &mut t.playhead);
                                color_row(ui, "Keyframe dot", &mut t.keyframe);
                                color_row(ui, "Span fill", &mut t.extension);
                                color_row(ui, "Span fill (active layer)", &mut t.extension_active);
                                color_row(ui, "Tween fill", &mut t.tween_fill);
                                color_row(ui, "Tween arrow", &mut t.tween_arrow);
                                color_row(ui, "Empty cell (inside)", &mut t.empty_inside);
                                color_row(
                                    ui,
                                    "Empty cell (active layer)",
                                    &mut t.empty_inside_active,
                                );
                                color_row(ui, "Beyond cell", &mut t.empty_beyond);
                                color_row(ui, "Beyond cell (every 5)", &mut t.empty_beyond_5);
                            });
                    });
                egui::CollapsingHeader::new("Onion-skin tints")
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::Grid::new("editor_theme_onion")
                            .num_columns(2)
                            .spacing([10.0, 6.0])
                            .show(ui, |ui| {
                                color_row(ui, "Past frames", &mut t.onion_past);
                                color_row(ui, "Future frames", &mut t.onion_future);
                            });
                    });
                egui::CollapsingHeader::new("q0lang syntax")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(
                                "Used by the script editor (View > Script Editor, F9).",
                            )
                            .small()
                            .color(self.settings.theme.text_dim.to_color32()),
                        );
                        egui::Grid::new("editor_theme_syntax")
                            .num_columns(2)
                            .spacing([10.0, 6.0])
                            .show(ui, |ui| {
                                color_row(ui, "Keyword", &mut t.syntax_keyword);
                                color_row(ui, "Built-in / library", &mut t.syntax_builtin);
                                color_row(ui, "String", &mut t.syntax_string);
                                color_row(ui, "Number", &mut t.syntax_number);
                                color_row(ui, "Comment", &mut t.syntax_comment);
                                color_row(ui, "Operator", &mut t.syntax_operator);
                                color_row(ui, "Identifier", &mut t.syntax_identifier);
                                color_row(ui, "Signal (X!)", &mut t.syntax_signal);
                                color_row(ui, "Gutter background", &mut t.syntax_gutter_bg);
                                color_row(ui, "Gutter text", &mut t.syntax_gutter_fg);
                            });
                    });
                if t != self.settings.theme {
                    self.settings.theme = t;
                    self.settings.theme.apply(ctx);
                    self.settings.save();
                }
                ui.separator();

                #[cfg(windows)]
                {
                    ui.heading("File association");
                    let association_state = match crate::assoc::is_registered_for_current_user() {
                        Ok(true) => "active for this q0editor",
                        Ok(false) => "not active",
                        Err(_) => "status unavailable",
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            ".q1s project files: {association_state}. This affects the current Windows user only."
                        ))
                        .small()
                        .color(self.settings.theme.text_dim.to_color32()),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Register .q1s").clicked() {
                            match crate::assoc::register_for_current_user() {
                                Ok(()) => {
                                    self.session.status =
                                        ".q1s files now open with q0editor".to_string()
                                }
                                Err(error) => {
                                    self.session.status =
                                        format!("association failed: {error}")
                                }
                            }
                        }
                        if ui.button("Remove .q1s association").clicked() {
                            match crate::assoc::unregister_for_current_user() {
                                Ok(()) => {
                                    self.session.status =
                                        ".q1s association removed".to_string()
                                }
                                Err(error) => {
                                    self.session.status = format!("removal failed: {error}")
                                }
                            }
                        }
                    });
                    ui.separator();
                }

                // Brush options live in the Properties panel (Stage
                // section when the Brush tool is active, plus per-asset
                // when a stroked shape is selected).

                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui.button("Close").clicked() {
                            close_requested = true;
                        }
                    },
                );
                    });
            });
        if !open || close_requested {
            self.session.show_settings = false;
        }
    }

    fn save_to(&mut self, path: &std::path::Path) -> bool {
        if self.another_project_tab_uses_path(path) {
            self.session.status =
                "save failed: this q1s file is already open in another tab".to_string();
            return false;
        }
        match file_io::save_project(path, &self.state.project) {
            Ok(_) => {
                self.state.dirty = false;
                self.state.file_path = Some(path.to_path_buf());
                self.settings.push_recent(path);
                self.session.status = match self.settings.try_save() {
                    Ok(()) => format!("saved {}", path.display()),
                    Err(error) => format!(
                        "saved {}; recent list was not saved: {error}",
                        path.display()
                    ),
                };
                true
            }
            Err(err) => {
                self.session.status = format!("save failed: {err}");
                false
            }
        }
    }

    fn open_from_path(&mut self, path: &std::path::Path) -> bool {
        if let Some(tab_id) = self.find_project_tab_by_path(path) {
            self.switch_to_project_tab(tab_id);
            self.session.status = format!("already open: {}", path.display());
            return true;
        }
        match file_io::load_project(path) {
            Ok(project) => {
                let workspace = self.workspace_for_project(project, Some(path.to_path_buf()));
                if !self.activate_new_workspace(workspace) {
                    return false;
                }
                self.new_project_dialog = None;
                self.settings.push_recent(path);
                self.session.status = match self.settings.try_save() {
                    Ok(()) => format!("opened {}", path.display()),
                    Err(error) => format!(
                        "opened {}; recent list was not saved: {error}",
                        path.display()
                    ),
                };
                true
            }
            Err(err) => {
                self.session.status = format!("open failed: {err}");
                false
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &Context) {
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        let Some(file) = dropped.first() else { return };
        let Some(path) = file.path.as_ref() else {
            self.session.status = "drop a .q1s project file from Explorer".to_string();
            return;
        };
        let is_project = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(file_io::Q1S_EXTENSION));
        if is_project {
            self.queue(Action::OpenProjectFromPath(path.clone()));
        } else if self.show_home {
            self.session.status = "open or create a project before importing media".to_string();
        } else if crate::bitmap_import::is_supported_bitmap_path(path)
            || file_io::is_supported_video_import_path(path)
        {
            self.queue(Action::ImportMediaFromPath(path.clone()));
        } else {
            self.session.status = format!(
                "unsupported dropped file: {} (expected .q1s, png, jpg, webp, mp4 or q0v)",
                path.display()
            );
        }
    }

    fn save_current_project(&mut self) -> bool {
        if let Some(path) = self.state.file_path.clone() {
            return self.save_to(&path);
        }

        let Some(path) = file_io::pick_save_path(None) else {
            return false;
        };
        self.save_to(&path)
    }

    fn render_new_project_dialog(&mut self, ctx: &Context) {
        let Some(mut draft) = self.new_project_dialog.clone() else {
            return;
        };

        let mut open = true;
        let mut create = false;
        let mut cancel = false;
        egui::Window::new("Create project")
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(430.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Project settings")
                        .strong()
                        .size(18.0),
                );
                ui.label(
                    egui::RichText::new(
                        "Choose the stage, timing and initial timeline before the project is created.",
                    )
                    .color(self.settings.theme.text_dim.to_color32()),
                );
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add_sized(
                        [330.0, 24.0],
                        egui::TextEdit::singleline(&mut draft.name).hint_text("untitled"),
                    );
                });

                ui.add_space(8.0);
                ui.label("Stage preset");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Classic 640x480").clicked() {
                        draft.width = 640;
                        draft.height = 480;
                    }
                    if ui.button("HD 1280x720").clicked() {
                        draft.width = 1280;
                        draft.height = 720;
                    }
                    if ui.button("Full HD 1920x1080").clicked() {
                        draft.width = 1920;
                        draft.height = 1080;
                    }
                    if ui.button("Portrait 1080x1920").clicked() {
                        draft.width = 1080;
                        draft.height = 1920;
                    }
                });

                ui.add_space(6.0);
                egui::Grid::new("new_project_settings_grid")
                    .num_columns(2)
                    .spacing([18.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Width");
                        ui.add(
                            egui::DragValue::new(&mut draft.width)
                                .clamp_range(1..=8192)
                                .suffix(" px"),
                        );
                        ui.end_row();

                        ui.label("Height");
                        ui.add(
                            egui::DragValue::new(&mut draft.height)
                                .clamp_range(1..=8192)
                                .suffix(" px"),
                        );
                        ui.end_row();

                        ui.label("Frame rate");
                        ui.add(
                            egui::DragValue::new(&mut draft.fps)
                                .clamp_range(1..=240)
                                .suffix(" fps"),
                        );
                        ui.end_row();

                        ui.label("Initial timeline");
                        ui.add(
                            egui::DragValue::new(&mut draft.frame_count)
                                .clamp_range(1..=u16::MAX)
                                .suffix(" frames"),
                        );
                        ui.end_row();
                    });

                let duration = f32::from(draft.frame_count) / f32::from(draft.fps.max(1));
                ui.label(
                    egui::RichText::new(format!(
                        "{}x{} stage - {:.2} seconds",
                        draft.width, draft.height, duration
                    ))
                    .color(self.settings.theme.text_dim.to_color32())
                    .small(),
                );
                ui.add_space(12.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Create project").clicked() {
                        create = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if create {
            self.new_project_dialog = None;
            self.queue(Action::CreateProject(draft));
        } else if cancel || !open {
            self.new_project_dialog = None;
        } else {
            self.new_project_dialog = Some(draft);
        }
    }
    fn render_unsaved_changes_dialog(&mut self, ctx: &Context) {
        let Some(action) = self.unsaved_action.as_ref() else {
            return;
        };

        let destination = match action {
            Action::CloseProjectTab(_) => "close this project tab",
            Action::Exit => "close q0editor",
            _ => "continue",
        };

        let mut save = false;
        let mut discard = false;
        let mut cancel = false;
        egui::Window::new("Unsaved changes")
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Save your changes before you {destination}?"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        save = true;
                    }
                    if ui.button("Discard").clicked() {
                        discard = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if save {
            if self.save_current_project() {
                if let Some(action) = self.unsaved_action.take() {
                    self.handle(ctx, action);
                }
            }
        } else if discard {
            if let Some(action) = self.unsaved_action.take() {
                match action {
                    Action::CloseProjectTab(_) | Action::Exit => {
                        self.state.dirty = false;
                        self.handle(ctx, action);
                    }
                    _ => {}
                }
            }
        } else if cancel {
            self.unsaved_action = None;
        }
    }

    fn render_destructive_change_dialogs(&mut self, ctx: &Context) {
        if let Some((asset_id, references)) = self.pending_asset_delete {
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Delete used asset?")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Asset {asset_id} is used by {references} placement(s)."
                    ));
                    ui.label("Deleting it also removes every one of those placements.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete asset and placements").clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if confirm {
                self.pending_asset_delete = None;
                self.session.selection = Selection::Asset(asset_id);
                if self.delete_selection() {
                    self.session.status =
                        format!("deleted asset {asset_id} and {references} placement(s)");
                }
            } else if cancel {
                self.pending_asset_delete = None;
                self.session.status = "asset deletion cancelled".to_string();
            }
        }

        if let Some((q0rg_id, references)) = self.pending_q0rg_delete {
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Delete used symbol?")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Symbol {q0rg_id} is used by {references} placement(s)."
                    ));
                    ui.label("Deleting it also removes every one of those placements.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete symbol and placements").clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if confirm {
                self.pending_q0rg_delete = None;
                self.apply_q0rg_delete(q0rg_id);
            } else if cancel {
                self.pending_q0rg_delete = None;
                self.session.status = "symbol deletion cancelled".to_string();
            }
        }

        if let Some((q0rg_id, layer_id, layer_count, item_count)) = self.pending_layer_delete {
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Delete layer?")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Delete {layer_count} layer row(s) and {item_count} timeline item(s)?"
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete layer").clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if confirm {
                self.pending_layer_delete = None;
                self.apply_layer_delete(q0rg_id, layer_id);
            } else if cancel {
                self.pending_layer_delete = None;
                self.session.status = "layer deletion cancelled".to_string();
            }
        }

        if let Some((q0rg_id, frames, keyframes, tweens)) = self.pending_frame_truncate {
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Shorten symbol?")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("Reduce the symbol to {frames} frame(s)?"));
                    ui.label(format!(
                        "This removes {keyframes} keyframe(s) and {tweens} tween(s)."
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Shorten symbol").clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if confirm {
                self.pending_frame_truncate = None;
                self.apply_frame_count(q0rg_id, frames);
            } else if cancel {
                self.pending_frame_truncate = None;
                self.session.status = "symbol length unchanged".to_string();
            }
        }
    }

    fn tick_playback(&mut self) {
        if !self.session.playing {
            self.session.last_tick = Instant::now();
            return;
        }
        let now = Instant::now();
        let dt = (now - self.session.last_tick).as_secs_f32();
        let fps = self.state.project.meta.fps.max(1) as f32;
        let frames = (dt * fps) as i32;
        if frames < 1 {
            return;
        }
        // Keep the sub-frame remainder instead of resetting to `now`.
        // Otherwise frequent repaints systematically lose time and make
        // playback run slower than the project's FPS.
        self.session.last_tick += Duration::from_secs_f32(frames as f32 / fps);
        let total = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q| q.q0rg_id == self.session.current_q0rg_id)
            .map(|q| q.frame_count.max(1))
            .unwrap_or(1);
        let mut next = self.session.current_frame as i32 + frames;
        next %= total as i32;
        if next < 0 {
            next += total as i32;
        }
        self.session.current_frame = next as u16;
    }
}

fn color_row(ui: &mut egui::Ui, label: &str, target: &mut crate::settings::ColorRgb) {
    ui.label(label);
    let mut c = [target.r, target.g, target.b];
    if ui.color_edit_button_srgb(&mut c).changed() {
        target.r = c[0];
        target.g = c[1];
        target.b = c[2];
    }
    ui.end_row();
}

impl App for EditorApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // First-frame init: install hardcoded base style, then layer the
        // persisted theme on top so user-picked accents survive restart.
        if !self.theme_applied {
            crate::theme::install(ctx);
            self.settings.theme.apply(ctx);
            self.theme_applied = true;
        }
        // Install the user's chosen q0lang font once a real ctx exists.
        // `set_fonts` is expensive (rebuilds the atlas), so do it once on
        // start and afterwards only when the user picks a different font
        // in the script editor toolbar (handled inline there).
        if !self.q0lang_font_installed {
            let _ = crate::q0lang::fonts::install(ctx, &self.settings.q0lang_font_name);
            self.q0lang_font_installed = true;
        }
        self.poll_media_import();
        if ctx.input(|input| input.viewport().close_requested()) && !self.close_requested {
            if self.media_import_job.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.session.status =
                    "media import is active; wait for it to finish before closing q0editor"
                        .to_string();
            } else if self.q0enc.has_active_job() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.session.status =
                    "q0enc export is active; cancel it before closing q0editor".to_string();
            } else if self.has_dirty_projects() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if self.unsaved_action.is_none() {
                    self.handle(ctx, Action::Exit);
                }
            } else {
                self.close_requested = true;
            }
        }
        self.handle_dropped_files(ctx);
        if self.show_home {
            panels::menu::handle_home_shortcuts(self, ctx);
        } else {
            panels::menu::handle_global_shortcuts(self, ctx);
        }
        self.tick_playback();
        self.session
            .clear_inactive_frame_selection(&self.state.project);

        egui::TopBottomPanel::top("menu_bar")
            .resizable(false)
            .show(ctx, |ui| {
                panels::menu::render(self, ui);
            });
        panels::tabs::render(self, ctx);

        if self.show_home {
            egui::CentralPanel::default().show(ctx, |ui| {
                panels::home::render(self, ui);
            });
        } else {
            egui::TopBottomPanel::bottom("timeline")
                .resizable(true)
                .default_height(170.0)
                .min_height(80.0)
                .show(ctx, |ui| {
                    panels::timeline::render(self, ui);
                });
            self.session
                .clear_inactive_frame_selection(&self.state.project);

            egui::SidePanel::left("toolbar")
                .resizable(false)
                .exact_width(40.0)
                .show(ctx, |ui| {
                    panels::toolbar::render(self, ui);
                });

            egui::SidePanel::right("right_dock")
                .resizable(true)
                .default_width(260.0)
                .min_width(200.0)
                .show(ctx, |ui| {
                    let half = ui.available_height() * 0.5;
                    egui::TopBottomPanel::top("dock_library")
                        .resizable(true)
                        .default_height(half)
                        .show_inside(ui, |ui| {
                            panels::library::render(self, ui);
                        });
                    egui::CentralPanel::default().show_inside(ui, |ui| {
                        panels::properties::render(self, ui);
                    });
                });

            egui::CentralPanel::default().show(ctx, |ui| {
                panels::stage::render(self, ui);
            });

            panels::q0enc::render(self, ctx);
            crate::q0lang::render(self, ctx);
        }

        if self.session.show_credits {
            self.render_credits_dialog(ctx);
        }
        if self.session.show_settings {
            self.render_settings_dialog(ctx);
        }
        self.render_new_project_dialog(ctx);
        self.render_destructive_change_dialogs(ctx);
        self.render_unsaved_changes_dialog(ctx);

        // Update window title
        let title = if self.show_home {
            "q0editor - home".to_string()
        } else {
            self.state.title()
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        self.drain_actions(ctx);
        for status in self.q0enc.poll_events() {
            self.session.status = status;
        }
        self.session
            .clear_inactive_frame_selection(&self.state.project);

        if self.media_import_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.session.playing || self.q0enc.queue_running {
            ctx.request_repaint();
        }
    }
}

#[derive(Debug, Clone)]
struct BreakApartPiece {
    placement: Placement,
    raw_path_count: usize,
}

fn affine_to_transform(affine: Affine) -> Option<Transform2D> {
    let sx = affine.a11.hypot(affine.a21);
    let determinant = affine.a11 * affine.a22 - affine.a12 * affine.a21;
    if !sx.is_finite() || sx <= 1.0e-7 || !determinant.is_finite() || determinant <= 1.0e-9 {
        return None;
    }
    let rotation = affine.a21.atan2(affine.a11);
    let (sin, cos) = rotation.sin_cos();
    let sheared_x = cos * affine.a12 + sin * affine.a22;
    let sy = -sin * affine.a12 + cos * affine.a22;
    if !sy.is_finite() || sy <= 1.0e-7 {
        return None;
    }
    let transform = Transform2D {
        tx: affine.tx,
        ty: affine.ty,
        sx,
        sy,
        rotation,
        skew_x: (sheared_x / sy).atan(),
        skew_y: 0.0,
    };
    let rebuilt = Affine::from_transform(transform);
    let error = [
        (rebuilt.a11 - affine.a11).abs(),
        (rebuilt.a12 - affine.a12).abs(),
        (rebuilt.a21 - affine.a21).abs(),
        (rebuilt.a22 - affine.a22).abs(),
        (rebuilt.tx - affine.tx).abs(),
        (rebuilt.ty - affine.ty).abs(),
    ]
    .into_iter()
    .fold(0.0_f32, f32::max);
    (error <= 2.0e-4).then_some(transform)
}

fn break_apart_result_selection(
    project: &q0s_format::v2::ProjectV2,
    q0rg_id: u16,
    layer_id: u16,
    first_idx: usize,
    pieces: &[BreakApartPiece],
) -> Selection {
    let mut raw_refs = Vec::new();
    let mut raw_placements = Vec::new();
    let mut objects = Vec::new();
    let mut bounds: Option<(f32, f32, f32, f32)> = None;

    for (offset, piece) in pieces.iter().enumerate() {
        let placement_idx = first_idx + offset;
        let placement_ref = PlacementRef {
            q0rg_id,
            layer_id,
            placement_idx,
        };
        if piece.raw_path_count > 0 {
            raw_placements.push(placement_ref);
            raw_refs.extend((0..piece.raw_path_count).map(|path_idx| PathRef {
                q0rg_id,
                layer_id,
                placement_idx,
                path_idx,
            }));
        } else {
            objects.push(placement_ref);
        }
        if let Some(piece_bounds) = crate::render::placement_bbox(project, &piece.placement) {
            bounds = Some(match bounds {
                None => piece_bounds,
                Some(current) => (
                    current.0.min(piece_bounds.0),
                    current.1.min(piece_bounds.1),
                    current.2.max(piece_bounds.2),
                    current.3.max(piece_bounds.3),
                ),
            });
        }
    }

    if objects.is_empty() {
        return match raw_refs.as_slice() {
            [] => Selection::None,
            [only] => Selection::Path {
                q0rg_id: only.q0rg_id,
                layer_id: only.layer_id,
                placement_idx: only.placement_idx,
                path_idx: only.path_idx,
            },
            _ => Selection::Paths(raw_refs),
        };
    }
    if raw_refs.is_empty() {
        return match objects.as_slice() {
            [only] => Selection::Placement {
                q0rg_id: only.q0rg_id,
                layer_id: only.layer_id,
                placement_idx: only.placement_idx,
            },
            _ => Selection::Multi(objects),
        };
    }
    let Some((min_x, min_y, max_x, max_y)) = bounds else {
        return Selection::None;
    };
    Selection::RawArea {
        placements: raw_placements,
        objects,
        bounds_min: Vec2::new(min_x, min_y),
        bounds_max: Vec2::new(max_x, max_y),
    }
}

fn bake_vector_asset(source: &VectorAsset, asset_id: u16, transform: Affine) -> VectorAsset {
    let mut result = source.clone();
    result.asset_id = asset_id;
    for path in &mut result.paths {
        for anchor in &mut path.anchors {
            anchor.point = transform.apply(anchor.point);
            if let Some(handle) = &mut anchor.in_handle {
                *handle = transform.apply(*handle);
            }
            if let Some(handle) = &mut anchor.out_handle {
                *handle = transform.apply(*handle);
            }
        }
    }
    if let Some(stroke) = &mut result.stroke {
        stroke.width *= transform.uniform_scale();
    }
    result
}

fn raw_vector_selection(
    q0rg_id: u16,
    layer_id: u16,
    placement_idx: usize,
    path_count: usize,
) -> Selection {
    let refs: Vec<PathRef> = (0..path_count)
        .map(|path_idx| PathRef {
            q0rg_id,
            layer_id,
            placement_idx,
            path_idx,
        })
        .collect();
    match refs.as_slice() {
        [] => Selection::None,
        [only] => Selection::Path {
            q0rg_id: only.q0rg_id,
            layer_id: only.layer_id,
            placement_idx: only.placement_idx,
            path_idx: only.path_idx,
        },
        _ => Selection::Paths(refs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    use std::io::Cursor;

    fn app_test_nonce() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    }

    fn test_q0v_bytes(fps: u32, frames: u32) -> Vec<u8> {
        let spec = q0video::q0v::Q0vSpec {
            width: 2,
            height: 2,
            fps,
            timeline_frames: frames,
            video: true,
            audio: false,
            audio_sample_rate: 0,
            audio_channels: 0,
        };
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[255, 0, 0, 255].repeat(4), 2, 2, image::ColorType::Rgba8)
            .expect("encode q0v test frame");
        let mut writer = q0video::q0v::Q0vWriter::new(Cursor::new(Vec::new()), spec)
            .expect("create q0v fixture");
        for frame in 0..frames {
            writer
                .write_video_frame(
                    u64::from(frame) * q0video::q0v::MEDIA_TICKS_PER_SECOND / u64::from(fps),
                    &png,
                )
                .expect("write q0v test frame");
        }
        writer.finish().expect("finish q0v fixture").into_inner()
    }

    fn test_square(min_x: f32, max_x: f32) -> q0s_format::v2::Path {
        q0s_format::v2::Path {
            anchors: vec![
                q0s_format::v2::Anchor {
                    point: q0s_format::v2::Vec2::new(min_x, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
                q0s_format::v2::Anchor {
                    point: q0s_format::v2::Vec2::new(max_x, 0.0),
                    in_handle: None,
                    out_handle: None,
                },
                q0s_format::v2::Anchor {
                    point: q0s_format::v2::Vec2::new(max_x, 20.0),
                    in_handle: None,
                    out_handle: None,
                },
                q0s_format::v2::Anchor {
                    point: q0s_format::v2::Vec2::new(min_x, 20.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
            closed: true,
        }
    }

    fn held_raw_app() -> EditorApp {
        let mut app = EditorApp::default();
        let fill = q0s_format::v2::Rgba {
            r: 20,
            g: 40,
            b: 80,
            a: 128,
        };
        app.state.project.assets = vec![
            Asset::Vector(q0s_format::v2::VectorAsset {
                asset_id: 1,
                paths: vec![test_square(0.0, 20.0)],
                fill: Some(fill),
                stroke: None,
            }),
            Asset::Vector(q0s_format::v2::VectorAsset {
                asset_id: 2,
                paths: vec![test_square(100.0, 120.0)],
                fill: Some(fill),
                stroke: None,
            }),
        ];
        app.state.project.q0rgs[0].frame_count = 12;
        app.state.project.q0rgs[0].layers[0].placements = vec![
            Placement {
                frame: 0,
                target: Target::Asset(1),
                transform: q0s_format::v2::Transform2D::IDENTITY,
                tween: Tween::None,
            },
            Placement {
                frame: 0,
                target: Target::Asset(2),
                transform: q0s_format::v2::Transform2D::IDENTITY,
                tween: Tween::None,
            },
        ];
        app.session.current_frame = 5;
        app
    }

    #[test]
    fn library_symbol_and_vector_rename_are_real_undoable_model_changes() {
        let mut app = EditorApp::default();
        app.handle(&Context::default(), Action::AddQ0rg);
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(0.0, 20.0)],
            fill: None,
            stroke: None,
        }));
        app.state.dirty = false;

        app.handle(
            &Context::default(),
            Action::RenameLibraryItem(LibraryItem::Q0rg(2), "  Hero Symbol  ".to_string()),
        );
        assert_eq!(app.state.project.q0rgs[1].name, "Hero Symbol");
        assert!(app.state.dirty);

        app.handle(
            &Context::default(),
            Action::RenameLibraryItem(LibraryItem::Asset(77), "Hero Vector".to_string()),
        );
        assert_eq!(
            app.state.project.asset_names.get(&77).map(String::as_str),
            Some("Hero Vector")
        );

        app.handle(&Context::default(), Action::Undo);
        assert!(!app.state.project.asset_names.contains_key(&77));
        assert_eq!(app.state.project.q0rgs[1].name, "Hero Symbol");
    }

    #[test]
    fn renaming_vector_back_to_default_removes_redundant_custom_name() {
        let mut app = EditorApp::default();
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(0.0, 20.0)],
            fill: None,
            stroke: None,
        }));
        app.state
            .project
            .asset_names
            .insert(77, "Temporary".to_string());

        app.handle(
            &Context::default(),
            Action::RenameLibraryItem(LibraryItem::Asset(77), "Vector 77".to_string()),
        );

        assert!(!app.state.project.asset_names.contains_key(&77));
    }

    #[test]
    fn deleting_named_vector_removes_its_persisted_name() {
        let mut app = EditorApp::default();
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(0.0, 20.0)],
            fill: None,
            stroke: None,
        }));
        app.state
            .project
            .asset_names
            .insert(77, "named vector".to_string());
        app.session.selection = Selection::Asset(77);

        assert!(app.delete_selection());
        assert!(!app.state.project.asset_names.contains_key(&77));
        q0s_format::v2::validate(&app.state.project).expect("project remains valid");
    }

    #[test]
    fn deleting_held_raw_fill_creates_current_key_without_touching_source_key() {
        let mut app = held_raw_app();
        app.session.selection = crate::tools::selection_at_point_pub(
            &app.state.project,
            1,
            5,
            q0s_format::v2::Vec2::new(10.0, 10.0),
        )
        .expect("held raw selection");
        assert!(app.delete_selection());
        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(crate::render::active_placements_at(layer, 0).len(), 2);
        assert_eq!(crate::render::active_placements_at(layer, 5).len(), 1);
        assert_eq!(layer.placements.iter().filter(|p| p.frame == 5).count(), 1);
    }

    #[test]
    fn pasting_on_held_frame_preserves_existing_layer_contents() {
        let mut app = held_raw_app();
        app.session.clipboard = Some(crate::state::ClipboardPayload {
            placements: Vec::new(),
            raw_vectors: vec![q0s_format::v2::VectorAsset {
                asset_id: 0,
                paths: vec![test_square(40.0, 60.0)],
                fill: Some(q0s_format::v2::Rgba {
                    r: 200,
                    g: 30,
                    b: 40,
                    a: 128,
                }),
                stroke: None,
            }],
        });
        app.handle(&Context::default(), Action::Paste);
        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(crate::render::active_placements_at(layer, 0).len(), 2);
        assert_eq!(crate::render::active_placements_at(layer, 5).len(), 3);
        assert_eq!(layer.placements.iter().filter(|p| p.frame == 5).count(), 3);
    }

    #[test]
    fn startup_opens_home_without_an_active_dummy_project() {
        let app = EditorApp::default();
        assert!(app.home_visible());
        assert!(!app.has_active_project());
    }

    #[test]
    fn new_project_action_opens_settings_without_replacing_current_work() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Current work".to_string(),
                ..NewProjectSpec::default()
            }),
        );

        app.handle(&Context::default(), Action::NewProject);

        assert_eq!(app.state.project.meta.name, "Current work");
        assert_eq!(app.new_project_dialog, Some(NewProjectSpec::default()));
        assert_eq!(app.project_tabs.len(), 1);
    }

    #[test]
    fn confirmed_project_settings_create_the_requested_project() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Vertical test".to_string(),
                width: 1080,
                height: 1920,
                fps: 30,
                frame_count: 90,
            }),
        );

        assert!(app.has_active_project());
        assert!(!app.home_visible());
        assert_eq!(app.state.project.meta.name, "Vertical test");
        assert_eq!(app.state.project.meta.stage_width, 1080);
        assert_eq!(app.state.project.meta.stage_height, 1920);
        assert_eq!(app.state.project.meta.fps, 30);
        assert_eq!(app.state.project.q0rgs[0].frame_count, 90);
    }

    #[test]
    fn dirty_project_stays_open_when_another_project_is_created() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Unsaved work".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let first_tab = app.active_project_tab.expect("first project tab");
        app.state.dirty = true;
        app.session.current_frame = 7;

        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Next".to_string(),
                ..NewProjectSpec::default()
            }),
        );

        assert_eq!(app.project_tabs.len(), 2);
        assert_eq!(app.state.project.meta.name, "Next");
        assert!(app.switch_to_project_tab(first_tab));
        assert_eq!(app.state.project.meta.name, "Unsaved work");
        assert!(app.state.dirty);
        assert_eq!(app.session.current_frame, 7);
    }

    #[test]
    fn home_and_project_tabs_preserve_independent_document_state() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Alpha".to_string(),
                frame_count: 40,
                ..NewProjectSpec::default()
            }),
        );
        let alpha = app.active_project_tab.expect("alpha tab");
        app.session.current_frame = 12;
        app.state.dirty = true;

        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Beta".to_string(),
                frame_count: 80,
                ..NewProjectSpec::default()
            }),
        );
        let beta = app.active_project_tab.expect("beta tab");
        app.session.current_frame = 3;

        assert!(app.switch_to_home());
        assert!(app.home_visible());
        assert_eq!(app.project_tab_summaries().len(), 2);
        assert!(app.switch_to_project_tab(alpha));
        assert_eq!(app.state.project.meta.name, "Alpha");
        assert_eq!(app.session.current_frame, 12);
        assert!(app.state.dirty);
        assert!(app.switch_to_project_tab(beta));
        assert_eq!(app.state.project.meta.name, "Beta");
        assert_eq!(app.session.current_frame, 3);
        assert!(!app.state.dirty);
    }

    #[test]
    fn ctrl_tab_cycle_order_includes_home_and_every_project() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Alpha".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let alpha = app.active_project_tab.expect("alpha tab");
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Beta".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let beta = app.active_project_tab.expect("beta tab");

        app.cycle_document_tab(1);
        assert!(app.home_visible());
        app.cycle_document_tab(1);
        assert_eq!(app.active_project_tab, Some(alpha));
        app.cycle_document_tab(1);
        assert_eq!(app.active_project_tab, Some(beta));
        app.cycle_document_tab(-1);
        assert_eq!(app.active_project_tab, Some(alpha));
    }

    #[test]
    fn closing_a_dirty_tab_waits_for_save_or_discard() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec::default()),
        );
        let tab_id = app.active_project_tab.expect("project tab");
        app.state.dirty = true;

        app.handle(&Context::default(), Action::CloseProjectTab(tab_id));

        assert_eq!(app.project_tabs.len(), 1);
        assert!(matches!(
            app.unsaved_action,
            Some(Action::CloseProjectTab(id)) if id == tab_id
        ));
    }

    #[test]
    fn exit_walks_every_dirty_project_tab_before_closing() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Alpha".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let alpha = app.active_project_tab.expect("alpha tab");
        app.state.dirty = true;
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Beta".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        let beta = app.active_project_tab.expect("beta tab");
        app.state.dirty = true;

        app.handle(&Context::default(), Action::Exit);
        assert_eq!(app.active_project_tab, Some(alpha));
        assert!(matches!(app.unsaved_action, Some(Action::Exit)));

        app.unsaved_action = None;
        app.state.dirty = false;
        app.handle(&Context::default(), Action::Exit);
        assert_eq!(app.active_project_tab, Some(beta));
        assert!(matches!(app.unsaved_action, Some(Action::Exit)));

        app.unsaved_action = None;
        app.state.dirty = false;
        app.handle(&Context::default(), Action::Exit);
        assert!(app.close_requested);
    }

    #[test]
    fn two_tabs_cannot_save_over_the_same_q1s_path() {
        let directory = std::env::temp_dir().join(format!(
            "q0editor-tab-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("create tab save directory");
        let path = directory.join("shared.q1s");
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "First".to_string(),
                ..NewProjectSpec::default()
            }),
        );
        assert!(app.save_to(&path));
        app.handle(
            &Context::default(),
            Action::CreateProject(NewProjectSpec {
                name: "Second".to_string(),
                ..NewProjectSpec::default()
            }),
        );

        assert!(!app.save_to(&path));
        assert!(app.state.file_path.is_none());
        assert!(app.session.status.contains("already open in another tab"));
        std::fs::remove_dir_all(directory).expect("cleanup tab save directory");
    }

    #[test]
    fn failed_save_does_not_adopt_the_requested_path() {
        let mut app = EditorApp::default();
        app.state.dirty = true;
        let missing_parent =
            std::env::temp_dir().join(format!("q0editor-missing-parent-{}", std::process::id()));
        let path = missing_parent.join("project.q1s");

        assert!(!app.save_to(&path));
        assert!(app.state.file_path.is_none());
        assert!(app.state.dirty);
    }

    #[test]
    fn stepping_inside_existing_timeline_does_not_mark_project_dirty() {
        let mut app = EditorApp::default();
        app.state.dirty = false;

        app.handle(&Context::default(), Action::InsertFrame);

        assert_eq!(app.session.current_frame, 1);
        assert!(!app.state.dirty);
        assert!(!app.history.can_undo());
    }

    fn app_with_one_timeline_object(frame_count: u16) -> EditorApp {
        let mut app = EditorApp::default();
        app.state.project.q0rgs[0].frame_count = frame_count;
        app.state.project.q0rgs[0].layers[0].placements = vec![Placement {
            frame: 0,
            target: Target::Asset(77),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        }];
        app.session.current_q0rg_id = app.state.project.q0rgs[0].q0rg_id;
        app.session.current_layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
        app.state.dirty = false;
        app
    }

    #[test]
    fn f6_on_held_frame_keyframes_current_frame_without_moving_playhead() {
        let mut app = app_with_one_timeline_object(12);
        app.session.current_frame = 5;

        app.handle(&Context::default(), Action::InsertKeyframe);

        assert_eq!(app.session.current_frame, 5);
        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer
            .placements
            .iter()
            .any(|placement| placement.frame == 5));
        assert!(layer
            .placements
            .iter()
            .any(|placement| placement.frame == 0));
    }

    #[test]
    fn f6_on_keyframe_creates_key_ahead_and_moves_onto_it() {
        let mut app = app_with_one_timeline_object(12);
        app.session.current_frame = 0;

        app.handle(&Context::default(), Action::InsertKeyframe);

        assert_eq!(app.session.current_frame, 1);
        assert!(app.state.project.q0rgs[0].layers[0]
            .placements
            .iter()
            .any(|placement| placement.frame == 1));
    }

    #[test]
    fn f6_on_last_keyframe_extends_timeline_with_key_under_playhead() {
        let mut app = app_with_one_timeline_object(1);
        app.session.current_frame = 0;

        app.handle(&Context::default(), Action::InsertKeyframe);

        assert_eq!(app.state.project.q0rgs[0].frame_count, 2);
        assert_eq!(app.session.current_frame, 1);
        assert!(app.state.project.q0rgs[0].layers[0]
            .placements
            .iter()
            .any(|placement| placement.frame == 1));
    }

    #[test]
    fn f7_on_held_frame_creates_blank_key_and_stops_hold() {
        let mut app = app_with_one_timeline_object(12);
        app.session.current_frame = 5;

        app.handle(&Context::default(), Action::InsertBlankKeyframe);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(app.session.current_frame, 5);
        assert!(layer.is_blank_keyframe(5));
        assert_eq!(crate::render::active_placements_at(layer, 4).len(), 1);
        assert!(crate::render::active_placements_at(layer, 5).is_empty());
        assert!(crate::render::active_placements_at(layer, 11).is_empty());
    }

    #[test]
    fn f6_on_completely_empty_layer_creates_a_blank_keyframe() {
        let mut app = EditorApp::default();
        app.state.project.q0rgs[0].frame_count = 8;
        app.state.project.q0rgs[0].layers[0].placements.clear();
        app.state.project.q0rgs[0].layers[0]
            .explicit_keyframes
            .clear();
        app.session.current_frame = 4;

        app.handle(&Context::default(), Action::InsertKeyframe);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.is_blank_keyframe(4));
        assert!(crate::render::active_placements_at(layer, 4).is_empty());
    }

    #[test]
    fn clear_keyframe_turns_a_held_frame_into_a_blank_key() {
        let mut app = app_with_one_timeline_object(12);
        app.session.current_frame = 5;

        app.handle(&Context::default(), Action::ClearKeyframe);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.is_blank_keyframe(5));
        assert_eq!(crate::render::active_placements_at(layer, 4).len(), 1);
        assert!(crate::render::active_placements_at(layer, 5).is_empty());
    }

    #[test]
    fn delete_on_selected_timeline_frame_creates_blank_key_and_stops_hold() {
        let mut app = app_with_one_timeline_object(12);
        let layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
        app.session.current_frame = 5;
        app.session.timeline_selection = Some(crate::state::TimelineSelection::single(layer_id, 5));

        app.handle(&Context::default(), Action::DeleteSelection);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.is_blank_keyframe(5));
        assert_eq!(crate::render::active_placements_at(layer, 4).len(), 1);
        assert!(crate::render::active_placements_at(layer, 5).is_empty());
        assert!(app.state.dirty);
    }

    #[test]
    fn delete_on_selected_timeline_keyframe_removes_all_contents_but_keeps_blank_key() {
        let mut app = app_with_one_timeline_object(12);
        let layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
        app.session.timeline_selection = Some(crate::state::TimelineSelection::single(layer_id, 0));

        app.handle(&Context::default(), Action::DeleteSelection);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.placements.is_empty());
        assert!(layer.is_blank_keyframe(0));
    }

    #[test]
    fn delete_on_timeline_range_blanks_every_selected_layer_and_frame() {
        let mut app = app_with_one_timeline_object(12);
        let first_layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
        app.state.project.q0rgs[0].layers.push(Layer {
            layer_id: first_layer_id + 1,
            name: "Layer 2".to_string(),
            explicit_keyframes: Vec::new(),
            placements: vec![Placement {
                frame: 0,
                target: Target::Asset(88),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            }],
        });
        let second_layer_id = first_layer_id + 1;
        app.session.timeline_selection = Some(crate::state::TimelineSelection {
            anchor_layer_id: first_layer_id,
            anchor_frame: 3,
            focus_layer_id: second_layer_id,
            focus_frame: 4,
        });

        app.handle(&Context::default(), Action::DeleteSelection);

        for layer in &app.state.project.q0rgs[0].layers {
            assert_eq!(crate::render::active_placements_at(layer, 2).len(), 1);
            assert!(layer.is_blank_keyframe(3));
            assert!(layer.is_blank_keyframe(4));
            assert!(crate::render::active_placements_at(layer, 3).is_empty());
            assert!(crate::render::active_placements_at(layer, 4).is_empty());
        }
    }

    #[test]
    fn deleting_tween_target_frame_removes_stale_incoming_tween() {
        let mut app = app_with_one_timeline_object(12);
        let layer_id = app.state.project.q0rgs[0].layers[0].layer_id;
        app.state.project.q0rgs[0].layers[0].placements[0].tween = Tween::Linear { to_frame: 5 };
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                frame: 5,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 25.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
            });
        app.session.timeline_selection = Some(crate::state::TimelineSelection::single(layer_id, 5));

        app.handle(&Context::default(), Action::DeleteSelection);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert!(layer.is_blank_keyframe(5));
        assert!(matches!(layer.placements[0].tween, Tween::None));
    }

    #[test]
    fn bitmap_import_embeds_rgba_and_selects_library_asset() {
        use image::ImageEncoder;

        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                &[255, 0, 0, 255, 0, 255, 0, 128],
                2,
                1,
                image::ColorType::Rgba8,
            )
            .expect("encode test png");
        let path = std::env::temp_dir().join(format!(
            "q0editor-bitmap-import-{}-{}.png",
            std::process::id(),
            app_test_nonce()
        ));
        std::fs::write(&path, png).expect("write test png");
        let mut app = EditorApp::default();
        let before = app.state.project.assets.len();

        app.import_bitmap_from_path(&path);

        let _ = std::fs::remove_file(&path);
        assert_eq!(app.state.project.assets.len(), before + 1);
        let Asset::Bitmap(bitmap) = app.state.project.assets.last().expect("bitmap asset") else {
            panic!("import must create a bitmap asset");
        };
        assert_eq!((bitmap.width, bitmap.height), (2, 1));
        assert_eq!(bitmap.rgba, vec![255, 0, 0, 255, 0, 255, 0, 128]);
        assert!(matches!(app.session.selection, Selection::Asset(id) if id == bitmap.asset_id));
        assert!(app.state.dirty);
    }

    #[test]
    fn credits_window_is_independent_from_settings_and_restarts_its_reveal() {
        let mut app = EditorApp::default();
        app.session.show_settings = true;
        app.session.show_credits = false;
        app.session.credits_opened_at = Instant::now() - Duration::from_secs(5);

        app.handle(&Context::default(), Action::ToggleCredits);
        assert!(app.session.show_credits);
        assert!(app.session.show_settings);
        assert!(app.session.credits_opened_at.elapsed() < Duration::from_secs(1));

        app.handle(&Context::default(), Action::ToggleCredits);
        assert!(!app.session.show_credits);
        assert!(app.session.show_settings);
    }

    #[test]
    fn q0enc_window_is_independent_from_settings_and_credits() {
        let mut app = EditorApp::default();
        app.session.show_settings = true;
        app.session.show_credits = true;

        app.handle(&Context::default(), Action::OpenQ0Enc);
        assert!(app.q0enc.open);
        assert!(app.session.show_settings);
        assert!(app.session.show_credits);
        assert_eq!(
            app.q0enc.source_q0rg_id,
            app.state.project.meta.entry_q0rg_id
        );
        assert_eq!(
            app.q0enc.width,
            u32::from(app.state.project.meta.stage_width)
        );
        assert_eq!(
            app.q0enc.height,
            u32::from(app.state.project.meta.stage_height)
        );

        app.handle(&Context::default(), Action::ToggleQ0Enc);
        assert!(!app.q0enc.open);
        assert!(app.session.show_settings);
        assert!(app.session.show_credits);
    }

    #[test]
    fn affine_decomposition_round_trips_composed_transform() {
        let parent = Transform2D {
            tx: 30.0,
            ty: -12.0,
            sx: 1.4,
            sy: 0.8,
            rotation: 0.35,
            skew_x: 0.2,
            skew_y: 0.0,
        };
        let child = Transform2D {
            tx: 9.0,
            ty: 5.0,
            sx: 0.7,
            sy: 1.2,
            rotation: -0.15,
            skew_x: -0.1,
            skew_y: 0.0,
        };
        let composed = Affine::compose(
            Affine::from_transform(parent),
            Affine::from_transform(child),
        );
        let decomposed = affine_to_transform(composed).expect("positive affine decomposition");
        let rebuilt = Affine::from_transform(decomposed);
        for point in [
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, -5.0),
            Vec2::new(-3.0, 11.0),
        ] {
            let expected = composed.apply(point);
            let actual = rebuilt.apply(point);
            assert!((expected.x - actual.x).abs() < 0.001);
            assert!((expected.y - actual.y).abs() < 0.001);
        }
    }

    #[test]
    fn break_apart_transformed_vector_bakes_it_into_raw_graphics() {
        let mut app = EditorApp::default();
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(0.0, 20.0)],
            fill: Some(q0s_format::v2::Rgba {
                r: 1,
                g: 2,
                b: 3,
                a: 255,
            }),
            stroke: None,
        }));
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                frame: 0,
                target: Target::Asset(77),
                transform: Transform2D {
                    tx: 50.0,
                    ty: 25.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
            });
        app.session.selection = Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };

        app.handle(&Context::default(), Action::BreakApartSelection);

        let placement = &app.state.project.q0rgs[0].layers[0].placements[0];
        let Target::Asset(new_id) = placement.target else {
            panic!("raw asset");
        };
        assert_ne!(new_id, 77);
        assert_eq!(placement.transform, Transform2D::IDENTITY);
        assert!(matches!(
            app.session.selection,
            Selection::Path { .. } | Selection::Paths(_)
        ));
        let Asset::Vector(vector) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == new_id)
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(vector.paths[0].anchors[0].point, Vec2::new(50.0, 25.0));
        assert_eq!(
            app.state
                .project
                .assets
                .iter()
                .find(|asset| asset.id() == 77)
                .unwrap()
                .id(),
            77
        );
    }

    #[test]
    fn break_apart_q0rg_flattens_visible_vector_child_at_composed_pose() {
        let mut app = EditorApp::default();
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(0.0, 20.0)],
            fill: Some(q0s_format::v2::Rgba {
                r: 1,
                g: 2,
                b: 3,
                a: 255,
            }),
            stroke: None,
        }));
        app.state.project.q0rgs.push(Q0rg {
            q0rg_id: 2,
            name: "Child".to_string(),
            frame_count: 1,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                name: "Art".to_string(),
                explicit_keyframes: Vec::new(),
                placements: vec![Placement {
                    frame: 0,
                    target: Target::Asset(77),
                    transform: Transform2D {
                        tx: 10.0,
                        ty: 5.0,
                        ..Transform2D::IDENTITY
                    },
                    tween: Tween::None,
                }],
            }],
        });
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                frame: 0,
                target: Target::Q0rg(2),
                transform: Transform2D {
                    tx: 50.0,
                    ty: 20.0,
                    ..Transform2D::IDENTITY
                },
                tween: Tween::None,
            });
        app.session.selection = Selection::Placement {
            q0rg_id: 1,
            layer_id: 1,
            placement_idx: 0,
        };

        app.handle(&Context::default(), Action::BreakApartSelection);

        let layer = &app.state.project.q0rgs[0].layers[0];
        assert_eq!(layer.placements.len(), 1);
        assert!(!matches!(layer.placements[0].target, Target::Q0rg(_)));
        assert_eq!(layer.placements[0].transform, Transform2D::IDENTITY);
        let Target::Asset(new_id) = layer.placements[0].target else {
            unreachable!()
        };
        let Asset::Vector(vector) = app
            .state
            .project
            .assets
            .iter()
            .find(|asset| asset.id() == new_id)
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(vector.paths[0].anchors[0].point, Vec2::new(60.0, 25.0));
        assert!(matches!(
            app.session.selection,
            Selection::Path { .. } | Selection::Paths(_)
        ));
    }

    #[test]
    fn placing_vector_from_library_bakes_a_raw_copy_instead_of_an_object_placement() {
        let mut app = EditorApp::default();
        app.state.project.assets.push(Asset::Vector(VectorAsset {
            asset_id: 77,
            paths: vec![test_square(0.0, 20.0)],
            fill: Some(q0s_format::v2::Rgba {
                r: 10,
                g: 20,
                b: 30,
                a: 255,
            }),
            stroke: None,
        }));
        let drop = Vec2::new(100.0, 80.0);

        app.handle(
            &Context::default(),
            Action::PlaceLibraryItemAt(LibraryItem::Asset(77), drop),
        );

        let layer = &app.state.project.q0rgs[0].layers[0];
        let placement = layer.placements.last().expect("placed vector copy");
        let Target::Asset(copy_id) = placement.target else {
            panic!("vector copy target");
        };
        assert_ne!(copy_id, 77, "library source asset must remain immutable");
        assert_eq!(placement.transform, Transform2D::IDENTITY);
        assert!(matches!(
            app.session.selection,
            Selection::Path { .. } | Selection::Paths(_)
        ));
        let Asset::Vector(source) = app
            .state
            .project
            .assets
            .iter()
            .find(|a| a.id() == 77)
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(source.paths[0].anchors[0].point, Vec2::new(0.0, 0.0));
        let Asset::Vector(copy) = app
            .state
            .project
            .assets
            .iter()
            .find(|a| a.id() == copy_id)
            .unwrap()
        else {
            unreachable!()
        };
        let points: Vec<Vec2> = copy
            .paths
            .iter()
            .flat_map(crate::render::flatten_path)
            .collect();
        let min_x = points.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let max_x = points.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
        let min_y = points.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let max_y = points.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        assert!((min_x - 90.0).abs() < 0.001 && (max_x - 110.0).abs() < 0.001);
        assert!((min_y - 70.0).abs() < 0.001 && (max_y - 90.0).abs() < 0.001);
    }

    #[test]
    fn placing_bitmap_from_library_centres_it_under_drop_position() {
        let mut app = EditorApp::default();
        app.state
            .project
            .assets
            .push(Asset::Bitmap(q0s_format::v2::BitmapAsset {
                asset_id: 77,
                width: 20,
                height: 10,
                rgba: vec![255; 20 * 10 * 4],
            }));
        let drop = q0s_format::v2::Vec2::new(100.0, 80.0);

        app.handle(
            &Context::default(),
            Action::PlaceLibraryItemAt(LibraryItem::Asset(77), drop),
        );

        let placement = app.state.project.q0rgs[0].layers[0]
            .placements
            .last()
            .expect("placed bitmap");
        assert_eq!(placement.target, Target::Asset(77));
        assert!((placement.transform.tx - 90.0).abs() < 0.001);
        assert!((placement.transform.ty - 75.0).abs() < 0.001);
        assert_eq!(placement.frame, app.session.current_frame);
    }

    #[test]
    fn shortening_over_content_waits_for_confirmation() {
        let mut app = EditorApp::default();
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                frame: 12,
                target: Target::Asset(1),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            });

        app.handle(&Context::default(), Action::SetQ0rgFrameCount(1, 8));

        assert_eq!(app.state.project.q0rgs[0].frame_count, 24);
        assert!(matches!(app.pending_frame_truncate, Some((1, 8, 1, 0))));
        assert!(!app.state.dirty);
    }
    #[test]
    fn deleting_symbol_removes_instances_and_is_undoable() {
        let mut app = EditorApp::default();
        app.handle(&Context::default(), Action::AddQ0rg);
        assert!(app.state.project.q0rgs.iter().any(|q0rg| q0rg.q0rg_id == 2));
        app.state.project.q0rgs[0].layers[0]
            .placements
            .push(Placement {
                frame: 0,
                target: Target::Q0rg(2),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            });

        app.handle(
            &Context::default(),
            Action::DeleteLibraryItem(LibraryItem::Q0rg(2)),
        );
        assert_eq!(app.pending_q0rg_delete, Some((2, 1)));
        app.apply_q0rg_delete(2);

        assert!(!app.state.project.q0rgs.iter().any(|q0rg| q0rg.q0rg_id == 2));
        assert!(app.state.project.q0rgs[0].layers[0].placements.is_empty());
        q0s_format::v2::validate(&app.state.project).expect("project after symbol delete");

        app.handle(&Context::default(), Action::Undo);
        assert!(app.state.project.q0rgs.iter().any(|q0rg| q0rg.q0rg_id == 2));
        assert!(matches!(
            app.state.project.q0rgs[0].layers[0].placements[0].target,
            Target::Q0rg(2)
        ));
    }

    #[test]
    fn stage_symbol_cannot_be_deleted() {
        let mut app = EditorApp::default();
        app.handle(
            &Context::default(),
            Action::DeleteLibraryItem(LibraryItem::Q0rg(1)),
        );
        assert!(app.state.project.q0rgs.iter().any(|q0rg| q0rg.q0rg_id == 1));
        assert_eq!(app.session.status, "the stage symbol cannot be deleted");
    }

    #[test]
    fn layer_rename_and_depth_move_are_undoable() {
        let mut app = EditorApp::default();
        app.handle(&Context::default(), Action::AddLayer);
        assert_eq!(
            app.state.project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        app.handle(
            &Context::default(),
            Action::RenameLayer(1, 2, "  Foreground  ".to_string()),
        );
        assert_eq!(app.state.project.q0rgs[0].layers[1].name, "Foreground");

        app.handle(&Context::default(), Action::MoveLayer(1, 1, -1));
        assert_eq!(
            app.state.project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![2, 1],
            "moving up in the front-to-back timeline must move later in render order"
        );
        q0s_format::v2::validate(&app.state.project).expect("moved layer project");

        app.handle(&Context::default(), Action::Undo);
        assert_eq!(
            app.state.project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn folder_owns_contiguous_children_and_cannot_receive_keyframes_or_artwork() {
        let mut app = EditorApp::default();
        app.session.current_layer_id = 1;
        app.handle(&Context::default(), Action::AddLayerFolder);
        let folder_id = app.session.current_layer_id;
        assert_eq!(folder_id, 2);
        assert!(app.state.project.layer_is_folder(1, folder_id));

        app.handle(&Context::default(), Action::IndentLayer(1, 1));
        assert_eq!(app.state.project.layer_parent_folder(1, 1), Some(folder_id));

        app.session.current_layer_id = 1;
        app.handle(&Context::default(), Action::AddLayer);
        let second_child = app.session.current_layer_id;
        assert_eq!(
            app.state.project.layer_parent_folder(1, second_child),
            Some(folder_id)
        );
        assert_eq!(
            app.state.project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![1, second_child, folder_id]
        );

        app.handle(&Context::default(), Action::MoveLayer(1, 1, -1));
        assert_eq!(
            app.state.project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![second_child, 1, folder_id]
        );

        app.session.current_layer_id = folder_id;
        app.handle(&Context::default(), Action::InsertBlankKeyframe);
        let folder = app.state.project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == folder_id)
            .unwrap();
        assert!(folder.explicit_keyframes.is_empty());
        assert!(folder.placements.is_empty());
        assert!(crate::tools::materialize_layer_keyframe_for_edit(
            &mut app.state.project,
            1,
            folder_id,
            0,
        )
        .is_none());

        q0s_format::v2::validate(&app.state.project).expect("folder project");
        app.request_layer_delete(1, folder_id);
        assert_eq!(app.pending_layer_delete, Some((1, folder_id, 3, 0)));
        app.apply_layer_delete(1, folder_id);
        assert_eq!(app.state.project.q0rgs[0].layers.len(), 1);
        assert!(!app
            .state
            .project
            .layer_is_folder(1, app.state.project.q0rgs[0].layers[0].layer_id));
        q0s_format::v2::validate(&app.state.project).expect("folder deleted project");
    }

    #[test]
    fn drag_drop_moves_layers_between_folders_and_preserves_tree_order() {
        let mut app = EditorApp::default();
        app.state.project.q0rgs[0].layers = vec![
            Layer {
                layer_id: 1,
                name: "a low".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 2,
                name: "a high".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 10,
                name: "folder a".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 3,
                name: "b child".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 20,
                name: "folder b".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
            Layer {
                layer_id: 5,
                name: "front".into(),
                explicit_keyframes: Vec::new(),
                placements: Vec::new(),
            },
        ];
        for folder_id in [10, 20] {
            app.state.project.layer_metadata.insert(
                LayerKey::new(1, folder_id),
                LayerMetadata {
                    kind: LayerKind::Folder,
                    parent_folder_id: None,
                    collapsed: false,
                },
            );
        }
        for (child_id, folder_id) in [(1, 10), (2, 10), (3, 20)] {
            app.state.project.layer_metadata.insert(
                LayerKey::new(1, child_id),
                LayerMetadata {
                    kind: LayerKind::Normal,
                    parent_folder_id: Some(folder_id),
                    collapsed: false,
                },
            );
        }

        app.handle(
            &Context::default(),
            Action::DropLayer(1, 2, LayerDropTarget::IntoFolder(20)),
        );

        assert_eq!(app.state.project.layer_parent_folder(1, 2), Some(20));
        assert_eq!(
            app.state.project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![1, 10, 3, 2, 20, 5]
        );
        q0s_format::v2::validate(&app.state.project).expect("cross-folder drag project");

        app.handle(
            &Context::default(),
            Action::DropLayer(
                1,
                3,
                LayerDropTarget::After {
                    layer_id: 3,
                    parent_folder_id: None,
                },
            ),
        );

        assert_eq!(app.state.project.layer_parent_folder(1, 3), None);
        assert_eq!(
            app.state.project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![1, 10, 3, 2, 20, 5]
        );
        q0s_format::v2::validate(&app.state.project).expect("outdented drag project");
    }

    #[test]
    fn dragging_a_folder_moves_its_children_as_one_block_and_is_undoable() {
        let mut app = EditorApp::default();
        app.handle(&Context::default(), Action::AddLayerFolder);
        let folder_id = app.session.current_layer_id;
        app.handle(&Context::default(), Action::IndentLayer(1, 1));
        app.session.current_layer_id = folder_id;
        app.handle(&Context::default(), Action::AddLayer);
        let front_layer = app.session.current_layer_id;

        app.handle(
            &Context::default(),
            Action::DropLayer(
                1,
                folder_id,
                LayerDropTarget::Before {
                    layer_id: front_layer,
                    parent_folder_id: None,
                },
            ),
        );

        assert_eq!(
            app.state.project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![front_layer, 1, folder_id]
        );
        assert_eq!(app.state.project.layer_parent_folder(1, 1), Some(folder_id));
        q0s_format::v2::validate(&app.state.project).expect("folder drag project");

        app.handle(&Context::default(), Action::Undo);
        assert_eq!(
            app.state.project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![1, folder_id, front_layer]
        );
    }

    #[test]
    fn outdented_child_stays_below_the_folder_group() {
        let mut app = EditorApp::default();
        app.handle(&Context::default(), Action::AddLayerFolder);
        let folder_id = app.session.current_layer_id;
        app.handle(&Context::default(), Action::IndentLayer(1, 1));
        app.session.current_layer_id = 1;
        app.handle(&Context::default(), Action::AddLayer);
        let top_child = app.session.current_layer_id;

        app.handle(&Context::default(), Action::OutdentLayer(1, top_child));

        assert_eq!(app.state.project.layer_parent_folder(1, top_child), None);
        assert_eq!(
            app.state.project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.layer_id)
                .collect::<Vec<_>>(),
            vec![top_child, 1, folder_id]
        );
        q0s_format::v2::validate(&app.state.project).expect("outdented project");
    }

    #[test]
    fn delete_selection_routes_library_symbol_through_symbol_delete() {
        let mut app = EditorApp::default();
        app.handle(&Context::default(), Action::AddQ0rg);
        app.session.selection = Selection::Q0rg(2);

        app.handle(&Context::default(), Action::DeleteSelection);

        assert!(!app.state.project.q0rgs.iter().any(|q0rg| q0rg.q0rg_id == 2));
        assert!(matches!(app.session.selection, Selection::None));
    }

    #[test]
    fn timeline_range_delete_never_creates_keys_on_folder_rows() {
        let mut app = EditorApp::default();
        app.handle(&Context::default(), Action::AddLayerFolder);
        let folder_id = app.session.current_layer_id;
        app.session.current_layer_id = 1;
        app.handle(&Context::default(), Action::AddLayer);
        let front_layer = app.session.current_layer_id;
        app.session.timeline_selection = Some(crate::state::TimelineSelection {
            anchor_layer_id: 1,
            anchor_frame: 0,
            focus_layer_id: front_layer,
            focus_frame: 1,
        });

        app.handle(&Context::default(), Action::DeleteSelection);

        let folder = app.state.project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id == folder_id)
            .unwrap();
        assert!(folder.explicit_keyframes.is_empty());
        assert!(folder.placements.is_empty());
        q0s_format::v2::validate(&app.state.project).expect("timeline delete with folder");
    }

    #[test]
    fn q0v_file_import_creates_a_named_embedded_library_asset() {
        let path = std::env::temp_dir().join(format!(
            "СЂРµС„РµСЂРµРЅСЃ РІРёРґРµРѕ СЃ РїСЂРѕР±РµР»РѕРј-{}.q0v",
            app_test_nonce()
        ));
        std::fs::write(&path, test_q0v_bytes(24, 2)).expect("write q0v fixture");
        let mut app = EditorApp::default();

        app.handle(
            &Context::default(),
            Action::ImportMediaFromPath(path.clone()),
        );
        for _ in 0..100 {
            app.poll_media_import();
            if app.media_import_job.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.media_import_job.is_none(), "q0v import worker finished");

        let imported = app
            .state
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                Asset::Q0v(video) => Some(video),
                _ => None,
            })
            .expect("imported q0v asset");
        let imported_name = app
            .state
            .project
            .asset_names
            .get(&imported.asset_id)
            .expect("imported q0v name");
        assert!(imported_name.starts_with("СЂРµС„РµСЂРµРЅСЃ РІРёРґРµРѕ СЃ РїСЂРѕР±РµР»РѕРј-"));
        assert!(!imported_name.ends_with(".q0v"));
        assert!(matches!(
            app.session.selection,
            Selection::Asset(id) if id == imported.asset_id
        ));
        q0s_format::v2::validate(&app.state.project).expect("imported q0v project");
        std::fs::remove_file(path).expect("cleanup q0v fixture");
    }

    #[test]
    fn q0v_timeline_drop_creates_an_independent_finite_video_layer() {
        let mut app = EditorApp::default();
        app.state.project.meta.fps = 20;
        app.state.project.q0rgs[0].frame_count = 20;
        app.state.project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 77,
            bytes: test_q0v_bytes(10, 4),
        }));
        app.state
            .project
            .asset_names
            .insert(77, "reference video".to_string());
        let original_layer = app.state.project.q0rgs[0].layers[0].clone();

        app.handle(
            &Context::default(),
            Action::PlaceLibraryItemOnTimeline(LibraryItem::Asset(77), 1, 3),
        );

        assert_eq!(app.state.project.q0rgs[0].layers.len(), 2);
        assert_eq!(app.state.project.q0rgs[0].layers[0], original_layer);
        let video_layer = app.state.project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id != 1)
            .expect("dedicated video layer");
        assert_eq!(video_layer.name, "reference video");
        assert_eq!(video_layer.placements.len(), 1);
        assert_eq!(video_layer.placements[0].frame, 3);
        assert_eq!(video_layer.placements[0].target, Target::Asset(77));
        assert!(video_layer.is_blank_keyframe(11));
        assert_eq!(app.state.project.q0rgs[0].frame_count, 20);
        assert!(matches!(
            app.session.selection,
            Selection::Placement {
                layer_id,
                placement_idx: 0,
                ..
            } if layer_id == video_layer.layer_id
        ));
        q0s_format::v2::validate(&app.state.project).expect("q0v timeline drop project");
    }

    #[test]
    fn q0v_timeline_drop_extends_a_short_scene_to_the_clip_end() {
        let mut app = EditorApp::default();
        app.state.project.meta.fps = 24;
        app.state.project.q0rgs[0].frame_count = 2;
        app.state.project.assets.push(Asset::Q0v(Q0vAsset {
            asset_id: 77,
            bytes: test_q0v_bytes(24, 5),
        }));

        app.handle(
            &Context::default(),
            Action::PlaceLibraryItemOnTimeline(LibraryItem::Asset(77), 1, 1),
        );

        assert_eq!(app.state.project.q0rgs[0].frame_count, 6);
        let video_layer = app.state.project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.layer_id != 1)
            .expect("video layer");
        assert!(video_layer.explicit_keyframes.is_empty());
        q0s_format::v2::validate(&app.state.project).expect("extended q0v scene");
    }
}
